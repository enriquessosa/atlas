use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use atlas::{
    AnalysisStatus, DiscoveryOptions, analyze_repository, analyze_repository_with,
    model::{EdgeKind, Node, ResolutionStatus},
    serialization::to_json,
};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("atlas-fixture-{}-{serial}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn write(&self, path: impl AsRef<Path>, content: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn package(&self, name: &str, root: &str) {
        self.write(
            format!("{root}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        );
        self.write(format!("{root}/src/lib.rs"), "pub fn root() {}\n");
    }

    fn analyze(&self) -> atlas::AnalysisResult {
        analyze_repository(&self.0)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn nodes(result: &atlas::AnalysisResult, predicate: impl Fn(&Node) -> bool) -> Vec<&Node> {
    result
        .graph
        .nodes
        .iter()
        .filter(|node| predicate(node))
        .collect()
}

fn diagnostic_codes(result: &atlas::AnalysisResult) -> Vec<&str> {
    result
        .graph
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.0.as_str())
        .collect()
}

#[test]
fn single_package_and_virtual_multi_package_workspaces_are_discovered() {
    let single = Fixture::new();
    single.package("single", ".");
    let result = single.analyze();
    assert_eq!(result.status, AnalysisStatus::Complete);
    assert_eq!(nodes(&result, |n| matches!(n, Node::Package(_))).len(), 1);
    assert_eq!(nodes(&result, |n| matches!(n, Node::Target(_))).len(), 1);

    let workspace = Fixture::new();
    workspace.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"3\"\n",
    );
    workspace.package("a", "crates/a");
    workspace.package("b", "crates/b");
    let result = workspace.analyze();
    assert_eq!(result.status, AnalysisStatus::Complete);
    assert_eq!(nodes(&result, |n| matches!(n, Node::Package(_))).len(), 2);
    assert_eq!(nodes(&result, |n| matches!(n, Node::Crate(_))).len(), 2);
}

#[test]
fn multiple_cargo_targets_are_discovered_without_scanning_unrelated_rust() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"targets\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    for path in [
        "src/lib.rs",
        "src/main.rs",
        "src/bin/tool.rs",
        "examples/example.rs",
        "tests/integration.rs",
        "benches/bench.rs",
        "unrelated/ignored.rs",
    ] {
        fixture.write(path, "pub fn item() {}\n");
    }
    let result = fixture.analyze();
    assert_eq!(result.status, AnalysisStatus::Complete);
    let targets = nodes(&result, |n| matches!(n, Node::Target(_)));
    assert_eq!(targets.len(), 6);
    assert!(
        nodes(
            &result,
            |n| matches!(n, Node::SourceFile(file) if file.path.as_str() == "unrelated/ignored.rs")
        )
        .is_empty()
    );
}

#[test]
fn targets_with_a_shared_source_keep_crate_context_but_share_the_file_node() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"shared\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[lib]\npath=\"src/shared.rs\"\n[[bin]]\nname=\"shared-bin\"\npath=\"src/shared.rs\"\n",
    );
    fixture.write("src/shared.rs", "pub mod nested;\n");
    fixture.write("src/nested.rs", "pub fn item() {}\n");
    let result = fixture.analyze();
    assert_eq!(
        result.status,
        AnalysisStatus::Complete,
        "{:?}",
        result.graph.diagnostics
    );
    assert_eq!(
        nodes(&result, |node| matches!(node, Node::Target(_))).len(),
        2
    );
    assert_eq!(
        nodes(
            &result,
            |node| matches!(node, Node::SourceFile(file) if file.path.as_str() == "src/shared.rs")
        )
        .len(),
        1
    );
    assert_eq!(
        nodes(
            &result,
            |node| matches!(node, Node::Module(module) if module.logical_path == "crate::nested")
        )
        .len(),
        2
    );
}

#[test]
fn inline_external_legacy_and_path_attribute_modules_are_followed() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"modules\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    fixture.write("src/lib.rs", "pub mod inline { pub mod child; }\npub mod normal;\npub mod legacy;\n#[path = \"custom/name.rs\"] pub mod custom;\npub mod nested { #[path = \"alternate.rs\"] pub mod chosen; }\n");
    fixture.write("src/inline/child.rs", "pub fn child() {}\n");
    fixture.write("src/normal.rs", "pub mod leaf;\n");
    fixture.write("src/normal/leaf.rs", "pub fn leaf() {}\n");
    fixture.write("src/legacy/mod.rs", "pub mod leaf;\n");
    fixture.write("src/legacy/leaf.rs", "pub fn legacy_leaf() {}\n");
    fixture.write("src/custom/name.rs", "pub mod leaf;\n");
    fixture.write("src/custom/leaf.rs", "pub fn custom_leaf() {}\n");
    fixture.write("src/nested/alternate.rs", "pub fn chosen() {}\n");
    let result = fixture.analyze();
    assert_eq!(
        result.status,
        AnalysisStatus::Complete,
        "{:?}",
        result.graph.diagnostics
    );
    let modules: Vec<_> = nodes(&result, |n| matches!(n, Node::Module(_)))
        .into_iter()
        .map(|n| match n {
            Node::Module(m) => m.logical_path.as_str(),
            _ => unreachable!(),
        })
        .collect();
    for expected in [
        "crate::inline",
        "crate::inline::child",
        "crate::normal::leaf",
        "crate::legacy::leaf",
        "crate::custom::leaf",
        "crate::nested::chosen",
    ] {
        assert!(
            modules.contains(&expected),
            "missing {expected}: {modules:?}"
        );
    }
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::LoadsSource
                && edge.evidence.iter().any(|e| e
                    .path
                    .as_ref()
                    .is_some_and(|p| p.as_str() == "src/custom/name.rs")))
    );
}

#[test]
fn symbolic_conditions_propagate_and_conditional_path_is_not_guessed() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"conditions\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    fixture.write("src/lib.rs", "#[cfg(any(unix, feature = \"extra\"))]\npub mod conditional;\n#[cfg_attr(feature = \"linux-path\", cfg_attr(unix, path = \"linux.rs\"))]\npub mod platform;\n");
    fixture.write("src/conditional.rs", "pub mod nested;\n");
    fixture.write("src/conditional/nested.rs", "pub fn nested() {}\n");
    fixture.write("src/platform.rs", "pub fn fallback() {}\n");
    fixture.write("src/linux.rs", "pub fn linux() {}\n");
    let result = fixture.analyze();
    assert_eq!(result.status, AnalysisStatus::Partial);
    assert!(diagnostic_codes(&result).contains(&"CONDITIONAL_MODULE_PATH_AMBIGUOUS"));
    let conditional = result
        .graph
        .edges
        .iter()
        .find(|edge| {
            edge.kind == EdgeKind::DeclaresModule && edge.evidence.iter().any(|e| e.span.is_some())
        })
        .unwrap();
    assert_eq!(conditional.resolution, ResolutionStatus::Conditional);
    let nested = result
        .graph
        .edges
        .iter()
        .find(|edge| {
            edge.kind == EdgeKind::DeclaresModule
                && edge
                    .source
                    .stable_string()
                    .contains("module:crate::conditional")
        })
        .unwrap();
    assert_eq!(nested.resolution, ResolutionStatus::Conditional);
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::UnresolvedRelationship
                && edge.unresolved_target.as_deref() == Some("module platform"))
    );
}

#[test]
fn missing_conflicting_and_invalid_sources_preserve_known_facts() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"partial\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    fixture.write(
        "src/lib.rs",
        "pub mod missing;\npub mod conflict;\npub mod invalid;\npub mod okay;\n",
    );
    fixture.write("src/conflict.rs", "pub fn a() {}\n");
    fixture.write("src/conflict/mod.rs", "pub fn b() {}\n");
    fixture.write("src/invalid.rs", "pub mod broken {\n");
    fixture.write("src/okay.rs", "pub fn okay() {}\n");
    let result = fixture.analyze();
    assert_eq!(result.status, AnalysisStatus::Partial);
    for code in [
        "MODULE_FILE_MISSING",
        "MODULE_FILE_CONFLICT",
        "RUST_PARSE_FAILED",
    ] {
        assert!(
            diagnostic_codes(&result).contains(&code),
            "missing {code}: {:?}",
            diagnostic_codes(&result)
        );
    }
    assert!(
        nodes(
            &result,
            |node| matches!(node, Node::Module(m) if m.logical_path == "crate::okay")
        )
        .len()
            == 1
    );
    assert!(
        nodes(
            &result,
            |node| matches!(node, Node::SourceFile(f) if f.path.as_str() == "src/invalid.rs")
        )
        .len()
            == 1
    );
}

#[cfg(unix)]
#[test]
fn symlink_boundary_and_source_cycles_are_reported() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"links\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    fixture.write(
        "src/lib.rs",
        "pub mod outside;\npub mod recursive;\npub mod alias;\n",
    );
    fixture.write(
        "src/recursive.rs",
        "#[path = \"lib.rs\"]\npub mod back_to_root;\n",
    );
    fixture.write("src/real/file.rs", "pub mod child;\n");
    fixture.write("src/alias/child.rs", "pub fn lexical_child() {}\n");
    fixture.write("src/real/file/child.rs", "pub fn physical_child() {}\n");
    let external = Fixture::new();
    external.write("outside.rs", "pub fn outside() {}\n");
    symlink(
        external.0.join("outside.rs"),
        fixture.0.join("src/outside.rs"),
    )
    .unwrap();
    symlink(
        fixture.0.join("src/real/file.rs"),
        fixture.0.join("src/alias.rs"),
    )
    .unwrap();
    let result = fixture.analyze();
    assert_eq!(result.status, AnalysisStatus::Partial);
    assert!(diagnostic_codes(&result).contains(&"SOURCE_OUTSIDE_REPOSITORY"));
    assert!(diagnostic_codes(&result).contains(&"MODULE_SOURCE_CYCLE"));
    assert!(nodes(&result, |node| matches!(node, Node::Module(module) if module.logical_path == "crate::alias::child")).len() == 1);
    assert!(result.graph.nodes.iter().all(|node| match node {
        Node::SourceFile(file) => !Path::new(file.path.as_str()).is_absolute(),
        _ => true,
    }));
}

#[cfg(unix)]
#[test]
fn shared_physical_source_is_revisited_for_distinct_module_directories() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"contexts\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    fixture.write("src/lib.rs", "#[cfg(unix)]\n#[path = \"a/shared.rs\"]\nmod shared;\n#[cfg(windows)]\n#[path = \"b/shared.rs\"]\nmod shared;\n");
    fixture.write("src/shared_source.rs", "mod child;\n");
    fixture.write("src/a/child.rs", "pub fn a() {}\n");
    fixture.write("src/b/child.rs", "pub fn b() {}\n");
    symlink(
        fixture.0.join("src/shared_source.rs"),
        fixture.0.join("src/a/shared.rs"),
    )
    .unwrap();
    symlink(
        fixture.0.join("src/shared_source.rs"),
        fixture.0.join("src/b/shared.rs"),
    )
    .unwrap();
    let result = fixture.analyze();
    assert_eq!(
        result.status,
        AnalysisStatus::Complete,
        "{:?}",
        result.graph.diagnostics
    );
    let mappings: Vec<_> = result
        .graph
        .edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::LoadsSource
                && edge
                    .source
                    .stable_string()
                    .contains("module:crate::shared::child")
        })
        .collect();
    assert_eq!(mappings.len(), 2);
    assert!(mappings.iter().any(|edge| {
        edge.target
            .as_ref()
            .is_some_and(|target| target.stable_string().ends_with("src/a/child.rs"))
    }));
    assert!(mappings.iter().any(|edge| {
        edge.target
            .as_ref()
            .is_some_and(|target| target.stable_string().ends_with("src/b/child.rs"))
    }));
}

#[test]
fn failure_and_offline_behavior_are_structured_without_lockfile_mutation() {
    let missing = std::env::temp_dir().join(format!("atlas-no-such-{}", std::process::id()));
    let failed = analyze_repository(missing);
    assert_eq!(failed.status, AnalysisStatus::Failed);
    assert!(diagnostic_codes(&failed).contains(&"REPOSITORY_PATH_UNAVAILABLE"));

    let fixture = Fixture::new();
    fixture.package("offline", ".");
    fixture.write("Cargo.toml", "this is not valid Cargo TOML =\n");
    let options = DiscoveryOptions {
        timeout: Duration::from_secs(5),
        ..DiscoveryOptions::default()
    };
    let failed = analyze_repository_with(&fixture.0, &options);
    assert_eq!(failed.status, AnalysisStatus::Failed);
    assert!(
        diagnostic_codes(&failed).contains(&"CARGO_METADATA_FAILED"),
        "{:?}",
        diagnostic_codes(&failed)
    );
    assert!(!fixture.0.join("Cargo.lock").exists());

    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"offline\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    let success = fixture.analyze();
    assert_eq!(success.status, AnalysisStatus::Complete);
    assert!(!fixture.0.join("Cargo.lock").exists());
}

#[cfg(unix)]
#[test]
fn cargo_metadata_subprocess_timeout_is_bounded() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.package("timeout", ".");
    let shim = fixture.0.join("fake-cargo");
    fs::write(&shim, "#!/bin/sh\nexec sleep 3\n").unwrap();
    let mut permissions = fs::metadata(&shim).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&shim, permissions).unwrap();
    let options = DiscoveryOptions {
        cargo_program: shim.into_os_string(),
        timeout: Duration::from_millis(250),
    };
    let started = std::time::Instant::now();
    let result = analyze_repository_with(&fixture.0, &options);
    assert_eq!(result.status, AnalysisStatus::Failed);
    assert!(diagnostic_codes(&result).contains(&"CARGO_METADATA_TIMEOUT"));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn spans_use_source_byte_offsets_with_utf8_and_crlf() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"spans\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    let source = "// π\r\nmod child;\r\n";
    fixture.write("src/lib.rs", source);
    fixture.write("src/child.rs", "pub fn child() {}\n");
    let result = fixture.analyze();
    let edge = result
        .graph
        .edges
        .iter()
        .find(|edge| edge.kind == EdgeKind::DeclaresModule)
        .unwrap();
    let span = edge.evidence[0].span.as_ref().unwrap();
    assert_eq!(span.start_byte as usize, source.find("mod child").unwrap());
    assert_eq!(span.start_line, Some(2));
}

#[test]
fn repeated_analyses_have_identical_json_and_no_absolute_paths() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname=\"stable\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    fixture.write("src/lib.rs", "pub mod inner;\n");
    fixture.write("src/inner.rs", "pub fn value() {}\n");
    let first = to_json(&fixture.analyze()).unwrap();
    let second = to_json(&fixture.analyze()).unwrap();
    assert_eq!(first, second);
    assert!(!first.contains(fixture.0.to_string_lossy().as_ref()));
}
