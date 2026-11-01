//! Per-turn closure receipts and bounded hook metadata, never conversation content.
use super::{StopEvent, default_config_path, event_key, read_event};
use crate::{
    config::VrgConfig,
    error::{VrgError, VrgResult},
    vault,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const NOTE_NAMES: [&str; 3] = ["STATUS.md", "DECISIONS.md", "SOLUTIONS.md"];
const MAX_RECORDS: i64 = 512;
const MAX_SESSIONS: i64 = 1024;
const CLOSED_STATE_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub(super) fn enabled() -> bool {
    env::var_os("VRG_CLOSURE_ENABLED").is_some_and(|x| x == "1")
}
pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn storage_error(error: impl std::fmt::Display) -> VrgError {
    VrgError::Storage(error.to_string())
}
fn chosen_config(path: Option<&Path>) -> VrgResult<PathBuf> {
    match path
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("VRG_CONFIG").map(PathBuf::from))
    {
        Some(path) => Ok(path),
        None => default_config_path(),
    }
}
fn session_id() -> VrgResult<String> {
    let id = env::var("CODEX_SESSION_ID")
        .map_err(|_| VrgError::AccessDenied("Codex session ID is unavailable".into()))?;
    validate_id(&id)?;
    Ok(id)
}
fn validate_id(id: &str) -> VrgResult<()> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(VrgError::AccessDenied(
            "invalid Codex event identifier".into(),
        ));
    }
    Ok(())
}
fn session_key(id: &str) -> String {
    hex::encode(Sha256::digest(id.as_bytes()))
}
fn ids(event: &StopEvent) -> VrgResult<(&str, &str)> {
    let session = event
        .session_id
        .as_deref()
        .ok_or_else(|| VrgError::AccessDenied("missing session ID".into()))?;
    let turn = event
        .turn_id
        .as_deref()
        .ok_or_else(|| VrgError::AccessDenied("missing turn ID".into()))?;
    validate_id(session)?;
    validate_id(turn)?;
    Ok((session, turn))
}

fn snapshot(config: &VrgConfig) -> VrgResult<String> {
    // A revision comparison is meaningful only for the same authorized scope.
    let scope = serde_json::to_string(&(&config.data_dir, &config.roots)).map_err(storage_error)?;
    if config.roots.is_empty() {
        return Ok(json!({"scope":scope,"revisions":[]}).to_string());
    }
    let root = config
        .roots
        .iter()
        .find(|root| root.id == "vrg-memory")
        .ok_or_else(|| VrgError::AccessDenied("vrg-memory root is not configured".into()))?;
    let canonical = root.path.canonicalize()?;
    if canonical != root.path || !canonical.is_dir() {
        return Err(VrgError::AccessDenied(
            "memory root is not a canonical directory".into(),
        ));
    }
    let names: Vec<&str> = root
        .allowed_files
        .as_ref()
        .map(|files| files.iter().map(String::as_str).collect())
        .unwrap_or_else(|| NOTE_NAMES.to_vec());
    let mut values = Vec::with_capacity(names.len());
    for name in names {
        let path = canonical.join(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => values.push((name, None)),
            Err(error) => return Err(error.into()),
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
                return Err(VrgError::AccessDenied(
                    "memory source is not a regular file".into(),
                ));
            }
            Ok(_) => {
                let source = vault::read_source(config, "vrg-memory", name, 4 * 1024 * 1024)?;
                values.push((name, Some(source.revision)));
            }
        }
    }
    Ok(json!({"scope":scope,"revisions":values}).to_string())
}

fn same_scope(baseline: &str, observed: &str) -> bool {
    let scope = |value: &str| {
        serde_json::from_str::<serde_json::Value>(value)
            .ok()
            .and_then(|v| v.get("scope").and_then(|s| s.as_str()).map(str::to_owned))
    };
    let baseline = scope(baseline);
    baseline.is_some() && baseline == scope(observed)
}
fn open(path: Option<&Path>) -> VrgResult<(VrgConfig, Connection, PathBuf)> {
    let config_path = chosen_config(path)?.canonicalize()?;
    let config = VrgConfig::load(&config_path)?;
    config.validate_layout()?;
    let data = config.data_dir.canonicalize()?;
    if data != config.data_dir {
        return Err(VrgError::AccessDenied(
            "derived directory is not canonical".into(),
        ));
    }
    let data_meta = fs::symlink_metadata(&data)?;
    if !data_meta.is_dir() || data_meta.file_type().is_symlink() {
        return Err(VrgError::AccessDenied(
            "derived directory is not a real directory".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        if data_meta.uid() != unsafe { geteuid() }
            || data_meta.permissions().mode() & 0o777 != 0o700
        {
            return Err(VrgError::AccessDenied(
                "derived directory must already be private and owned by this user".into(),
            ));
        }
    }
    // Only an already-initialized VRG store may acquire closure state.
    let main_db = data.join("vrg.sqlite3");
    let main_meta = fs::symlink_metadata(&main_db)?;
    if main_meta.file_type().is_symlink() || !main_meta.is_file() {
        return Err(VrgError::AccessDenied(
            "VRG derived database is unavailable".into(),
        ));
    }
    let db = data.join("vrg-closure.sqlite3");
    for path in [
        db.clone(),
        data.join("vrg-closure.sqlite3-journal"),
        data.join("vrg-closure.sqlite3-wal"),
        data.join("vrg-closure.sqlite3-shm"),
    ] {
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if meta.file_type().is_symlink() || !meta.is_file() {
                return Err(VrgError::AccessDenied(
                    "closure storage is not a regular file".into(),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if meta.uid() != data_meta.uid()
                    || meta.nlink() != 1
                    || meta.permissions().mode() & 0o077 != 0
                {
                    return Err(VrgError::AccessDenied(
                        "closure storage must be private, owned and not hard-linked".into(),
                    ));
                }
            }
        }
    }
    if !db.exists() {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&db) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    let meta = fs::symlink_metadata(&db)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(VrgError::AccessDenied("invalid closure database".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != data_meta.uid()
            || meta.nlink() != 1
            || meta.permissions().mode() & 0o077 != 0
        {
            return Err(VrgError::AccessDenied(
                "closure database must be private, owned and not hard-linked".into(),
            ));
        }
    }
    let mut conn = Connection::open(&db).map_err(storage_error)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(storage_error)?;
    conn.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE IF NOT EXISTS turn_state (session_key TEXT PRIMARY KEY, turn_key TEXT NOT NULL, baseline TEXT NOT NULL, result TEXT, reason TEXT, proof TEXT, updated_ms INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, event TEXT NOT NULL, session_key TEXT NOT NULL, turn_key TEXT, started_ms INTEGER NOT NULL, finished_ms INTEGER NOT NULL, outcome TEXT NOT NULL, sync_state TEXT NOT NULL);").map_err(storage_error)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let has_continued = {
        let mut columns = tx
            .prepare("PRAGMA table_info(turn_state)")
            .map_err(storage_error)?;
        let names = columns
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(storage_error)?;
        let names = names
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        names.iter().any(|name| name == "continued")
    };
    if !has_continued {
        tx.execute_batch("ALTER TABLE turn_state ADD COLUMN continued INTEGER NOT NULL DEFAULT 0")
            .map_err(storage_error)?;
    }
    tx.commit().map_err(storage_error)?;
    Ok((config, conn, config_path))
}
fn shquote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn run_turn_hook(config_override: Option<PathBuf>) -> VrgResult<()> {
    let started = now_ms();
    let event = match read_event() {
        Ok(Some(e)) if e.hook_event_name == "UserPromptSubmit" => e,
        _ => {
            println!(
                "{{\"systemMessage\":\"VRG documentation check could not initialize for this turn.\"}}"
            );
            return Ok(());
        }
    };
    let result = begin_turn(&event, config_override.as_deref());
    let output = match result {
        Ok((token, config, memory_state)) => {
            let executable = env::current_exe()?;
            let command = format!(
                "cd {} && {} --config {} codex-close --token {}",
                shquote(&env::current_dir()?.to_string_lossy()),
                shquote(&executable.to_string_lossy()),
                shquote(&config.to_string_lossy()),
                token
            );
            let instruction = format!(
                "VRG memory state: {memory_state}. At the end of this turn, decide whether it produced new durable project knowledge. If yes, update an authorized memory file and run {command} --result documented. If no, do not edit memory and run {command} --result no-new --reason REASON, replacing REASON with no-change, trivial, already-recorded, or no-durable-result. No configured memory root means no authorized documentation destination; do not create notes there. A scope mismatch requires configuration repair and a fresh turn, never a false declaration. This records only the declaration, bounded reason and source revisions, not conversation content."
            );
            json!({"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":instruction}})
        }
        Err(error) => {
            let category = match error {
                VrgError::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
                    "configured source or storage path is missing"
                }
                VrgError::Io(ref e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    "source or storage access failed"
                }
                VrgError::InvalidConfig(_) => "configuration is invalid",
                VrgError::AccessDenied(_)
                | VrgError::PathEscape(_)
                | VrgError::SymlinkRejected(_) => {
                    "source authorization or storage safety check failed"
                }
                _ => "closure storage is unavailable",
            };
            json!({"systemMessage":format!("VRG documentation check could not initialize for this turn: {category}.")})
        }
    };
    let mut bytes = serde_json::to_vec(&output).map_err(storage_error)?;
    bytes.push(b'\n');
    let written = std::io::stdout().lock().write_all(&bytes);
    let outcome = if output.get("hookSpecificOutput").is_some() {
        "turn_initialized"
    } else {
        "turn_initialization_failed"
    };
    let logged = audit(&event, config_override.as_deref(), started, outcome);
    if let Err(e) = written {
        return Err(e.into());
    }
    logged
}
fn begin_turn(
    event: &StopEvent,
    path: Option<&Path>,
) -> VrgResult<(String, PathBuf, &'static str)> {
    let (session, turn) = ids(event)?;
    let (config, mut conn, config_path) = open(path)?;
    let baseline = snapshot(&config)?;
    let key = session_key(session);
    let token = event_key(session, turn);
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    // This database contains only our derived turn receipts; never prune source notes.
    tx.execute(
        "DELETE FROM turn_state WHERE updated_ms < ?1 AND session_key != ?2",
        params![now_ms().saturating_sub(CLOSED_STATE_RETENTION_MS), key],
    )
    .map_err(storage_error)?;
    let known: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM turn_state WHERE session_key=?1)",
            [&key],
            |r| r.get(0),
        )
        .map_err(storage_error)?;
    let count: i64 = tx
        .query_row("SELECT COUNT(*) FROM turn_state", [], |r| r.get(0))
        .map_err(storage_error)?;
    if !known && count >= MAX_SESSIONS {
        return Err(VrgError::Storage("closure session limit reached".into()));
    }
    tx.execute("INSERT INTO turn_state(session_key,turn_key,baseline,result,reason,proof,updated_ms) VALUES(?1,?2,?3,NULL,NULL,NULL,?4) ON CONFLICT(session_key) DO UPDATE SET turn_key=excluded.turn_key,baseline=excluded.baseline,result=NULL,reason=NULL,proof=NULL,continued=0,updated_ms=excluded.updated_ms WHERE turn_state.turn_key != excluded.turn_key", params![key, token, baseline, now_ms()]).map_err(storage_error)?;
    tx.commit().map_err(storage_error)?;
    let memory_state = if config.roots.is_empty() {
        "unconfigured (no authorized documentation destination)"
    } else {
        "configured (an empty authorized memory directory is legitimate)"
    };
    Ok((token, config_path, memory_state))
}

pub fn attest_close(
    path: Option<PathBuf>,
    token: &str,
    result: &str,
    reason: Option<&str>,
) -> VrgResult<()> {
    if token.len() != 64
        || !token.bytes().all(|b| b.is_ascii_hexdigit())
        || !matches!(result, "documented" | "no-new")
    {
        return Err(VrgError::AccessDenied(
            "invalid closure token or result".into(),
        ));
    }
    let valid_reason = reason.is_some_and(|reason| {
        matches!(
            reason,
            "no-change" | "trivial" | "already-recorded" | "no-durable-result"
        )
    });
    if (result == "no-new" && !valid_reason) || (result == "documented" && reason.is_some()) {
        return Err(VrgError::AccessDenied(
            "no-new requires a bounded reason; documented must not include one".into(),
        ));
    }
    let session = session_id()?;
    let key = session_key(&session);
    let (config, mut conn, _) = open(path.as_deref())?;
    let observed = snapshot(&config)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let row: Option<(String, String)> = tx
        .query_row(
            "SELECT turn_key,baseline FROM turn_state WHERE session_key=?1",
            [&key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(storage_error)?;
    let (current, baseline) = row
        .ok_or_else(|| VrgError::AccessDenied("no active closure turn for this session".into()))?;
    if current != token {
        return Err(VrgError::AccessDenied(
            "closure token is not current for this session".into(),
        ));
    }
    if !same_scope(&baseline, &observed) {
        return Err(VrgError::Conflict(
            "closure source scope changed; restore the original scope or begin a fresh turn".into(),
        ));
    }
    if (result == "documented") == (baseline == observed) {
        return Err(VrgError::Conflict(
            "declared closure does not match observed memory revision changes".into(),
        ));
    }
    tx.execute("UPDATE turn_state SET result=?1,reason=?2,proof=?3,updated_ms=?4 WHERE session_key=?5 AND turn_key=?6", params![result, reason, observed, now_ms(), key, token]).map_err(storage_error)?;
    tx.commit().map_err(storage_error)?;
    println!(
        "{}",
        json!({"recorded":true,"declaration":result,"note":"hash evidence does not establish authorship or semantic importance"})
    );
    Ok(())
}

pub(super) fn check_stop(event: &StopEvent, path: Option<&Path>) -> VrgResult<String> {
    let (session, turn) = ids(event)?;
    let token = event_key(session, turn);
    let key = session_key(session);
    let (config, mut conn, _) = open(path)?;
    let observed = snapshot(&config)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    let row: Option<(String, Option<String>, Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT turn_key,result,reason,proof FROM turn_state WHERE session_key=?1",
            [&key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(storage_error)?;
    let (current, result, reason, proof) =
        row.ok_or_else(|| VrgError::AccessDenied("missing per-turn closure state".into()))?;
    if current != token || proof.as_deref() != Some(observed.as_str()) {
        return Err(VrgError::AccessDenied(
            "closure proof is missing, stale or changed".into(),
        ));
    }
    let result = result
        .filter(|r| matches!(r.as_str(), "documented" | "no-new"))
        .ok_or_else(|| VrgError::AccessDenied("closure declaration is missing".into()))?;
    if result == "no-new"
        && !reason.as_deref().is_some_and(|reason| {
            matches!(
                reason,
                "no-change" | "trivial" | "already-recorded" | "no-durable-result"
            )
        })
    {
        return Err(VrgError::AccessDenied("bounded reason is missing".into()));
    }
    // Keep the receipt until the next UserPromptSubmit rotates the turn token.
    // A duplicate Stop for the same turn is then idempotent, while a later turn
    // cannot reuse it because its token differs.
    tx.commit().map_err(storage_error)?;
    if result == "no-new" {
        return Ok(format!("no-new:{}", reason.unwrap_or_default()));
    }
    Ok(result)
}

pub(super) fn claim_continuation(event: &StopEvent, path: Option<&Path>) -> VrgResult<bool> {
    let (session, turn) = ids(event)?;
    let (_, conn, _) = open(path)?;
    let changed = conn.execute(
        "UPDATE turn_state SET continued=1 WHERE session_key=?1 AND turn_key=?2 AND continued=0",
        params![session_key(session), event_key(session, turn)],
    ).map_err(storage_error)?;
    Ok(changed == 1)
}

pub(super) fn audit(
    event: &StopEvent,
    path: Option<&Path>,
    started: i64,
    outcome: &str,
) -> VrgResult<()> {
    let session = event
        .session_id
        .as_deref()
        .ok_or_else(|| VrgError::AccessDenied("missing session ID".into()))?;
    validate_id(session)?;
    let turn = event
        .turn_id
        .as_deref()
        .map(|turn| {
            validate_id(turn)?;
            Ok::<_, VrgError>(event_key(session, turn))
        })
        .transpose()?;
    let (config, mut conn, _) = open(path)?;
    // This is a point-in-time observation; another process may have reconciled.
    let sync_state = Connection::open_with_flags(
        config.data_dir.join("vrg.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()
    .and_then(|c| {
        c.query_row(
            "SELECT state FROM index_sync_status WHERE singleton=1",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
    })
    .filter(|state| {
        matches!(
            state.as_str(),
            "up_to_date" | "stale" | "indexing" | "failed"
        )
    })
    .unwrap_or_else(|| "unavailable".into());
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    tx.execute("INSERT INTO events(event,session_key,turn_key,started_ms,finished_ms,outcome,sync_state) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![event.hook_event_name,session_key(session),turn,started,now_ms(),outcome,sync_state]).map_err(storage_error)?;
    tx.execute(
        "DELETE FROM events WHERE id NOT IN (SELECT id FROM events ORDER BY id DESC LIMIT ?1)",
        [MAX_RECORDS],
    )
    .map_err(storage_error)?;
    tx.commit().map_err(storage_error)
}
