use std::{ffi::OsString, path::PathBuf};

use crate::query::{DependencyFamily, Direction};
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "atlas",
    version,
    about = "Explore Rust repository architecture with source evidence."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Summarize a repository's packages, targets, and architecture.
    Inspect(PathArgs),
    /// List modules in a repository.
    Modules(PathArgs),
    /// Show details for an architecture component.
    Component(ComponentArgs),
    /// Show direct dependencies for a component.
    Deps(DependencyArgs),
    /// Find a dependency path between two components.
    Path(PathQueryArgs),
    /// Report dependency cycles (structural observations, not defect diagnoses).
    Cycles(CycleArgs),
    /// Show repository analysis diagnostics.
    Diagnostics(PathArgs),
    /// Export the architecture graph as deterministic JSON.
    Export(PathArgs),
}

#[derive(Debug, Args)]
pub struct PathArgs {
    /// Repository directory or Cargo manifest path.
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,
    /// Output format for future analysis results.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
}

#[derive(Debug, Args)]
pub struct ComponentArgs {
    /// Stable Atlas node identifier.
    #[arg(value_name = "ID", value_parser = non_empty)]
    pub id: String,
    /// Repository directory or Cargo manifest path.
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
}

#[derive(Debug, Args)]
pub struct DependencyArgs {
    /// Stable Atlas node identifier.
    #[arg(value_name = "ID", value_parser = non_empty)]
    pub id: String,
    /// Repository directory or Cargo manifest path.
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
    /// Include package, import, re-export, and local module relationships.
    #[arg(long, value_enum, default_value_t = DependencyFamilyArg::All)]
    pub family: DependencyFamilyArg,
    /// Show incoming dependents instead of outgoing dependencies.
    #[arg(long)]
    pub reverse: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum DependencyFamilyArg {
    #[default]
    All,
    Modules,
    Packages,
}

#[derive(Debug, Args)]
pub struct PathQueryArgs {
    /// Stable Atlas source node identifier.
    #[arg(value_name = "FROM-ID", value_parser = non_empty)]
    pub from_id: String,
    /// Stable Atlas destination node identifier.
    #[arg(value_name = "TO-ID", value_parser = non_empty)]
    pub to_id: String,
    /// Repository directory or Cargo manifest path.
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,
    /// Output format for future analysis results.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
    /// Dependency graph to traverse.
    #[arg(long, value_enum, value_name = "FAMILY")]
    pub family: PathFamilyArg,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum PathFamilyArg {
    Modules,
    Packages,
}

#[derive(Debug, Args)]
pub struct CycleArgs {
    /// Repository directory or Cargo manifest path.
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,
    /// Dependency graph to inspect.
    #[arg(long, value_enum)]
    pub family: PathFamilyArg,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Human,
    Json,
}

fn non_empty(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Err("ID must not be empty".into())
    } else {
        Ok(value.into())
    }
}

/// Parse and dispatch an invocation. Clap uses 2 for usage errors, 0 for
/// help/version, and 3 denotes a valid but not-yet-implemented command.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    match cli.command {
        Command::Inspect(args) => run_supported("inspect", args.path, args.format),
        Command::Modules(args) => run_supported("modules", args.path, args.format),
        Command::Diagnostics(args) => run_supported("diagnostics", args.path, args.format),
        Command::Export(args) => run_supported("export", args.path, OutputFormat::Json),
        Command::Component(args) => run_component(args),
        Command::Deps(args) => run_dependencies(args),
        Command::Path(args) => run_path(args),
        Command::Cycles(args) => run_cycles(args),
    }
}

fn selected_family(family: PathFamilyArg) -> DependencyFamily {
    match family {
        PathFamilyArg::Modules => DependencyFamily::Modules,
        PathFamilyArg::Packages => DependencyFamily::Packages,
    }
}

fn run_path(args: PathQueryArgs) -> i32 {
    let analysis = crate::discovery::analyze_repository(&args.path);
    if analysis.status == crate::diagnostics::AnalysisStatus::Failed {
        return query_analysis_failed(&analysis, args.format);
    }
    match crate::query::dependency_path(
        &analysis,
        &args.from_id,
        &args.to_id,
        selected_family(args.family),
    ) {
        Ok(view) => {
            if args.format == OutputFormat::Json {
                match serde_json::to_string_pretty(&view) {
                    Ok(value) => println!("{value}"),
                    Err(error) => {
                        eprintln!("atlas: serialization failed: {error}");
                        return 1;
                    }
                }
            } else {
                println!(
                    "Dependency path ({:?}, {:?}; analysis {:?})",
                    view.family, view.status, view.analysis_status
                );
                if view.nodes.is_empty() {
                    println!("No path found in the supported graph.");
                    if view.absence_is_inconclusive {
                        println!(
                            "Dependency analysis is partial; absence is inconclusive. See `atlas diagnostics`."
                        );
                    }
                } else {
                    for (index, node) in view.nodes.iter().enumerate() {
                        println!(
                            "{}{}",
                            if index == 0 { "" } else { "  -> " },
                            node.stable_string()
                        );
                        if let Some(edge) = view.edges.get(index) {
                            for evidence in &edge.evidence {
                                if let Some(path) = &evidence.path {
                                    let loc = evidence
                                        .span
                                        .as_ref()
                                        .and_then(|s| s.start_line.zip(s.start_column))
                                        .map(|(l, c)| format!(":{l}:{c}"))
                                        .unwrap_or_default();
                                    println!(
                                        "       {:?} at {}{}{}",
                                        edge.kind,
                                        path.as_str(),
                                        loc,
                                        edge.condition
                                            .as_ref()
                                            .map(|c| format!(" when {c:?}"))
                                            .unwrap_or_default()
                                    );
                                }
                            }
                        }
                    }
                    if view.status == crate::query::PathStatus::Conditional {
                        println!("This path depends on configuration conditions.");
                    }
                }
            }
            0
        }
        Err(error) => {
            eprintln!("atlas: {error}");
            2
        }
    }
}

fn run_cycles(args: CycleArgs) -> i32 {
    let analysis = crate::discovery::analyze_repository(&args.path);
    if analysis.status == crate::diagnostics::AnalysisStatus::Failed {
        return query_analysis_failed(&analysis, args.format);
    }
    match crate::query::cycles(&analysis, selected_family(args.family)) {
        Ok(view) => {
            if args.format == OutputFormat::Json {
                match serde_json::to_string_pretty(&view) {
                    Ok(value) => println!("{value}"),
                    Err(error) => {
                        eprintln!("atlas: serialization failed: {error}");
                        return 1;
                    }
                }
            } else {
                println!(
                    "Cycles ({:?}; analysis {:?})",
                    view.family, view.analysis_status
                );
                if view.cycles.is_empty() {
                    println!("No cycles found in the supported graph.");
                    if view.dependency_status != crate::diagnostics::AnalysisStatus::Complete {
                        println!(
                            "Dependency analysis is partial; absence is inconclusive. See `atlas diagnostics`."
                        );
                    }
                }
                for (index, cycle) in view.cycles.iter().enumerate() {
                    println!(
                        "Cycle component {}{}",
                        index + 1,
                        if cycle.conditional {
                            " (configuration-dependent)"
                        } else {
                            ""
                        }
                    );
                    for node in &cycle.nodes {
                        println!("  {}", node.stable_string());
                    }
                    println!(
                        "  {} relationship(s); cycles are observations, not automatic defects.",
                        cycle.edges.len()
                    );
                }
            }
            0
        }
        Err(error) => {
            eprintln!("atlas: {error}");
            2
        }
    }
}

fn run_component(args: ComponentArgs) -> i32 {
    let analysis = crate::discovery::analyze_repository(&args.path);
    if analysis.status == crate::diagnostics::AnalysisStatus::Failed {
        return query_analysis_failed(&analysis, args.format);
    }
    match crate::query::component(&analysis, &args.id) {
        Ok(view) => {
            if args.format == OutputFormat::Json {
                match serde_json::to_string_pretty(&view) {
                    Ok(value) => println!("{value}"),
                    Err(error) => {
                        eprintln!("atlas: serialization failed: {error}");
                        return 1;
                    }
                }
            } else {
                print_component(&view);
            }
            if analysis.status == crate::diagnostics::AnalysisStatus::Failed {
                4
            } else {
                0
            }
        }
        Err(error) => {
            eprintln!("atlas: {error}");
            2
        }
    }
}

fn run_dependencies(args: DependencyArgs) -> i32 {
    let analysis = crate::discovery::analyze_repository(&args.path);
    if analysis.status == crate::diagnostics::AnalysisStatus::Failed {
        return query_analysis_failed(&analysis, args.format);
    }
    let family = match args.family {
        DependencyFamilyArg::All => DependencyFamily::All,
        DependencyFamilyArg::Modules => DependencyFamily::Modules,
        DependencyFamilyArg::Packages => DependencyFamily::Packages,
    };
    let direction = if args.reverse {
        Direction::Incoming
    } else {
        Direction::Outgoing
    };
    match crate::query::dependencies(&analysis, &args.id, family, direction) {
        Ok(view) => {
            if args.format == OutputFormat::Json {
                match serde_json::to_string_pretty(&view) {
                    Ok(value) => println!("{value}"),
                    Err(error) => {
                        eprintln!("atlas: serialization failed: {error}");
                        return 1;
                    }
                }
            } else {
                print_dependencies(&view);
            }
            if analysis.status == crate::diagnostics::AnalysisStatus::Failed {
                4
            } else {
                0
            }
        }
        Err(error) => {
            eprintln!("atlas: {error}");
            2
        }
    }
}

fn query_analysis_failed(result: &crate::analysis::AnalysisResult, format: OutputFormat) -> i32 {
    if format == OutputFormat::Json {
        match crate::serialization::to_json(result) {
            Ok(value) => println!("{value}"),
            Err(error) => {
                eprintln!("atlas: serialization failed: {error}");
                return 1;
            }
        }
    } else {
        print_diagnostics(result);
    }
    4
}

fn print_component(view: &crate::query::ComponentView) {
    use crate::model::Node;
    println!("Component: {}", view.component.id().stable_string());
    println!(
        "Kind: {:?}; analysis: {:?} (structure {:?}, dependencies {:?})",
        view.component.id().kind(),
        view.analysis_status,
        view.structural_status,
        view.dependency_status
    );
    if let Node::Definition(definition) = &view.component {
        println!("Definition: {} ({:?})", definition.name, definition.kind);
    }
    print_edges("Parents", &view.parents);
    print_edges("Children", &view.children);
    println!("Definitions:");
    if view.definitions.is_empty() {
        println!("  (none)");
    }
    for node in &view.definitions {
        if let Node::Definition(definition) = node {
            let evidence = view
                .definition_evidence
                .iter()
                .filter(|edge| edge.target.as_ref() == Some(&node.id()))
                .flat_map(|edge| &edge.evidence)
                .map(|item| {
                    let path = item.path.as_ref().map(|path| path.as_str()).unwrap_or("?");
                    let location = item
                        .span
                        .as_ref()
                        .and_then(|span| span.start_line.zip(span.start_column))
                        .map(|(line, column)| format!(":{line}:{column}"))
                        .unwrap_or_default();
                    format!("{path}{location}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "  {} ({:?}) [{}]{}",
                definition.name,
                definition.kind,
                node.id().stable_string(),
                if evidence.is_empty() {
                    String::new()
                } else {
                    format!(" — {evidence}")
                }
            );
        }
    }
    print_edges("Dependencies", &view.dependencies);
    print_edges("Dependents", &view.dependents);
    print_edges("Unresolved", &view.unresolved);
    print_relevant_diagnostics(&view.diagnostics);
}

fn print_dependencies(view: &crate::query::DependenciesView) {
    println!(
        "{:?} {:?} for {} (analysis {:?}; structure {:?}; dependencies {:?})",
        view.direction,
        view.family,
        view.component.id().stable_string(),
        view.analysis_status,
        view.structural_status,
        view.dependency_status
    );
    print_edges("Relationships", &view.relationships);
    print_edges("Unresolved", &view.unresolved);
    print_relevant_diagnostics(&view.diagnostics);
    if view.relationships.is_empty() && view.unresolved.is_empty() {
        println!(
            "No supported relationships found; inspect analysis_status and diagnostics before concluding none exist."
        );
    }
}

fn print_edges(label: &str, edges: &[crate::model::Edge]) {
    println!("{label}:");
    if edges.is_empty() {
        println!("  (none)");
        return;
    }
    for edge in edges {
        let target = edge
            .target
            .as_ref()
            .map(crate::model::NodeId::stable_string)
            .or_else(|| edge.unresolved_target.clone())
            .unwrap_or_default();
        let evidence = edge
            .evidence
            .iter()
            .filter_map(|e| {
                e.path.as_ref().map(|path| {
                    let location = e
                        .span
                        .as_ref()
                        .and_then(|span| span.start_line.zip(span.start_column))
                        .map(|(line, col)| format!(":{line}:{col}"))
                        .unwrap_or_default();
                    format!("{}{}", path.as_str(), location)
                })
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  {} --{:?}--> {} [{:?}]{}{}",
            edge.source.stable_string(),
            edge.kind,
            target,
            edge.resolution,
            edge.syntax
                .as_ref()
                .map(|s| format!(" `{s}`"))
                .unwrap_or_default(),
            if evidence.is_empty() {
                String::new()
            } else {
                format!(" — {evidence}")
            }
        );
    }
}

fn print_relevant_diagnostics(diagnostics: &[crate::diagnostics::Diagnostic]) {
    if diagnostics.is_empty() {
        return;
    }
    println!("Relevant diagnostics:");
    for diagnostic in diagnostics {
        let path = diagnostic
            .path
            .as_ref()
            .map(|value| format!("{}: ", value.as_str()))
            .unwrap_or_default();
        println!(
            "  {:?} {}: {}{}",
            diagnostic.severity, diagnostic.code.0, path, diagnostic.message
        );
    }
}

fn run_supported(command: &str, path: PathBuf, format: OutputFormat) -> i32 {
    let result = crate::discovery::analyze_repository(path);
    if format == OutputFormat::Json {
        match crate::serialization::to_json(&result) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("atlas: could not serialize analysis result: {error}");
                return 1;
            }
        }
    } else {
        match command {
            "inspect" => print_inspect(&result),
            "modules" => print_modules(&result),
            "diagnostics" => print_diagnostics(&result),
            _ => print_inspect(&result),
        }
    }
    if result.status == crate::diagnostics::AnalysisStatus::Failed {
        4
    } else {
        0
    }
}

fn print_inspect(result: &crate::analysis::AnalysisResult) {
    use crate::model::Node;
    let count = |predicate: fn(&Node) -> bool| {
        result
            .graph
            .nodes
            .iter()
            .filter(|node| predicate(node))
            .count()
    };
    if let Some(Node::Repository(repository)) = result
        .graph
        .nodes
        .iter()
        .find(|node| matches!(node, Node::Repository(_)))
    {
        println!(
            "Repository: {} ({})",
            repository.name,
            repository.root.as_str()
        );
    }
    println!(
        "Analysis: {:?} (structural {:?}; dependency {:?}; supported scope only)",
        result.status, result.structural_status, result.dependency_status
    );
    println!("Packages: {}", count(|n| matches!(n, Node::Package(_))));
    println!("Targets: {}", count(|n| matches!(n, Node::Target(_))));
    println!("Crates: {}", count(|n| matches!(n, Node::Crate(_))));
    println!("Modules: {}", count(|n| matches!(n, Node::Module(_))));
    println!(
        "Source files: {}",
        count(|n| matches!(n, Node::SourceFile(_)))
    );
    println!(
        "Definitions: {}",
        count(|n| matches!(n, Node::Definition(_)))
    );
    let edge_count = |kind| {
        result
            .graph
            .edges
            .iter()
            .filter(|edge| edge.kind == kind)
            .count()
    };
    println!(
        "Import declarations: {}",
        edge_count(crate::model::EdgeKind::Imports) + edge_count(crate::model::EdgeKind::Reexports)
    );
    println!(
        "Resolved local module references: {}",
        edge_count(crate::model::EdgeKind::ReferencesModule)
    );
    println!(
        "Cargo package dependencies: {}",
        edge_count(crate::model::EdgeKind::PackageDependency)
    );
    println!(
        "Unresolved relationships: {}",
        edge_count(crate::model::EdgeKind::UnresolvedRelationship)
    );
    println!("Diagnostics: {}", result.graph.diagnostics.len());
    print_diagnostic_summary(&result.graph.diagnostics);
}

fn print_modules(result: &crate::analysis::AnalysisResult) {
    use crate::model::Node;
    println!("Modules ({:?} analysis)", result.status);
    for node in &result.graph.nodes {
        if let Node::Module(module) = node {
            let depth = module.logical_path.matches("::").count();
            println!(
                "{}{} [{}]",
                "  ".repeat(depth),
                module.logical_path,
                Node::Module(module.clone()).id().stable_string()
            );
        }
    }
    if !result.graph.diagnostics.is_empty() {
        println!(
            "{} diagnostic(s); run `atlas diagnostics` for details.",
            result.graph.diagnostics.len()
        );
    }
}

fn print_diagnostics(result: &crate::analysis::AnalysisResult) {
    println!(
        "Analysis: {:?} (structural {:?}; dependency {:?}; supported scope only)",
        result.status, result.structural_status, result.dependency_status
    );
    if result.graph.diagnostics.is_empty() {
        println!("No diagnostics.");
        return;
    }
    print_diagnostic_summary(&result.graph.diagnostics);
}

fn print_diagnostic_summary(diagnostics: &[crate::diagnostics::Diagnostic]) {
    use std::collections::BTreeMap;
    let mut groups = BTreeMap::<
        (&str, crate::diagnostics::Severity),
        Vec<&crate::diagnostics::Diagnostic>,
    >::new();
    for diagnostic in diagnostics {
        groups
            .entry((&diagnostic.code.0, diagnostic.severity))
            .or_default()
            .push(diagnostic);
    }
    for ((code, severity), items) in groups {
        let sample = items[0];
        println!(
            "  {:?} {} ({} occurrence(s)): {}{}",
            severity,
            code,
            items.len(),
            diagnostic_location(sample),
            sample.message
        );
        if items.len() > 1 {
            println!("    Use `atlas export` for every diagnostic with source evidence.");
        }
    }
}

fn diagnostic_location(diagnostic: &crate::diagnostics::Diagnostic) -> String {
    let Some(path) = diagnostic.path.as_ref() else {
        return String::new();
    };
    let mut location = path.as_str().to_owned();
    if let Some(span) = diagnostic.span.as_ref()
        && let (Some(line), Some(column)) = (span.start_line, span.start_column)
    {
        location.push_str(&format!(":{line}:{column}"));
    }
    format!("{location}: ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    #[test]
    fn clap_contract_is_valid() {
        Cli::command().debug_assert();
    }
    #[test]
    fn blank_id_is_rejected() {
        assert!(Cli::try_parse_from(["atlas", "component", " "]).is_err());
    }
}
