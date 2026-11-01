//! Project-local Codex integration planner. It never writes files or changes trust.
use crate::setup::PlannedFile;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

const OBSERVER: &str = include_str!("../scripts/vrg_hook_observer.py");
/// Hash of the packaged v1 supervisor, useful when displaying resource identity.
pub const OBSERVER_SHA256: &str =
    "b9dfd518b867a0083f00666470bf48b09b79b8eba79adbdc8140b90143055900";
/// Return the stable installation path and packaged contents without reading or writing.
pub fn supervisor_resource(data_dir: &Path) -> (PathBuf, &'static str) {
    (data_dir.join("resources/vrg-hook-observer-v1.py"), OBSERVER)
}
const BEGIN: &str = "<!-- VRG OPERATING INSTRUCTIONS BEGIN -->";
const END: &str = "<!-- VRG OPERATING INSTRUCTIONS END -->";

fn read_existing(path: &Path) -> Result<Option<String>, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => Err(format!(
            "refusing non-regular destination: {}",
            path.display()
        )),
        Ok(_) => fs::read_to_string(path)
            .map(Some)
            .map_err(|e| format!("cannot read {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot inspect {}: {e}", path.display())),
    }
}
fn safe_path(path: &Path) -> Result<&str, String> {
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {}", path.display()));
    }
    let value = path.to_str().ok_or("paths must be valid UTF-8")?;
    if value.chars().any(|c| c.is_control()) {
        return Err("control characters in paths are not supported".into());
    }
    Ok(value)
}
fn quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}
fn add(out: &mut Vec<PlannedFile>, path: PathBuf, old: Option<String>, new: String) {
    if old.as_deref() != Some(new.as_str()) {
        out.push(PlannedFile {
            path,
            content: new.into_bytes(),
        });
    }
}

/// Build changes for project-local MCP, hooks and instructions; no trust is granted.
/// `data_dir` is dedicated VRG storage and must be created privately by the caller.
pub fn plan_codex(
    project: &Path,
    executable: &Path,
    config: &Path,
    data_dir: &Path,
    python: &Path,
    memory: Option<&Path>,
) -> Result<Vec<PlannedFile>, String> {
    let project_s = safe_path(project)?;
    let executable_s = safe_path(executable)?;
    let config_s = safe_path(config)?;
    let data_s = safe_path(data_dir)?;
    let python_s = safe_path(python)?;
    let (observer, _) = supervisor_resource(data_dir);
    let observer_s = safe_path(&observer)?;
    let mut out = Vec::new();
    if memory.is_some() {
        let observer_old = read_existing(&observer)?;
        if let Some(ref old) = observer_old {
            if old != OBSERVER {
                return Err(format!(
                    "customized VRG supervisor conflicts with packaged resource: {}",
                    observer.display()
                ));
            }
        }
        add(
            &mut out,
            observer.clone(),
            observer_old,
            OBSERVER.to_owned(),
        );
    }

    let codex_config = project.join(".codex/config.toml");
    let old = read_existing(&codex_config)?;
    let parsed: toml::Value = match old.as_deref() {
        Some(text) => toml::from_str(text).map_err(|e| format!("invalid Codex TOML: {e}"))?,
        None => toml::Value::Table(Default::default()),
    };
    if parsed.get("hooks").is_some() {
        return Err(
            "project has inline hooks; resolve the hook source conflict before setup".into(),
        );
    }
    let strval = |s: &str| serde_json::to_string(s).expect("string serialization");
    let section = format!(
        "[mcp_servers.vrg]\ncommand = {}\nargs = [\"--config\", {}, \"mcp\", \"serve\"]\ncwd = {}\nstartup_timeout_sec = 30\ntool_timeout_sec = 60\nrequired = false\nenabled_tools = [\"vrg_status\", \"vrg_search\", \"vrg_read_source\", \"vrg_graph_neighbors\"]\n[mcp_servers.vrg.env]\nVRG_DATA_DIR = {}\n",
        strval(executable_s),
        strval(config_s),
        strval(project_s),
        strval(data_s)
    );
    let expected: toml::Value =
        toml::from_str(&section).map_err(|e| format!("cannot construct Codex config: {e}"))?;
    if let Some(existing) = parsed.get("mcp_servers").and_then(|v| v.get("vrg")) {
        if Some(existing) != expected.get("mcp_servers").and_then(|v| v.get("vrg")) {
            return Err("existing Codex MCP server 'vrg' differs; refusing to overwrite it".into());
        }
    } else {
        let mut text = old.clone().unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&section);
        // Validate the merged file before proposing it.
        toml::from_str::<toml::Value>(&text)
            .map_err(|e| format!("cannot merge Codex config: {e}"))?;
        add(&mut out, codex_config, old, text);
    }

    if memory.is_some() {
        let hooks_path = project.join(".codex/hooks.json");
        let old = read_existing(&hooks_path)?;
        let mut hooks: Value = match old.as_deref() {
            Some(text) => {
                serde_json::from_str(text).map_err(|e| format!("invalid hooks JSON: {e}"))?
            }
            None => json!({"hooks": {}}),
        };
        let root = hooks
            .as_object_mut()
            .ok_or("hooks JSON must be an object")?;
        let all_hooks = root
            .entry("hooks")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or("hooks must be an object")?;
        let mut changed = false;
        for (event, mode) in [
            ("SessionStart", "codex-start"),
            ("UserPromptSubmit", "codex-turn"),
            ("Stop", "codex-stop"),
        ] {
            let command = format!(
                "env VRG_HOOK_EXECUTABLE={} VRG_HOOK_EVIDENCE_DIR={} VRG_CONFIG={} VRG_DATA_DIR={} {} {} {}",
                quote(executable_s),
                quote(data_s),
                quote(config_s),
                quote(data_s),
                quote(python_s),
                quote(observer_s),
                mode
            );
            let entries = all_hooks
                .entry(event)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or_else(|| format!("{event} hooks must be an array"))?;
            let entry = json!({"hooks": [{"type":"command", "command":command, "timeout":600}]});
            if entries.iter().any(|e| e == &entry) {
                continue;
            }
            // A pre-existing VRG handler may have been customized or trusted. Do not duplicate or replace it.
            if entries.iter().any(|e| {
                e.get("hooks").and_then(Value::as_array).is_some_and(|a| {
                    a.iter().any(|h| {
                        h.get("command").and_then(Value::as_str).is_some_and(|c| {
                            c.contains("vrg_hook_observer.py")
                                || c.contains("vrg-hook-observer-v1.py")
                        })
                    })
                })
            }) {
                return Err(format!(
                    "existing {event} VRG hook differs; refusing to replace or duplicate it"
                ));
            }
            entries.push(entry);
            changed = true;
        }
        if changed {
            let mut text = serde_json::to_string_pretty(&hooks).map_err(|e| e.to_string())?;
            text.push('\n');
            add(&mut out, hooks_path, old, text);
        }
    }
    let instructions_path = project.join("AGENTS.md");
    let old = read_existing(&instructions_path)?;
    let memory_rule = if let Some(path) = memory {
        format!(
            "Durable, verified project knowledge may be recorded only in STATUS.md, DECISIONS.md or SOLUTIONS.md under {}; do not write when nothing new warrants memory.",
            safe_path(path)?
        )
    } else {
        "Do not write operational memory unless the user separately authorizes its location."
            .to_owned()
    };
    let block = format!(
        "{BEGIN}\nWhen prior decisions, solutions or state are needed, retrieve relevant context with VRG MCP and verify source and freshness. Inspect and modify real code using normal development tools. {memory_rule} Never record conversations, prompts, internal reasoning or extensive logs. MCP tools are read-only.\n{END}"
    );
    let current = old.clone().unwrap_or_default();
    let new = match (current.find(BEGIN), current.find(END)) {
        (None, None) => {
            let mut s = current.clone();
            if !s.is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&block);
            s.push('\n');
            s
        }
        (Some(start), Some(end))
            if start < end
                && !current[end + END.len()..].contains(BEGIN)
                && !current[end + END.len()..].contains(END) =>
        {
            let mut s = current[..start].to_owned();
            s.push_str(&block);
            s.push_str(&current[end + END.len()..]);
            s
        }
        _ => {
            return Err(
                "ambiguous VRG instruction markers; refusing to overwrite instructions".into(),
            );
        }
    };
    add(&mut out, instructions_path, old, new);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("vrg-codex-plan-{}-{unique}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn plans_conservatively_and_is_idempotent() {
        let fixture = Fixture::new();
        let project = fixture.0.join("project space's");
        let codex = project.join(".codex");
        fs::create_dir_all(&codex).unwrap();
        fs::write(
            codex.join("config.toml"),
            "# keep\n[mcp_servers.other]\ncommand = 'other'\n",
        )
        .unwrap();
        fs::write(
            codex.join("hooks.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"other-notification"}]}]}}"#,
        )
        .unwrap();
        fs::write(project.join("AGENTS.md"), "Preserve these instructions.\n").unwrap();
        let exe = fixture.0.join("binary's dir/vrg");
        let config = fixture.0.join("profile config.toml");
        let data = fixture.0.join("data space");
        let python = Path::new("/usr/bin/python3");
        let memory = project.join("memory");
        let plan = plan_codex(&project, &exe, &config, &data, python, Some(&memory)).unwrap();
        assert_eq!(plan.len(), 4);
        assert!(!data.exists(), "planning must not write resources");
        for item in plan {
            fs::create_dir_all(item.path.parent().unwrap()).unwrap();
            fs::write(item.path, item.content).unwrap();
        }
        let merged = fs::read_to_string(codex.join("config.toml")).unwrap();
        assert!(merged.contains("# keep"));
        let parsed: toml::Value = toml::from_str(&merged).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["vrg"]["args"][1].as_str(),
            config.to_str()
        );
        assert_eq!(
            parsed["mcp_servers"]["vrg"]["env"]["VRG_DATA_DIR"].as_str(),
            data.to_str()
        );
        let hooks: Value =
            serde_json::from_str(&fs::read_to_string(codex.join("hooks.json")).unwrap()).unwrap();
        let stops = hooks["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stops[0]["hooks"][0]["command"], "other-notification");
        let command = stops[1]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains(&quote(exe.to_str().unwrap())));
        assert!(command.contains(&quote(config.to_str().unwrap())));
        assert!(command.contains(&quote(data.to_str().unwrap())));
        assert!(
            plan_codex(&project, &exe, &config, &data, python, Some(&memory))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn packaged_supervisor_identity_matches_content() {
        use sha2::{Digest, Sha256};
        let (path, content) = supervisor_resource(Path::new("/tmp/vrg-data"));
        assert_eq!(
            path,
            Path::new("/tmp/vrg-data/resources/vrg-hook-observer-v1.py")
        );
        assert_eq!(
            hex::encode(Sha256::digest(content.as_bytes())),
            OBSERVER_SHA256
        );
    }

    #[test]
    fn conflicting_vrg_server_is_never_overwritten() {
        let fixture = Fixture::new();
        let codex = fixture.0.join(".codex");
        fs::create_dir(&codex).unwrap();
        let original = "# do not change\n[mcp_servers.vrg]\ncommand = 'custom'\n";
        fs::write(codex.join("config.toml"), original).unwrap();
        assert!(
            plan_codex(
                &fixture.0,
                Path::new("/bin/vrg"),
                Path::new("/tmp/profile"),
                Path::new("/tmp/data"),
                Path::new("/bin/python3"),
                None
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(codex.join("config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn without_authorized_memory_no_hook_resource_is_planned() {
        let fixture = Fixture::new();
        let plan = plan_codex(
            &fixture.0,
            Path::new("/bin/vrg"),
            Path::new("/tmp/profile"),
            Path::new("/tmp/data"),
            Path::new("/bin/python3"),
            None,
        )
        .unwrap();
        assert_eq!(plan.len(), 2);
        assert!(plan.iter().all(
            |p| !p.path.ends_with("hooks.json") && !p.path.ends_with("vrg-hook-observer-v1.py")
        ));
    }
}
