// Copyright (c) The cargo-guppy Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::graph::resolve_core::{ResolveCore, Topo};
use crate::graph::select_core::SelectParams;
use crate::graph::{
    DependencyDirection, DependencyEdge, DependencyLink, PackageGraph, PackageIx, PackageMetadata,
};
use crate::petgraph_support::dot::{DotFmt, DotVisitor, DotWrite};
use crate::petgraph_support::reversed::ReverseFlip;
use cargo_metadata::PackageId;
use petgraph::prelude::*;
use petgraph::visit::{NodeFiltered, NodeRef};
use std::fmt;

/// A set of resolved packages in a package graph.
///
/// Created by `PackageSelect::resolve`.
#[derive(Clone, Debug)]
pub struct PackageResolve<'g> {
    package_graph: &'g PackageGraph,
    core: ResolveCore<PackageGraph>,
}

impl<'g> PackageResolve<'g> {
    pub(super) fn new(package_graph: &'g PackageGraph, params: SelectParams<PackageGraph>) -> Self {
        Self {
            package_graph,
            core: ResolveCore::new(package_graph.dep_graph(), params),
        }
    }

    pub(super) fn with_resolver(
        package_graph: &'g PackageGraph,
        params: SelectParams<PackageGraph>,
        resolver: impl PackageResolver<'g>,
    ) -> Self {
        Self {
            package_graph,
            core: ResolveCore::with_edge_filter(package_graph.dep_graph(), params, |edge_ref| {
                let link = package_graph.edge_to_link(
                    edge_ref.source(),
                    edge_ref.target(),
                    edge_ref.weight(),
                );
                resolver.accept(link)
            }),
        }
    }

    /// Returns the number of packages in this set.
    pub fn len(&self) -> usize {
        self.core.len()
    }

    /// Returns true if no packages were resolved in this set.
    pub fn is_empty(&self) -> bool {
        self.core.is_empty()
    }

    /// Returns true if this package ID is contained in this resolve set, false if it isn't, and
    /// None if the package ID wasn't found.
    pub fn contains(&self, package_id: &PackageId) -> Option<bool> {
        Some(
            self.core
                .contains(self.package_graph.package_ix(package_id)?),
        )
    }

    // ---
    // Set operations
    // ---

    /// Returns a `PackageResolve` that contains all packages present in at least one of `self`
    /// and `other`.
    ///
    /// ## Panics
    ///
    /// Panics if the package graphs associated with `self` and `other` don't match.
    pub fn union(&self, other: &Self) -> Self {
        assert!(
            ::std::ptr::eq(self.package_graph, other.package_graph),
            "package graphs passed into union() match"
        );
        let mut res = self.clone();
        res.core.union_with(&other.core);
        res
    }

    /// Returns a `PackageResolve` that contains all packages present in both `self` and `other`.
    ///
    /// ## Panics
    ///
    /// Panics if the package graphs associated with `self` and `other` don't match.
    pub fn intersection(&self, other: &Self) -> Self {
        assert!(
            ::std::ptr::eq(self.package_graph, other.package_graph),
            "package graphs passed into intersection() match"
        );
        let mut res = self.clone();
        res.core.intersect_with(&other.core);
        res
    }

    /// Returns a `PackageResolve` that contains all packages present in `self` but not `other`.
    ///
    /// ## Panics
    ///
    /// Panics if the package graphs associated with `self` and `other` don't match.
    pub fn difference(&self, other: &Self) -> Self {
        assert!(
            ::std::ptr::eq(self.package_graph, other.package_graph),
            "package graphs passed into difference() match"
        );
        Self {
            package_graph: self.package_graph,
            core: self.core.difference(&other.core),
        }
    }

    /// Returns a `PackageResolve` that contains all packages present in exactly one of `self` and
    /// `other`.
    ///
    /// ## Panics
    ///
    /// Panics if the package graphs associated with `self` and `other` don't match.
    pub fn symmetric_difference(&self, other: &Self) -> Self {
        assert!(
            ::std::ptr::eq(self.package_graph, other.package_graph),
            "package graphs passed into symmetric_difference() match"
        );
        let mut res = self.clone();
        res.core.symmetric_difference_with(&other.core);
        res
    }

    // ---
    // Iterators
    // ---

    /// Iterates over package IDs, in topological order in the direction specified.
    ///
    /// ## Cycles
    ///
    /// The packages within a dependency cycle will be returned in arbitrary order, but overall
    /// topological order will be maintained.
    pub fn into_ids(self, direction: DependencyDirection) -> IntoIds<'g> {
        IntoIds {
            graph: self.package_graph.dep_graph(),
            inner: self.core.topo(self.package_graph.sccs(), direction),
        }
    }

    /// Iterates over package metadatas, in topological order in the direction specified.
    ///
    /// ## Cycles
    ///
    /// The packages within a dependency cycle will be returned in arbitrary order, but overall
    /// topological order will be maintained.
    pub fn into_metadatas(self, direction: DependencyDirection) -> IntoMetadatas<'g> {
        IntoMetadatas {
            graph: self.package_graph,
            inner: self.core.topo(self.package_graph.sccs(), direction),
        }
    }

    /// Returns the set of "root package" IDs in the specified direction.
    ///
    /// * If direction is Forward, return the set of packages that do not have any dependencies
    ///   within the selected graph.
    /// * If direction is Reverse, return the set of packages that do not have any dependents within
    ///   the selected graph.
    ///
    /// ## Cycles
    ///
    /// If a root consists of a dependency cycle, all the packages in it will be returned in
    /// arbitrary order.
    pub fn into_root_ids(
        self,
        direction: DependencyDirection,
    ) -> impl Iterator<Item = &'g PackageId> + ExactSizeIterator + 'g {
        let dep_graph = &self.package_graph.dep_graph;
        self.core
            .roots(
                self.package_graph.dep_graph(),
                self.package_graph.sccs(),
                direction,
            )
            .into_iter()
            .map(move |package_ix| &dep_graph[package_ix])
    }

    /// Returns the set of "root package" metadatas in the specified direction.
    ///
    /// * If direction is Forward, return the set of packages that do not have any dependencies
    ///   within the selected graph.
    /// * If direction is Reverse, return the set of packages that do not have any dependents within
    ///   the selected graph.
    ///
    /// ## Cycles
    ///
    /// If a root consists of a dependency cycle, all the packages in it will be returned in
    /// arbitrary order.
    pub fn into_root_metadatas(
        self,
        direction: DependencyDirection,
    ) -> impl Iterator<Item = &'g PackageMetadata> + ExactSizeIterator + 'g {
        let package_graph = self.package_graph;
        self.core
            .roots(
                self.package_graph.dep_graph(),
                self.package_graph.sccs(),
                direction,
            )
            .into_iter()
            .map(move |package_ix| {
                package_graph
                    .metadata(&package_graph.dep_graph[package_ix])
                    .expect("invalid node index")
            })
    }

    /// Constructs a representation of the selected packages in `dot` format.
    pub fn into_dot<V: PackageDotVisitor + 'g>(self, visitor: V) -> impl fmt::Display + 'g {
        let node_filtered = NodeFiltered(self.package_graph.dep_graph(), self.core.included);
        DotFmt::new(node_filtered, VisitorWrap::new(self.package_graph, visitor))
    }
}

/// Represents whether a particular link within a package graph should be followed during a
/// resolve operation.
///
/// This trait is implemented for all functions that match `Fn(DependencyLink<'g>) -> bool`.
pub trait PackageResolver<'g> {
    /// Returns true if this link should be followed during a resolve operation.
    ///
    /// Returning false does not prevent the `to` package (or `from` package with `select_reverse`)
    /// from being included if it's reachable through other means.
    fn accept(&self, link: DependencyLink<'g>) -> bool;
}

impl<'g, 'a, T> PackageResolver<'g> for &'a T
where
    T: PackageResolver<'g>,
{
    fn accept(&self, link: DependencyLink<'g>) -> bool {
        (**self).accept(link)
    }
}

impl<'g, 'a> PackageResolver<'g> for Box<dyn PackageResolver<'g> + 'a> {
    fn accept(&self, link: DependencyLink<'g>) -> bool {
        (**self).accept(link)
    }
}

impl<'g, 'a> PackageResolver<'g> for &'a dyn PackageResolver<'g> {
    fn accept(&self, link: DependencyLink<'g>) -> bool {
        (**self).accept(link)
    }
}

pub(super) struct ResolverFn<F>(pub(super) F);

impl<'g, F> PackageResolver<'g> for ResolverFn<F>
where
    F: Fn(DependencyLink<'g>) -> bool,
{
    fn accept(&self, link: DependencyLink<'g>) -> bool {
        (self.0)(link)
    }
}

/// An iterator over package IDs in topological order.
///
/// The items returned are of type `&'g PackageId`. Returned by `PackageResolve::into_ids`.
pub struct IntoIds<'g> {
    graph: &'g Graph<PackageId, DependencyEdge, Directed, PackageIx>,
    inner: Topo<'g, PackageGraph>,
}

impl<'g> IntoIds<'g> {
    /// Returns the direction the iteration is happening in.
    pub fn direction(&self) -> DependencyDirection {
        self.inner.direction()
    }
}

impl<'g> Iterator for IntoIds<'g> {
    type Item = &'g PackageId;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|package_ix| &self.graph[package_ix])
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'g> ExactSizeIterator for IntoIds<'g> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

/// An iterator over package metadata in topological order.
///
/// The items returned are of type `&'g PackageMetadata`. Returned by
/// `PackageResolve::into_metadatas`.
#[derive(Clone, Debug)]
pub struct IntoMetadatas<'g> {
    graph: &'g PackageGraph,
    inner: Topo<'g, PackageGraph>,
}

impl<'g> IntoMetadatas<'g> {
    /// Returns the direction the iteration is happening in.
    pub fn direction(&self) -> DependencyDirection {
        self.inner.direction()
    }
}

impl<'g> Iterator for IntoMetadatas<'g> {
    type Item = &'g PackageMetadata;

    fn next(&mut self) -> Option<Self::Item> {
        let next_ix = self.inner.next()?;
        let package_id = &self.graph.dep_graph[next_ix];
        Some(self.graph.metadata(package_id).unwrap_or_else(|| {
            panic!(
                "known package ID '{}' not found in metadata map",
                package_id
            )
        }))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'g> ExactSizeIterator for IntoMetadatas<'g> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

/// A visitor used for formatting `dot` graphs.
pub trait PackageDotVisitor {
    /// Visits this package. The implementation may output a label for this package to the given
    /// `DotWrite`.
    fn visit_package(&self, package: &PackageMetadata, f: DotWrite<'_, '_>) -> fmt::Result;

    /// Visits this dependency link. The implementation may output a label for this link to the
    /// given `DotWrite`.
    fn visit_link(&self, link: DependencyLink<'_>, f: DotWrite<'_, '_>) -> fmt::Result;
}

struct VisitorWrap<'g, V> {
    graph: &'g PackageGraph,
    inner: V,
}

impl<'g, V> VisitorWrap<'g, V> {
    fn new(graph: &'g PackageGraph, inner: V) -> Self {
        Self { graph, inner }
    }
}

impl<'g, V, NR, ER> DotVisitor<NR, ER> for VisitorWrap<'g, V>
where
    V: PackageDotVisitor,
    NR: NodeRef<NodeId = NodeIndex<PackageIx>, Weight = PackageId>,
    ER: EdgeRef<NodeId = NodeIndex<PackageIx>, Weight = DependencyEdge> + ReverseFlip,
{
    fn visit_node(&self, node: NR, f: DotWrite<'_, '_>) -> fmt::Result {
        let metadata = self
            .graph
            .metadata(node.weight())
            .expect("visited node should have associated metadata");
        self.inner.visit_package(metadata, f)
    }

    fn visit_edge(&self, edge: ER, f: DotWrite<'_, '_>) -> fmt::Result {
        let (source_idx, target_idx) = ER::reverse_flip(edge.source(), edge.target());
        let link = self
            .graph
            .edge_to_link(source_idx, target_idx, edge.weight());
        self.inner.visit_link(link, f)
    }
}