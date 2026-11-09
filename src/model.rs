use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::diagnostics::Diagnostic;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(RepositoryId);
id_type!(PackageId);
id_type!(TargetId);
id_type!(CrateId);
id_type!(ModuleId);
id_type!(SourceFileId);
id_type!(DefinitionId);
id_type!(EdgeId);

/// Repository-relative, slash-separated path. `.` denotes the root.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoPath(String);

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RepoPathError {
    #[error("repository path must not be empty")]
    Empty,
    #[error("repository path must be relative")]
    Absolute,
    #[error("repository path must not escape the repository root")]
    EscapesRoot,
    #[error("repository path must use normalized slash-separated components")]
    NotNormalized,
}

impl RepoPath {
    pub fn new(path: impl Into<String>) -> Result<Self, RepoPathError> {
        let path = path.into();
        if path.is_empty() {
            return Err(RepoPathError::Empty);
        }
        if path.starts_with('/') || path.starts_with('\\') || path.as_bytes().get(1) == Some(&b':')
        {
            return Err(RepoPathError::Absolute);
        }
        if path.contains('\\') {
            return Err(RepoPathError::NotNormalized);
        }
        if path.split('/').any(|part| part == "..") {
            return Err(RepoPathError::EscapesRoot);
        }
        if path != "." && path.split('/').any(|part| part.is_empty() || part == ".") {
            return Err(RepoPathError::NotNormalized);
        }
        Ok(Self(path))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl FromStr for RepoPath {
    type Err = RepoPathError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}
impl TryFrom<String> for RepoPath {
    type Error = RepoPathError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<RepoPath> for String {
    fn from(value: RepoPath) -> Self {
        value.0
    }
}

macro_rules! node_struct {
    ($name:ident, $id:ident { $($fields:tt)* }) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
        pub struct $name { pub id: $id, $($fields)* }
    };
}
node_struct!(RepositoryNode, RepositoryId { pub root: RepoPath, pub name: String });
node_struct!(PackageNode, PackageId { pub name: String, pub manifest: RepoPath });
node_struct!(TargetNode, TargetId { pub package: PackageId, pub name: String, pub kind: TargetKind, pub crate_types: Vec<String>, pub source_root: RepoPath });
node_struct!(CrateNode, CrateId { pub target: TargetId, pub name: String, pub edition: String });
node_struct!(ModuleNode, ModuleId { pub crate_id: CrateId, pub logical_path: String });
node_struct!(SourceFileNode, SourceFileId { pub path: RepoPath, pub generated: GeneratedStatus });
node_struct!(DefinitionNode, DefinitionId { pub module: ModuleId, pub name: String, pub kind: DefinitionKind, pub visibility: DefinitionVisibility });

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Lib,
    Bin,
    Example,
    Test,
    Bench,
    Other,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedStatus {
    KnownGenerated,
    NotKnownGenerated,
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionKind {
    Function,
    Struct,
    Enum,
    Union,
    Trait,
    TypeAlias,
    Constant,
    Static,
    Macro,
    Implementation,
    AssociatedItem,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionVisibility {
    Private,
    Crate,
    Public,
    Restricted,
}

/// IDs are tagged with their node kind, preventing accidental cross-kind use.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum NodeId {
    Repository(RepositoryId),
    Package(PackageId),
    Target(TargetId),
    Crate(CrateId),
    Module(ModuleId),
    SourceFile(SourceFileId),
    Definition(DefinitionId),
}

impl NodeId {
    pub fn kind(&self) -> NodeKind {
        match self {
            Self::Repository(_) => NodeKind::Repository,
            Self::Package(_) => NodeKind::Package,
            Self::Target(_) => NodeKind::Target,
            Self::Crate(_) => NodeKind::Crate,
            Self::Module(_) => NodeKind::Module,
            Self::SourceFile(_) => NodeKind::SourceFile,
            Self::Definition(_) => NodeKind::Definition,
        }
    }
    pub fn stable_string(&self) -> String {
        let (kind, id) = match self {
            Self::Repository(v) => ("repo", &v.0),
            Self::Package(v) => ("package", &v.0),
            Self::Target(v) => ("target", &v.0),
            Self::Crate(v) => ("crate", &v.0),
            Self::Module(v) => ("module", &v.0),
            Self::SourceFile(v) => ("file", &v.0),
            Self::Definition(v) => ("definition", &v.0),
        };
        format!("{kind}:{id}")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Repository,
    Package,
    Target,
    Crate,
    Module,
    SourceFile,
    Definition,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Node {
    Repository(RepositoryNode),
    Package(PackageNode),
    Target(TargetNode),
    Crate(CrateNode),
    Module(ModuleNode),
    SourceFile(SourceFileNode),
    Definition(DefinitionNode),
}
impl Node {
    pub fn id(&self) -> NodeId {
        match self {
            Self::Repository(v) => NodeId::Repository(v.id.clone()),
            Self::Package(v) => NodeId::Package(v.id.clone()),
            Self::Target(v) => NodeId::Target(v.id.clone()),
            Self::Crate(v) => NodeId::Crate(v.id.clone()),
            Self::Module(v) => NodeId::Module(v.id.clone()),
            Self::SourceFile(v) => NodeId::SourceFile(v.id.clone()),
            Self::Definition(v) => NodeId::Definition(v.id.clone()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Contains,
    DeclaresModule,
    LoadsSource,
    Imports,
    Reexports,
    PackageDependency,
    ReferencesModule,
    UnresolvedRelationship,
}

/// Symbolic `cfg` expression; Atlas does not evaluate conditions in v0.1.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum RelationshipCondition {
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
    Constant(bool),
    Unknown,
    Predicate { name: String, value: Option<String> },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    DefinitionDeclaration,
    ModuleDeclaration,
    SourceMapping,
    Import,
    Reexport,
    PackageMetadata,
    ModuleReference,
    UnsupportedConstruct,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    RustSyntax,
    CargoMetadata,
    Analyzer,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start_byte: u64,
    pub end_byte: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_column: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
}
impl SourceSpan {
    pub fn new(start_byte: u64, end_byte: u64) -> Result<Self, GraphError> {
        if start_byte > end_byte {
            return Err(GraphError::InvalidSpan);
        }
        Ok(Self {
            start_byte,
            end_byte,
            start_line: None,
            start_column: None,
            end_line: None,
            end_column: None,
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub provenance: Provenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<RepoPath>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    Confirmed,
    Resolved,
    Conditional,
    Unresolved,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub kind: EdgeKind,
    pub source: NodeId,
    /// `None` is allowed only for an unresolved relationship, whose intended
    /// target is recorded in `unresolved_target` instead of a placeholder node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<NodeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unresolved_target: Option<String>,
    pub resolution: ResolutionStatus,
    /// Normalized source-level spelling when the relationship comes from use syntax.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syntax: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<RelationshipCondition>,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}
impl Edge {
    pub fn key(&self) -> EdgeKey {
        EdgeKey {
            kind: self.kind,
            source: self.source.clone(),
            target: self.target.clone(),
            unresolved_target: self.unresolved_target.clone(),
            resolution: self.resolution.clone(),
            condition: self.condition.clone(),
            syntax: self.syntax.clone(),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct EdgeKey {
    pub kind: EdgeKind,
    pub source: NodeId,
    pub target: Option<NodeId>,
    pub unresolved_target: Option<String>,
    pub resolution: ResolutionStatus,
    pub condition: Option<RelationshipCondition>,
    pub syntax: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureGraph {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}
impl ArchitectureGraph {
    pub fn sorted(mut self) -> Self {
        self.nodes.sort_by_key(Node::id);
        self.edges.sort_by(|a, b| {
            (
                a.kind,
                &a.source,
                &a.target,
                &a.unresolved_target,
                &a.condition,
                &a.id,
            )
                .cmp(&(
                    b.kind,
                    &b.source,
                    &b.target,
                    &b.unresolved_target,
                    &b.condition,
                    &b.id,
                ))
        });
        for edge in &mut self.edges {
            edge.evidence.sort();
            edge.evidence.dedup();
        }
        self.diagnostics.sort_by(|a, b| {
            (&a.path, &a.span, &a.phase, &a.code, &a.message)
                .cmp(&(&b.path, &b.span, &b.phase, &b.code, &b.message))
        });
        self.diagnostics.dedup();
        self
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum GraphError {
    #[error("node identifier has conflicting definitions: {0}")]
    ConflictingNode(String),
    #[error("node and edge identifiers must not be empty")]
    EmptyIdentifier,
    #[error("edge source node does not exist: {0}")]
    MissingSource(String),
    #[error("edge target node does not exist: {0}")]
    MissingTarget(String),
    #[error("relationship {kind:?} does not permit {source_kind:?} -> {target:?}")]
    InvalidEndpoints {
        kind: EdgeKind,
        source_kind: NodeKind,
        target: Option<NodeKind>,
    },
    #[error("unresolved relationships require a target description and no target node")]
    InvalidUnresolvedTarget,
    #[error("edge kind and resolution status are inconsistent")]
    InconsistentResolution,
    #[error("source span start exceeds end")]
    InvalidSpan,
    #[error("edge identifier collides for different relationships: {0}")]
    ConflictingEdge(String),
}

#[derive(Default)]
pub struct GraphIndex {
    nodes: BTreeMap<NodeId, Node>,
    edges: BTreeMap<EdgeKey, Edge>,
}
impl GraphIndex {
    pub fn add_node(&mut self, node: Node) -> Result<(), GraphError> {
        let id = node.id();
        if node_id_value(&id).is_empty() {
            return Err(GraphError::EmptyIdentifier);
        }
        if let Some(existing) = self.nodes.get(&id) {
            if existing != &node {
                return Err(GraphError::ConflictingNode(id.stable_string()));
            }
        } else {
            self.nodes.insert(id, node);
        }
        Ok(())
    }
    pub fn add_edge(&mut self, mut edge: Edge) -> Result<(), GraphError> {
        if edge.id.0.is_empty() {
            return Err(GraphError::EmptyIdentifier);
        }
        if !self.nodes.contains_key(&edge.source) {
            return Err(GraphError::MissingSource(edge.source.stable_string()));
        }
        if edge.kind == EdgeKind::UnresolvedRelationship {
            if edge.target.is_some()
                || edge
                    .unresolved_target
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                return Err(GraphError::InvalidUnresolvedTarget);
            }
            if !matches!(
                edge.resolution,
                ResolutionStatus::Unresolved | ResolutionStatus::Unsupported
            ) {
                return Err(GraphError::InconsistentResolution);
            }
        } else {
            if edge.resolution == ResolutionStatus::Unresolved {
                return Err(GraphError::InconsistentResolution);
            }
            if edge.resolution == ResolutionStatus::Conditional && edge.condition.is_none() {
                return Err(GraphError::InconsistentResolution);
            }
            let target = edge.target.as_ref().ok_or(GraphError::InvalidEndpoints {
                kind: edge.kind,
                source_kind: edge.source.kind(),
                target: None,
            })?;
            if !self.nodes.contains_key(target) {
                return Err(GraphError::MissingTarget(target.stable_string()));
            }
            if !valid_endpoint_types(edge.kind, edge.source.kind(), target.kind()) {
                return Err(GraphError::InvalidEndpoints {
                    kind: edge.kind,
                    source_kind: edge.source.kind(),
                    target: Some(target.kind()),
                });
            }
            if edge.unresolved_target.is_some() {
                return Err(GraphError::InvalidUnresolvedTarget);
            }
        }
        edge.evidence.sort();
        edge.evidence.dedup();
        let key = edge.key();
        if self
            .edges
            .values()
            .any(|existing| existing.id == edge.id && existing.key() != key)
        {
            return Err(GraphError::ConflictingEdge(edge.id.0));
        }
        if let Some(existing) = self.edges.get_mut(&key) {
            if edge.id < existing.id {
                existing.id = edge.id;
            }
            existing.evidence.extend(edge.evidence);
            existing.evidence.sort();
            existing.evidence.dedup();
        } else {
            self.edges.insert(key, edge);
        }
        Ok(())
    }
    pub fn finish(self, diagnostics: Vec<Diagnostic>) -> ArchitectureGraph {
        ArchitectureGraph {
            nodes: self.nodes.into_values().collect(),
            edges: self.edges.into_values().collect(),
            diagnostics,
        }
        .sorted()
    }
}

fn node_id_value(id: &NodeId) -> &str {
    match id {
        NodeId::Repository(v) => &v.0,
        NodeId::Package(v) => &v.0,
        NodeId::Target(v) => &v.0,
        NodeId::Crate(v) => &v.0,
        NodeId::Module(v) => &v.0,
        NodeId::SourceFile(v) => &v.0,
        NodeId::Definition(v) => &v.0,
    }
}

fn valid_endpoint_types(kind: EdgeKind, source: NodeKind, target: NodeKind) -> bool {
    use EdgeKind::*;
    use NodeKind::*;
    match kind {
        Contains => matches!(
            (source, target),
            (Repository, Package)
                | (Package, Target)
                | (Target, Crate)
                | (Crate, Module)
                | (Module, Module)
                | (Module, Definition)
                | (Definition, Definition)
        ),
        DeclaresModule | ReferencesModule => source == Module && target == Module,
        LoadsSource => source == Module && target == SourceFile,
        Imports | Reexports => source == Module && matches!(target, Module | Definition),
        PackageDependency => source == Package && target == Package,
        UnresolvedRelationship => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn module(id: &str) -> Node {
        Node::Module(ModuleNode {
            id: ModuleId::from(id),
            crate_id: CrateId::from("crate:test"),
            logical_path: id.into(),
        })
    }
    fn reference(id: &str, source: &str, target: &str, evidence: Vec<Evidence>) -> Edge {
        Edge {
            id: EdgeId::from(id),
            kind: EdgeKind::ReferencesModule,
            source: NodeId::Module(ModuleId::from(source)),
            target: Some(NodeId::Module(ModuleId::from(target))),
            unresolved_target: None,
            resolution: ResolutionStatus::Resolved,
            condition: None,
            syntax: None,
            evidence,
        }
    }
    #[test]
    fn repo_paths_reject_absolute_and_parent_paths() {
        assert!(RepoPath::new("/tmp/a").is_err());
        assert!(RepoPath::new("src/../private").is_err());
        assert_eq!(RepoPath::new("src/lib.rs").unwrap().as_str(), "src/lib.rs");
        assert!(serde_json::from_str::<RepoPath>("\"/home/private.rs\"").is_err());
    }
    #[test]
    fn conflicting_node_id_is_rejected() {
        let mut g = GraphIndex::default();
        g.add_node(module("m")).unwrap();
        let n = Node::Module(ModuleNode {
            id: ModuleId::from("m"),
            crate_id: CrateId::from("other"),
            logical_path: "m".into(),
        });
        assert!(matches!(g.add_node(n), Err(GraphError::ConflictingNode(_))));
    }
    #[test]
    fn graph_rejects_bad_endpoints_and_accepts_cycles() {
        let mut g = GraphIndex::default();
        g.add_node(module("a")).unwrap();
        g.add_node(module("b")).unwrap();
        g.add_edge(reference("ab", "a", "b", vec![])).unwrap();
        g.add_edge(reference("ba", "b", "a", vec![])).unwrap();
        let invalid = Edge {
            kind: EdgeKind::PackageDependency,
            ..reference("invalid", "a", "b", vec![])
        };
        assert!(matches!(
            g.add_edge(invalid),
            Err(GraphError::InvalidEndpoints { .. })
        ));
        let missing_target = Edge {
            target: Some(NodeId::Module(ModuleId::from("missing"))),
            ..reference("missing-target", "a", "b", vec![])
        };
        assert!(matches!(
            g.add_edge(missing_target),
            Err(GraphError::MissingTarget(_))
        ));
        assert_eq!(g.finish(vec![]).edges.len(), 2);
    }

    #[test]
    fn unresolved_relationships_are_source_only_and_status_consistent() {
        let mut g = GraphIndex::default();
        g.add_node(module("a")).unwrap();
        let edge = Edge {
            id: EdgeId::from("unresolved"),
            kind: EdgeKind::UnresolvedRelationship,
            source: NodeId::Module(ModuleId::from("a")),
            target: None,
            unresolved_target: Some("crate::unknown".into()),
            resolution: ResolutionStatus::Unresolved,
            condition: None,
            syntax: None,
            evidence: vec![],
        };
        g.add_edge(edge.clone()).unwrap();
        assert!(matches!(
            g.add_edge(Edge {
                resolution: ResolutionStatus::Resolved,
                ..edge
            }),
            Err(GraphError::InconsistentResolution)
        ));
    }
    #[test]
    fn identical_edges_merge_and_sort_evidence() {
        let mut g = GraphIndex::default();
        g.add_node(module("a")).unwrap();
        g.add_node(module("b")).unwrap();
        let ev = |n| Evidence {
            kind: EvidenceKind::ModuleReference,
            provenance: Provenance::RustSyntax,
            path: Some(RepoPath::new("src/lib.rs").unwrap()),
            span: Some(SourceSpan::new(n, n + 1).unwrap()),
        };
        g.add_edge(reference("z-edge", "a", "b", vec![ev(3)]))
            .unwrap();
        g.add_edge(reference("a-edge", "a", "b", vec![ev(1)]))
            .unwrap();
        let graph = g.finish(vec![]);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].id, EdgeId::from("a-edge"));
        assert_eq!(graph.edges[0].evidence.len(), 2);
        assert!(graph.edges[0].evidence[0] < graph.edges[0].evidence[1]);
    }
}
