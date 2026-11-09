use serde::{Deserialize, Serialize};

use crate::{
    diagnostics::{AnalysisStatus, Diagnostic},
    model::{ArchitectureGraph, CrateId, Edge, GraphIndex, Node, PackageId, RepoPath, TargetId},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryResult {
    pub repository_root: RepoPath,
    pub packages: Vec<PackageId>,
    pub targets: Vec<TargetId>,
    pub crate_roots: Vec<CrateRoot>,
    pub diagnostics: Vec<Diagnostic>,
    pub status: AnalysisStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrateRoot {
    pub crate_id: CrateId,
    pub source_path: RepoPath,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceAnalysisResult {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub status: AnalysisStatus,
    pub structural_status: AnalysisStatus,
    pub dependency_status: AnalysisStatus,
    pub graph: ArchitectureGraph,
}

/// Incremental graph construction boundary for later discovery and parsing.
#[derive(Default)]
pub struct GraphBuilder {
    index: GraphIndex,
    diagnostics: Vec<Diagnostic>,
}

impl GraphBuilder {
    pub fn add_node(&mut self, node: Node) -> Result<(), crate::model::GraphError> {
        self.index.add_node(node)
    }
    pub fn add_edge(&mut self, edge: Edge) -> Result<(), crate::model::GraphError> {
        self.index.add_edge(edge)
    }
    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }
    pub fn extend_source_result(
        &mut self,
        result: SourceAnalysisResult,
    ) -> Result<(), crate::model::GraphError> {
        for node in result.nodes {
            self.add_node(node)?;
        }
        for edge in result.edges {
            self.add_edge(edge)?;
        }
        self.diagnostics.extend(result.diagnostics);
        Ok(())
    }
    pub fn finish(self, status: AnalysisStatus) -> AnalysisResult {
        AnalysisResult {
            status,
            structural_status: status,
            dependency_status: status,
            graph: self.index.finish(self.diagnostics),
        }
    }

    pub fn finish_scoped(
        self,
        structural_status: AnalysisStatus,
        dependency_status: AnalysisStatus,
    ) -> AnalysisResult {
        let status = if structural_status == AnalysisStatus::Failed
            || dependency_status == AnalysisStatus::Failed
        {
            AnalysisStatus::Failed
        } else if structural_status == AnalysisStatus::Partial
            || dependency_status == AnalysisStatus::Partial
        {
            AnalysisStatus::Partial
        } else {
            AnalysisStatus::Complete
        };
        AnalysisResult {
            status,
            structural_status,
            dependency_status,
            graph: self.index.finish(self.diagnostics),
        }
    }
}
