use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use vrg::doctor::{CheckStatus, inspect};

fn fixture() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "vrg-doctor-{}-{nonce}-{sequence}",
        std::process::id()
    ));
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

#[cfg(unix)]
#[test]
fn doctor_rejects_a_source_replaced_by_a_symlink() {
    let root = fixture();
    let source = root.join("source");
    let other = root.join("other");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&other).unwrap();
    let config = root.join("config.toml");
    let q = |p: &std::path::Path| serde_json::to_string(&p.to_string_lossy().to_string()).unwrap();
    fs::write(
        &config,
        format!(
            "data_dir = {}\n[[roots]]\nid = \"fixture\"\npath = {}\n",
            q(&root.join("data")),
            q(&source)
        ),
    )
    .unwrap();
    fs::rename(&source, root.join("saved")).unwrap();
    std::os::unix::fs::symlink(&other, &source).unwrap();
    let report = inspect(Some(&config), false);
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.id == "source_root" && c.status == CheckStatus::Fail)
    );
    assert_eq!(report.exit_code(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[test]
fn intel_setup_checks_runtime_before_model_download_or_writes() {
    let root = fixture();
    let project = root.join("project");
    let notes = root.join("notes");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&notes).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vrg"))
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env_remove("VRG_CONFIG")
        .env_remove("VRG_DATA_DIR")
        .args(["setup", "--project"])
        .arg(&project)
        .arg("--root")
        .arg(&notes)
        .args(["--cli-only", "--download-model"])
        .arg(root.join("models/e5"))
        .args(["--non-interactive", "--yes"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!root.join("models").exists());
    assert!(!root.join("config").exists());
    assert!(!root.join("data").exists());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[test]
fn intel_setup_rejects_runtime_without_api_24_without_panicking() {
    let root = fixture();
    let project = root.join("project");
    let notes = root.join("notes");
    let model = root.join("model");
    let bin_dir = root.join("bin");
    for dir in [&project, &notes, &model, &bin_dir] {
        fs::create_dir(dir).unwrap();
    }
    let executable = bin_dir.join("vrg");
    fs::copy(env!("CARGO_BIN_EXE_vrg"), &executable).unwrap();
    let fake_source = root.join("fake.c");
    fs::write(&fake_source, r#"
        #include <stddef.h>
        typedef struct { const void *(*GetApi)(unsigned); const char *(*GetVersionString)(void); } Base;
        static const void *get_api(unsigned n) { (void)n; return NULL; }
        static const char *get_version(void) { return "1.24.1"; }
        const Base *OrtGetApiBase(void) { static const Base b = { get_api, get_version }; return &b; }
    "#).unwrap();
    let library = bin_dir.join("libonnxruntime.dylib");
    let compiled = std::process::Command::new("cc")
        .args(["-dynamiclib", "-o"])
        .arg(&library)
        .arg(&fake_source)
        .status()
        .unwrap();
    assert!(compiled.success());
    let output = std::process::Command::new(&executable)
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env_remove("VRG_CONFIG")
        .env_remove("VRG_DATA_DIR")
        .args(["setup", "--project"])
        .arg(&project)
        .arg("--root")
        .arg(&notes)
        .args(["--cli-only", "--model-dir"])
        .arg(&model)
        .args(["--non-interactive", "--yes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not implement API 24"));
    assert!(!root.join("config").exists());
    assert!(!root.join("data").exists());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[test]
fn hidden_intel_abi_probe_distinguishes_api_24() {
    let root = fixture();
    let source = root.join("fake.c");
    let library = root.join("fake.dylib");
    let binary = env!("CARGO_BIN_EXE_vrg");
    for valid in [false, true] {
        let result = if valid { "(const void*)1" } else { "NULL" };
        fs::write(&source, format!(r#"
            #include <stddef.h>
            typedef struct {{ const void *(*GetApi)(unsigned); const char *(*GetVersionString)(void); }} Base;
            static const void *get_api(unsigned n) {{ (void)n; return {result}; }}
            static const char *get_version(void) {{ return "1.24.1"; }}
            const Base *OrtGetApiBase(void) {{ static const Base b = {{ get_api, get_version }}; return &b; }}
        "#)).unwrap();
        assert!(
            std::process::Command::new("cc")
                .args(["-dynamiclib", "-o"])
                .arg(&library)
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        let outcome = std::process::Command::new(binary)
            .env_clear()
            .arg("__verify-onnx-runtime")
            .arg(&library)
            .output()
            .unwrap();
        assert_eq!(outcome.status.success(), valid);
    }
    fs::remove_dir_all(root).unwrap();
}
