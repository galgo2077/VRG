//! Read-only configuration diagnostics. This module never opens the engine or database.
use crate::config::VrgConfig;
use serde::Serialize;
use std::{
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Unknown,
    ActionRequired,
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub status: CheckStatus,
    pub evidence: String,
    pub recommendation: String,
    pub automatic_fix: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub schema_version: u8,
    pub checks: Vec<Check>,
}

impl DoctorReport {
    /// 1 means an actual failure; 2 means a human action is required; 0 includes warnings/unknowns.
    pub fn exit_code(&self) -> i32 {
        if self.checks.iter().any(|c| c.status == CheckStatus::Fail) {
            1
        } else if self
            .checks
            .iter()
            .any(|c| c.status == CheckStatus::ActionRequired)
        {
            2
        } else {
            0
        }
    }
    pub fn print_human(&self) {
        for check in &self.checks {
            println!("{:?} {}: {}", check.status, check.id, check.evidence);
            if !check.recommendation.is_empty() {
                println!("  {}", check.recommendation);
            }
        }
    }
}

fn add(
    checks: &mut Vec<Check>,
    id: &'static str,
    status: CheckStatus,
    evidence: impl Into<String>,
    recommendation: impl Into<String>,
) {
    checks.push(Check {
        id,
        status,
        evidence: evidence.into(),
        recommendation: recommendation.into(),
        automatic_fix: false,
    });
}

/// Match the CLI's configuration precedence without disclosing environment values.
pub fn resolve_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }
    if let Some(path) = env::var_os("VRG_CONFIG") {
        return Some(path.into());
    }
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(path);
        return path.is_absolute().then(|| path.join("vrg/config.toml"));
    }
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/vrg/config.toml"))
}

pub fn inspect(explicit: Option<&Path>, check_runtime: bool) -> DoctorReport {
    let mut checks = Vec::new();
    let executable = env::current_exe().ok();
    add(
        &mut checks,
        "executable",
        if executable.is_some() {
            CheckStatus::Pass
        } else {
            CheckStatus::Unknown
        },
        format!(
            "VRG {} ({} / {})",
            env!("CARGO_PKG_VERSION"),
            env::consts::OS,
            env::consts::ARCH
        ),
        "",
    );
    let path = resolve_config_path(explicit);
    let config = match path {
        Some(ref path) => match VrgConfig::load(path) {
            Ok(config) => {
                add(
                    &mut checks,
                    "config",
                    CheckStatus::Pass,
                    "Selected configuration is readable and valid",
                    "",
                );
                Some(config)
            }
            Err(_) => {
                add(
                    &mut checks,
                    "config",
                    CheckStatus::ActionRequired,
                    "Selected configuration is missing, unreadable, or invalid",
                    "Run vrg setup or inspect the selected configuration.",
                );
                None
            }
        },
        None => {
            add(
                &mut checks,
                "config",
                CheckStatus::ActionRequired,
                "No valid configuration location could be selected",
                "Set an absolute XDG_CONFIG_HOME or pass --config.",
            );
            None
        }
    };
    if let Some(config) = config {
        if let (Ok(project), Some(config_path)) = (env::current_dir(), path.as_deref()) {
            inspect_project(&mut checks, &project, config_path, &config);
        }
        match config.validate_layout() {
            Ok(()) => add(
                &mut checks,
                "layout",
                CheckStatus::Pass,
                "Source, model and derived locations do not overlap",
                "",
            ),
            Err(_) => add(
                &mut checks,
                "layout",
                CheckStatus::Fail,
                "Configured locations overlap or cannot be resolved",
                "Choose separate source, model and derived locations.",
            ),
        }
        for root in &config.roots {
            let mut cursor = PathBuf::new();
            let mut status = CheckStatus::Pass;
            for component in root.path.components() {
                if matches!(component, std::path::Component::ParentDir) {
                    status = CheckStatus::Fail;
                    break;
                }
                cursor.push(component.as_os_str());
                match fs::symlink_metadata(&cursor) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        status = CheckStatus::Fail;
                        break;
                    }
                    Ok(meta) if !meta.is_dir() => {
                        status = CheckStatus::ActionRequired;
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        status = CheckStatus::ActionRequired;
                        break;
                    }
                }
            }
            let evidence = match status {
                CheckStatus::Pass => "available without symbolic links",
                CheckStatus::Fail => "contains a symbolic link or parent traversal",
                _ => "unavailable or not a directory",
            };
            add(
                &mut checks,
                "source_root",
                status,
                format!("Root {}: {evidence}", root.id),
                if status == CheckStatus::Pass {
                    ""
                } else {
                    "Review the authorized root without following links or broadening access."
                },
            );
        }
        if config.roots.is_empty() {
            add(
                &mut checks,
                "source_roots",
                CheckStatus::Warn,
                "No source roots configured",
                "Configure an explicitly authorized Markdown root if search is needed.",
            );
        }
        let db = config.data_dir.join("vrg.sqlite3");
        match fs::symlink_metadata(&db) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => add(
                &mut checks,
                "storage",
                CheckStatus::Fail,
                "Database path is not a regular non-symlink file",
                "Inspect derived storage without deleting source notes.",
            ),
            Ok(_) => add(
                &mut checks,
                "storage",
                CheckStatus::Unknown,
                "Database file exists; content, locks and freshness were not inspected",
                "Use an authorized active status check to establish freshness.",
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => add(
                &mut checks,
                "storage",
                CheckStatus::Warn,
                "Derived database does not exist",
                "Run an explicitly authorized initial synchronization when ready.",
            ),
            Err(_) => add(
                &mut checks,
                "storage",
                CheckStatus::Unknown,
                "Database metadata is not accessible",
                "Inspect derived storage permissions.",
            ),
        }
        match config.semantic_model {
            None => add(
                &mut checks,
                "semantic_model",
                CheckStatus::Pass,
                "Text-only mode selected",
                "",
            ),
            Some(model) => {
                let files = [
                    "model.onnx",
                    "tokenizer.json",
                    "config.json",
                    "special_tokens_map.json",
                    "tokenizer_config.json",
                ];
                let complete = files.iter().all(|name| {
                    fs::symlink_metadata(model.model_dir.join(name))
                        .map(|m| m.is_file() && !m.file_type().is_symlink())
                        .unwrap_or(false)
                });
                add(
                    &mut checks,
                    "semantic_model",
                    if complete {
                        CheckStatus::Unknown
                    } else {
                        CheckStatus::ActionRequired
                    },
                    if complete {
                        "Required local model files exist; model loading and integrity were not verified"
                    } else {
                        "One or more required local model files are absent or not regular files"
                    },
                    "Verify the selected model and ONNX runtime before using semantic search.",
                );
            }
        }
    }
    add(
        &mut checks,
        "codex_trust",
        CheckStatus::Unknown,
        "Hook trust and execution are not observable from configuration alone",
        "Review /hooks in Codex and verify events in a real session.",
    );
    if check_runtime {
        match probe_runtime() {
            Ok(()) => add(
                &mut checks,
                "mcp_runtime",
                CheckStatus::Pass,
                "Isolated direct MCP client completed handshake, tool catalog and status call",
                "This does not verify a Codex session or hook trust.",
            ),
            Err(_) => add(
                &mut checks,
                "mcp_runtime",
                CheckStatus::Fail,
                "Isolated direct MCP runtime probe failed or timed out",
                "Inspect the installed binary and its runtime dependencies.",
            ),
        }
    } else {
        add(
            &mut checks,
            "mcp_runtime",
            CheckStatus::Unknown,
            "Runtime connectivity was not checked",
            "A configuration entry alone does not prove a connection.",
        );
    }
    DoctorReport {
        schema_version: 1,
        checks,
    }
}

/// Inspect project-local integration without invoking Codex or opening storage.
fn inspect_project(
    checks: &mut Vec<Check>,
    project: &Path,
    config_path: &Path,
    config: &VrgConfig,
) {
    let codex = project.join(".codex/config.toml");
    match fs::symlink_metadata(&codex) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => add(
            checks,
            "codex_mcp",
            CheckStatus::Unknown,
            "No project-local Codex configuration found",
            "Configure Codex for this project if integration is required.",
        ),
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
            let parsed = fs::read_to_string(&codex)
                .ok()
                .and_then(|text| toml::from_str::<toml::Value>(&text).ok());
            match parsed {
                None => add(
                    checks,
                    "codex_mcp",
                    CheckStatus::ActionRequired,
                    "Project Codex configuration cannot be parsed",
                    "Inspect the existing project configuration without replacing unrelated entries.",
                ),
                Some(value) => match value.get("mcp_servers").and_then(|v| v.get("vrg")) {
                    None => add(
                        checks,
                        "codex_mcp",
                        CheckStatus::Unknown,
                        "No project-local VRG MCP entry found",
                        "Configure Codex for this project if integration is required.",
                    ),
                    Some(vrg) => {
                        let args = vrg.get("args").and_then(toml::Value::as_array);
                        let expected_config = config_path.canonicalize().ok();
                        let configured_config = args
                            .and_then(|a| a.windows(2).find(|w| w[0].as_str() == Some("--config")))
                            .and_then(|w| w[1].as_str())
                            .and_then(|s| Path::new(s).canonicalize().ok());
                        let configured_data = vrg
                            .get("env")
                            .and_then(|e| e.get("VRG_DATA_DIR"))
                            .and_then(toml::Value::as_str);
                        let configured_command = vrg
                            .get("command")
                            .and_then(toml::Value::as_str)
                            .and_then(|s| Path::new(s).canonicalize().ok());
                        let current_command =
                            env::current_exe().ok().and_then(|p| p.canonicalize().ok());
                        let same_profile = expected_config.is_some()
                            && configured_config == expected_config
                            && configured_data == config.data_dir.to_str();
                        let same_command =
                            current_command.is_some() && configured_command == current_command;
                        let configured_cwd = vrg
                            .get("cwd")
                            .and_then(toml::Value::as_str)
                            .and_then(|s| Path::new(s).canonicalize().ok());
                        let same_cwd = project
                            .canonicalize()
                            .ok()
                            .is_some_and(|p| configured_cwd == Some(p));
                        let mcp_serve = args.is_some_and(|a| {
                            a.windows(2).any(|w| {
                                w[0].as_str() == Some("mcp") && w[1].as_str() == Some("serve")
                            })
                        });
                        let good = same_profile && same_command && same_cwd && mcp_serve;
                        add(
                            checks,
                            "codex_mcp",
                            if good {
                                CheckStatus::Pass
                            } else {
                                CheckStatus::ActionRequired
                            },
                            if good {
                                "Project MCP command, working directory and configured profile agree; connectivity not checked"
                            } else {
                                "Project MCP command, working directory or profile differs from this installation"
                            },
                            if good {
                                ""
                            } else {
                                "Review the project-local VRG MCP entry before repairing it."
                            },
                        );
                    }
                },
            }
        }
        _ => add(
            checks,
            "codex_mcp",
            CheckStatus::ActionRequired,
            "Project Codex configuration is inaccessible or not a regular file",
            "Inspect the project configuration and permissions.",
        ),
    }
    let hook_path = project.join(".codex/hooks.json");
    let hooks = fs::symlink_metadata(&hook_path)
        .ok()
        .filter(|m| m.is_file() && !m.file_type().is_symlink())
        .and_then(|_| fs::read_to_string(&hook_path).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    if fs::symlink_metadata(&hook_path).is_ok() && hooks.is_none() {
        add(
            checks,
            "codex_hooks",
            CheckStatus::ActionRequired,
            "Project hooks file is unreadable, malformed or not a regular file",
            "Inspect the project hooks without replacing unrelated definitions.",
        );
    }
    let (resource, packaged) = crate::setup_codex::supervisor_resource(&config.data_dir);
    let referenced = hooks
        .as_ref()
        .is_some_and(|h| h.to_string().contains("vrg-hook-observer-v1.py"));
    if referenced {
        match fs::symlink_metadata(&resource) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let repairable =
                    repair_plan(project, Some(config_path)).is_ok_and(|p| !p.changes().is_empty());
                checks.push(Check { id: "supervisor_resource", status: CheckStatus::ActionRequired, evidence: "The project references a missing packaged supervisor".into(), recommendation: if repairable { "Review doctor --fix --dry-run and run --fix if appropriate." } else { "Inspect hook definitions and profile; ownership is not sufficiently clear for automatic repair." }.into(), automatic_fix: repairable });
            }
            Ok(meta)
                if meta.is_file()
                    && !meta.file_type().is_symlink()
                    && meta.len() <= 4 * 1024 * 1024 =>
            {
                match fs::read(&resource) {
                    Ok(bytes) if bytes == packaged.as_bytes() => add(
                        checks,
                        "supervisor_resource",
                        CheckStatus::Pass,
                        "Packaged supervisor matches the installed resource",
                        "",
                    ),
                    Ok(_) => add(
                        checks,
                        "supervisor_resource",
                        CheckStatus::ActionRequired,
                        "Installed supervisor differs from the packaged resource",
                        "Review the customized or damaged resource; it will not be overwritten automatically.",
                    ),
                    Err(_) => add(
                        checks,
                        "supervisor_resource",
                        CheckStatus::ActionRequired,
                        "Installed supervisor cannot be read",
                        "Inspect resource permissions.",
                    ),
                }
            }
            _ => add(
                checks,
                "supervisor_resource",
                CheckStatus::ActionRequired,
                "Supervisor path is inaccessible or not a regular non-symlink file",
                "Inspect the resource path without overwriting it.",
            ),
        }
    }
}

/// A bounded direct MCP probe using a private disposable profile with no source roots.
fn probe_runtime() -> Result<(), ()> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ())?
        .as_nanos();
    let dir = env::temp_dir().join(format!("vrg-doctor-{}-{stamp}", std::process::id()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir).map_err(|_| ())?;
    let result = (|| {
        let mut config = VrgConfig::default();
        config.data_dir = dir.canonicalize().map_err(|_| ())?.join("derived");
        config.roots.clear();
        config.semantic_model = None;
        let path = dir.join("config.toml");
        fs::write(&path, toml::to_string(&config).map_err(|_| ())?).map_err(|_| ())?;
        let mut child = Command::new(env::current_exe().map_err(|_| ())?)
            .arg("--config")
            .arg(&path)
            .args(["mcp", "serve"])
            .env_remove("VRG_CONFIG")
            .env_remove("VRG_DATA_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ())?;
        let mut input = child.stdin.take().ok_or(())?;
        let stdout = child.stdout.take().ok_or(())?;
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                {
                    Some(value) => {
                        if sender.send(value).is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        });
        let run = (|| {
            let mut send = |value: serde_json::Value| -> Result<(), ()> {
                writeln!(input, "{value}").map_err(|_| ())?;
                input.flush().map_err(|_| ())
            };
            let receive = |id: u64| -> Result<serde_json::Value, ()> {
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                loop {
                    let left = deadline
                        .checked_duration_since(std::time::Instant::now())
                        .ok_or(())?;
                    let response = receiver.recv_timeout(left).map_err(|_| ())?;
                    if response.get("id").and_then(|id| id.as_u64()) == Some(id) {
                        return if response.get("result").is_some() {
                            Ok(response)
                        } else {
                            Err(())
                        };
                    }
                }
            };
            send(
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"vrg-doctor","version":"1"}}}),
            )?;
            receive(1)?;
            send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
            send(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))?;
            let tools = receive(2)?;
            let list = tools["result"]["tools"].as_array().ok_or(())?;
            for expected in [
                "vrg_status",
                "vrg_search",
                "vrg_read_source",
                "vrg_graph_neighbors",
            ] {
                if !list
                    .iter()
                    .any(|tool| tool["name"].as_str() == Some(expected))
                {
                    return Err(());
                }
            }
            send(
                serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"vrg_status","arguments":{}}}),
            )?;
            let status = receive(3)?;
            if status["result"]["isError"].as_bool() == Some(true) {
                return Err(());
            }
            Ok(())
        })();
        drop(input);
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        run
    })();
    let _ = fs::remove_dir_all(&dir);
    result
}

/// Plan only restoration of the versioned observer referenced by all three project hooks.
/// Missing or ambiguous ownership evidence yields an empty plan, never a guessed repair.
pub fn repair_plan(
    project: &Path,
    explicit: Option<&Path>,
) -> crate::error::VrgResult<crate::setup::FilePlan> {
    use crate::setup::{FilePlan, PlannedFile};
    let empty = || FilePlan::new(Vec::new());
    let Some(config_path) = resolve_config_path(explicit) else {
        return empty();
    };
    // Parse exactly the bytes whose continued presence is a repair precondition.
    let Ok(canonical_config_path) = config_path.canonicalize() else {
        return empty();
    };
    let Ok(config_bytes) = fs::read(&canonical_config_path) else {
        return empty();
    };
    let Ok(config_text) = std::str::from_utf8(&config_bytes) else {
        return empty();
    };
    let Ok(config) = toml::from_str::<VrgConfig>(config_text) else {
        return empty();
    };
    if config.validate().is_err() || config.validate_layout().is_err() {
        return empty();
    }
    let (observer, observer_content) = crate::setup_codex::supervisor_resource(&config.data_dir);
    match fs::symlink_metadata(&observer) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ => return empty(),
    }
    let hook_path = project.join(".codex/hooks.json");
    let Ok(meta) = fs::symlink_metadata(&hook_path) else {
        return empty();
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return empty();
    }
    let Ok(text) = fs::read_to_string(&hook_path) else {
        return empty();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return empty();
    };
    let Some(observer) = observer.to_str() else {
        return empty();
    };
    let Some(config_path) = config_path.to_str() else {
        return empty();
    };
    let Some(data_dir) = config.data_dir.to_str() else {
        return empty();
    };
    fn quoted(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
    let valid = [
        ("SessionStart", "codex-start"),
        ("UserPromptSubmit", "codex-turn"),
        ("Stop", "codex-stop"),
    ]
    .iter()
    .all(|(event, mode)| {
        json["hooks"][*event].as_array().is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry["hooks"].as_array().is_some_and(|handlers| {
                    handlers.iter().any(|handler| {
                        handler["type"].as_str() == Some("command")
                            && handler["command"].as_str().is_some_and(|command| {
                                command.starts_with("env VRG_HOOK_EXECUTABLE=")
                                    && command
                                        .contains(&format!(" VRG_CONFIG={} ", quoted(config_path)))
                                    && command
                                        .contains(&format!(" VRG_DATA_DIR={} ", quoted(data_dir)))
                                    && command.ends_with(&format!(" {} {}", quoted(observer), mode))
                            })
                    })
                })
            })
        })
    });
    if !valid {
        return empty();
    }
    FilePlan::new(vec![PlannedFile {
        path: PathBuf::from(observer),
        content: observer_content.as_bytes().to_vec(),
    }])?
    .with_preconditions(vec![
        (canonical_config_path, config_bytes),
        (hook_path, text.into_bytes()),
    ])
}
