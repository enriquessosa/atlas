use serde::{Deserialize, Serialize};

use crate::model::{NodeId, RepoPath, SourceSpan};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPhase {
    Cli,
    Discovery,
    Parsing,
    GraphConstruction,
    Query,
    Serialization,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DiagnosticCode(pub String);

impl DiagnosticCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub phase: DiagnosticPhase,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<RepoPath>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_nodes: Vec<NodeId>,
}

impl Diagnostic {
    pub fn new(
        code: impl Into<String>,
        severity: Severity,
        phase: DiagnosticPhase,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: DiagnosticCode::new(code),
            severity,
            phase,
            message: message.into(),
            path: None,
            span: None,
            related_nodes: Vec::new(),
        }
    }
}
