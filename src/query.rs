use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    analysis::AnalysisResult,
    diagnostics::{AnalysisStatus, Diagnostic},
    model::{ArchitectureGraph, Edge, EdgeKind, Node, NodeId},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Inspect,
    Modules,
    Component,
    Dependencies,
    DependencyPath,
    Diagnostics,
    Export,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryRequest {
    pub kind: QueryKind,
    pub source: Option<NodeId>,
    pub destination: Option<NodeId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub status: AnalysisStatus,
    pub graph: ArchitectureGraph,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum QueryError {
    #[error("query request is invalid: {0}")]
    InvalidRequest(String),
    #[error("component identifier does not exist: {0}")]
    MissingComponent(String),
    #[error("query cannot be completed because analysis is unavailable")]
    AnalysisUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyFamily {
    All,
    Modules,
    Packages,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Outgoing,
    Incoming,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathStatus {
    Found,
    Conditional,
    NotFound,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DependencyPathView {
    pub schema_version: &'static str,
    pub analysis_status: AnalysisStatus,
    pub structural_status: AnalysisStatus,
    pub dependency_status: AnalysisStatus,
    pub family: DependencyFamily,
    pub status: PathStatus,
    pub from: NodeId,
    pub to: NodeId,
    pub nodes: Vec<NodeId>,
    pub edges: Vec<Edge>,
    pub absence_is_inconclusive: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CycleComponent {
    pub nodes: Vec<NodeId>,
    pub edges: Vec<Edge>,
    pub conditional: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CyclesView {
    pub schema_version: &'static str,
    pub analysis_status: AnalysisStatus,
    pub structural_status: AnalysisStatus,
    pub dependency_status: AnalysisStatus,
    pub family: DependencyFamily,
    pub cycles: Vec<CycleComponent>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn dependency_path(
    result: &AnalysisResult,
    from: &str,
    to: &str,
    family: DependencyFamily,
) -> Result<DependencyPathView, QueryError> {
    if family == DependencyFamily::All {
        return Err(QueryError::InvalidRequest(
            "path family must be modules or packages".into(),
        ));
    }
    let (_, source) = find_node(&result.graph, from)?;
    let (_, target) = find_node(&result.graph, to)?;
    let expected = if family == DependencyFamily::Modules {
        crate::model::NodeKind::Module
    } else {
        crate::model::NodeKind::Package
    };
    if source.kind() != expected || target.kind() != expected {
        return Err(QueryError::InvalidRequest(format!(
            "{} paths require {expected:?} endpoints",
            if family == DependencyFamily::Modules {
                "module"
            } else {
                "package"
            }
        )));
    }
    let mut adjacency = std::collections::BTreeMap::<NodeId, Vec<(NodeId, Edge)>>::new();
    let graph_nodes: std::collections::BTreeSet<_> =
        result.graph.nodes.iter().map(Node::id).collect();
    for edge in &result.graph.edges {
        if graph_nodes.contains(&edge.source)
            && path_edge_matches(family, edge)
            && let Some(next) = edge
                .target
                .as_ref()
                .filter(|target| graph_nodes.contains(*target))
        {
            adjacency
                .entry(edge.source.clone())
                .or_default()
                .push((next.clone(), edge.clone()));
        }
    }
    for edges in adjacency.values_mut() {
        edges.sort_by(|a, b| (&a.0, &a.1.id).cmp(&(&b.0, &b.1.id)));
    }
    let mut queue = std::collections::VecDeque::from([source.clone()]);
    let mut previous = std::collections::BTreeMap::<NodeId, (NodeId, Edge)>::new();
    let mut seen = std::collections::BTreeSet::from([source.clone()]);
    while let Some(current) = queue.pop_front() {
        if current == target {
            break;
        }
        for (next, edge) in adjacency.get(&current).into_iter().flatten() {
            if seen.insert(next.clone()) {
                previous.insert(next.clone(), (current.clone(), edge.clone()));
                queue.push_back(next.clone());
            }
        }
    }
    let (status, nodes, edges) = if source == target {
        (PathStatus::Found, vec![source.clone()], vec![])
    } else if seen.contains(&target) {
        let mut nodes = vec![target.clone()];
        let mut edges = Vec::new();
        let mut cursor = target.clone();
        while cursor != source {
            let Some((prior, edge)) = previous.get(&cursor) else {
                return Err(QueryError::InvalidRequest(
                    "dependency graph is malformed".into(),
                ));
            };
            nodes.push(prior.clone());
            edges.push(edge.clone());
            cursor = prior.clone();
        }
        nodes.reverse();
        edges.reverse();
        let conditional = edges.iter().any(|edge| {
            edge.resolution == crate::model::ResolutionStatus::Conditional
                || edge.condition.is_some()
        });
        (
            if conditional {
                PathStatus::Conditional
            } else {
                PathStatus::Found
            },
            nodes,
            edges,
        )
    } else {
        (PathStatus::NotFound, vec![], vec![])
    };
    Ok(DependencyPathView {
        schema_version: "1",
        analysis_status: result.status,
        structural_status: result.structural_status,
        dependency_status: result.dependency_status,
        family,
        status,
        from: source,
        to: target,
        nodes,
        edges,
        absence_is_inconclusive: status == PathStatus::NotFound
            && result.dependency_status != AnalysisStatus::Complete,
        diagnostics: if result.dependency_status == AnalysisStatus::Complete {
            vec![]
        } else {
            result.graph.diagnostics.clone()
        },
    })
}

pub fn cycles(result: &AnalysisResult, family: DependencyFamily) -> Result<CyclesView, QueryError> {
    if family == DependencyFamily::All {
        return Err(QueryError::InvalidRequest(
            "cycle family must be modules or packages".into(),
        ));
    }
    let kind = if family == DependencyFamily::Modules {
        crate::model::NodeKind::Module
    } else {
        crate::model::NodeKind::Package
    };
    let mut nodes: Vec<_> = result
        .graph
        .nodes
        .iter()
        .map(Node::id)
        .filter(|id| id.kind() == kind)
        .collect();
    nodes.sort();
    nodes.dedup();
    let node_set: std::collections::BTreeSet<_> = nodes.iter().cloned().collect();
    let mut forward = std::collections::BTreeMap::<NodeId, Vec<NodeId>>::new();
    let mut reverse = std::collections::BTreeMap::<NodeId, Vec<NodeId>>::new();
    let eligible: Vec<_> = result
        .graph
        .edges
        .iter()
        .filter(|edge| {
            node_set.contains(&edge.source)
                && path_edge_matches(family, edge)
                && edge
                    .target
                    .as_ref()
                    .is_some_and(|target| node_set.contains(target))
        })
        .cloned()
        .collect();
    for edge in &eligible {
        let Some(target) = edge.target.as_ref() else {
            continue;
        };
        forward
            .entry(edge.source.clone())
            .or_default()
            .push(target.clone());
        reverse
            .entry(target.clone())
            .or_default()
            .push(edge.source.clone());
    }
    for map in [&mut forward, &mut reverse] {
        for adjacent in map.values_mut() {
            adjacent.sort();
            adjacent.dedup();
        }
    }
    // Iterative Kosaraju avoids recursion on large dependency graphs.
    let mut visited = std::collections::BTreeSet::new();
    let mut order = Vec::new();
    for start in &nodes {
        if visited.contains(start) {
            continue;
        }
        visited.insert(start.clone());
        let mut stack = vec![(start.clone(), 0usize)];
        while let Some((node, index)) = stack.last_mut() {
            let adjacent = forward.get(node).map(Vec::as_slice).unwrap_or(&[]);
            if *index < adjacent.len() {
                let next = adjacent[*index].clone();
                *index += 1;
                if visited.insert(next.clone()) {
                    stack.push((next, 0));
                }
            } else {
                if let Some((done, _)) = stack.pop() {
                    order.push(done);
                }
            }
        }
    }
    visited.clear();
    let mut components = Vec::<Vec<NodeId>>::new();
    for start in order.into_iter().rev() {
        if !visited.insert(start.clone()) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            component.push(node.clone());
            for next in reverse.get(&node).into_iter().flatten().rev() {
                if visited.insert(next.clone()) {
                    stack.push(next.clone());
                }
            }
        }
        component.sort();
        components.push(component);
    }
    let mut cycles = Vec::new();
    for component in components {
        let set: std::collections::BTreeSet<_> = component.iter().cloned().collect();
        let edges: Vec<_> = eligible
            .iter()
            .filter(|edge| {
                set.contains(&edge.source)
                    && edge
                        .target
                        .as_ref()
                        .is_some_and(|target| set.contains(target))
            })
            .cloned()
            .collect();
        let cyclic = component.len() > 1
            || edges
                .iter()
                .any(|edge| edge.target.as_ref() == Some(&component[0]));
        if cyclic {
            let conditional = edges.iter().any(|edge| {
                edge.condition.is_some()
                    || edge.resolution == crate::model::ResolutionStatus::Conditional
            });
            cycles.push(CycleComponent {
                nodes: component,
                edges,
                conditional,
            });
        }
    }
    cycles.sort_by(|a, b| a.nodes.cmp(&b.nodes));
    Ok(CyclesView {
        schema_version: "1",
        analysis_status: result.status,
        structural_status: result.structural_status,
        dependency_status: result.dependency_status,
        family,
        cycles,
        diagnostics: if result.dependency_status == AnalysisStatus::Complete {
            vec![]
        } else {
            result.graph.diagnostics.clone()
        },
    })
}

fn path_edge_matches(family: DependencyFamily, edge: &Edge) -> bool {
    let expected_kind = if family == DependencyFamily::Modules {
        crate::model::NodeKind::Module
    } else {
        crate::model::NodeKind::Package
    };
    let relationship = if family == DependencyFamily::Modules {
        edge.kind == EdgeKind::ReferencesModule
    } else {
        edge.kind == EdgeKind::PackageDependency
    };
    relationship
        && edge.source.kind() == expected_kind
        && edge
            .target
            .as_ref()
            .is_some_and(|target| target.kind() == expected_kind)
        && matches!(
            edge.resolution,
            crate::model::ResolutionStatus::Confirmed
                | crate::model::ResolutionStatus::Resolved
                | crate::model::ResolutionStatus::Conditional
        )
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ComponentView {
    pub schema_version: &'static str,
    pub analysis_status: AnalysisStatus,
    pub structural_status: AnalysisStatus,
    pub dependency_status: AnalysisStatus,
    pub component: Node,
    pub parents: Vec<Edge>,
    pub children: Vec<Edge>,
    pub definitions: Vec<Node>,
    pub definition_evidence: Vec<Edge>,
    pub dependencies: Vec<Edge>,
    pub dependents: Vec<Edge>,
    pub unresolved: Vec<Edge>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DependenciesView {
    pub schema_version: &'static str,
    pub analysis_status: AnalysisStatus,
    pub structural_status: AnalysisStatus,
    pub dependency_status: AnalysisStatus,
    pub component: Node,
    pub family: DependencyFamily,
    pub direction: Direction,
    pub relationships: Vec<Edge>,
    pub unresolved: Vec<Edge>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn component(result: &AnalysisResult, id: &str) -> Result<ComponentView, QueryError> {
    let (node, node_id) = find_node(&result.graph, id)?;
    let parents = result
        .graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Contains && edge.target.as_ref() == Some(&node_id))
        .cloned()
        .collect();
    let children: Vec<_> = result
        .graph
        .edges
        .iter()
        .filter(|edge| {
            edge.source == node_id
                && matches!(edge.kind, EdgeKind::Contains | EdgeKind::LoadsSource)
        })
        .cloned()
        .collect();
    let module_scope = match &node {
        Node::Module(module) => Some(module.id.clone()),
        Node::Definition(definition) => Some(definition.module.clone()),
        _ => None,
    };
    let definitions: Vec<_> = result.graph.nodes.iter().filter(|node| matches!(node, Node::Definition(definition) if module_scope.as_ref() == Some(&definition.module))).cloned().collect();
    let definition_ids: std::collections::BTreeSet<_> = definitions.iter().map(Node::id).collect();
    let definition_evidence = result
        .graph
        .edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::Contains
                && edge
                    .target
                    .as_ref()
                    .is_some_and(|target| definition_ids.contains(target))
        })
        .cloned()
        .collect();
    let dependencies = result
        .graph
        .edges
        .iter()
        .filter(|edge| edge.source == node_id && is_dependency(edge.kind))
        .cloned()
        .collect();
    let dependents = result
        .graph
        .edges
        .iter()
        .filter(|edge| edge.target.as_ref() == Some(&node_id) && is_dependency(edge.kind))
        .cloned()
        .collect();
    let unresolved = result
        .graph
        .edges
        .iter()
        .filter(|edge| edge.source == node_id && edge.kind == EdgeKind::UnresolvedRelationship)
        .cloned()
        .collect();
    let relevant = related_diagnostics(result, &node_id);
    Ok(ComponentView {
        schema_version: "1",
        analysis_status: result.status,
        structural_status: result.structural_status,
        dependency_status: result.dependency_status,
        component: node,
        parents,
        children,
        definitions,
        definition_evidence,
        dependencies,
        dependents,
        unresolved,
        diagnostics: relevant,
    })
}

pub fn dependencies(
    result: &AnalysisResult,
    id: &str,
    family: DependencyFamily,
    direction: Direction,
) -> Result<DependenciesView, QueryError> {
    let (component, node_id) = find_node(&result.graph, id)?;
    let relationships = result
        .graph
        .edges
        .iter()
        .filter(|edge| {
            if !family_matches(family, edge.kind) || edge.kind == EdgeKind::UnresolvedRelationship {
                return false;
            }
            match direction {
                Direction::Outgoing => edge.source == node_id,
                Direction::Incoming => edge.target.as_ref() == Some(&node_id),
            }
        })
        .cloned()
        .collect();
    let unresolved = result
        .graph
        .edges
        .iter()
        .filter(|edge| {
            if edge.kind != EdgeKind::UnresolvedRelationship
                || !family_matches_unresolved(family, edge)
            {
                return false;
            }
            match direction {
                Direction::Outgoing => edge.source == node_id,
                Direction::Incoming => false,
            }
        })
        .cloned()
        .collect();
    let diagnostics = related_diagnostics(result, &node_id);
    Ok(DependenciesView {
        schema_version: "1",
        analysis_status: result.status,
        structural_status: result.structural_status,
        dependency_status: result.dependency_status,
        component,
        family,
        direction,
        relationships,
        unresolved,
        diagnostics,
    })
}

fn find_node(graph: &ArchitectureGraph, id: &str) -> Result<(Node, NodeId), QueryError> {
    graph
        .nodes
        .iter()
        .find_map(|node| {
            let node_id = node.id();
            (node_id.stable_string() == id).then(|| (node.clone(), node_id))
        })
        .ok_or_else(|| QueryError::MissingComponent(id.to_owned()))
}

fn related_diagnostics(result: &AnalysisResult, id: &NodeId) -> Vec<Diagnostic> {
    let module_id = match id {
        NodeId::Module(module) => Some(module),
        NodeId::Definition(definition) => result.graph.nodes.iter().find_map(|node| match node {
            Node::Definition(value) if &value.id == definition => Some(&value.module),
            _ => None,
        }),
        _ => None,
    };
    let source_paths: Vec<_> = module_id
        .into_iter()
        .flat_map(|module| {
            result
                .graph
                .edges
                .iter()
                .filter(move |edge| {
                    edge.kind == EdgeKind::LoadsSource
                        && edge.source == NodeId::Module(module.clone())
                })
                .filter_map(|edge| edge.target.as_ref())
                .filter_map(|target| {
                    result.graph.nodes.iter().find_map(|node| match node {
                        Node::SourceFile(file) if node.id() == *target => Some(file.path.clone()),
                        _ => None,
                    })
                })
        })
        .collect();
    result
        .graph
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.related_nodes.contains(id)
                || diagnostic
                    .path
                    .as_ref()
                    .is_some_and(|path| source_paths.contains(path))
        })
        .cloned()
        .collect()
}

fn is_dependency(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Imports
            | EdgeKind::Reexports
            | EdgeKind::ReferencesModule
            | EdgeKind::PackageDependency
    )
}

fn family_matches(family: DependencyFamily, kind: EdgeKind) -> bool {
    match family {
        DependencyFamily::All => is_dependency(kind),
        DependencyFamily::Modules => matches!(
            kind,
            EdgeKind::Imports | EdgeKind::Reexports | EdgeKind::ReferencesModule
        ),
        DependencyFamily::Packages => kind == EdgeKind::PackageDependency,
    }
}

fn family_matches_unresolved(family: DependencyFamily, edge: &Edge) -> bool {
    match family {
        DependencyFamily::All => {
            family_matches_unresolved(DependencyFamily::Modules, edge)
                || family_matches_unresolved(DependencyFamily::Packages, edge)
        }
        DependencyFamily::Modules => {
            edge.source.kind() == crate::model::NodeKind::Module
                && edge.evidence.iter().any(|evidence| {
                    matches!(
                        evidence.kind,
                        crate::model::EvidenceKind::Import
                            | crate::model::EvidenceKind::Reexport
                            | crate::model::EvidenceKind::ModuleReference
                    )
                })
        }
        DependencyFamily::Packages => {
            edge.source.kind() == crate::model::NodeKind::Package
                && edge.syntax.is_some()
                && edge
                    .evidence
                    .iter()
                    .any(|evidence| evidence.kind == crate::model::EvidenceKind::PackageMetadata)
        }
    }
}

#[cfg(test)]
mod graph_query_tests {
    use super::*;
    use crate::{
        analysis::GraphBuilder,
        diagnostics::AnalysisStatus,
        model::{
            CrateId, EdgeId, ModuleId, ModuleNode, NodeId, PackageId, PackageNode, RepoPath,
            ResolutionStatus,
        },
    };

    fn fixture(arcs: &[(&str, &str)]) -> AnalysisResult {
        let mut builder = GraphBuilder::default();
        for name in ["a", "b", "c", "d"] {
            builder
                .add_node(Node::Module(ModuleNode {
                    id: ModuleId::from(name),
                    crate_id: CrateId::from("fixture"),
                    logical_path: name.into(),
                }))
                .unwrap();
        }
        for (i, (from, to)) in arcs.iter().enumerate() {
            builder
                .add_edge(Edge {
                    id: EdgeId::from(format!("e{i}")),
                    kind: EdgeKind::ReferencesModule,
                    source: NodeId::Module(ModuleId::from(*from)),
                    target: Some(NodeId::Module(ModuleId::from(*to))),
                    unresolved_target: None,
                    resolution: ResolutionStatus::Resolved,
                    syntax: None,
                    condition: None,
                    evidence: vec![],
                })
                .unwrap();
        }
        builder.finish(AnalysisStatus::Complete)
    }
    #[test]
    fn paths_choose_deterministic_shortest_and_report_absence() {
        let result = fixture(&[("a", "c"), ("c", "d"), ("a", "b"), ("b", "d")]);
        let path =
            dependency_path(&result, "module:a", "module:d", DependencyFamily::Modules).unwrap();
        assert_eq!(
            path.nodes
                .iter()
                .map(NodeId::stable_string)
                .collect::<Vec<_>>(),
            ["module:a", "module:b", "module:d"]
        );
        let missing =
            dependency_path(&result, "module:d", "module:a", DependencyFamily::Modules).unwrap();
        assert_eq!(missing.status, PathStatus::NotFound);
        assert!(!missing.absence_is_inconclusive);
    }
    #[test]
    fn cycles_include_self_loops_and_exclude_acyclic_components() {
        let result = fixture(&[("a", "b"), ("b", "a"), ("c", "c"), ("a", "d")]);
        let found = cycles(&result, DependencyFamily::Modules).unwrap();
        assert_eq!(found.cycles.len(), 2);
        assert!(found.cycles.iter().any(|c| c.nodes.len() == 2));
        assert!(
            found
                .cycles
                .iter()
                .any(|c| c.nodes == vec![NodeId::Module(ModuleId::from("c"))])
        );
    }

    #[test]
    fn package_paths_and_cycles_use_only_package_edges() {
        let mut builder = GraphBuilder::default();
        for name in ["a", "b", "c"] {
            builder
                .add_node(Node::Package(PackageNode {
                    id: PackageId::from(name),
                    name: name.into(),
                    manifest: RepoPath::new(format!("{name}/Cargo.toml")).unwrap(),
                }))
                .unwrap();
        }
        for (i, (from, to)) in [("a", "b"), ("b", "a")].iter().enumerate() {
            builder
                .add_edge(Edge {
                    id: EdgeId::from(format!("p{i}")),
                    kind: EdgeKind::PackageDependency,
                    source: NodeId::Package(PackageId::from(*from)),
                    target: Some(NodeId::Package(PackageId::from(*to))),
                    unresolved_target: None,
                    resolution: ResolutionStatus::Confirmed,
                    syntax: None,
                    condition: None,
                    evidence: vec![],
                })
                .unwrap();
        }
        let result = builder.finish(AnalysisStatus::Complete);
        let path = dependency_path(
            &result,
            "package:a",
            "package:b",
            DependencyFamily::Packages,
        )
        .unwrap();
        assert_eq!(path.status, PathStatus::Found);
        assert_eq!(path.edges.len(), 1);
        let found = cycles(&result, DependencyFamily::Packages).unwrap();
        assert_eq!(found.cycles.len(), 1);
        assert_eq!(found.cycles[0].nodes.len(), 2);
    }

    #[test]
    fn conditional_edges_make_paths_configuration_dependent() {
        let mut result = fixture(&[("a", "b")]);
        result.graph.edges[0].condition = Some(crate::model::RelationshipCondition::Predicate {
            name: "unix".into(),
            value: None,
        });
        result.graph.edges[0].resolution = crate::model::ResolutionStatus::Conditional;
        let path =
            dependency_path(&result, "module:a", "module:b", DependencyFamily::Modules).unwrap();
        assert_eq!(path.status, PathStatus::Conditional);
        assert_eq!(path.edges[0].condition, result.graph.edges[0].condition);
    }
}
