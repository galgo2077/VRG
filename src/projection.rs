//! Explicit local projection policies. Schema1 remains withdraw-only baseline.
pub mod updates;
use crate::{
    config::VrgConfig,
    error::{VrgError, VrgResult},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionConfig {
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionManifest {
    pub schema: u32,
    /// One immutable authorization belongs to one canonical private operational store.
    pub data_dir: PathBuf,
    pub original_roots: Vec<PathBuf>,
    pub sources: Vec<ProjectionSource>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionSource {
    pub original_root: PathBuf,
    pub relative_path: String,
    pub expected_sha256: String,
    /// Includes every dependent *view* of project notes, never canonical notes.
    pub views: Vec<ProjectionView>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionView {
    pub root_id: String,
    pub relative_path: String,
    pub sha256: String,
}
fn denied() -> VrgError {
    VrgError::AccessDenied(
        "projection authorization or integrity check failed; no content may be served".into(),
    )
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn parts(path: &Path) -> VrgResult<Vec<std::ffi::OsString>> {
    let mut result = Vec::new();
    for part in path.components() {
        match part {
            Component::Normal(p) => result.push(p.to_os_string()),
            _ => return Err(denied()),
        }
    }
    if result.is_empty() {
        return Err(denied());
    }
    Ok(result)
}
#[cfg(unix)]
fn open_dir(path: &Path) -> VrgResult<File> {
    use rustix::fs::{Mode, OFlags, openat};
    if !path.is_absolute() {
        return Err(denied());
    }
    let mut directory = File::open("/")?;
    for part in parts(path.strip_prefix("/").map_err(|_| denied())?)? {
        directory = File::from(
            openat(
                &directory,
                &part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| denied())?,
        );
    }
    Ok(directory)
}
#[cfg(unix)]
fn parent(root: &File, relative: &str) -> VrgResult<(File, std::ffi::OsString)> {
    use rustix::fs::{Mode, OFlags, openat};
    let mut components = parts(Path::new(relative))?;
    let name = components.pop().ok_or_else(denied)?;
    let mut directory = root.try_clone()?;
    for part in components {
        directory = File::from(
            openat(
                &directory,
                &part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| denied())?,
        );
    }
    Ok((directory, name))
}
#[cfg(unix)]
fn open_file(directory: &File, name: &std::ffi::OsStr) -> VrgResult<Option<File>> {
    use rustix::fs::{Mode, OFlags, openat};
    match openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => {
            let file = File::from(fd);
            if !file.metadata()?.is_file() {
                return Err(denied());
            }
            Ok(Some(file))
        }
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(_) => Err(denied()),
    }
}
fn digest(mut file: File) -> VrgResult<String> {
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}
#[cfg(unix)]
fn private_dir(root: &File, name: &str) -> VrgResult<File> {
    use rustix::fs::{Mode, OFlags, mkdirat, openat};
    use std::os::unix::fs::MetadataExt;
    match mkdirat(root, name, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => (),
        Err(_) => return Err(denied()),
    }
    let dir = File::from(
        openat(
            root,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| denied())?,
    );
    let m = dir.metadata()?;
    if m.mode() & 0o077 != 0 || m.uid() != rustix::process::geteuid().as_raw() {
        return Err(denied());
    }
    Ok(dir)
}
#[cfg(unix)]
fn check_private_file(file: &File) -> VrgResult<()> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != rustix::process::geteuid().as_raw()
        || m.mode() & 0o077 != 0
        || m.nlink() != 1
    {
        return Err(denied());
    }
    Ok(())
}
/// Returns true only if approved views were withdrawn. Original files are opened read-only.
/// Every configured call checks authorization again; allow_stale cannot bypass errors.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn reconcile(config: &VrgConfig) -> VrgResult<bool> {
    use rustix::fs::{Mode, OFlags, RenameFlags, openat, renameat_with};
    use std::os::unix::fs::MetadataExt;
    let Some(pin) = &config.projection_invalidation else {
        return Ok(false);
    };
    if !valid_hash(&pin.manifest_sha256) {
        return Err(denied());
    }
    let manifest_parent = open_dir(pin.manifest_path.parent().ok_or_else(denied)?)?;
    let file = open_file(
        &manifest_parent,
        pin.manifest_path.file_name().ok_or_else(denied)?,
    )?
    .ok_or_else(denied)?;
    let meta = file.metadata()?;
    if meta.mode() & 0o077 != 0 || meta.uid() != rustix::process::geteuid().as_raw() {
        return Err(denied());
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 || hex::encode(Sha256::digest(&bytes)) != pin.manifest_sha256
    {
        return Err(denied());
    }
    let schema: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| denied())?;
    if schema.get("schema").and_then(serde_json::Value::as_u64) == Some(2) {
        return updates::reconcile(config, pin, &bytes);
    }
    let manifest: ProjectionManifest = serde_json::from_slice(&bytes).map_err(|_| denied())?;
    if manifest.schema != 1
        || manifest.data_dir != config.data_dir
        || manifest.data_dir.canonicalize().map_err(|_| denied())? != manifest.data_dir
    {
        return Err(denied());
    }
    let data = open_dir(&config.data_dir)?;
    let work = private_dir(&data, "projection-invalidation")?;
    let lock = File::from(
        openat(
            &work,
            "lock",
            OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| denied())?,
    );
    check_private_file(&lock)?;
    lock.lock().map_err(|_| denied())?;
    let backups = private_dir(&work, "backup")?;
    let roots: BTreeSet<_> = manifest.original_roots.iter().cloned().collect();
    if roots.len() != manifest.original_roots.len() {
        return Err(denied());
    }
    let mut original_handles = BTreeMap::new();
    for root in &roots {
        if root.canonicalize().map_err(|_| denied())? != *root
            || config.data_dir.starts_with(root)
            || root.starts_with(&config.data_dir)
            || config
                .roots
                .iter()
                .any(|r| r.path.starts_with(root) || root.starts_with(&r.path))
        {
            return Err(denied());
        }
        original_handles.insert(root.clone(), open_dir(root)?);
    }
    let mut views: BTreeMap<(String, String), (String, bool)> = BTreeMap::new();
    let mut pending = Vec::new();
    let mut seen_sources = BTreeSet::new();
    for source in &manifest.sources {
        if !valid_hash(&source.expected_sha256)
            || !seen_sources.insert((&source.original_root, &source.relative_path))
        {
            return Err(denied());
        }
        parts(Path::new(&source.relative_path))?;
        let root = original_handles
            .get(&source.original_root)
            .ok_or_else(denied)?;
        // Unavailable or symlinked originals revoke their projections, never gain access.
        let current = parent(root, &source.relative_path)
            .and_then(|(d, n)| open_file(&d, &n))
            .and_then(|f| f.ok_or_else(denied))
            .and_then(digest);
        let changed = current.as_ref().ok() != Some(&source.expected_sha256);
        if changed {
            pending.push(serde_json::json!({"original_root":source.original_root,"relative_path":source.relative_path,"state":"needs_read","expected_sha256":source.expected_sha256}));
        }
        for view in &source.views {
            if !valid_hash(&view.sha256) {
                return Err(denied());
            }
            let entry = views
                .entry((view.root_id.clone(), view.relative_path.clone()))
                .or_insert((view.sha256.clone(), false));
            if entry.0 != view.sha256 {
                return Err(denied());
            }
            entry.1 |= changed;
        }
    }
    // Full preflight: no withdrawal if any admitted view was edited unexpectedly.
    let mut retire = Vec::new();
    for ((root_id, relative), (expected, changed)) in views {
        let root = config
            .roots
            .iter()
            .find(|r| r.id == root_id)
            .ok_or_else(denied)?;
        if root.path.canonicalize().map_err(|_| denied())? != root.path
            || config.data_dir.starts_with(&root.path)
        {
            return Err(denied());
        }
        let root_dir = open_dir(&root.path)?;
        let (directory, name) = parent(&root_dir, &relative)?;
        if let Some(file) = open_file(&directory, &name)? {
            if digest(file)? != expected {
                return Err(denied());
            }
            if changed {
                retire.push((root_id, relative, expected, directory, name));
            }
        }
    }
    // Durable needs_read observation is deliberately separate from semantic reading credit.
    let mut journal = File::from(
        openat(
            &work,
            "journal.jsonl",
            OFlags::CREATE
                | OFlags::APPEND
                | OFlags::WRONLY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| denied())?,
    );
    check_private_file(&journal)?;
    // Pending semantic reading credit is monotonic for this immutable manifest;
    // restoring original bytes never silently credits a retired view as read.
    if let Some(mut previous) = open_file(&work, std::ffi::OsStr::new("needs_read.json"))? {
        let mut text = String::new();
        previous.read_to_string(&mut text)?;
        let receipt: serde_json::Value = serde_json::from_str(&text).map_err(|_| denied())?;
        if receipt["manifest_sha256"].as_str() == Some(pin.manifest_sha256.as_str()) {
            for entry in receipt["sources"].as_array().ok_or_else(denied)? {
                if !pending.contains(entry) {
                    pending.push(entry.clone());
                }
            }
        }
    }
    // Atomic replace is confined to the private operational directory.
    let temp = "needs_read.next.json";
    let mut receipt = File::from(
        openat(
            &work,
            temp,
            OFlags::CREATE
                | OFlags::EXCL
                | OFlags::WRONLY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| denied())?,
    );
    check_private_file(&receipt)?;
    serde_json::to_writer(
        &mut receipt,
        &serde_json::json!({"manifest_sha256":pin.manifest_sha256,"sources":pending}),
    )
    .map_err(|_| denied())?;
    receipt.sync_all()?;
    rustix::fs::renameat(&work, temp, &work, "needs_read.json").map_err(|_| denied())?;
    work.sync_all()?;
    for (root, path, hash, directory, name) in &retire {
        let identity = hex::encode(Sha256::digest(
            format!("{}\0{}\0{}\0{}", pin.manifest_sha256, root, path, hash).as_bytes(),
        ));
        let backup = format!("{identity}.md");
        writeln!(
            journal,
            "{}",
            serde_json::json!({"event":"withdraw_intent","root_id":root,"relative_path":path,"sha256":hash,"backup":backup})
        )?;
        journal.sync_all()?;
        // NOREPLACE preserves any prior backup. EXDEV fails closed; no copy/delete fallback.
        renameat_with(
            directory,
            name,
            &backups,
            backup.as_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(|_| denied())?;
        directory.sync_all()?;
        backups.sync_all()?;
        if digest(open_file(&backups, std::ffi::OsStr::new(&backup))?.ok_or_else(denied)?)? != *hash
        {
            return Err(denied());
        }
        writeln!(
            journal,
            "{}",
            serde_json::json!({"event":"withdrawn","root_id":root,"relative_path":path,"sha256":hash})
        )?;
    }
    journal.sync_all()?;
    work.sync_all()?;
    Ok(!retire.is_empty())
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn reconcile(config: &VrgConfig) -> VrgResult<bool> {
    if config.projection_invalidation.is_some() {
        Err(denied())
    } else {
        Ok(false)
    }
}
