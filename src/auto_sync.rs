use std::{
    env,
    io::{self, Read},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    VrgEngine,
    config::VrgConfig,
    contracts::{IndexSyncStatus, StopSyncOutcome},
};

mod closure;
pub use closure::{attest_close, run_turn_hook};

const MAX_HOOK_INPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct StopEvent {
    hook_event_name: String,
    session_id: Option<String>,
    turn_id: Option<String>,
    cwd: Option<String>,
    source: Option<String>,
    #[serde(default)]
    stop_hook_active: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HookResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

impl HookResponse {
    fn quiet() -> Self {
        Self {
            system_message: None,
            decision: None,
            reason: None,
        }
    }
    fn message(message: impl Into<String>) -> Self {
        Self {
            system_message: Some(message.into()),
            decision: None,
            reason: None,
        }
    }
}

/// Consume a Codex `Stop` event and emit only the JSON response permitted by
/// Codex hooks. Operational errors are reported to the turn without blocking it.
pub fn run_stop_hook(config_override: Option<PathBuf>) -> crate::VrgResult<()> {
    let started = closure::now_ms();
    let event = read_event();
    let event_copy = event.as_ref().ok().and_then(|event| event.clone());
    let mut response = match event {
        Ok(Some(event)) => run_event(event, config_override.clone()),
        Ok(None) => HookResponse::message("VRG synchronization skipped: invalid Codex Stop event."),
        Err(()) => HookResponse::message(
            "VRG synchronization skipped: Codex hook input exceeded its size limit or could not be read.",
        ),
    };
    if closure::enabled() {
        if let Some(event) = event_copy
            .as_ref()
            .filter(|event| event.hook_event_name == "Stop")
        {
            let outcome = closure::check_stop(event, config_override.as_deref());
            match outcome.as_deref() {
                Ok("documented") => {}
                Ok(value) if value.starts_with("no-new:") => {}
                _ if !event.stop_hook_active => {
                    response.decision = Some("block");
                    response.reason = Some(
                        "Complete the VRG documentation check for this turn using its per-turn token. Record a new durable result in the authorized memory if appropriate, or explicitly declare no new durable knowledge; do not save conversation text.",
                    );
                }
                _ => response.system_message = Some(
                    "VRG documentation closure was not verified after the automatic continuation."
                        .into(),
                ),
            }
            let audit_outcome = match outcome.as_deref() {
                Ok("documented") => "declared_documented_hash_changed".to_string(),
                Ok(value) if value.starts_with("no-new:") => {
                    format!("declared_no_new_hash_unchanged:{value}")
                }
                _ if event.stop_hook_active => "closure_unverified_after_continuation".to_string(),
                _ => "closure_continuation_requested".to_string(),
            };
            if closure::audit(event, config_override.as_deref(), started, &audit_outcome).is_err() {
                response.system_message =
                    Some("VRG could not persist hook evidence; closure is unverified.".into());
            }
        }
    }
    emit_response(&response)
}

pub fn run_start_hook(config_override: Option<PathBuf>) -> crate::VrgResult<()> {
    let started = closure::now_ms();
    let event = read_event();
    let event_copy = event.as_ref().ok().and_then(|event| event.clone());
    let mut response = match event {
        Ok(Some(event)) if event.hook_event_name == "SessionStart" => {
            run_start_event(event, config_override.clone())
        }
        Ok(Some(_)) => HookResponse::quiet(),
        Ok(None) => HookResponse::message(
            "VRG startup reconciliation skipped: invalid Codex SessionStart event.",
        ),
        Err(()) => HookResponse::message(
            "VRG startup reconciliation skipped: invalid or oversized hook input.",
        ),
    };
    if closure::enabled() {
        if let Some(event) = event_copy
            .as_ref()
            .filter(|event| event.hook_event_name == "SessionStart")
        {
            // Silence can mean an in-progress or duplicate operation, so it
            // does not by itself prove that reconciliation succeeded.
            let outcome = "reconciliation_unclassified";
            if closure::audit(event, config_override.as_deref(), started, outcome).is_err() {
                response.system_message =
                    Some("VRG could not persist SessionStart evidence.".into());
            }
        }
    }
    emit_response(&response)
}

fn emit_response(response: &HookResponse) -> crate::VrgResult<()> {
    use std::io::Write;
    let mut output =
        serde_json::to_vec(response).map_err(|e| crate::error::VrgError::Storage(e.to_string()))?;
    output.push(b'\n');
    io::stdout().lock().write_all(&output)?;
    Ok(())
}

fn read_event() -> Result<Option<StopEvent>, ()> {
    let mut bytes = Vec::new();
    io::stdin()
        .take((MAX_HOOK_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > MAX_HOOK_INPUT_BYTES {
        return Err(());
    }
    let event: StopEvent = serde_json::from_slice(&bytes).map_err(|_| ())?;
    Ok(Some(event))
}

fn run_event(event: StopEvent, config_override: Option<PathBuf>) -> HookResponse {
    if event.hook_event_name != "Stop" || (event.stop_hook_active && !closure::enabled()) {
        return HookResponse::quiet();
    }
    let (Some(session_id), Some(turn_id)) = (event.session_id.as_deref(), event.turn_id.as_deref())
    else {
        return HookResponse::message(
            "VRG synchronization skipped: Codex did not provide a session and turn identifier.",
        );
    };
    let config_path = match config_override.or_else(|| env::var_os("VRG_CONFIG").map(PathBuf::from))
    {
        Some(path) => path,
        None => match default_config_path() {
            Ok(path) => path,
            Err(_) => {
                return HookResponse::message(
                    "VRG synchronization failed: VRG configuration path is unavailable.",
                );
            }
        },
    };
    let config = match VrgConfig::load(config_path) {
        Ok(config) => config,
        Err(_) => {
            return HookResponse::message(
                "VRG synchronization failed: configuration could not be loaded; the last valid generation was retained.",
            );
        }
    };
    if config.roots.is_empty() && !config.capture_session_events {
        return HookResponse::message(
            "VRG synchronization skipped: no authorized source roots are configured.",
        );
    }
    let engine = match VrgEngine::open(config.clone()) {
        Ok(engine) => engine,
        Err(_) => {
            return HookResponse::message(
                "VRG synchronization failed while opening derived storage; the last valid generation was retained where available.",
            );
        }
    };
    if config.capture_session_events {
        if engine
            .record_session_event(
                session_id,
                turn_id,
                event.cwd.as_deref().unwrap_or(""),
                "Stop",
            )
            .is_err()
        {
            return HookResponse::message(
                "VRG session event could not be persisted; index synchronization was skipped.",
            );
        }
    }
    if config.roots.is_empty() {
        return HookResponse::message(
            "VRG synchronization skipped: no authorized source roots are configured.",
        );
    }
    let event_key = if event.stop_hook_active {
        event_key(session_id, &format!("{turn_id}:continuation"))
    } else {
        event_key(session_id, turn_id)
    };
    match engine.sync_on_stop(&event_key) {
        Ok(StopSyncOutcome::Disabled) => HookResponse::message(
            "VRG synchronization skipped: no authorized source roots are configured.",
        ),
        Ok(StopSyncOutcome::InProgress) => HookResponse::quiet(),
        Ok(StopSyncOutcome::SkippedCurrent(_)) => HookResponse::quiet(),
        Ok(StopSyncOutcome::Duplicate(IndexSyncStatus::Failed { .. }))
        | Ok(StopSyncOutcome::Failed(IndexSyncStatus::Failed { .. })) => HookResponse::message(
            "VRG synchronization failed; the last valid generation was retained. Check `vrg status` for degraded state.",
        ),
        Ok(StopSyncOutcome::Completed { report, .. }) if report.indexed > 0 => {
            HookResponse::message(format!(
                "VRG derived index updated incrementally: {} changed source(s); {} unchanged.",
                report.indexed, report.unchanged
            ))
        }
        Ok(StopSyncOutcome::Completed { .. } | StopSyncOutcome::Duplicate(_)) => {
            HookResponse::quiet()
        }
        Ok(StopSyncOutcome::Failed(_)) => HookResponse::message(
            "VRG synchronization failed; the last valid generation was retained where available. Check `vrg status` for degraded state.",
        ),
        Err(_) => HookResponse::message(
            "VRG synchronization failed; the last valid generation was retained where available. Check `vrg status` for degraded state.",
        ),
    }
}

fn run_start_event(event: StopEvent, config_override: Option<PathBuf>) -> HookResponse {
    let Some(session_id) = event.session_id.as_deref() else {
        return HookResponse::message(
            "VRG startup reconciliation skipped: Codex did not provide a session identifier.",
        );
    };
    let config_path = match config_override.or_else(|| env::var_os("VRG_CONFIG").map(PathBuf::from))
    {
        Some(path) => path,
        None => match default_config_path() {
            Ok(path) => path,
            Err(_) => {
                return HookResponse::message(
                    "VRG startup reconciliation failed: VRG configuration path is unavailable.",
                );
            }
        },
    };
    let config = match VrgConfig::load(config_path) {
        Ok(config) => config,
        Err(_) => {
            return HookResponse::message(
                "VRG startup reconciliation failed: configuration could not be loaded.",
            );
        }
    };
    let source = event.source.as_deref().unwrap_or("startup");
    let turn_id = event.turn_id.as_deref().unwrap_or(source);
    if config.roots.is_empty() && !config.capture_session_events {
        return HookResponse::message(
            "VRG startup reconciliation skipped: no authorized source roots are configured.",
        );
    }
    let engine = match VrgEngine::open(config.clone()) {
        Ok(engine) => engine,
        Err(_) => {
            return HookResponse::message(
                "VRG startup reconciliation failed while opening derived storage.",
            );
        }
    };
    if config.capture_session_events
        && engine
            .record_session_event(
                session_id,
                turn_id,
                event.cwd.as_deref().unwrap_or(""),
                "SessionStart",
            )
            .is_err()
    {
        return HookResponse::message("VRG SessionStart event could not be persisted.");
    }
    if config.roots.is_empty() {
        return HookResponse::message(
            "VRG startup reconciliation skipped: no authorized source roots are configured.",
        );
    }
    let key = event_key(session_id, &format!("start:{source}:{turn_id}"));
    match engine.sync_on_stop(&key) {
        Ok(StopSyncOutcome::Failed(_)) | Err(_) => HookResponse::message(
            "VRG startup reconciliation failed; the last valid generation was retained.",
        ),
        Ok(StopSyncOutcome::Completed { report, .. }) if report.indexed > 0 => {
            HookResponse::message(format!(
                "VRG startup reconciliation updated {} changed source(s).",
                report.indexed
            ))
        }
        _ => HookResponse::quiet(),
    }
}

fn event_key(session_id: &str, turn_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(session_id.as_bytes());
    digest.update([0]);
    digest.update(turn_id.as_bytes());
    hex::encode(digest.finalize())
}

fn project_key(cwd: &str) -> String {
    let canonical = PathBuf::from(cwd)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(cwd));
    let mut digest = Sha256::new();
    digest.update(canonical.to_string_lossy().as_bytes());
    hex::encode(digest.finalize())
}

pub(crate) fn hash_project(cwd: &str) -> String {
    project_key(cwd)
}

fn default_config_path() -> crate::VrgResult<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(crate::VrgError::InvalidConfig(
                "XDG_CONFIG_HOME must be absolute".into(),
            ));
        }
        return Ok(path.join("vrg/config.toml"));
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| crate::VrgError::InvalidConfig("HOME is not set".into()))?;
    Ok(home.join(".config/vrg/config.toml"))
}
