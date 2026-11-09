use std::process::Command;

fn atlas() -> Command {
    Command::new(env!("CARGO_BIN_EXE_atlas"))
}

#[test]
fn help_and_version_work() {
    let help = atlas().arg("--help").output().unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for command in [
        "inspect",
        "modules",
        "component",
        "deps",
        "path",
        "diagnostics",
        "export",
    ] {
        assert!(text.contains(command));
    }
    let version = atlas().arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8(version.stdout).unwrap().contains("0.1.0"));
    let subcommand_help = atlas().args(["deps", "--help"]).output().unwrap();
    assert!(subcommand_help.status.success());
    assert!(
        String::from_utf8(subcommand_help.stdout)
            .unwrap()
            .contains("direct dependencies")
    );
}

#[test]
fn invalid_and_unimplemented_commands_have_distinct_exit_codes() {
    let invalid = atlas().args(["component", "   "]).output().unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    let pending = atlas().args(["inspect", "."]).output().unwrap();
    assert_eq!(pending.status.code(), Some(0));
    assert!(
        String::from_utf8(pending.stdout)
            .unwrap()
            .contains("Packages:")
    );
}

#[test]
fn dependency_path_is_functional_and_requires_family() {
    let pending = atlas()
        .args(["path", "module:x", "module:y", "--family", "modules"])
        .output()
        .unwrap();
    assert_eq!(pending.status.code(), Some(2)); // unknown component
    let invalid = atlas()
        .args(["path", "module:x", "module:y"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2)); // required family omitted
}

#[test]
fn component_dependency_and_reverse_commands_return_graph_evidence() {
    let root = std::env::temp_dir().join(format!("atlas-cli-rel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"cli-fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub mod child; use crate::child;\n",
    )
    .unwrap();
    std::fs::write(root.join("src/child.rs"), "pub struct Child;\n").unwrap();
    let inspect = atlas()
        .args(["inspect", root.to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    let child = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "module" && node["data"]["logical_path"] == "crate::child")
        .unwrap();
    let id = format!("module:{}", child["data"]["id"].as_str().unwrap());
    let root_module = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "module" && node["data"]["logical_path"] == "crate")
        .unwrap();
    let root_id = format!("module:{}", root_module["data"]["id"].as_str().unwrap());
    let path_query = atlas()
        .args([
            "path",
            &root_id,
            &id,
            root.to_str().unwrap(),
            "--family",
            "modules",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(path_query.status.success());
    let path_json: serde_json::Value = serde_json::from_slice(&path_query.stdout).unwrap();
    assert_eq!(path_json["status"], "found");
    let cycles = atlas()
        .args([
            "cycles",
            root.to_str().unwrap(),
            "--family",
            "modules",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(cycles.status.success());
    assert!(serde_json::from_slice::<serde_json::Value>(&cycles.stdout).is_ok());
    let component = atlas()
        .args(["component", &id, root.to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    assert!(component.status.success());
    assert!(
        String::from_utf8(component.stdout)
            .unwrap()
            .contains("Child")
    );
    let component_human = atlas()
        .args(["component", &id, root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(component_human.status.success());
    assert!(
        String::from_utf8(component_human.stdout)
            .unwrap()
            .contains("Definitions:")
    );
    let direct = atlas()
        .args(["deps", &id, root.to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    assert!(direct.status.success());
    let direct_human = atlas()
        .args(["deps", &id, root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(direct_human.status.success());
    assert!(
        String::from_utf8(direct_human.stdout)
            .unwrap()
            .contains("Relationships:")
    );
    let reverse = atlas()
        .args([
            "deps",
            &id,
            root.to_str().unwrap(),
            "--reverse",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(reverse.status.success());
    assert!(
        String::from_utf8(reverse.stdout)
            .unwrap()
            .contains("references_module")
    );
    let missing = atlas()
        .args(["component", "module:not-present", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    let absent =
        std::env::temp_dir().join(format!("atlas-cli-query-failed-{}", std::process::id()));
    let failed = atlas()
        .args(["deps", &id, absent.to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(4));
    assert!(
        String::from_utf8(failed.stdout)
            .unwrap()
            .contains("\"status\": \"failed\"")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn human_json_modules_and_diagnostics_are_functional() {
    let human = atlas().args(["inspect", "."]).output().unwrap();
    assert!(human.status.success());
    assert!(
        String::from_utf8(human.stdout)
            .unwrap()
            .contains("Modules:")
    );

    let modules = atlas().args(["modules", "."]).output().unwrap();
    assert!(modules.status.success());
    assert!(String::from_utf8(modules.stdout).unwrap().contains("crate"));

    let diagnostics = atlas().args(["diagnostics", "."]).output().unwrap();
    assert!(diagnostics.status.success());
    assert!(
        String::from_utf8(diagnostics.stdout)
            .unwrap()
            .contains("Analysis:")
    );

    let json = atlas()
        .args(["inspect", ".", "--format", "json"])
        .output()
        .unwrap();
    assert!(json.status.success());
    let json = String::from_utf8(json.stdout).unwrap();
    assert!(json.contains("\"schema_version\": \"1\""));
    assert!(json.contains("\"nodes\""));
    assert!(!json.contains(env!("CARGO_MANIFEST_DIR")));
}

#[test]
fn failed_discovery_returns_json_diagnostics_and_analysis_exit_code() {
    let missing = std::env::temp_dir().join(format!("atlas-cli-no-such-{}", std::process::id()));
    let result = atlas()
        .args(["inspect", missing.to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(4));
    let json = String::from_utf8(result.stdout).unwrap();
    assert!(json.contains("\"status\": \"failed\""));
    assert!(json.contains("MANIFEST_NOT_FOUND") || json.contains("REPOSITORY_PATH_UNAVAILABLE"));
    assert!(!json.contains(missing.to_string_lossy().as_ref()));
}
