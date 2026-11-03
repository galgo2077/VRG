//! Read-only adapter for the existing pinned source registry. Never reconciles,
//! indexes or rewrites projections. Frozen source revisions remain authoritative.
use crate::{config::VrgConfig, contracts::SourceRef};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

type Result<T> = std::result::Result<T, &'static str>;
fn source_registry_path(
    override_path: Option<std::ffi::OsString>,
    config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> PathBuf {
    if let Some(path) = override_path {
        return PathBuf::from(path);
    }
    config_home
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home.unwrap_or_default()).join(".config"))
        .join("vrg/viewer-sources.json")
}
fn registry_path() -> PathBuf {
    source_registry_path(
        std::env::var_os("VRG_VIEW_SOURCE_REGISTRY"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}
const MAX_METADATA: usize = 64 * 1024 * 1024;
const MAX_SOURCE: usize = 64 * 1024 * 1024;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRegistration {
    #[serde(rename = "project_id")]
    pub label: String,
    pub project_root: Option<PathBuf>,
    pub selection_kind: String,
    #[serde(rename = "config_path")]
    pub profile: PathBuf,
    pub config_sha256: String,
    #[serde(default)]
    pub aliases: Vec<PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    binary_path: PathBuf,
    binary_sha256: String,
    profiles: Vec<SourceRegistration>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    manifest_path: PathBuf,
    manifest_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    data_dir: PathBuf,
    original_roots: Vec<PathBuf>,
    sources: Vec<Dependency>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dependency {
    original_root: PathBuf,
    relative_path: String,
    expected_sha256: String,
    views: Vec<View>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct View {
    root_id: String,
    relative_path: String,
    sha256: String,
}
#[derive(Clone)]
struct ViewGrant {
    sha256: String,
    dependencies: Vec<usize>,
    metadata: Value,
}
struct Policy {
    profile: PathBuf,
    pin: Pin,
    dependencies: Vec<Dependency>,
    views: BTreeMap<(String, String), ViewGrant>,
    pending: BTreeSet<(PathBuf, String)>,
    pending_hash: String,
    stamp: String,
    head_path: Option<PathBuf>,
    head_hash: String,
    indexed_generation: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
struct Stamp(u64, u64, u64, i64, i64, i64, i64);
static HASHES: OnceLock<Mutex<BTreeMap<PathBuf, (Stamp, String)>>> = OnceLock::new();
static POLICIES: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Policy>>>> = OnceLock::new();
fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn hash_valid(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn relative(p: &str) -> Result<()> {
    if p.is_empty()
        || p.len() > 4096
        || p.contains('\\')
        || p.chars().any(char::is_control)
        || p.split('/').any(|p| p.is_empty() || p == "." || p == "..")
        || !Path::new(p)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err("ROOT_REVOKED");
    }
    Ok(())
}
// The same component-by-component NOFOLLOW approach as the native projection
// validator, without its journal, withdrawal or recovery writes.
fn open(path: &Path) -> Result<File> {
    use rustix::fs::{Mode, OFlags, openat};
    if !path.is_absolute() {
        return Err("ROOT_REVOKED");
    }
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err("ROOT_REVOKED");
    }
    let parts: Vec<_> = components.collect();
    let mut file = File::open("/").map_err(|_| "ROOT_REVOKED")?;
    for (i, component) in parts.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err("ROOT_REVOKED");
        };
        let flags = OFlags::RDONLY
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC
            | OFlags::NONBLOCK
            | if i + 1 < parts.len() {
                OFlags::DIRECTORY
            } else {
                OFlags::empty()
            };
        file = File::from(openat(&file, *name, flags, Mode::empty()).map_err(|_| "ROOT_REVOKED")?);
    }
    Ok(file)
}
fn canonical_directory(path: &Path) -> Result<()> {
    let file = open(path)?;
    if !file.metadata().map_err(|_| "ROOT_REVOKED")?.is_dir() {
        return Err("ROOT_REVOKED");
    }
    Ok(())
}
fn stamp(file: &File) -> Result<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().map_err(|_| "ROOT_REVOKED")?;
    if !m.is_file() {
        return Err("ROOT_REVOKED");
    }
    Ok(Stamp(
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}
fn read_private(path: &Path, max: usize) -> Result<Vec<u8>> {
    read_regular(path, max, true)
}
fn read_regular(path: &Path, max: usize, private: bool) -> Result<Vec<u8>> {
    use std::os::unix::fs::MetadataExt;
    let file = open(path)?;
    let m = file.metadata().map_err(|_| "ROOT_REVOKED")?;
    if !m.is_file()
        || (private && (m.uid() != rustix::process::geteuid().as_raw() || m.mode() & 0o077 != 0))
        || m.len() > max as u64
    {
        return Err("ROOT_REVOKED");
    }
    let before = stamp(&file)?;
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "ROOT_REVOKED")?;
    if bytes.len() > max || stamp(&open(path)?)? != before {
        return Err("ROOT_REVOKED");
    }
    Ok(bytes)
}
fn current_private_hash(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let m = open(path)?.metadata().map_err(|_| "ROOT_REVOKED")?;
    if !m.is_file() || m.uid() != rustix::process::geteuid().as_raw() || m.mode() & 0o077 != 0 {
        return Err("ROOT_REVOKED");
    }
    current_hash(path)
}
fn current_hash(path: &Path) -> Result<String> {
    let mut file = open(path)?;
    let before = stamp(&file)?;
    if before.2 > MAX_SOURCE as u64 {
        return Err("ROOT_REVOKED");
    }
    let hashes = HASHES.get_or_init(Default::default);
    if let Some((known, hash)) = hashes.lock().map_err(|_| "INTERNAL_ERROR")?.get(path) {
        if *known == before {
            return Ok(hash.clone());
        }
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    let mut read_bytes = 0usize;
    loop {
        let size = file.read(&mut buffer).map_err(|_| "ROOT_REVOKED")?;
        if size == 0 {
            break;
        }
        read_bytes = read_bytes.saturating_add(size);
        if read_bytes > MAX_SOURCE {
            return Err("ROOT_REVOKED");
        }
        digest.update(&buffer[..size]);
    }
    if stamp(&file)? != before || stamp(&open(path)?)? != before {
        return Err("ROOT_REVOKED");
    }
    let hash = hex::encode(digest.finalize());
    hashes
        .lock()
        .map_err(|_| "INTERNAL_ERROR")?
        .insert(path.into(), (before, hash.clone()));
    Ok(hash)
}
fn registry() -> Result<Vec<SourceRegistration>> {
    // Resolve only the explicit override or this installation's XDG config.
    // No historical deployment path, directory discovery or host-state fallback.
    let path = registry_path();
    if !path.exists() {
        return if std::env::var_os("VRG_VIEW_SOURCE_REGISTRY").is_some() {
            Err("ROOT_REVOKED")
        } else {
            Ok(Vec::new())
        };
    }
    let raw = read_private(&path, 4 * 1024 * 1024)?;
    let value: Value = serde_json::from_slice(&raw).map_err(|_| "ROOT_REVOKED")?;
    let registry: Registry = if value.get("schema").is_some() {
        if value["schema"] != 1 {
            return Err("ROOT_REVOKED");
        }
        let profiles = value["profiles"]
            .as_array()
            .ok_or("ROOT_REVOKED")?
            .iter()
            .map(|p| {
                let text = |name: &str| p[name].as_str().map(str::to_owned).ok_or("ROOT_REVOKED");
                let aliases = p["aliases"]
                    .as_array()
                    .ok_or("ROOT_REVOKED")?
                    .iter()
                    .map(|v| v.as_str().map(PathBuf::from).ok_or("ROOT_REVOKED"))
                    .collect::<Result<Vec<_>>>()?;
                Ok(SourceRegistration {
                    label: text("project_id")?,
                    project_root: Some(PathBuf::from(text("project_root")?)),
                    selection_kind: "canonical_project".into(),
                    profile: PathBuf::from(text("config_path")?),
                    config_sha256: text("config_sha256")?,
                    aliases,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Registry {
            binary_path: PathBuf::from(value["binary_path"].as_str().ok_or("ROOT_REVOKED")?),
            binary_sha256: value["binary_sha256"]
                .as_str()
                .ok_or("ROOT_REVOKED")?
                .into(),
            profiles,
        }
    } else {
        serde_json::from_value(value).map_err(|_| "ROOT_REVOKED")?
    };
    // The viewer does not execute the registered retrieval binary; its path/hash
    // remain manifest metadata, not authority for a different binary to run.
    if !registry.binary_path.is_absolute() || !hash_valid(&registry.binary_sha256) {
        return Err("ROOT_REVOKED");
    }
    let mut ids = BTreeSet::new();
    let mut roots = BTreeSet::new();
    for p in &registry.profiles {
        if p.label.is_empty()
            || p.label.len() > 256
            || !ids.insert(p.label.clone())
            || !hash_valid(&p.config_sha256)
            || !p.profile.is_absolute()
        {
            return Err("ROOT_REVOKED");
        }
        for alias in &p.aliases {
            canonical_directory(alias)?;
        }
        match (&p.project_root, p.selection_kind.as_str()) {
            (Some(root), "canonical_project") if roots.insert(root.clone()) => {
                canonical_directory(root)?
            }
            (None, "explicit_private_system_context") if p.label == "linux-system" => (),
            _ => return Err("ROOT_REVOKED"),
        }
    }
    Ok(registry.profiles)
}
pub fn explicit_contexts() -> Result<Vec<String>> {
    Ok(registry()?
        .into_iter()
        .filter(|p| p.selection_kind != "canonical_project")
        .map(|p| p.label)
        .collect())
}
/// Full dependency provenance, independent of a copied profile's store path.
/// Absolute original roots distinguish identical relative paths/content on disk.
fn canonical_source_identity<'a>(dependencies: impl IntoIterator<Item = &'a Dependency>) -> String {
    let mut tuples: Vec<_> = dependencies
        .into_iter()
        .map(|d| {
            (
                &d.original_root,
                d.relative_path.as_str(),
                d.expected_sha256.as_str(),
            )
        })
        .collect();
    tuples.sort();
    tuples.dedup();
    let mut digest = Sha256::new();
    digest.update(b"vrg-source-origins-v1");
    for (root, path, revision) in tuples {
        for bytes in [
            root.as_os_str().as_encoded_bytes(),
            path.as_bytes(),
            revision.as_bytes(),
        ] {
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        }
    }
    hex::encode(digest.finalize())
}
fn hierarchy_path(source: &Dependency, project_root: Option<&PathBuf>) -> String {
    if let Some(root) = project_root {
        if let Ok(prefix) = source.original_root.strip_prefix(root) {
            prefix
                .join(&source.relative_path)
                .to_string_lossy()
                .into_owned()
        } else {
            PathBuf::from(source.original_root.file_name().unwrap_or_default())
                .join(&source.relative_path)
                .to_string_lossy()
                .into_owned()
        }
    } else {
        source.relative_path.clone()
    }
}
pub fn registered_projects() -> Result<Vec<SourceRegistration>> {
    Ok(registry()?
        .into_iter()
        .filter(|p| p.selection_kind == "canonical_project")
        .collect())
}
fn registration(profile: &Path, bytes: &[u8]) -> Result<SourceRegistration> {
    verify_registration(profile, bytes, registry()?)
}
fn verify_registration(
    profile: &Path,
    bytes: &[u8],
    registrations: Vec<SourceRegistration>,
) -> Result<SourceRegistration> {
    let registration = registrations
        .into_iter()
        .find(|p| p.profile == profile)
        .ok_or("ROOT_REVOKED")?;
    if sha(bytes) != registration.config_sha256 {
        return Err("ROOT_REVOKED");
    }
    Ok(registration)
}
fn parse_pin(raw: &[u8]) -> Result<Option<Pin>> {
    let value: toml::Value = toml::from_str(std::str::from_utf8(raw).map_err(|_| "ROOT_REVOKED")?)
        .map_err(|_| "ROOT_REVOKED")?;
    value
        .get("projection_invalidation")
        .cloned()
        .map(|v| v.try_into().map_err(|_| "ROOT_REVOKED"))
        .transpose()
}
/// Keep the engine configuration contract unchanged. Strip only the separately
/// validated viewer source-policy extension, never other unknown configuration.
pub fn parse_config(profile: &Path, raw: &[u8]) -> Result<VrgConfig> {
    let mut value: toml::Value =
        toml::from_str(std::str::from_utf8(raw).map_err(|_| "ROOT_REVOKED")?)
            .map_err(|_| "ROOT_REVOKED")?;
    if value.get("projection_invalidation").is_some() {
        registration(profile, raw)?;
        let pin = parse_pin(raw)?.ok_or("ROOT_REVOKED")?;
        if !hash_valid(&pin.manifest_sha256) {
            return Err("ROOT_REVOKED");
        }
        value
            .as_table_mut()
            .ok_or("ROOT_REVOKED")?
            .remove("projection_invalidation");
    } else if registry()?.iter().any(|p| p.profile == profile) {
        return Err("ROOT_REVOKED");
    }
    value.try_into().map_err(|_| "ROOT_REVOKED")
}
fn optional_private_hash(path: &Path) -> Result<Option<String>> {
    let parent = path.parent().ok_or("ROOT_REVOKED")?;
    match std::fs::symlink_metadata(parent) {
        Ok(_) => canonical_directory(parent)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("ROOT_REVOKED"),
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => current_private_hash(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("ROOT_REVOKED"),
    }
}
fn pending(config: &VrgConfig, pin: &Pin) -> Result<(BTreeSet<(PathBuf, String)>, String)> {
    let file = config
        .data_dir
        .join("projection-invalidation/needs_read.json");
    if optional_private_hash(&file)?.is_none() {
        return Ok((BTreeSet::new(), String::new()));
    }
    let bytes = read_private(&file, MAX_METADATA)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "ROOT_REVOKED")?;
    if value["manifest_sha256"] != pin.manifest_sha256 {
        return Err("ROOT_REVOKED");
    }
    let mut pending = BTreeSet::new();
    for source in value["sources"].as_array().ok_or("ROOT_REVOKED")? {
        pending.insert((
            PathBuf::from(source["original_root"].as_str().ok_or("ROOT_REVOKED")?),
            source["relative_path"]
                .as_str()
                .ok_or("ROOT_REVOKED")?
                .into(),
        ));
    }
    Ok((pending, sha(&bytes)))
}
fn dependency_current(dependency: &Dependency, pending: &BTreeSet<(PathBuf, String)>) -> bool {
    !pending.contains(&(
        dependency.original_root.clone(),
        dependency.relative_path.clone(),
    )) && current_hash(&dependency.original_root.join(&dependency.relative_path))
        .as_ref()
        .ok()
        == Some(&dependency.expected_sha256)
}
// Normalize the already-authorized schema2 HEAD into read-only source grants.
// HEAD may select known source revisions/views; it cannot introduce roots or IDs.
fn schema_two_manifest(
    config: &VrgConfig,
    pin: &Pin,
    bytes: &[u8],
) -> Result<(Manifest, Vec<View>, PathBuf, String, String)> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "ROOT_REVOKED")?;
    let text = |v: &Value, key: &str| v[key].as_str().map(str::to_owned).ok_or("ROOT_REVOKED");
    let array = |v: &Value, key: &str| v[key].as_array().cloned().ok_or("ROOT_REVOKED");
    if value["schema"] != 2
        || PathBuf::from(text(&value, "data_dir")?) != config.data_dir
        || config.roots.len() != 1
        || text(&value, "corpus_root_id")? != config.roots[0].id
        || PathBuf::from(text(&value, "corpus_path")?) != config.roots[0].path
    {
        return Err("ROOT_REVOKED");
    }
    let original_roots = array(&value, "original_roots")?
        .iter()
        .map(|v| v.as_str().map(PathBuf::from).ok_or("ROOT_REVOKED"))
        .collect::<Result<Vec<_>>>()?;
    let note_roots = array(&value, "note_roots")?
        .iter()
        .map(|v| v.as_str().map(PathBuf::from).ok_or("ROOT_REVOKED"))
        .collect::<Result<Vec<_>>>()?;
    let mut permitted_roots = original_roots.clone();
    permitted_roots.extend(note_roots.clone());
    let head_path = config.data_dir.join("projection-updates-v2/HEAD.json");
    let raw = read_private(&head_path, MAX_METADATA)?;
    let head_hash = sha(&raw);
    let head: Value = serde_json::from_slice(&raw).map_err(|_| "ROOT_REVOKED")?;
    if head["schema"] != 2 || head["authorization_sha256"] != pin.manifest_sha256 {
        return Err("ROOT_REVOKED");
    }
    let states = head["sources"].as_object().ok_or("ROOT_REVOKED")?;
    let outputs = head["outputs"].as_object().ok_or("ROOT_REVOKED")?;
    for (path, hash) in outputs {
        relative(path)?;
        if !hash.as_str().is_some_and(hash_valid) {
            return Err("ROOT_REVOKED");
        }
    }
    let generation = text(&head, "indexed_generation")?;
    let db = rusqlite::Connection::open_with_flags(
        config.data_dir.join("vrg.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "SOURCE_UNAVAILABLE")?;
    let active: String = db
        .query_row(
            "SELECT generation_id FROM active_generation WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| "SOURCE_UNAVAILABLE")?;
    if active != generation {
        return Err("SOURCE_CHANGED");
    }
    let sources = array(&value, "sources")?;
    if states.len() != sources.len() {
        return Err("ROOT_REVOKED");
    }
    let mut ids = BTreeSet::new();
    let mut dependencies = Vec::new();
    let mut expected = BTreeSet::new();
    for source in &sources {
        let id = text(source, "source_id")?;
        if !hash_valid(&id) || !ids.insert(id.clone()) {
            return Err("ROOT_REVOKED");
        }
        let root = PathBuf::from(text(source, "original_root")?);
        if !original_roots.contains(&root) {
            return Err("ROOT_REVOKED");
        }
        let path = text(source, "relative_path")?;
        relative(&path)?;
        let state = states.get(&id).ok_or("ROOT_REVOKED")?;
        let kind = text(state, "state")?;
        let parts = array(state, "parts")?;
        if kind != "baseline"
            && kind != "mechanical_projected"
            && kind != "missing"
            && !kind.starts_with("held_")
        {
            return Err("ROOT_REVOKED");
        }
        if kind != "baseline" && text(state, "read_status")? != "pending_review" {
            return Err("ROOT_REVOKED");
        }
        if kind != "baseline" && kind != "mechanical_projected" {
            if !parts.is_empty() {
                return Err("ROOT_REVOKED");
            }
            continue;
        }
        if kind == "baseline"
            && source["initial_sha256"].is_null()
            && state["observed_sha256"].is_null()
            && parts.is_empty()
        {
            continue;
        }
        let revision = text(state, "observed_sha256")?;
        if !hash_valid(&revision) {
            return Err("ROOT_REVOKED");
        }
        if kind == "baseline" && source["initial_sha256"] != state["observed_sha256"] {
            return Err("ROOT_REVOKED");
        }
        if kind == "mechanical_projected"
            && (source["kind"] != "utf8_text" || source["initial_read_scope"] != "full_source")
        {
            return Err("ROOT_REVOKED");
        }
        let initial = array(source, "initial_parts")?;
        if kind == "baseline" && parts.len() != initial.len() {
            return Err("ROOT_REVOKED");
        }
        let mut views = Vec::new();
        let mut previous_end = 0u64;
        for (index, part) in parts.iter().enumerate() {
            let path = text(part, "relative_path")?;
            let hash = text(part, "sha256")?;
            relative(&path)?;
            if !hash_valid(&hash) || outputs.get(&path) != Some(&json!(hash)) {
                return Err("ROOT_REVOKED");
            }
            if kind == "baseline" {
                if initial[index]["relative_path"] != part["relative_path"]
                    || initial[index]["sha256"] != part["sha256"]
                    || initial[index]["root_id"] != config.roots[0].id
                {
                    return Err("ROOT_REVOKED");
                }
            } else {
                if !path.starts_with(&format!("incremental-v2/s-{id}-{revision}-")) {
                    return Err("ROOT_REVOKED");
                }
                let range = part["source_byte_range"].as_array().ok_or("ROOT_REVOKED")?;
                if range.len() != 2 {
                    return Err("ROOT_REVOKED");
                }
                let start = range[0].as_u64().ok_or("ROOT_REVOKED")?;
                let end = range[1].as_u64().ok_or("ROOT_REVOKED")?;
                if start != previous_end || start >= end || end > MAX_SOURCE as u64 {
                    return Err("ROOT_REVOKED");
                }
                previous_end = end;
            }
            expected.insert(path.clone());
            views.push(View {
                root_id: config.roots[0].id.clone(),
                relative_path: path,
                sha256: hash,
            });
        }
        dependencies.push(Dependency {
            original_root: root,
            relative_path: path,
            expected_sha256: revision,
            views,
        });
    }
    let retired = array(&head, "retired_notes")?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or("ROOT_REVOKED"))
        .collect::<Result<BTreeSet<_>>>()?;
    let notes = array(&value, "notes")?;
    let note_paths = notes
        .iter()
        .map(|v| text(&v["view"], "relative_path"))
        .collect::<Result<BTreeSet<_>>>()?;
    if !retired.is_subset(&note_paths) {
        return Err("ROOT_REVOKED");
    }
    for note in &notes {
        let root = PathBuf::from(text(note, "original_root")?);
        if !note_roots.contains(&root) {
            return Err("ROOT_REVOKED");
        }
        let path = text(note, "relative_path")?;
        relative(&path)?;
        let hash = text(note, "expected_sha256")?;
        if !hash_valid(&hash) {
            return Err("ROOT_REVOKED");
        }
        let view: View =
            serde_json::from_value(note["view"].clone()).map_err(|_| "ROOT_REVOKED")?;
        if retired.contains(&view.relative_path) {
            continue;
        }
        if view.root_id != config.roots[0].id
            || outputs.get(&view.relative_path) != Some(&json!(view.sha256))
        {
            return Err("ROOT_REVOKED");
        }
        expected.insert(view.relative_path.clone());
        dependencies.push(Dependency {
            original_root: root,
            relative_path: path,
            expected_sha256: hash,
            views: vec![view],
        });
    }
    let mut structural = Vec::new();
    for hub in array(&value, "hubs")? {
        let path = text(&hub, "relative_path")?;
        relative(&path)?;
        for id in array(&hub, "source_ids")? {
            if !id.as_str().is_some_and(|id| ids.contains(id)) {
                return Err("ROOT_REVOKED");
            }
        }
        let hash = outputs
            .get(&path)
            .and_then(Value::as_str)
            .ok_or("ROOT_REVOKED")?
            .to_owned();
        expected.insert(path.clone());
        structural.push(View {
            root_id: config.roots[0].id.clone(),
            relative_path: path,
            sha256: hash,
        });
    }
    for v in array(&value, "passthrough_views")? {
        let view: View = serde_json::from_value(v).map_err(|_| "ROOT_REVOKED")?;
        if view.root_id != config.roots[0].id
            || outputs.get(&view.relative_path) != Some(&json!(view.sha256))
        {
            return Err("ROOT_REVOKED");
        }
        expected.insert(view.relative_path.clone());
        structural.push(view);
    }
    if expected != outputs.keys().cloned().collect() {
        return Err("ROOT_REVOKED");
    }
    Ok((
        Manifest {
            schema: 1,
            data_dir: config.data_dir.clone(),
            original_roots: permitted_roots,
            sources: dependencies,
        },
        structural,
        head_path,
        head_hash,
        generation,
    ))
}

pub fn load_policy(profile: &Path, config: &VrgConfig) -> Result<Option<String>> {
    let raw = read_regular(profile, 4 * 1024 * 1024, false)?;
    let Some(pin) = parse_pin(&raw)? else {
        return Ok(None);
    };
    let registered = registration(profile, &raw)?;
    if current_private_hash(profile)? != registered.config_sha256 {
        return Err("ROOT_REVOKED");
    }
    if current_private_hash(&pin.manifest_path)? != pin.manifest_sha256 {
        return Err("ROOT_REVOKED");
    }
    let policies = POLICIES.get_or_init(Default::default);
    let cached = policies
        .lock()
        .map_err(|_| "INTERNAL_ERROR")?
        .get(&config.data_dir)
        .cloned();
    if cached.as_ref().is_some_and(|p| p.profile != profile) {
        return Err("ROOT_REVOKED");
    }
    // The immutable manifest pin authenticates the cached schema. Paging a large
    // graph must not parse its multi-megabyte source manifest on every request.
    let schema = if let Some(known) = cached
        .as_ref()
        .filter(|p| p.pin.manifest_sha256 == pin.manifest_sha256)
    {
        if known.head_path.is_some() { 2 } else { 1 }
    } else {
        let bytes = read_private(&pin.manifest_path, MAX_METADATA)?;
        serde_json::from_slice::<Value>(&bytes).map_err(|_| "ROOT_REVOKED")?["schema"]
            .as_u64()
            .ok_or("ROOT_REVOKED")?
    };
    let (pending, pending_hash) = if schema == 2 {
        (BTreeSet::new(), String::new())
    } else {
        pending(config, &pin)?
    };
    let head_path = (schema == 2).then(|| config.data_dir.join("projection-updates-v2/HEAD.json"));
    let head_hash = match &head_path {
        Some(path) => current_private_hash(path)?,
        None => String::new(),
    };
    let key = sha(format!(
        "{}:{}:{}:{}",
        registered.config_sha256, pin.manifest_sha256, pending_hash, head_hash
    )
    .as_bytes());
    let policy = if let Some(policy) = cached.filter(|p| p.profile == profile && p.stamp == key) {
        policy
    } else {
        let bytes = read_private(&pin.manifest_path, MAX_METADATA)?;
        if sha(&bytes) != pin.manifest_sha256 {
            return Err("ROOT_REVOKED");
        }
        let (mut manifest, structural, indexed_generation) = if schema == 2 {
            let (manifest, structural, _, observed, generation) =
                schema_two_manifest(config, &pin, &bytes)?;
            if observed != head_hash {
                return Err("ROOT_REVOKED");
            }
            (manifest, structural, Some(generation))
        } else {
            (
                serde_json::from_slice::<Manifest>(&bytes).map_err(|_| "ROOT_REVOKED")?,
                Vec::new(),
                None,
            )
        };
        if manifest.schema != 1 || manifest.data_dir != config.data_dir {
            return Err("ROOT_REVOKED");
        }
        canonical_directory(&config.data_dir)?;
        let roots: BTreeSet<_> = manifest.original_roots.iter().cloned().collect();
        if roots.len() != manifest.original_roots.len() {
            return Err("ROOT_REVOKED");
        }
        for root in &roots {
            canonical_directory(root)?;
            if root.starts_with(&config.data_dir)
                || config.data_dir.starts_with(root)
                || config
                    .roots
                    .iter()
                    .any(|r| root.starts_with(&r.path) || r.path.starts_with(root))
            {
                return Err("ROOT_REVOKED");
            }
        }
        let mut views: BTreeMap<(String, String), ViewGrant> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for (index, dependency) in manifest.sources.iter().enumerate() {
            relative(&dependency.relative_path)?;
            if !roots.contains(&dependency.original_root)
                || !hash_valid(&dependency.expected_sha256)
                || !seen.insert((
                    dependency.original_root.clone(),
                    dependency.relative_path.clone(),
                ))
            {
                return Err("ROOT_REVOKED");
            }
            for view in &dependency.views {
                relative(&view.relative_path)?;
                if !hash_valid(&view.sha256) || !config.roots.iter().any(|r| r.id == view.root_id) {
                    return Err("ROOT_REVOKED");
                }
                let grant = views
                    .entry((view.root_id.clone(), view.relative_path.clone()))
                    .or_insert_with(|| ViewGrant {
                        sha256: view.sha256.clone(),
                        dependencies: Vec::new(),
                        metadata: Value::Null,
                    });
                if grant.sha256 != view.sha256 {
                    return Err("ROOT_REVOKED");
                }
                grant.dependencies.push(index);
            }
        }
        for view in structural {
            relative(&view.relative_path)?;
            if !hash_valid(&view.sha256) {
                return Err("ROOT_REVOKED");
            }
            views.insert(
                (view.root_id, view.relative_path),
                ViewGrant {
                    sha256: view.sha256,
                    dependencies: Vec::new(),
                    metadata: Value::Null,
                },
            );
        }
        for ((_, path), grant) in &mut views {
            let origins:Vec<_> = grant.dependencies.iter().take(8).map(|i| { let d = &manifest.sources[*i]; json!({"original_root":d.original_root,"relative_path":d.relative_path,"revision":d.expected_sha256}) }).collect();
            // A projected note can depend on many original files. Its chunks
            // cannot be attributed to an arbitrary dependency's byte ranges.
            let exact = grant.dependencies.len() == 1;
            let primary = grant.dependencies.iter().min_by_key(|i| {
                let source = &manifest.sources[**i];
                (
                    Some(&source.original_root) != registered.project_root.as_ref(),
                    source.original_root.clone(),
                    source.relative_path.clone(),
                )
            });
            let display = primary
                .map(|index| {
                    hierarchy_path(&manifest.sources[*index], registered.project_root.as_ref())
                })
                .unwrap_or_else(|| "INDEX.md".into());
            let hierarchy_paths:Vec<_>=grant.dependencies.iter().map(|index|{
                let source=&manifest.sources[*index];json!({"original_root":source.original_root,"relative_path":source.relative_path,
                    "hierarchy_path":hierarchy_path(source,registered.project_root.as_ref()),"revision":source.expected_sha256})
            }).collect();
            let folder = Path::new(&display)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let canonical_identity = if grant.dependencies.is_empty() {
                sha(format!("{}:{path}", profile.display()).as_bytes())
            } else {
                canonical_source_identity(
                    grant
                        .dependencies
                        .iter()
                        .map(|index| &manifest.sources[*index]),
                )
            };
            grant.metadata = json!({"canonical_source_identity":canonical_identity,"relative_path":display,"folder":folder,"indexed_path":path,"attribution":if grant.dependencies.is_empty(){"derived_structure"}else if exact{"single_source_projection"}else{"multiple_source_dependencies"},"origin_count":grant.dependencies.len(),"origins":origins,"origins_partial":grant.dependencies.len()>8,"hierarchy_paths":hierarchy_paths});
        }
        // Reverse grants and their complete identities are now derived. The
        // per-source copies of those same view lists are no longer needed for
        // live original-revision validation.
        for dependency in &mut manifest.sources {
            dependency.views = Vec::new();
        }
        let policy = Arc::new(Policy {
            profile: profile.into(),
            pin: pin.clone(),
            dependencies: manifest.sources,
            views,
            pending,
            pending_hash,
            stamp: key,
            head_path: head_path.clone(),
            head_hash: head_hash.clone(),
            indexed_generation,
        });
        policies
            .lock()
            .map_err(|_| "INTERNAL_ERROR")?
            .insert(config.data_dir.clone(), policy.clone());
        policy
    };
    let mut digest = Sha256::new();
    digest.update(policy.stamp.as_bytes());
    // Revocation changes snapshot identity, including an original changing while
    // its frozen projected file still exists. No withdrawal/reindex is performed.
    for dependency in &policy.dependencies {
        digest.update([dependency_current(dependency, &policy.pending) as u8]);
    }
    Ok(Some(hex::encode(digest.finalize())))
}
/// Bind the read-only SQLite snapshot to the reconciled source HEAD even when
/// its metadata grants were cached before a concurrent publisher changed DB.
pub fn check_generation(config: &VrgConfig, generation: &str) -> Result<()> {
    let policies = POLICIES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "INTERNAL_ERROR")?;
    if let Some(policy) = policies.get(&config.data_dir) {
        if policy
            .indexed_generation
            .as_deref()
            .is_some_and(|expected| expected != generation)
        {
            return Err("SOURCE_CHANGED");
        }
    }
    Ok(())
}
/// Called before the existing vault reader; no source-policy file is a new root.
pub fn check_source(config: &VrgConfig, source: &SourceRef) -> Result<Option<Value>> {
    let policy = POLICIES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "INTERNAL_ERROR")?
        .get(&config.data_dir)
        .cloned();
    let Some(policy) = policy else {
        return Ok(None);
    };
    let grant = policy
        .views
        .get(&(source.root_id.clone(), source.relative_path.clone()))
        .ok_or("ROOT_REVOKED")?;
    for i in &grant.dependencies {
        if !dependency_current(&policy.dependencies[*i], &policy.pending) {
            return Err("ROOT_REVOKED");
        }
    }
    let root = config
        .roots
        .iter()
        .find(|r| r.id == source.root_id)
        .ok_or("ROOT_REVOKED")?;
    if current_hash(&root.path.join(&source.relative_path))? != grant.sha256 {
        return Err("ROOT_REVOKED");
    }
    // Verify policy bytes again as details may be read after the snapshot request.
    if current_private_hash(&policy.pin.manifest_path)? != policy.pin.manifest_sha256 {
        return Err("ROOT_REVOKED");
    }
    if let Some(path) = &policy.head_path {
        if current_private_hash(path)? != policy.head_hash {
            return Err("SOURCE_CHANGED");
        }
    } else {
        let pending_path = config
            .data_dir
            .join("projection-invalidation/needs_read.json");
        if optional_private_hash(&pending_path)?.unwrap_or_default() != policy.pending_hash {
            return Err("ROOT_REVOKED");
        }
    }
    Ok(Some(grant.metadata.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        sync::atomic::{AtomicU64, Ordering},
    };
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "vrg-source-policy-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self(root)
        }
        fn private(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn schema_two_fixture() -> (Fixture, VrgConfig, Pin, Vec<u8>, Value) {
        let fixture = Fixture::new();
        for name in ["original", "corpus", "store", "store/projection-updates-v2"] {
            let path = fixture.0.join(name);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fixture.private("original/code.rs", b"verified source\n");
        fixture.private("corpus/source.md", b"verified source\n");
        let hash = sha(b"verified source\n");
        let id = sha(b"authorized-source-id");
        let config = VrgConfig {
            data_dir: fixture.0.join("store"),
            roots: vec![crate::config::RootConfig {
                id: "approved".into(),
                path: fixture.0.join("corpus"),
                exclusions: Vec::new(),
                allowed_files: None,
            }],
            ..VrgConfig::default()
        };
        let db = rusqlite::Connection::open(config.data_dir.join("vrg.sqlite3")).unwrap();
        db.execute_batch("CREATE TABLE active_generation(singleton INTEGER PRIMARY KEY,generation_id TEXT); INSERT INTO active_generation VALUES(1,'current-generation');").unwrap();
        drop(db);
        let view = json!({"root_id":"approved","relative_path":"source.md","sha256":hash});
        let manifest = json!({"schema":2,"data_dir":config.data_dir,"corpus_root_id":"approved","corpus_path":config.roots[0].path,"original_roots":[fixture.0.join("original")],"note_roots":[],"sources":[{"source_id":id,"original_root":fixture.0.join("original"),"relative_path":"code.rs","kind":"utf8_text","initial_read_scope":"full_source","initial_sha256":hash,"initial_parts":[view]}],"notes":[],"hubs":[],"passthrough_views":[]});
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let path = fixture.private("manifest.json", &bytes);
        let pin = Pin {
            manifest_path: path,
            manifest_sha256: sha(&bytes),
        };
        let head = json!({"schema":2,"authorization_sha256":pin.manifest_sha256,"sources":{id:{"observed_sha256":hash,"state":"baseline","read_status":"historical_evidence_unchanged","parts":[{"relative_path":"source.md","sha256":hash,"source_byte_range":null}]}},"retired_notes":[],"outputs":{"source.md":hash},"indexed_generation":"current-generation"});
        fixture.private(
            "store/projection-updates-v2/HEAD.json",
            &serde_json::to_vec(&head).unwrap(),
        );
        (fixture, config, pin, bytes, head)
    }
    #[test]
    fn portable_registry_paths_use_only_explicit_configuration() {
        assert_eq!(
            source_registry_path(
                None,
                Some("/isolated/config".into()),
                Some("/account".into())
            ),
            PathBuf::from("/isolated/config/vrg/viewer-sources.json")
        );
        assert_eq!(
            source_registry_path(None, None, Some("/account".into())),
            PathBuf::from("/account/.config/vrg/viewer-sources.json")
        );
        assert_eq!(
            source_registry_path(
                Some("/explicit/sources.json".into()),
                Some("/isolated/config".into()),
                Some("/account".into())
            ),
            PathBuf::from("/explicit/sources.json")
        );
    }
    #[test]
    fn schema_two_reads_exact_authorized_head_without_source_writes() {
        let (fixture, config, pin, bytes, _) = schema_two_fixture();
        let before = fs::read(fixture.0.join("original/code.rs")).unwrap();
        let (manifest, structural, _, _, generation) =
            schema_two_manifest(&config, &pin, &bytes).unwrap();
        assert_eq!(generation, "current-generation");
        assert!(structural.is_empty());
        assert_eq!(manifest.sources.len(), 1);
        assert_eq!(manifest.sources[0].relative_path, "code.rs");
        assert_eq!(manifest.sources[0].views[0].relative_path, "source.md");
        assert_eq!(
            fs::read(fixture.0.join("original/code.rs")).unwrap(),
            before
        );
    }
    #[test]
    fn schema_two_rejects_head_outputs_outside_authorized_manifest() {
        let (fixture, config, pin, bytes, mut head) = schema_two_fixture();
        head["outputs"]["unapproved.md"] = json!(sha(b"unapproved"));
        fixture.private(
            "store/projection-updates-v2/HEAD.json",
            &serde_json::to_vec(&head).unwrap(),
        );
        assert!(schema_two_manifest(&config, &pin, &bytes).is_err());
    }
    #[test]
    fn schema_two_rejects_head_generation_not_active_in_sqlite() {
        let (fixture, config, pin, bytes, mut head) = schema_two_fixture();
        head["indexed_generation"] = json!("different-generation");
        fixture.private(
            "store/projection-updates-v2/HEAD.json",
            &serde_json::to_vec(&head).unwrap(),
        );
        assert!(matches!(
            schema_two_manifest(&config, &pin, &bytes),
            Err("SOURCE_CHANGED")
        ));
    }
    #[test]
    fn source_registry_pins_bind_exact_profile_and_bytes() {
        let profile = PathBuf::from("/existing/profile.toml");
        let bytes = b"roots=[]\ndata_dir='/existing/store'\n";
        let registration = SourceRegistration {
            label: "project".into(),
            project_root: Some(PathBuf::from("/existing/project")),
            selection_kind: "canonical_project".into(),
            profile: profile.clone(),
            config_sha256: sha(bytes),
            aliases: Vec::new(),
        };
        assert!(verify_registration(&profile, bytes, vec![registration.clone()]).is_ok());
        assert!(verify_registration(&profile, b"changed", vec![registration.clone()]).is_err());
        assert!(
            verify_registration(
                Path::new("/different/profile.toml"),
                bytes,
                vec![registration]
            )
            .is_err()
        );
    }
    #[test]
    fn source_policy_revocation_and_withdrawal_preserve_original_bytes() {
        let fixture = Fixture::new();
        let original = fixture.private("original.rs", b"approved original\n");
        let dependency = Dependency {
            original_root: fixture.0.clone(),
            relative_path: "original.rs".into(),
            expected_sha256: sha(b"approved original\n"),
            views: Vec::new(),
        };
        let before = stamp(&open(&original).unwrap()).unwrap();
        assert!(dependency_current(&dependency, &BTreeSet::new()));
        assert!(stamp(&open(&original).unwrap()).unwrap() == before);
        assert_eq!(fs::read(&original).unwrap(), b"approved original\n");
        fs::write(&original, b"changed original\n").unwrap();
        assert!(!dependency_current(&dependency, &BTreeSet::new()));
        fs::write(&original, b"approved original\n").unwrap();
        let withdrawn = BTreeSet::from([(fixture.0.clone(), "original.rs".into())]);
        assert!(!dependency_current(&dependency, &withdrawn));
        assert!(dependency_current(&dependency, &BTreeSet::new()));
    }
    #[test]
    fn source_policy_pending_and_metadata_fail_closed_without_writes() {
        let fixture = Fixture::new();
        let pending = fixture.0.join("needs_read.json");
        assert!(optional_private_hash(&pending).unwrap().is_none());
        symlink(fixture.0.join("absent-target"), &pending).unwrap();
        assert!(optional_private_hash(&pending).is_err());
        fs::remove_file(&pending).unwrap();
        let parent_link = fixture.0.join("linked-directory");
        symlink(fixture.0.join("absent-directory"), &parent_link).unwrap();
        assert!(optional_private_hash(&parent_link.join("needs_read.json")).is_err());
        let path = fixture.private("needs_read.json", b"pinned metadata");
        let before = stamp(&open(&path).unwrap()).unwrap();
        assert_eq!(
            optional_private_hash(&path).unwrap(),
            Some(sha(b"pinned metadata"))
        );
        assert!(stamp(&open(&path).unwrap()).unwrap() == before);
        assert_eq!(fs::read(&path).unwrap(), b"pinned metadata");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(optional_private_hash(&path).is_err());
        // Ordinary viewer profiles retain their existing readable-mode contract.
        assert_eq!(
            read_regular(&path, 1024, false).unwrap(),
            b"pinned metadata"
        );
    }
    #[test]
    fn source_identity_distinguishes_physical_origins_and_deduplicates_copies() {
        let a = Dependency {
            original_root: PathBuf::from("/projects/a"),
            relative_path: "src/main.rs".into(),
            expected_sha256: sha(b"same"),
            views: Vec::new(),
        };
        let mut b = a.clone();
        b.original_root = PathBuf::from("/projects/b");
        assert_ne!(
            canonical_source_identity([&a]),
            canonical_source_identity([&b])
        );
        let copied = a.clone();
        assert_eq!(
            canonical_source_identity([&a]),
            canonical_source_identity([&copied])
        );
        assert_eq!(
            canonical_source_identity([&a, &b]),
            canonical_source_identity([&b, &a])
        );
    }
}
