use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::{
    analysis::{AnalysisResult, GraphBuilder},
    diagnostics::{AnalysisStatus, Diagnostic, DiagnosticPhase, Severity},
    model::{
        CrateId, CrateNode, Edge, EdgeId, EdgeKind, Evidence, EvidenceKind, GeneratedStatus, Node,
        NodeId, PackageId, PackageNode, Provenance, RelationshipCondition, RepoPath, RepositoryId,
        RepositoryNode, ResolutionStatus, TargetId, TargetKind, TargetNode,
    },
    modules::discover_crate_modules,
};

const MAX_STDOUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct DiscoveryOptions {
    pub cargo_program: OsString,
    pub timeout: Duration,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            cargo_program: OsString::from("cargo"),
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    workspace_root: PathBuf,
    workspace_members: Vec<String>,
    packages: Vec<CargoPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    id: String,
    name: String,
    version: String,
    manifest_path: PathBuf,
    edition: String,
    targets: Vec<CargoTarget>,
    #[serde(default)]
    dependencies: Vec<CargoDependency>,
}

#[derive(Clone, Debug, Deserialize)]
struct CargoDependency {
    name: String,
    kind: Option<String>,
    rename: Option<String>,
    optional: bool,
    target: Option<String>,
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
    crate_types: Vec<String>,
    src_path: PathBuf,
}

/// Discover one Cargo package or workspace without network access or project
/// code execution. All failures are represented in the returned graph.
pub fn analyze_repository(path: impl AsRef<Path>) -> AnalysisResult {
    analyze_repository_with(path, &DiscoveryOptions::default())
}

pub fn analyze_repository_with(
    path: impl AsRef<Path>,
    options: &DiscoveryOptions,
) -> AnalysisResult {
    let mut builder = GraphBuilder::default();
    let input = path.as_ref();
    let (manifest, selected_root) = match locate_manifest(input) {
        Ok(located) => located,
        Err((code, message, known_root)) => {
            if let Some(root) = known_root.as_deref() {
                add_repository_node(&mut builder, root);
            }
            builder.add_diagnostic(Diagnostic::new(
                code,
                Severity::Error,
                DiagnosticPhase::Discovery,
                message,
            ));
            return builder.finish(AnalysisStatus::Failed);
        }
    };

    let metadata = match run_metadata(&manifest, &selected_root, options) {
        Ok(metadata) => metadata,
        Err((code, message)) => {
            add_repository_node(&mut builder, &selected_root);
            builder.add_diagnostic(Diagnostic::new(
                code,
                Severity::Error,
                DiagnosticPhase::Discovery,
                message,
            ));
            return builder.finish(AnalysisStatus::Failed);
        }
    };
    let workspace_root = match metadata.workspace_root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            add_repository_node(&mut builder, &selected_root);
            builder.add_diagnostic(Diagnostic::new(
                "WORKSPACE_ROOT_UNAVAILABLE",
                Severity::Error,
                DiagnosticPhase::Discovery,
                format!(
                    "Cargo reported a workspace root that cannot be accessed ({})",
                    error.kind()
                ),
            ));
            return builder.finish(AnalysisStatus::Failed);
        }
    };
    let repo_node = add_repository_node(&mut builder, &workspace_root);
    let member_ids: std::collections::BTreeSet<_> =
        metadata.workspace_members.into_iter().collect();
    let mut status = AnalysisStatus::Complete;
    let mut dependency_status = AnalysisStatus::Complete;
    let mut any_target = false;

    let mut packages: Vec<_> = metadata
        .packages
        .into_iter()
        .filter(|p| member_ids.contains(&p.id))
        .collect();
    packages.sort_by(|a, b| {
        (
            a.name.as_str(),
            a.version.as_str(),
            a.manifest_path.as_os_str(),
        )
            .cmp(&(
                b.name.as_str(),
                b.version.as_str(),
                b.manifest_path.as_os_str(),
            ))
    });
    let mut package_locations =
        std::collections::BTreeMap::<String, Vec<(PackageId, PathBuf)>>::new();
    let mut pending_package_dependencies = Vec::new();
    for package in packages {
        let manifest_rel = match relative_path(&workspace_root, &package.manifest_path) {
            Ok(path) => path,
            Err(message) => {
                status = AnalysisStatus::Partial;
                builder.add_diagnostic(Diagnostic::new(
                    "PACKAGE_OUTSIDE_REPOSITORY",
                    Severity::Error,
                    DiagnosticPhase::Discovery,
                    message,
                ));
                continue;
            }
        };
        let manifest_path = match RepoPath::new(manifest_rel) {
            Ok(path) => path,
            Err(_) => {
                status = AnalysisStatus::Partial;
                builder.add_diagnostic(Diagnostic::new(
                    "INVALID_MANIFEST_PATH",
                    Severity::Error,
                    DiagnosticPhase::Discovery,
                    "Cargo manifest path is not a valid repository-relative path",
                ));
                continue;
            }
        };
        let package_key = format!(
            "{}@{}:{}",
            package.name,
            package.version,
            manifest_path.as_str()
        );
        let package_id = PackageId::from(package_key.clone());
        let package_node_id = NodeId::Package(package_id.clone());
        let package_node = Node::Package(PackageNode {
            id: package_id.clone(),
            name: package.name.clone(),
            manifest: manifest_path.clone(),
        });
        if let Err(error) = builder.add_node(package_node) {
            status = AnalysisStatus::Partial;
            add_graph_error(&mut builder, error);
            continue;
        }
        if let Some(package_dir) = package
            .manifest_path
            .parent()
            .and_then(|path| path.canonicalize().ok())
        {
            package_locations
                .entry(package.name.clone())
                .or_default()
                .push((package_id.clone(), package_dir));
        }
        pending_package_dependencies.push((
            package_id.clone(),
            manifest_path.clone(),
            package.dependencies.clone(),
        ));
        let package_edge = metadata_edge(
            EdgeKind::Contains,
            repo_node.id(),
            package_node_id.clone(),
            format!("metadata:package:{package_key}"),
        );
        if let Err(error) = builder.add_edge(package_edge) {
            status = AnalysisStatus::Partial;
            add_graph_error(&mut builder, error);
        }

        let mut targets = package.targets;
        targets.sort_by(|a, b| {
            (a.name.as_str(), &a.kind, a.src_path.as_os_str()).cmp(&(
                b.name.as_str(),
                &b.kind,
                b.src_path.as_os_str(),
            ))
        });
        for target in targets {
            any_target = true;
            let target_rel = match relative_path(&workspace_root, &target.src_path) {
                Ok(path) => path,
                Err(message) => {
                    status = AnalysisStatus::Partial;
                    builder.add_diagnostic(Diagnostic::new(
                        "TARGET_OUTSIDE_REPOSITORY",
                        Severity::Error,
                        DiagnosticPhase::Discovery,
                        message,
                    ));
                    continue;
                }
            };
            let Ok(source_root) = RepoPath::new(target_rel) else {
                status = AnalysisStatus::Partial;
                continue;
            };
            let target_kind = classify_target(&target.kind);
            let kind_key = target.kind.join("+");
            let target_key = format!(
                "{package_key}::target:{}:{kind_key}:{}",
                target.name,
                source_root.as_str()
            );
            let target_id = TargetId::from(target_key.clone());
            let target_node = Node::Target(TargetNode {
                id: target_id.clone(),
                package: package_id.clone(),
                name: target.name.clone(),
                kind: target_kind,
                crate_types: target.crate_types.clone(),
                source_root: source_root.clone(),
            });
            if let Err(error) = builder.add_node(target_node) {
                status = AnalysisStatus::Partial;
                add_graph_error(&mut builder, error);
                continue;
            }
            let target_node_id = NodeId::Target(target_id.clone());
            if let Err(error) = builder.add_edge(metadata_edge(
                EdgeKind::Contains,
                package_node_id.clone(),
                target_node_id.clone(),
                format!("metadata:target:{target_key}"),
            )) {
                status = AnalysisStatus::Partial;
                add_graph_error(&mut builder, error);
            }

            let crate_key = format!("crate:{target_key}");
            let crate_id = CrateId::from(crate_key.clone());
            let crate_node = Node::Crate(CrateNode {
                id: crate_id.clone(),
                target: target_id.clone(),
                name: target.name.clone(),
                edition: package.edition.clone(),
            });
            if let Err(error) = builder.add_node(crate_node) {
                status = AnalysisStatus::Partial;
                add_graph_error(&mut builder, error);
                continue;
            }
            let crate_node_id = NodeId::Crate(crate_id.clone());
            if let Err(error) = builder.add_edge(metadata_edge(
                EdgeKind::Contains,
                target_node_id,
                crate_node_id,
                format!("metadata:crate:{target_key}"),
            )) {
                status = AnalysisStatus::Partial;
                add_graph_error(&mut builder, error);
            }

            let mut external_crates: std::collections::BTreeSet<String> = package
                .dependencies
                .iter()
                .map(|dependency| {
                    dependency
                        .rename
                        .as_deref()
                        .unwrap_or(&dependency.name)
                        .replace('-', "_")
                })
                .collect();
            external_crates.insert(package.name.replace('-', "_"));
            let module_status = discover_crate_modules(
                &mut builder,
                &workspace_root,
                &source_root,
                &crate_id,
                &package.edition,
                external_crates,
            );
            if module_status.status != AnalysisStatus::Complete {
                status = AnalysisStatus::Partial;
                dependency_status = AnalysisStatus::Partial;
            }
            if module_status.dependency_status != AnalysisStatus::Complete {
                dependency_status = AnalysisStatus::Partial;
            }
        }
    }
    for (source, manifest, dependencies) in pending_package_dependencies {
        for dependency in dependencies {
            let dependency_path = dependency
                .path
                .as_ref()
                .and_then(|path| path.canonicalize().ok());
            let candidates = package_locations.get(&dependency.name);
            let matching: Vec<_> = candidates
                .into_iter()
                .flatten()
                .filter(|(_, package_dir)| dependency_path.as_ref() == Some(package_dir))
                .collect();
            if let [(target, _)] = matching.as_slice() {
                let condition = dependency_condition(&dependency);
                let mut edge = metadata_edge(
                    EdgeKind::PackageDependency,
                    NodeId::Package(source.clone()),
                    NodeId::Package((*target).clone()),
                    format!(
                        "metadata:dependency:{}:{}:{}:{}:{}",
                        source.0,
                        dependency.name,
                        dependency.rename.as_deref().unwrap_or(""),
                        dependency.kind.as_deref().unwrap_or("normal"),
                        dependency.target.as_deref().unwrap_or("")
                    ),
                );
                edge.condition = condition.clone();
                edge.resolution = if condition.is_some() {
                    ResolutionStatus::Conditional
                } else {
                    ResolutionStatus::Confirmed
                };
                edge.syntax = Some(format!(
                    "{}{} [{}{}]",
                    dependency
                        .rename
                        .as_deref()
                        .map(|name| format!("{name} = "))
                        .unwrap_or_default(),
                    dependency.name,
                    dependency.kind.as_deref().unwrap_or("normal"),
                    dependency
                        .target
                        .as_deref()
                        .map(|t| format!("; {t}"))
                        .unwrap_or_default()
                ));
                edge.evidence[0].path = Some(manifest.clone());
                if let Err(error) = builder.add_edge(edge) {
                    status = AnalysisStatus::Partial;
                    add_graph_error(&mut builder, error);
                }
            } else {
                let reason = if matching.len() > 1
                    || candidates.is_some_and(|items| items.len() > 1) && dependency_path.is_none()
                {
                    "ambiguous workspace package name"
                } else {
                    "dependency package is outside workspace metadata"
                };
                let mut edge = metadata_edge_unresolved(
                    NodeId::Package(source.clone()),
                    format!(
                        "Cargo dependency {}{} ({})",
                        dependency
                            .rename
                            .as_deref()
                            .map(|r| format!("{r} → "))
                            .unwrap_or_default(),
                        dependency.name,
                        reason
                    ),
                    format!(
                        "metadata:dependency-unresolved:{}:{}:{}:{}:{}",
                        source.0,
                        dependency.name,
                        dependency.rename.as_deref().unwrap_or(""),
                        dependency.kind.as_deref().unwrap_or("normal"),
                        dependency.target.as_deref().unwrap_or("")
                    ),
                    dependency_condition(&dependency),
                    dependency.name.clone(),
                );
                edge.evidence[0].path = Some(manifest.clone());
                edge.syntax = Some(format!(
                    "{}{} [{}{}]",
                    dependency
                        .rename
                        .as_deref()
                        .map(|name| format!("{name} = "))
                        .unwrap_or_default(),
                    dependency.name,
                    dependency.kind.as_deref().unwrap_or("normal"),
                    dependency
                        .target
                        .as_deref()
                        .map(|t| format!("; {t}"))
                        .unwrap_or_default()
                ));
                if let Err(error) = builder.add_edge(edge) {
                    add_graph_error(&mut builder, error);
                }
                let mut diagnostic = Diagnostic::new(
                    if reason == "ambiguous workspace package name" {
                        "CARGO_DEPENDENCY_AMBIGUOUS"
                    } else {
                        "CROSS_CRATE_RESOLUTION_UNSUPPORTED"
                    },
                    if reason == "ambiguous workspace package name" {
                        Severity::Warning
                    } else {
                        Severity::Info
                    },
                    DiagnosticPhase::Discovery,
                    format!("{}: {}", reason, dependency.name),
                );
                diagnostic
                    .related_nodes
                    .push(NodeId::Package(source.clone()));
                builder.add_diagnostic(diagnostic);
                dependency_status = AnalysisStatus::Partial;
            }
        }
    }
    if !any_target && status == AnalysisStatus::Complete {
        status = AnalysisStatus::Partial;
        builder.add_diagnostic(Diagnostic::new(
            "NO_TARGETS",
            Severity::Warning,
            DiagnosticPhase::Discovery,
            "workspace contains no compilation targets",
        ));
    }
    builder.finish_scoped(status, dependency_status)
}

fn locate_manifest(
    input: &Path,
) -> Result<(PathBuf, PathBuf), (&'static str, String, Option<PathBuf>)> {
    let canonical = input.canonicalize().map_err(|error| {
        (
            "REPOSITORY_PATH_UNAVAILABLE",
            format!("repository path cannot be accessed ({})", error.kind()),
            None,
        )
    })?;
    let (manifest, selected_root) = if canonical.is_dir() {
        (canonical.join("Cargo.toml"), canonical)
    } else if canonical.is_file() {
        (
            canonical.clone(),
            canonical.parent().unwrap_or(Path::new(".")).to_path_buf(),
        )
    } else {
        return Err((
            "REPOSITORY_PATH_INVALID",
            "repository path is not a file or directory".into(),
            None,
        ));
    };
    if !manifest.is_file() {
        let rel = selected_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repository");
        return Err((
            "MANIFEST_NOT_FOUND",
            format!("Cargo.toml was not found at the selected repository root ({rel})"),
            Some(selected_root),
        ));
    }
    Ok((manifest, selected_root))
}

fn run_metadata(
    manifest: &Path,
    selected_root: &Path,
    options: &DiscoveryOptions,
) -> Result<CargoMetadata, (&'static str, String)> {
    let mut child = Command::new(&options.cargo_program)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
            "--locked",
            "--manifest-path",
        ])
        .arg(manifest)
        .current_dir(selected_root)
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_TERM_COLOR", "never")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            (
                "CARGO_METADATA_LAUNCH_FAILED",
                format!("could not start Cargo metadata ({})", error.kind()),
            )
        })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out_thread = thread::spawn(move || read_limited(stdout, MAX_STDOUT_BYTES));
    let err_thread = thread::spawn(move || read_limited(stderr, MAX_STDERR_BYTES));
    let started = Instant::now();
    let mut wait_error = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < options.timeout => {
                thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(error) => {
                wait_error = Some(error.kind());
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    if status.is_none() {
        if let Some(error_kind) = wait_error {
            return Err((
                "CARGO_METADATA_WAIT_FAILED",
                format!("could not monitor Cargo metadata process ({error_kind})"),
            ));
        }
        return Err((
            "CARGO_METADATA_TIMEOUT",
            format!(
                "Cargo metadata did not complete within {} seconds",
                options.timeout.as_secs_f64()
            ),
        ));
    }
    let status = status.expect("checked above");
    if !status.success() {
        let suffix = if stderr.truncated {
            " (stderr truncated)"
        } else {
            ""
        };
        return Err((
            "CARGO_METADATA_FAILED",
            format!(
                "cargo metadata failed with {}{suffix}",
                status
                    .code()
                    .map(|code| format!("exit code {code}"))
                    .unwrap_or_else(|| "a signal".into())
            ),
        ));
    }
    if stdout.truncated {
        return Err((
            "CARGO_METADATA_OUTPUT_LIMIT",
            "Cargo metadata output exceeded the 32 MiB safety limit".into(),
        ));
    }
    serde_json::from_slice(&stdout.bytes).map_err(|_| {
        (
            "CARGO_METADATA_INVALID_JSON",
            "Cargo metadata returned invalid or incomplete JSON".into(),
        )
    })
}

#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}
fn read_limited(mut reader: impl Read, limit: usize) -> Captured {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut truncated = false;
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let available = limit.saturating_sub(bytes.len());
                bytes.extend_from_slice(&buffer[..count.min(available)]);
                if count > available {
                    truncated = true;
                }
            }
        }
    }
    Captured { bytes, truncated }
}

fn add_repository_node(builder: &mut GraphBuilder, root: &Path) -> Node {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repository")
        .to_owned();
    let node = Node::Repository(RepositoryNode {
        id: RepositoryId::from("repository:root"),
        root: RepoPath::new(".").expect("root path"),
        name,
    });
    let _ = builder.add_node(node.clone());
    node
}

fn relative_path(root: &Path, path: &Path) -> Result<String, String> {
    let lexical = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if !lexical.starts_with(root) {
        return Err("a Cargo-reported path is outside the selected workspace".into());
    }
    let mut normalized = PathBuf::new();
    for component in lexical.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err("a Cargo-reported path escapes the workspace".into());
                }
                if !normalized.starts_with(root) {
                    return Err("a Cargo-reported path escapes the workspace".into());
                }
            }
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }
    let canonical = normalized
        .canonicalize()
        .map_err(|_| "a Cargo-reported path cannot be accessed".to_owned())?;
    if !canonical.starts_with(root) {
        return Err("a Cargo-reported source path is outside the selected workspace".to_owned());
    }
    let relative = normalized
        .strip_prefix(root)
        .map_err(|_| "a Cargo-reported source path is outside the selected workspace".to_owned())?;
    let path = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if path.is_empty() {
        return Ok(".".into());
    }
    Ok(path)
}

fn classify_target(kinds: &[String]) -> TargetKind {
    if kinds.iter().any(|kind| {
        matches!(
            kind.as_str(),
            "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
        )
    }) {
        TargetKind::Lib
    } else if kinds.iter().any(|kind| kind == "bin") {
        TargetKind::Bin
    } else if kinds.iter().any(|kind| kind == "example") {
        TargetKind::Example
    } else if kinds.iter().any(|kind| kind == "test") {
        TargetKind::Test
    } else if kinds.iter().any(|kind| kind == "bench") {
        TargetKind::Bench
    } else {
        TargetKind::Other
    }
}

fn metadata_edge(
    kind: EdgeKind,
    source: crate::model::NodeId,
    target: crate::model::NodeId,
    identity: String,
) -> Edge {
    Edge {
        id: EdgeId::from(identity),
        kind,
        source,
        target: Some(target),
        unresolved_target: None,
        resolution: crate::model::ResolutionStatus::Confirmed,
        condition: None,
        syntax: None,
        evidence: vec![Evidence {
            kind: EvidenceKind::PackageMetadata,
            provenance: Provenance::CargoMetadata,
            path: None,
            span: None,
        }],
    }
}

fn metadata_edge_unresolved(
    source: NodeId,
    description: String,
    identity: String,
    condition: Option<RelationshipCondition>,
    _dependency_name: String,
) -> Edge {
    Edge {
        id: EdgeId::from(identity),
        kind: EdgeKind::UnresolvedRelationship,
        source,
        target: None,
        unresolved_target: Some(description),
        resolution: if condition.is_some() {
            ResolutionStatus::Unsupported
        } else {
            ResolutionStatus::Unresolved
        },
        condition,
        syntax: None,
        evidence: vec![Evidence {
            kind: EvidenceKind::PackageMetadata,
            provenance: Provenance::CargoMetadata,
            path: None,
            span: None,
        }],
    }
}

fn dependency_condition(dependency: &CargoDependency) -> Option<RelationshipCondition> {
    let mut conditions = Vec::new();
    if dependency.optional {
        conditions.push(RelationshipCondition::Predicate {
            name: "optional_dependency".into(),
            value: Some(
                dependency
                    .rename
                    .as_deref()
                    .unwrap_or(&dependency.name)
                    .into(),
            ),
        });
    }
    if let Some(target) = &dependency.target {
        let target_condition = if target.starts_with("cfg(") {
            crate::modules::normalize_cfg_expression(target)
                .unwrap_or(RelationshipCondition::Unknown)
        } else {
            RelationshipCondition::Predicate {
                name: "target".into(),
                value: Some(target.clone()),
            }
        };
        conditions.push(target_condition);
    }
    match conditions.len() {
        0 => None,
        1 => conditions.pop(),
        _ => Some(RelationshipCondition::All(conditions)),
    }
}

fn add_graph_error(builder: &mut GraphBuilder, error: crate::model::GraphError) {
    builder.add_diagnostic(Diagnostic::new(
        "GRAPH_CONSTRUCTION_FAILED",
        Severity::Error,
        DiagnosticPhase::GraphConstruction,
        error.to_string(),
    ));
}

// Used by module traversal after constructing the repository-relative source path.
pub(crate) fn source_file_node(path: RepoPath) -> Node {
    Node::SourceFile(crate::model::SourceFileNode {
        id: crate::model::SourceFileId::from(format!("file:{}", path.as_str())),
        path,
        generated: GeneratedStatus::Unknown,
    })
}

pub(crate) fn module_edge(
    kind: EdgeKind,
    source: crate::model::NodeId,
    target: Option<crate::model::NodeId>,
    condition: Option<crate::model::RelationshipCondition>,
    evidence: Evidence,
    unresolved_target: Option<String>,
) -> Edge {
    let target_key = target
        .as_ref()
        .map(crate::model::NodeId::stable_string)
        .unwrap_or_else(|| {
            format!(
                "unresolved:{}",
                unresolved_target.as_deref().unwrap_or("unknown")
            )
        });
    let condition_key = condition
        .as_ref()
        .map(|condition| format!("::{condition:?}"))
        .unwrap_or_default();
    let identity = format!(
        "edge:{kind:?}:{}->{target_key}{condition_key}",
        source.stable_string()
    );
    Edge {
        id: EdgeId::from(identity),
        kind,
        source,
        target,
        unresolved_target,
        resolution: if kind == EdgeKind::UnresolvedRelationship {
            crate::model::ResolutionStatus::Unresolved
        } else if condition.as_ref().is_some_and(condition_contains_unknown) {
            crate::model::ResolutionStatus::Unsupported
        } else if condition.is_some() {
            crate::model::ResolutionStatus::Conditional
        } else {
            crate::model::ResolutionStatus::Confirmed
        },
        condition,
        syntax: None,
        evidence: vec![evidence],
    }
}

fn condition_contains_unknown(condition: &crate::model::RelationshipCondition) -> bool {
    use crate::model::RelationshipCondition;
    match condition {
        RelationshipCondition::Unknown => true,
        RelationshipCondition::All(items) | RelationshipCondition::Any(items) => {
            items.iter().any(condition_contains_unknown)
        }
        RelationshipCondition::Not(item) => condition_contains_unknown(item),
        RelationshipCondition::Constant(_) | RelationshipCondition::Predicate { .. } => false,
    }
}

pub(crate) fn diagnostic(
    code: &'static str,
    severity: Severity,
    phase: DiagnosticPhase,
    message: impl Into<String>,
    path: Option<RepoPath>,
    span: Option<crate::model::SourceSpan>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(code, severity, phase, message);
    diagnostic.path = path;
    diagnostic.span = span;
    diagnostic
}
