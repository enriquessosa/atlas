use serde::Serialize;

use crate::{
    analysis::AnalysisResult,
    diagnostics::{AnalysisStatus, Diagnostic},
    model::{Edge, Node},
};

pub const SCHEMA_VERSION: &str = "1";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JsonDocument<'a> {
    pub schema_version: &'static str,
    pub analysis: AnalysisMetadata,
    pub nodes: &'a [Node],
    pub edges: &'a [Edge],
    pub diagnostics: &'a [Diagnostic],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AnalysisMetadata {
    pub status: AnalysisStatus,
    pub structural_status: AnalysisStatus,
    pub dependency_status: AnalysisStatus,
    pub scope: &'static str,
}

pub fn document(result: &AnalysisResult) -> JsonDocument<'_> {
    JsonDocument {
        schema_version: SCHEMA_VERSION,
        analysis: AnalysisMetadata {
            status: result.status,
            structural_status: result.structural_status,
            dependency_status: result.dependency_status,
            scope: "atlas_supported_scope",
        },
        nodes: &result.graph.nodes,
        edges: &result.graph.edges,
        diagnostics: &result.graph.diagnostics,
    }
}

pub fn to_json(result: &AnalysisResult) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&document(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        analysis::GraphBuilder,
        diagnostics::{DiagnosticPhase, Severity},
        model::{CrateId, ModuleId, ModuleNode, Node, RelationshipCondition},
    };

    fn fixture() -> AnalysisResult {
        let mut builder = GraphBuilder::default();
        builder
            .add_node(Node::Module(ModuleNode {
                id: ModuleId::from("crate:demo::module:root"),
                crate_id: CrateId::from("crate:demo"),
                logical_path: "crate".into(),
            }))
            .unwrap();
        builder.add_diagnostic(Diagnostic::new(
            "ANALYSIS_PARTIAL",
            Severity::Warning,
            DiagnosticPhase::Parsing,
            "synthetic fixture: analysis has not run",
        ));
        builder.finish(AnalysisStatus::Partial)
    }
    #[test]
    fn json_is_versioned_and_deterministic() {
        let a = to_json(&fixture()).unwrap();
        let b = to_json(&fixture()).unwrap();
        assert_eq!(a, b);
        assert!(a.contains("\"schema_version\": \"1\""));
        assert!(a.contains("\"status\": \"partial\""));
    }
    #[test]
    fn no_absolute_machine_paths_are_emitted_by_default() {
        assert!(!to_json(&fixture()).unwrap().contains("/home/"));
    }
    #[test]
    fn diagnostics_are_serialized_structurally() {
        let json = to_json(&fixture()).unwrap();
        assert!(json.contains("ANALYSIS_PARTIAL"));
        assert!(json.contains("warning"));
    }

    #[test]
    fn conditional_expression_serializes_symbolically() {
        let condition = RelationshipCondition::All(vec![
            RelationshipCondition::Predicate {
                name: "unix".into(),
                value: None,
            },
            RelationshipCondition::Not(Box::new(RelationshipCondition::Predicate {
                name: "feature".into(),
                value: Some("experimental".into()),
            })),
        ]);
        let json = serde_json::to_string(&condition).unwrap();
        assert!(json.contains("\"kind\":\"all\""));
        assert!(json.contains("\"kind\":\"not\""));
        assert!(json.contains("experimental"));
    }
}
