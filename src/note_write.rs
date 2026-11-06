use crate::{
    config::VrgConfig,
    contracts::WritePlan,
    error::{VrgError, VrgResult},
    vault,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, io::{Read, Write}, path::{Component, Path}};

const MAX_PROPOSED_BYTES: usize = 1_000_000;

pub fn prepare_plan(
    config: &VrgConfig,
    root_id: &str,
    relative_path: &str,
    proposed_markdown: String,
    reason: String,
) -> VrgResult<WritePlan> {
    if proposed_markdown.len() > MAX_PROPOSED_BYTES {
        return Err(VrgError::InvalidConfig(
            "proposed note exceeds the review-plan size limit".into(),
        ));
    }
    if reason.trim().is_empty() || reason.len() > 2048 {
        return Err(VrgError::InvalidConfig(
            "write plan requires a bounded non-empty reason".into(),
        ));
    }
    let source = vault::read_source(config, root_id, relative_path, MAX_PROPOSED_BYTES)?;
    Ok(WritePlan {
        source: source.source,
        expected_revision: source.revision,
        proposed_markdown,
        reason,
    })
}

/// Source-note application is deliberately unavailable until separately
/// authorized and certified; rollback/journaling of derived state is separate.
pub fn apply_disabled(_plan: &WritePlan) -> VrgResult<()> {
    Err(VrgError::WritesDisabled(
        "source-note writes require separate authorization and certification".into(),
    ))
}

/// Private, opt-in source write used only by the shared server's managed route.
/// MCP's source-note write surface remains disabled.
#[derive(Debug, Serialize)]
pub struct ManagedReceipt {
    pub revision: String,
    pub previous_revision: String,
    pub idempotent: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    operation_id: String,
    relative_path: String,
    expected_revision: String,
    new_revision: String,
}

fn denied() -> VrgError {
    VrgError::AccessDenied("managed note write authorization or integrity failed".into())
}
fn hash(bytes: &[u8]) -> String { hex::encode(Sha256::digest(bytes)) }
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(unix)]
fn secure_dir(path: &Path) -> VrgResult<File> {
    use rustix::fs::{openat, Mode, OFlags};
    if !path.is_absolute() { return Err(denied()); }
    let mut current = File::open("/")?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            if component == Component::RootDir { continue; }
            return Err(denied());
        };
        current = File::from(openat(&current, name, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()).map_err(|_| denied())?);
    }
    Ok(current)
}

#[cfg(unix)]
fn open_at(dir: &File, name: &str) -> VrgResult<Option<File>> {
    use rustix::fs::{openat, Mode, OFlags};
    match openat(dir, name, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) {
        Ok(fd) => Ok(Some(File::from(fd))),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(_) => Err(denied()),
    }
}

#[cfg(unix)]
fn checked_file(file: &File, private: bool) -> VrgResult<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != rustix::process::geteuid().as_raw()
        || (private && meta.mode() & 0o077 != 0) {
        return Err(denied());
    }
    Ok(())
}

#[cfg(unix)]
fn read_at(dir: &File, name: &str, private: bool) -> VrgResult<Option<Vec<u8>>> {
    let Some(mut file) = open_at(dir, name)? else { return Ok(None); };
    checked_file(&file, private)?;
    let mut bytes = Vec::new();
    file.take((MAX_PROPOSED_BYTES + 4096) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_PROPOSED_BYTES + 4095 { return Err(denied()); }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn create_at(dir: &File, name: &str, bytes: &[u8]) -> VrgResult<()> {
    use rustix::fs::{openat, Mode, OFlags};
    let mut file = File::from(openat(dir, name, OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::from_raw_mode(0o600)).map_err(|_| denied())?);
    checked_file(&file, true)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    dir.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn complete(data: &File, marker: &str) -> VrgResult<()> {
    if read_at(data, marker, true)?.is_none() { create_at(data, marker, b"")?; }
    Ok(())
}

/// Compare and replace an existing authorized memory note under a per-note
/// cross-process lock. The old bytes and intent are durable before rename;
/// an interrupted operation can be retried using the same operation ID.
#[cfg(unix)]
pub fn apply_managed(config: &VrgConfig, relative_path: &str, expected_revision: &str, proposed_markdown: &str, operation_id: &str) -> VrgResult<ManagedReceipt> {
    use rustix::fs::{flock, openat, renameat, FlockOperation, Mode, OFlags};
    use std::os::unix::fs::MetadataExt;
    if !valid_hash(expected_revision) || !valid_hash(operation_id)
        || proposed_markdown.len() > MAX_PROPOSED_BYTES || proposed_markdown.contains('\0')
        || !matches!(relative_path, "STATUS.md" | "DECISIONS.md" | "SOLUTIONS.md") {
        return Err(denied());
    }
    config.validate_layout()?;
    let root = config.roots.iter().find(|r| matches!(r.id.as_str(), "vrg-memory" | "auto-memory")).ok_or_else(denied)?;
    if !root.allowed_files.as_ref().is_some_and(|files| files.iter().any(|f| f == relative_path))
        || root.path.canonicalize()? != root.path || config.data_dir.canonicalize()? != config.data_dir {
        return Err(denied());
    }
    let source_dir = secure_dir(&root.path)?;
    let data_dir = secure_dir(&config.data_dir)?;
    let data_meta = data_dir.metadata()?;
    if data_meta.uid() != rustix::process::geteuid().as_raw() || data_meta.mode() & 0o077 != 0 { return Err(denied()); }
    let lock_name = format!("vrg-note-{}.lock", hash(format!("{}\0{relative_path}", root.path.display()).as_bytes()));
    let lock = File::from(openat(&data_dir, lock_name.as_str(), OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::from_raw_mode(0o600)).map_err(|_| denied())?);
    checked_file(&lock, true)?;
    flock(&lock, FlockOperation::LockExclusive).map_err(|_| denied())?;

    let current = read_at(&source_dir, relative_path, false)?.ok_or_else(denied)?;
    if current.len() > MAX_PROPOSED_BYTES { return Err(denied()); }
    let current_revision = hash(&current);
    let new_revision = hash(proposed_markdown.as_bytes());
    let intent = Intent { operation_id: operation_id.into(), relative_path: relative_path.into(), expected_revision: expected_revision.into(), new_revision: new_revision.clone() };
    let intent_name = format!("vrg-note-{operation_id}.json");
    let done_name = format!("vrg-note-{operation_id}.done");
    let backup_name = format!("vrg-note-{operation_id}.bak");
    if let Some(existing) = read_at(&data_dir, &intent_name, true)? {
        let recorded: Intent = serde_json::from_slice(&existing).map_err(|_| denied())?;
        if recorded.operation_id != intent.operation_id || recorded.relative_path != intent.relative_path
            || recorded.expected_revision != intent.expected_revision || recorded.new_revision != intent.new_revision
            || read_at(&data_dir, &backup_name, true)?.as_deref().map(hash).as_deref() != Some(expected_revision) {
            return Err(denied());
        }
        if read_at(&data_dir, &done_name, true)?.is_some() {
            return Ok(ManagedReceipt { revision:new_revision, previous_revision:expected_revision.into(), idempotent:true });
        }
        if current_revision == new_revision {
            complete(&data_dir, &done_name)?;
            return Ok(ManagedReceipt { revision:new_revision, previous_revision:expected_revision.into(), idempotent:true });
        }
    }
    if current_revision != expected_revision {
        return Err(VrgError::Conflict(format!("REVISION_CONFLICT:{current_revision}")));
    }
    // The source must still be authorized and public according to the native reader.
    if vault::read_source(config, &root.id, relative_path, MAX_PROPOSED_BYTES)?.revision != current_revision { return Err(VrgError::Conflict(format!("REVISION_CONFLICT:{current_revision}"))); }
    if read_at(&data_dir, &intent_name, true)?.is_none() {
        match read_at(&data_dir, &backup_name, true)? {
            Some(bytes) if hash(&bytes) == expected_revision => {},
            Some(_) => return Err(denied()),
            None => create_at(&data_dir, &backup_name, &current)?,
        }
        create_at(&data_dir, &intent_name, &serde_json::to_vec(&intent).map_err(|_| denied())?)?;
    }
    let temp_name = format!(".vrg-{operation_id}.tmp");
    match read_at(&source_dir, &temp_name, true)? {
        Some(bytes) if hash(&bytes) == new_revision => {},
        Some(_) => return Err(denied()),
        None => create_at(&source_dir, &temp_name, proposed_markdown.as_bytes())?,
    }
    let latest = read_at(&source_dir, relative_path, false)?.ok_or_else(denied)?;
    if hash(&latest) != expected_revision { return Err(VrgError::Conflict(format!("REVISION_CONFLICT:{}", hash(&latest)))); }
    renameat(&source_dir, temp_name.as_str(), &source_dir, relative_path).map_err(|_| denied())?;
    source_dir.sync_all()?;
    complete(&data_dir, &done_name)?;
    Ok(ManagedReceipt { revision:new_revision, previous_revision:expected_revision.into(), idempotent:false })
}
