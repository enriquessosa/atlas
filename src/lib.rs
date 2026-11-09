//! Evidence-oriented repository discovery and architecture contracts for Atlas.
pub mod analysis;
pub mod cli;
pub mod diagnostics;
pub mod discovery;
pub mod model;
pub mod modules;
pub mod query;
pub mod serialization;

pub use analysis::{AnalysisResult, DiscoveryResult, GraphBuilder, SourceAnalysisResult};
pub use diagnostics::{AnalysisStatus, Diagnostic, DiagnosticCode, DiagnosticPhase, Severity};
pub use discovery::{DiscoveryOptions, analyze_repository, analyze_repository_with};
pub use model::{ArchitectureGraph, Edge, EdgeKind, Node, NodeId, RelationshipCondition};
