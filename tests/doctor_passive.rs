use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use vrg::doctor::{CheckStatus, inspect};

fn fixture() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("vrg-doctor-{}-{nonce}", std::process::id()));
    fs::create_dir(&root).unwrap();
    root
}

#[test]
fn missing_config_is_action_required_without_creating_it() {
    let root = fixture();
    let config = root.join("missing.toml");
    let report = inspect(Some(&config), false);
    assert_eq!(report.exit_code(), 2);
    assert!(!config.exists());
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.id == "config" && c.status == CheckStatus::ActionRequired)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn doctor_does_not_create_database_and_json_reports_unknown_runtime() {
    let root = fixture();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    let data = root.join("data");
    let config = root.join("config.toml");
    let quoted =
        |p: &std::path::Path| serde_json::to_string(&p.to_string_lossy().to_string()).unwrap();
    fs::write(
        &config,
        format!(
            "data_dir = {}\n[[roots]]\nid = \"fixture\"\npath = {}\n",
            quoted(&data),
            quoted(&source)
        ),
    )
    .unwrap();
    let before = fs::read(&config).unwrap();
    let report = inspect(Some(&config), false);
    assert_eq!(before, fs::read(&config).unwrap());
    assert!(!data.exists());
    let json = serde_json::to_value(report).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert!(
        json["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == "mcp_runtime" && v["status"] == "UNKNOWN")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repair_plan_requires_all_matching_hooks_and_does_not_overwrite_resource() {
    use vrg::doctor::repair_plan;
    let root = fixture();
    let project = root.join("project");
    let data = root.join("data");
    fs::create_dir_all(project.join(".codex")).unwrap();
    let config = root.join("config.toml");
    let quoted =
        |p: &std::path::Path| serde_json::to_string(&p.to_string_lossy().to_string()).unwrap();
    fs::write(&config, format!("data_dir = {}\n", quoted(&data))).unwrap();
    let observer = data.join("resources/vrg-hook-observer-v1.py");
    let command = |mode: &str| {
        format!(
            "env VRG_HOOK_EXECUTABLE='/tmp/vrg' VRG_HOOK_EVIDENCE_DIR='{}' VRG_CONFIG='{}' VRG_DATA_DIR='{}' '/usr/bin/python3' '{}' {}",
            data.display(),
            config.display(),
            data.display(),
            observer.display(),
            mode
        )
    };
    let handler =
        |mode: &str| serde_json::json!([{"hooks":[{"type":"command","command":command(mode)}]}]);
    let hooks = serde_json::json!({"hooks":{"SessionStart":handler("codex-start"),"UserPromptSubmit":handler("codex-turn"),"Stop":handler("codex-stop")}});
    fs::write(
        project.join(".codex/hooks.json"),
        serde_json::to_vec(&hooks).unwrap(),
    )
    .unwrap();
    let plan = repair_plan(&project, Some(&config)).unwrap();
    assert_eq!(plan.changes(), vec![observer.as_path()]);
    assert!(!observer.exists());
    let hooks_path = project.join(".codex/hooks.json");
    let original_hooks = fs::read(&hooks_path).unwrap();
    fs::write(&hooks_path, b"{}\n").unwrap();
    assert!(
        plan.apply(&root.join("backups"), &project, &config)
            .is_err()
    );
    assert!(!observer.exists());
    fs::write(&hooks_path, &original_hooks).unwrap();
    let original_config = fs::read(&config).unwrap();
    fs::write(&config, b"# changed\n").unwrap();
    assert!(
        plan.apply(&root.join("backups"), &project, &config)
            .is_err()
    );
    assert!(!observer.exists());
    fs::write(&config, &original_config).unwrap();
    let applied = plan
        .apply(&root.join("backups"), &project, &config)
        .unwrap();
    assert_eq!(applied.changed, 1);
    assert!(applied.backup.unwrap().join("manifest.json").exists());
    assert!(!fs::read(&observer).unwrap().is_empty());
    assert!(
        repair_plan(&project, Some(&config))
            .unwrap()
            .changes()
            .is_empty()
    );
    fs::write(&observer, b"customized").unwrap();
    assert!(
        repair_plan(&project, Some(&config))
            .unwrap()
            .changes()
            .is_empty()
    );
    assert_eq!(fs::read(&observer).unwrap(), b"customized");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_rejects_runtime_probe_in_fix_dry_run() {
    let root = fixture();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_vrg"))
        .current_dir(&root)
        .args(["doctor", "--fix", "--dry-run", "--check-runtime"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn customized_referenced_supervisor_is_action_required() {
    let root = fixture();
    let project = root.join("project");
    let data = root.join("data");
    fs::create_dir_all(project.join(".codex")).unwrap();
    fs::create_dir_all(data.join("resources")).unwrap();
    let observer = data.join("resources/vrg-hook-observer-v1.py");
    fs::write(&observer, b"customized").unwrap();
    let hooks = serde_json::json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":format!("python3 {} codex-stop",observer.display())}]}]}});
    fs::write(
        project.join(".codex/hooks.json"),
        serde_json::to_vec(&hooks).unwrap(),
    )
    .unwrap();
    let config = root.join("config.toml");
    fs::write(
        &config,
        format!(
            "data_dir = {}\n",
            serde_json::to_string(&data.to_string_lossy().to_string()).unwrap()
        ),
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vrg"))
        .current_dir(&project)
        .arg("--config")
        .arg(&config)
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "supervisor_resource"
                && c["status"] == "ACTION_REQUIRED"
                && c["automatic_fix"] == false)
    );
    assert_eq!(fs::read(&observer).unwrap(), b"customized");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_recognizes_a_valid_project_mcp_table() {
    let root = fixture();
    let project = root.join("project");
    fs::create_dir_all(project.join(".codex")).unwrap();
    let config = root.join("config.toml");
    let q = |p: &std::path::Path| serde_json::to_string(&p.to_string_lossy().to_string()).unwrap();
    fs::write(&config, format!("data_dir = {}\n", q(&root.join("data")))).unwrap();
    fs::write(project.join(".codex/config.toml"), format!("# preserve comment\n[mcp_servers.vrg]\ncommand = {}\nargs = [\"--config\", {}, \"mcp\", \"serve\"]\ncwd = {}\n[mcp_servers.vrg.env]\nVRG_DATA_DIR = {}\n", q(std::path::Path::new(env!("CARGO_BIN_EXE_vrg"))), q(&config), q(&project), q(&root.join("data")))).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vrg"))
        .current_dir(&project)
        .args(["--config", config.to_str().unwrap(), "doctor", "--json"])
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "codex_mcp")
        .unwrap();
    assert_eq!(check["status"], "PASS", "{check}");
    fs::remove_dir_all(root).unwrap();
}
