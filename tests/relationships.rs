use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use atlas::{AnalysisStatus, EdgeKind, Node, analyze_repository};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("atlas-rel-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn package(&self, name: &str) {
        self.write(
            "Cargo.toml",
            &format!("[package]\nname = \"{name}\"\nversion=\"0.1.0\"\nedition=\"2021\"\n"),
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct ExternalFixture(PathBuf);
impl Drop for ExternalFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn definitions(result: &atlas::AnalysisResult) -> Vec<&atlas::model::DefinitionNode> {
    result
        .graph
        .nodes
        .iter()
        .filter_map(|node| {
            if let Node::Definition(def) = node {
                Some(def)
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn definitions_are_module_scoped_and_cover_supported_item_kinds() {
    let fixture = Fixture::new();
    fixture.package("definitions");
    fixture.write("src/lib.rs", "pub fn root() {}\npub struct Root;\npub enum Choice { A }\npub union Raw { value: u32 }\npub trait Contract { fn required(&self); type Output; }\npub type Alias = u8;\npub const LIMIT: usize = 5;\npub static GLOBAL: u8 = 1;\nmod left { pub fn same() {} }\nmod right { pub fn same() {} }\nimpl Root { pub fn method(&self) {} const VALUE: u8 = 1; type Assoc = u8; }\nmacro_rules! helper { () => {} }\n");
    let result = analyze_repository(&fixture.0);
    let defs = definitions(&result);
    for name in [
        "root",
        "Root",
        "Choice",
        "Raw",
        "Contract",
        "Alias",
        "LIMIT",
        "GLOBAL",
        "helper",
        "fn required",
        "type Output",
    ] {
        assert!(defs.iter().any(|def| def.name == name), "missing {name}");
    }
    let same: Vec<_> = defs.iter().filter(|def| def.name == "same").collect();
    assert_eq!(same.len(), 2);
    assert_ne!(same[0].id, same[1].id);
    assert!(
        defs.iter()
            .any(|def| def.kind == atlas::model::DefinitionKind::Implementation)
    );
    assert!(
        defs.iter()
            .any(|def| def.kind == atlas::model::DefinitionKind::AssociatedItem)
    );
    assert!(
        result
            .graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Contains
                && edge
                    .target
                    .as_ref()
                    .is_some_and(|id| matches!(id, atlas::model::NodeId::Definition(_))))
            .all(|edge| edge.evidence.iter().any(|e| e.span.is_some()))
    );
}

#[test]
fn imports_groups_aliases_globs_reexports_and_explicit_module_paths_are_retained() {
    let fixture = Fixture::new();
    fixture.package("imports");
    fixture.write("src/lib.rs", "pub mod a; pub mod b;\nuse crate::a::{self, Thing as T, nested::*};\nuse crate::a as alias; use alias::Thing;\npub use crate::b::B as PublicB;\npub(crate) use crate::b::B as CrateB;\nuse ::std::fmt::Display;\nmod child { use super::a::Thing; use crate::b::B; }\n");
    fixture.write(
        "src/a.rs",
        "pub struct Thing; pub mod nested { pub fn inner() {} }\n",
    );
    fixture.write("src/b.rs", "pub struct B;\n");
    let result = analyze_repository(&fixture.0);
    let imports: Vec<_> = result
        .graph
        .edges
        .iter()
        .filter(|edge| matches!(edge.kind, EdgeKind::Imports | EdgeKind::Reexports))
        .collect();
    assert!(imports.iter().any(|edge| edge.kind == EdgeKind::Imports
        && edge.syntax.as_deref() == Some("crate::a::Thing as T")));
    assert!(imports.iter().any(|edge| edge.kind == EdgeKind::Reexports
        && edge.syntax.as_deref() == Some("crate::b::B as PublicB")));
    assert!(imports.iter().any(|edge| edge.kind == EdgeKind::Reexports
        && edge.syntax.as_deref() == Some("crate::b::B as CrateB")));
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.syntax.as_deref() == Some("crate::a::self"))
    );
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.syntax.as_deref() == Some("::std::fmt::Display"))
    );
    assert!(
        imports
            .iter()
            .any(|edge| edge.syntax.as_deref() == Some("crate::a::nested::*"))
    );
    assert!(imports.iter().any(|edge| {
        edge.target
            .as_ref()
            .is_some_and(|target| matches!(target, atlas::model::NodeId::Definition(_)))
    }));
    assert!(result.graph.edges.iter().any(|edge| {
        edge.kind == EdgeKind::ReferencesModule
            && edge
                .target
                .as_ref()
                .is_some_and(|target| matches!(target, atlas::model::NodeId::Module(_)))
    }));
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::UnresolvedRelationship
                && edge.syntax.as_deref() == Some("::std::fmt::Display"))
    );
    assert!(
        result
            .graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Imports)
            .all(|edge| edge.evidence.iter().all(|e| e.span.is_some()))
    );
    assert_eq!(result.status, AnalysisStatus::Partial);
    assert_eq!(result.structural_status, AnalysisStatus::Complete);
    assert_eq!(result.dependency_status, AnalysisStatus::Partial);
}

#[test]
fn repeated_imports_merge_evidence_and_cfg_is_preserved() {
    let fixture = Fixture::new();
    fixture.package("conditions");
    fixture.write("src/lib.rs", "pub mod child;\n#[cfg(feature = \"x\")] use crate::child;\nuse crate::child;\nuse crate::child;\n");
    fixture.write("src/child.rs", "#[cfg(unix)] pub fn item() {}\n");
    let first = analyze_repository(&fixture.0);
    let second = analyze_repository(&fixture.0);
    assert_eq!(
        atlas::serialization::to_json(&first).unwrap(),
        atlas::serialization::to_json(&second).unwrap()
    );
    let module_import = first
        .graph
        .edges
        .iter()
        .find(|edge| {
            edge.kind == EdgeKind::Imports
                && edge.syntax.as_deref() == Some("crate::child")
                && edge.condition.is_none()
        })
        .unwrap();
    assert_eq!(module_import.evidence.len(), 2);
    let conditional = first
        .graph
        .edges
        .iter()
        .find(|edge| edge.kind == EdgeKind::Imports && edge.condition.is_some())
        .unwrap();
    assert_eq!(
        conditional.resolution,
        atlas::model::ResolutionStatus::Conditional
    );
    assert!(definitions(&first).iter().any(|def| def.name == "item"));
    let item = first
        .graph
        .nodes
        .iter()
        .find(|node| matches!(node, Node::Definition(definition) if definition.name == "item"))
        .unwrap();
    let definition_edge = first
        .graph
        .edges
        .iter()
        .find(|edge| edge.kind == EdgeKind::Contains && edge.target.as_ref() == Some(&item.id()))
        .unwrap();
    assert!(definition_edge.condition.is_some());
}

#[test]
fn mutually_conditional_module_alternatives_are_not_resolved_as_unconditional() {
    let fixture = Fixture::new();
    fixture.package("cfg-alternatives");
    fixture.write(
        "src/lib.rs",
        "#[cfg(unix)] mod choice;\n#[cfg(windows)] mod choice;\nuse crate::choice;\n",
    );
    fixture.write("src/choice.rs", "pub struct Choice;\n");
    let result = analyze_repository(&fixture.0);
    assert!(
        result
            .graph
            .diagnostics
            .iter()
            .any(|d| d.code.0 == "CONDITIONAL_RESOLUTION_AMBIGUOUS")
    );
    assert!(
        !result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::ReferencesModule
                && edge.syntax.as_deref() == Some("crate::choice"))
    );
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::UnresolvedRelationship
                && edge.syntax.as_deref() == Some("crate::choice"))
    );
}

#[test]
fn definition_and_import_spans_use_source_bytes_with_unicode_and_crlf() {
    let fixture = Fixture::new();
    fixture.package("evidence");
    let source = "// π\r\npub mod child;\r\nuse crate::child;\r\npub fn root() {}\r\n";
    fixture.write("src/lib.rs", source);
    fixture.write("src/child.rs", "pub struct Child;\r\n");
    let result = analyze_repository(&fixture.0);
    let function = result
        .graph
        .nodes
        .iter()
        .find(|node| matches!(node, Node::Definition(definition) if definition.name == "root"))
        .unwrap();
    let edge = result
        .graph
        .edges
        .iter()
        .find(|edge| {
            edge.kind == EdgeKind::Contains && edge.target.as_ref() == Some(&function.id())
        })
        .unwrap();
    let span = edge.evidence[0].span.as_ref().unwrap();
    assert_eq!(
        span.start_byte as usize,
        source.find("pub fn root").unwrap()
    );
    assert!(source[span.start_byte as usize..span.end_byte as usize].starts_with("pub fn root"));
    let import = result
        .graph
        .edges
        .iter()
        .find(|edge| {
            edge.kind == EdgeKind::Imports && edge.syntax.as_deref() == Some("crate::child")
        })
        .unwrap();
    assert_eq!(
        import.evidence[0].span.as_ref().unwrap().start_byte as usize,
        source.find("use crate").unwrap()
    );
}

#[test]
fn inaccessible_private_definitions_are_not_resolved_from_sibling_modules() {
    let fixture = Fixture::new();
    fixture.package("visibility");
    fixture.write("src/lib.rs", "mod a; mod b;\n");
    fixture.write(
        "src/a.rs",
        "fn hidden() {}\npub fn visible() {}\nuse self::hidden;\n",
    );
    fixture.write(
        "src/b.rs",
        "use crate::a::hidden;\nuse crate::a::visible;\n",
    );
    let result = analyze_repository(&fixture.0);
    assert!(result.graph.edges.iter().any(
        |edge| edge.kind == EdgeKind::Imports && edge.syntax.as_deref() == Some("self::hidden")
    ));
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::Imports
                && edge.syntax.as_deref() == Some("crate::a::visible"))
    );
    assert!(
        !result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::Imports
                && edge.syntax.as_deref() == Some("crate::a::hidden"))
    );
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::UnresolvedRelationship
                && edge.syntax.as_deref() == Some("crate::a::hidden"))
    );
}

#[test]
fn inaccessible_private_nested_modules_are_not_resolved_from_siblings() {
    let fixture = Fixture::new();
    fixture.package("module-visibility");
    fixture.write("src/lib.rs", "mod a; mod b;\n");
    fixture.write("src/a.rs", "mod private; pub mod public;\n");
    fixture.write("src/a/private.rs", "pub struct HiddenPath;\n");
    fixture.write("src/a/public.rs", "pub struct VisiblePath;\n");
    fixture.write(
        "src/b.rs",
        "use crate::a::private::HiddenPath; use crate::a::public::VisiblePath;\n",
    );
    let result = analyze_repository(&fixture.0);
    assert!(
        !result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::Imports
                && edge.syntax.as_deref() == Some("crate::a::private::HiddenPath"))
    );
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::UnresolvedRelationship
                && edge.syntax.as_deref() == Some("crate::a::private::HiddenPath"))
    );
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::Imports
                && edge.syntax.as_deref() == Some("crate::a::public::VisiblePath"))
    );
}

#[test]
fn conflicting_module_aliases_are_not_used_to_resolve_later_paths() {
    let fixture = Fixture::new();
    fixture.package("alias-collision");
    fixture.write("src/lib.rs", "pub mod a; pub mod b;\nuse crate::a as selected; use crate::b as selected; use selected::Thing;\n");
    fixture.write("src/a.rs", "pub struct Thing;\n");
    fixture.write("src/b.rs", "pub struct Thing;\n");
    let result = analyze_repository(&fixture.0);
    assert!(
        result
            .graph
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.0 == "IMPORT_ALIAS_AMBIGUOUS")
    );
    assert!(
        result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::UnresolvedRelationship
                && edge.syntax.as_deref() == Some("selected::Thing"))
    );
    assert!(
        !result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::Imports
                && edge.syntax.as_deref() == Some("selected::Thing"))
    );
}

#[test]
fn workspace_package_dependencies_resolve_locally_and_keep_rename_and_conditions() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[workspace]\nmembers=[\"a\",\"b\"]\nresolver=\"2\"\n",
    );
    fixture.write("a/Cargo.toml", "[package]\nname=\"a\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[target.'cfg(target_os = \"linux\")'.dependencies]\nrenamed-b={package=\"b\",path=\"../b\",optional=true}\n");
    fixture.write("a/src/lib.rs", "pub fn a() {}\n");
    fixture.write(
        "b/Cargo.toml",
        "[package]\nname=\"b\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    );
    fixture.write("b/src/lib.rs", "pub fn b() {}\n");
    let result = analyze_repository(&fixture.0);
    let edge = result
        .graph
        .edges
        .iter()
        .find(|edge| edge.kind == EdgeKind::PackageDependency)
        .expect("local package edge");
    assert_eq!(edge.resolution, atlas::model::ResolutionStatus::Conditional);
    assert!(
        edge.syntax
            .as_deref()
            .is_some_and(|syntax| syntax.starts_with("renamed-b = b [normal; cfg(target_os"))
    );
    assert!(edge.condition.is_some());
    let condition = serde_json::to_string(&edge.condition).unwrap();
    assert!(condition.contains("optional_dependency"));
    assert!(condition.contains("target_os"));
    let source = edge.source.stable_string();
    let target = edge.target.as_ref().unwrap().stable_string();
    let outgoing = atlas::query::dependencies(
        &result,
        &source,
        atlas::query::DependencyFamily::Packages,
        atlas::query::Direction::Outgoing,
    )
    .unwrap();
    assert_eq!(outgoing.relationships.len(), 1);
    let incoming = atlas::query::dependencies(
        &result,
        &target,
        atlas::query::DependencyFamily::Packages,
        atlas::query::Direction::Incoming,
    )
    .unwrap();
    assert_eq!(incoming.relationships.len(), 1);
}

#[test]
fn package_name_collision_does_not_fake_a_workspace_dependency_target() {
    let fixture = Fixture::new();
    let external_path = fixture.0.parent().unwrap().join(format!(
        "atlas-rel-external-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _external = ExternalFixture(external_path.clone());
    fs::create_dir_all(external_path.join("src")).unwrap();
    fixture.write(
        "Cargo.toml",
        "[workspace]\nmembers=[\"a\",\"b\"]\nresolver=\"2\"\n",
    );
    fixture.write("a/Cargo.toml", &format!("[package]\nname=\"a\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[dependencies]\nb={{path=\"../../{}\"}}\n", external_path.file_name().unwrap().to_string_lossy()));
    fixture.write("a/src/lib.rs", "pub fn a() {}\n");
    fixture.write(
        "b/Cargo.toml",
        "[package]\nname=\"b\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    );
    fixture.write("b/src/lib.rs", "pub fn workspace_b() {}\n");
    fs::write(
        external_path.join("Cargo.toml"),
        "[package]\nname=\"b\"\nversion=\"0.2.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    fs::write(external_path.join("src/lib.rs"), "pub fn external_b() {}\n").unwrap();
    let result = analyze_repository(&fixture.0);
    assert!(
        !result
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::PackageDependency)
    );
    assert!(
        result.graph.edges.iter().any(|edge| {
            edge.kind == EdgeKind::UnresolvedRelationship
                && edge
                    .syntax
                    .as_deref()
                    .is_some_and(|syntax| syntax.contains("b"))
        }),
        "status={:?} diagnostics={:?} edges={:?}",
        result.status,
        result.graph.diagnostics,
        result.graph.edges
    );
}

#[test]
fn queries_support_direct_reverse_component_and_missing_ids() {
    let fixture = Fixture::new();
    fixture.package("queries");
    fixture.write("src/lib.rs", "pub mod a; pub mod b;\nuse crate::a;\n");
    fixture.write("src/a.rs", "pub struct A;\n");
    fixture.write("src/b.rs", "pub struct B;\n");
    let result = analyze_repository(&fixture.0);
    let b_id = result
        .graph
        .nodes
        .iter()
        .find_map(|node| {
            if let Node::Module(module) = node
                && module.logical_path == "crate::b"
            {
                Some(node.id().stable_string())
            } else {
                None
            }
        })
        .unwrap();
    assert!(atlas::query::component(&result, &b_id).is_ok());
    assert!(atlas::query::component(&result, "module:nope").is_err());
    let a_id = result
        .graph
        .nodes
        .iter()
        .find_map(|node| {
            if let Node::Module(module) = node
                && module.logical_path == "crate::a"
            {
                Some(node.id().stable_string())
            } else {
                None
            }
        })
        .unwrap();
    let reverse = atlas::query::dependencies(
        &result,
        &a_id,
        atlas::query::DependencyFamily::Modules,
        atlas::query::Direction::Incoming,
    )
    .unwrap();
    assert!(
        reverse
            .relationships
            .iter()
            .any(|edge| edge.kind == EdgeKind::ReferencesModule)
    );
}
