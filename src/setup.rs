//! Conservative setup planning and atomic per-file application.
//! Planning never writes. Existing VRG configuration is never silently replaced.
use crate::{
    config::{RootConfig, SemanticModelConfig, VrgConfig},
    error::{VrgError, VrgResult},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub struct PlannedFile {
    pub path: PathBuf,
    pub content: Vec<u8>,
}
#[derive(Clone, Debug)]
struct Change {
    file: PlannedFile,
    before: Option<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct FilePlan {
    changes: Vec<Change>,
    preconditions: Vec<(PathBuf, Vec<u8>)>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ApplyReport {
    pub changed: usize,
    pub backup: Option<PathBuf>,
}
#[derive(Serialize, Deserialize)]
struct BackupEntry {
    path: PathBuf,
    backup: Option<String>,
    after_sha256: String,
}
#[derive(Serialize, Deserialize)]
struct BackupManifest {
    project: PathBuf,
    config_path: PathBuf,
    entries: Vec<BackupEntry>,
}
#[derive(Clone, Debug)]
pub struct SetupOptions {
    pub project: PathBuf,
    pub roots: Vec<PathBuf>,
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub model_dir: Option<PathBuf>,
    pub memory_dir: Option<PathBuf>,
    pub synthetic_root: bool,
}

fn invalid(message: impl Into<String>) -> VrgError {
    VrgError::InvalidConfig(message.into())
}
fn clean_absolute(path: &Path) -> VrgResult<()> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid(format!(
            "path must be absolute without parent traversal: {}",
            path.display()
        )));
    }
    Ok(())
}
/// Resolve existing ancestors and reject symlinks; do not create missing paths.
fn checked_path(path: &Path) -> VrgResult<PathBuf> {
    clean_absolute(path)?;
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(VrgError::SymlinkRejected(current.display().to_string()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path.to_path_buf())
}
fn current(path: &Path) -> VrgResult<Option<Vec<u8>>> {
    checked_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => Err(invalid(format!(
            "destination is not a regular file: {}",
            path.display()
        ))),
        Ok(_) => Ok(Some(fs::read(path)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
impl FilePlan {
    pub fn new(files: Vec<PlannedFile>) -> VrgResult<Self> {
        let mut seen = HashSet::new();
        let mut changes = Vec::new();
        for file in files {
            if !seen.insert(file.path.clone()) {
                return Err(invalid(format!(
                    "duplicate planned destination: {}",
                    file.path.display()
                )));
            }
            let before = current(&file.path)?;
            if before.as_deref() != Some(file.content.as_slice()) {
                changes.push(Change { file, before });
            }
        }
        Ok(Self {
            changes,
            preconditions: Vec::new(),
        })
    }
    pub fn with_preconditions(mut self, conditions: Vec<(PathBuf, Vec<u8>)>) -> VrgResult<Self> {
        for (path, _) in &conditions {
            checked_path(path)?;
        }
        self.preconditions.extend(conditions);
        Ok(self)
    }
    pub fn changes(&self) -> Vec<&Path> {
        self.changes.iter().map(|c| c.file.path.as_path()).collect()
    }
    /// Backups are private and preimages are verified before each replacement.
    /// A crash can leave a partial transaction; its backup manifest permits manual recovery.
    pub fn apply(
        &self,
        backup_dir: &Path,
        project: &Path,
        config_path: &Path,
    ) -> VrgResult<ApplyReport> {
        ensure_supported_platform()?;
        if self.changes.is_empty() {
            return Ok(ApplyReport {
                changed: 0,
                backup: None,
            });
        }
        let (project, _project_lock) = lock_project(project)?;
        let (config_path, _config_lock) = lock_config(config_path)?;
        checked_path(backup_dir)?;
        private_dirs(backup_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(backup_dir)?.permissions().mode() & 0o077 != 0 {
                return Err(invalid("backup directory must be private (0700)"));
            }
        }
        let lock_path = backup_dir.join("setup.lock");
        checked_path(&lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)?;
        lock.try_lock()
            .map_err(|_| VrgError::Conflict("another setup or repair is active".into()))?;
        for (path, expected) in &self.preconditions {
            if current(path)?.as_deref() != Some(expected.as_slice()) {
                return Err(VrgError::Conflict(format!(
                    "precondition changed since planning: {}",
                    path.display()
                )));
            }
        }
        for c in &self.changes {
            if current(&c.file.path)? != c.before {
                return Err(VrgError::Conflict(format!(
                    "destination changed since planning: {}",
                    c.file.path.display()
                )));
            }
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| invalid(e.to_string()))?
            .as_nanos();
        let backup = backup_dir.join(format!("transaction-{}-{stamp}", std::process::id()));
        private_dirs(&backup)?;
        let mut entries = Vec::new();
        for (i, c) in self.changes.iter().enumerate() {
            let name = c.before.as_ref().map(|_| format!("{i}.before"));
            if let (Some(bytes), Some(name)) = (&c.before, &name) {
                write_new(&backup.join(name), bytes)?;
            }
            entries.push(BackupEntry {
                path: c.file.path.clone(),
                backup: name,
                after_sha256: hex::encode(Sha256::digest(&c.file.content)),
            });
        }
        write_new(
            &backup.join("manifest.json"),
            &serde_json::to_vec_pretty(&BackupManifest {
                project,
                config_path,
                entries,
            })
            .map_err(|e| invalid(e.to_string()))?,
        )?;
        sync_parent(&backup.join("manifest.json"))?;
        let mut applied: Vec<&Change> = Vec::new();
        for c in &self.changes {
            let result = (|| {
                if current(&c.file.path)? != c.before {
                    return Err(VrgError::Conflict(format!(
                        "destination changed during apply: {}",
                        c.file.path.display()
                    )));
                }
                let parent = c
                    .file
                    .path
                    .parent()
                    .ok_or_else(|| invalid("file has no parent"))?;
                private_dirs(parent)?;
                atomic_replace(&c.file.path, &c.file.content)?;
                // Rename already changed the destination; remember it before fsync can fail.
                applied.push(c);
                sync_parent(&c.file.path)
            })();
            if let Err(e) = result {
                for previous in applied.iter().rev() {
                    if current(&previous.file.path).ok().flatten().as_deref()
                        == Some(previous.file.content.as_slice())
                    {
                        match &previous.before {
                            Some(bytes) => {
                                let _ = atomic_replace(&previous.file.path, bytes);
                                let _ = sync_parent(&previous.file.path);
                            }
                            None => {
                                let _ = fs::remove_file(&previous.file.path);
                                let _ = sync_parent(&previous.file.path);
                            }
                        }
                    }
                }
                return Err(VrgError::Recovery(format!(
                    "{e}; inspect backup {}",
                    backup.display()
                )));
            }
        }
        Ok(ApplyReport {
            changed: applied.len(),
            backup: Some(backup),
        })
    }
}
/// Revert only files which still match the exact written revision. Refuses external edits.
pub fn recover_transaction(backup: &Path) -> VrgResult<usize> {
    ensure_supported_platform()?;
    checked_path(backup)?;
    let manifest = current(&backup.join("manifest.json"))?
        .ok_or_else(|| invalid("backup manifest missing"))?;
    let manifest: BackupManifest =
        serde_json::from_slice(&manifest).map_err(|e| invalid(e.to_string()))?;
    let (_, _project_lock) = lock_project(&manifest.project)?;
    let (_, _config_lock) = lock_config(&manifest.config_path)?;
    let backup_dir = backup
        .parent()
        .ok_or_else(|| invalid("backup has no parent"))?;
    let lock_path = backup_dir.join("setup.lock");
    checked_path(&lock_path)?;
    let lock = OpenOptions::new().read(true).write(true).open(&lock_path)?;
    lock.try_lock()
        .map_err(|_| VrgError::Conflict("another setup or repair is active".into()))?;
    let mut todo = Vec::new();
    for entry in manifest.entries {
        let before = match &entry.backup {
            Some(name)
                if name.ends_with(".before")
                    && name
                        .trim_end_matches(".before")
                        .bytes()
                        .all(|b| b.is_ascii_digit())
                    && name != ".before" =>
            {
                current(&backup.join(name))?
                    .ok_or_else(|| invalid("backup preimage missing"))
                    .map(Some)?
            }
            Some(_) => return Err(invalid("invalid backup entry")),
            None => None,
        };
        let now = current(&entry.path)?;
        if now == before {
            continue;
        }
        if now.as_ref().map(|bytes| hex::encode(Sha256::digest(bytes))) != Some(entry.after_sha256)
        {
            return Err(VrgError::Conflict(format!(
                "cannot recover externally changed file: {}",
                entry.path.display()
            )));
        }
        todo.push((entry.path, before, now));
    }
    let count = todo.len();
    for (path, before, expected) in todo.into_iter().rev() {
        if current(&path)? != expected {
            return Err(VrgError::Conflict(format!(
                "file changed during recovery: {}",
                path.display()
            )));
        }
        match before {
            Some(bytes) => atomic_replace(&path, &bytes)?,
            None => fs::remove_file(&path)?,
        }
        sync_parent(&path)?;
    }
    Ok(count)
}
/// Serializes cooperating setup and repair operations on this project only.
/// Operations from separate projects sharing a destination are not serialized.
fn lock_project(project: &Path) -> VrgResult<(PathBuf, File)> {
    let path = checked_path(project)?.canonicalize()?;
    if !path.is_dir() {
        return Err(invalid("project must be an existing directory"));
    }
    let lock = File::open(&path)?;
    lock.try_lock().map_err(|_| {
        VrgError::Conflict("another setup or recovery is active for this project".into())
    })?;
    Ok((path, lock))
}
/// Cooperating transactions with the same configuration share this lock even
/// when they use different projects and data directories.
/// Reject unsupported systems before any setup or download side effects.
pub fn ensure_supported_platform() -> VrgResult<()> {
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        Ok(())
    } else {
        Err(invalid(
            "safe configuration locking is currently supported on Linux and macOS",
        ))
    }
}
fn lock_config(config_path: &Path) -> VrgResult<(PathBuf, File)> {
    ensure_supported_platform()?;
    checked_path(config_path)?;
    let parent = config_path
        .parent()
        .ok_or_else(|| invalid("configuration has no parent"))?;
    private_dirs(parent)?;
    let parent = checked_path(parent)?.canonicalize()?;
    let name = config_path
        .file_name()
        .ok_or_else(|| invalid("configuration has no filename"))?;
    let normalized = parent.join(name);
    checked_path(&normalized)?;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (parent, name);
        return Err(invalid(
            "safe configuration locking is currently supported on Linux and macOS",
        ));
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        };
        let lock_path = parent.join(format!(
            ".vrg-config-{}.lock",
            hex::encode(Sha256::digest(name.as_bytes()))
        ));
        checked_path(&lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&lock_path)?;
        let meta = lock.metadata()?;
        if !meta.is_file() || meta.nlink() != 1 || meta.mode() & 0o077 != 0 {
            return Err(invalid(
                "configuration lock must be a private regular file with one link",
            ));
        }
        lock.try_lock().map_err(|_| {
            VrgError::Conflict("another setup or recovery uses this configuration".into())
        })?;
        Ok((normalized, lock))
    }
}
pub(crate) fn private_dirs(path: &Path) -> VrgResult<()> {
    checked_path(path)?;
    if path.exists() {
        if !path.is_dir() {
            return Err(invalid("directory destination is not a directory"));
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        private_dirs(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> VrgResult<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
fn atomic_replace(path: &Path, bytes: &[u8]) -> VrgResult<()> {
    let parent = path.parent().ok_or_else(|| invalid("file has no parent"))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| invalid(e.to_string()))?
        .as_nanos();
    let temp = parent.join(format!(".vrg-write-{}-{stamp}", std::process::id()));
    write_new(&temp, bytes)?;
    #[cfg(unix)]
    if let Ok(metadata) = fs::metadata(path) {
        fs::set_permissions(&temp, metadata.permissions())?;
    }
    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(temp);
        return Err(e.into());
    }
    Ok(())
}
fn sync_parent(path: &Path) -> VrgResult<()> {
    let parent = path.parent().ok_or_else(|| invalid("file has no parent"))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn plan_setup(options: &SetupOptions, mut extra: Vec<PlannedFile>) -> VrgResult<FilePlan> {
    let project = checked_path(&options.project)?.canonicalize()?;
    if !project.is_dir() {
        return Err(invalid("project must be an existing directory"));
    }
    if std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|p| p.canonicalize().ok())
        .as_ref()
        == Some(&project)
    {
        return Err(invalid(
            "home directory cannot be used implicitly as a project",
        ));
    }
    let data = checked_path(&options.data_dir)?;
    checked_path(&options.config_path)?;
    if options.roots.is_empty() {
        return Err(invalid(
            "at least one explicitly authorized root is required",
        ));
    }
    let mut roots = Vec::new();
    for (i, p) in options.roots.iter().enumerate() {
        let checked = checked_path(p)?;
        let sample = options.synthetic_root && checked == project.join(".vrg-synthetic-notes");
        let root = if sample && !checked.exists() {
            checked
        } else {
            checked.canonicalize()?
        };
        if !sample && !root.is_dir() {
            return Err(invalid("source root must be a directory"));
        }
        if sample {
            if root.exists() && !root.is_dir() {
                return Err(invalid("synthetic source is not a directory"));
            }
            let path = root.join("sample.md");
            let content =
                b"# Synthetic VRG sample\n\nThis is an isolated example, not a personal note.\n";
            match current(&path)? {
                None => extra.push(PlannedFile {
                    path,
                    content: content.to_vec(),
                }),
                Some(existing) if existing == content => {}
                Some(_) => {
                    return Err(VrgError::Conflict(
                        "existing synthetic sample has been modified".into(),
                    ));
                }
            }
        }
        if roots
            .iter()
            .any(|r: &RootConfig| root.starts_with(&r.path) || r.path.starts_with(&root))
        {
            return Err(invalid("source roots overlap"));
        }
        roots.push(RootConfig {
            id: format!("root-{}", i + 1),
            path: root,
            exclusions: vec![],
            allowed_files: if sample {
                Some(vec!["sample.md".into()])
            } else {
                None
            },
        });
    }
    if let Some(path) = &options.memory_dir {
        let memory = checked_path(path)?;
        if !memory.starts_with(&project) || memory == project {
            return Err(invalid(
                "memory directory must be inside the selected project",
            ));
        }
        if roots
            .iter()
            .any(|r| memory.starts_with(&r.path) || r.path.starts_with(&memory))
        {
            return Err(invalid("memory directory overlaps another source root"));
        }
        for (name, title) in [
            ("STATUS.md", "Status"),
            ("DECISIONS.md", "Decisions"),
            ("SOLUTIONS.md", "Solutions"),
        ] {
            let destination = memory.join(name);
            if current(&destination)?.is_none() {
                extra.push(PlannedFile {
                    path: destination,
                    content: format!("# {title}\n").into_bytes(),
                });
            }
        }
        roots.push(RootConfig {
            id: "vrg-memory".into(),
            path: memory,
            exclusions: vec![],
            allowed_files: Some(vec![
                "STATUS.md".into(),
                "DECISIONS.md".into(),
                "SOLUTIONS.md".into(),
            ]),
        });
    }
    let model = match &options.model_dir {
        Some(path) => {
            let path = checked_path(path)?;
            Some(SemanticModelConfig {
                model_id: "intfloat/multilingual-e5-small".into(),
                model_dir: path,
                threads: 4,
                max_length: 512,
            })
        }
        None => None,
    };
    let config = VrgConfig {
        roots,
        data_dir: data,
        semantic_model: model,
        ..VrgConfig::default()
    };
    config.validate()?;
    config.validate_layout()?;
    let generated = toml::to_string_pretty(&config).map_err(|e| invalid(e.to_string()))?;
    if let Some(existing) = current(&options.config_path)? {
        let old: VrgConfig =
            toml::from_str(std::str::from_utf8(&existing).map_err(|e| invalid(e.to_string()))?)
                .map_err(|e| invalid(e.to_string()))?;
        // Equality through normalized serialization leaves user comments and formatting intact.
        if toml::to_string(&old).map_err(|e| invalid(e.to_string()))?
            != toml::to_string(&config).map_err(|e| invalid(e.to_string()))?
        {
            return Err(VrgError::Conflict(
                "existing VRG configuration differs; review it manually before setup".into(),
            ));
        }
    } else {
        extra.push(PlannedFile {
            path: options.config_path.clone(),
            content: generated.into_bytes(),
        });
    }
    FilePlan::new(extra)
}
