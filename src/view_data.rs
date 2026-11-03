//! Passive, scoped reads for the local graph viewer. This module never opens VrgEngine.
use crate::{
    adaptive::{self, StoredMemory},
    adaptive_codec::ArchiveStore,
    config::VrgConfig,
    contracts::{SourceDocument, SourceRef},
    vault,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const SOURCE_LIMIT: usize = 4 * 1024 * 1024;
const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
#[derive(Debug, Clone)]
pub struct ViewError {
    pub code: &'static str,
}
impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code)
    }
}
impl std::error::Error for ViewError {}
type Result<T> = std::result::Result<T, ViewError>;
fn err(code: &'static str) -> ViewError {
    ViewError { code }
}
fn row_usize(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<usize> {
    let value = row.get::<_, i64>(index)?;
    usize::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
fn sql(error: rusqlite::Error) -> ViewError {
    match error {
        rusqlite::Error::SqliteFailure(ref e, _)
            if matches!(
                e.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            err("STORE_BUSY")
        }
        _ => err("SCHEMA_UNSUPPORTED"),
    }
}
fn digest(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    hex::encode(h.finalize())
}
fn bounded_file(path: &Path, max: usize) -> Result<Vec<u8>> {
    let mut f = fs::File::open(path).map_err(|_| err("PROFILE_NOT_FOUND"))?;
    if !f
        .metadata()
        .map_err(|_| err("PROFILE_NOT_FOUND"))?
        .is_file()
    {
        return Err(err("PROFILE_NOT_FOUND"));
    }
    let mut b = Vec::new();
    f.by_ref()
        .take(max as u64 + 1)
        .read_to_end(&mut b)
        .map_err(|_| err("PROFILE_NOT_FOUND"))?;
    if b.len() > max {
        return Err(err("LIMIT_EXCEEDED"));
    }
    Ok(b)
}
fn canonical(path: &Path) -> Result<PathBuf> {
    let c = path.canonicalize().map_err(|_| err("PROFILE_NOT_FOUND"))?;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|_| err("PROFILE_NOT_FOUND"))?
            .join(path)
    };
    let mut p = PathBuf::new();
    for component in absolute.components() {
        p.push(component.as_os_str());
        if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(err("SCOPE_MISMATCH"));
        }
    }
    Ok(c)
}
#[derive(Clone, Deserialize)]
struct Registration {
    project: PathBuf,
    label: String,
    profile: PathBuf,
    #[serde(default)]
    aliases: Vec<PathBuf>,
}
#[derive(Deserialize)]
struct Registry {
    projects: Vec<Registration>,
    data_base: PathBuf,
    #[serde(default)]
    excluded_projects: Vec<PathBuf>,
}
#[derive(Clone)]
struct Project {
    id: String,
    label: String,
    profile: PathBuf,
    registered_project: Option<PathBuf>,
    registry_path: Option<PathBuf>,
    canonical_root: Option<PathBuf>,
    canonical_label: String,
    profile_kind: String,
}
#[derive(Clone, Debug)]
pub struct GraphQuery {
    pub snapshot_id: Option<String>,
    pub cursor: Option<String>,
    pub query: Option<String>,
    pub limit: usize,
    pub mode: String,
}
impl Default for GraphQuery {
    fn default() -> Self {
        Self {
            snapshot_id: None,
            cursor: None,
            query: None,
            limit: 500,
            mode: "structural".into(),
        }
    }
}
pub struct ViewCatalog {
    cancelled: AtomicBool,
    projects: Vec<Project>,
    selected: Option<String>,
    mapping: Option<Value>,
    secret: String,
    cached: Mutex<BTreeMap<String, BTreeMap<String, CachedNode>>>,
    tracked: Mutex<BTreeMap<String, BTreeMap<SourceRef, TrackedSource>>>,
    full: Mutex<BTreeMap<String, FullGraph>>,
}
struct Live {
    config: VrgConfig,
    policy: String,
    scope: String,
}
struct Snapshot {
    connection: Connection,
    generation: String,
    identity: String,
    memories: Vec<StoredMemory>,
    live: Live,
    archive_stamp: String,
}
#[derive(Clone)]
struct Chunk {
    native_id: String,
    source: SourceRef,
    revision: String,
    ordinal: u32,
    start: usize,
    end: usize,
    title: String,
}

impl ViewCatalog {
    /// `@all` and the legacy collection alias are not source directories.
    pub fn resolve(path: &Path, explicit_config: Option<&Path>) -> Result<Self> {
        let registry_path = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".config")
            })
            .join("vrg/global-projects.json");
        let registry = if registry_path.exists() {
            Some(
                serde_json::from_slice::<Registry>(&bounded_file(&registry_path, 256 * 1024)?)
                    .map_err(|_| err("PROFILE_NOT_FOUND"))?,
            )
        } else {
            None
        };
        let collection = path == Path::new("@all") || path == Path::new("/home/vrg-database");
        // A requested path chooses initial context, not an additional permission.
        // The catalogue always exposes already-registered project scopes; an
        // unknown path still fails closed instead of discovering directories.
        let selected_path = if collection {
            None
        } else if explicit_config.is_some() {
            // Explicit profiles do not authorize new filesystem aliases.
            Some(canonical(path)?)
        } else {
            let resolved = path.canonicalize().map_err(|_| err("PROFILE_NOT_FOUND"))?;
            Some(canonical(&resolved)?)
        };
        if selected_path.as_ref().is_some_and(|p| !p.is_dir()) {
            return Err(err("PROFILE_NOT_FOUND"));
        }
        let source_registrations = crate::view_sources::registered_projects().map_err(err)?;
        let mut projects = Vec::new();
        let mut source_matches = Vec::new();
        let mut memory_matches = Vec::new();
        if let Some(profile) = explicit_config {
            let profile = canonical(profile)?;
            let registration = source_registrations.iter().find(|r| r.profile == profile);
            let memory = registry
                .as_ref()
                .and_then(|r| r.projects.iter().find(|m| m.profile == profile));
            let canonical_root = registration
                .and_then(|r| r.project_root.clone())
                .or_else(|| memory.map(|m| m.project.clone()));
            let canonical_label = registration
                .map(|r| r.label.clone())
                .or_else(|| memory.map(|m| m.label.clone()))
                .unwrap_or_else(|| "Perfil explícito".into());
            let p = Project {
                id: digest(&[&profile.to_string_lossy()]),
                label: canonical_label.clone(),
                profile,
                registered_project: None,
                registry_path: None,
                canonical_root: canonical_root.clone(),
                canonical_label,
                profile_kind: if registration.is_some() {
                    "source".into()
                } else {
                    "memory".into()
                },
            };
            let live = load_live(&p)?;
            let registered = selected_path.as_ref() == canonical_root.as_ref()
                || registration
                    .is_some_and(|r| r.aliases.iter().any(|a| Some(a) == selected_path.as_ref()));
            if !collection
                && !registered
                && !selected_path
                    .as_ref()
                    .is_some_and(|v| matches_path(v, &live.config))
            {
                return Err(err("SCOPE_MISMATCH"));
            }
            projects.push(p);
        } else {
            if let Some(registry) = &registry {
                let mut profiles = BTreeSet::new();
                for r in &registry.projects {
                    if registry.excluded_projects.contains(&r.project) {
                        continue;
                    }
                    let Ok(profile) = canonical(&r.profile) else {
                        continue;
                    };
                    if !profiles.insert(profile.clone()) {
                        continue;
                    }
                    let p = Project {
                        id: digest(&[&profile.to_string_lossy()]),
                        label: r.label.clone(),
                        profile,
                        registered_project: Some(r.project.clone()),
                        registry_path: Some(registry_path.clone()),
                        canonical_root: Some(r.project.clone()),
                        canonical_label: r.label.clone(),
                        profile_kind: "memory".into(),
                    };
                    let Ok(live) = load_live(&p) else {
                        continue;
                    };
                    if selected_path.as_ref() == Some(&r.project)
                        || r.aliases.iter().any(|a| Some(a) == selected_path.as_ref())
                        || selected_path
                            .as_ref()
                            .is_some_and(|v| matches_path(v, &live.config))
                    {
                        memory_matches.push(p.id.clone());
                    }
                    projects.push(p);
                }
            }
            for registration in &source_registrations {
                let profile = canonical(&registration.profile)?;
                if projects.iter().any(|p| p.profile == profile) {
                    continue;
                }
                let p = Project {
                    id: digest(&[&profile.to_string_lossy()]),
                    label: registration.label.clone(),
                    profile,
                    registered_project: None,
                    registry_path: None,
                    canonical_root: registration.project_root.clone(),
                    canonical_label: registration.label.clone(),
                    profile_kind: "source".into(),
                };
                let live = load_live(&p)?;
                if selected_path.as_ref() == registration.project_root.as_ref()
                    || registration
                        .aliases
                        .iter()
                        .any(|a| Some(a) == selected_path.as_ref())
                    || selected_path
                        .as_ref()
                        .is_some_and(|v| matches_path(v, &live.config))
                {
                    source_matches.push(p.id.clone());
                }
                projects.push(p);
            }
            if !collection && source_matches.is_empty() && memory_matches.is_empty() {
                return Err(err("PROFILE_NOT_FOUND"));
            }
        }
        let selected = if collection {
            None
        } else {
            source_matches
                .first()
                .or_else(|| memory_matches.first())
                .cloned()
                .or_else(|| projects.first().map(|p| p.id.clone()))
        };
        let memory_profiles_included = projects
            .iter()
            .filter(|p| p.profile_kind == "memory")
            .count();
        let source_profiles_available = projects
            .iter()
            .filter(|p| p.profile_kind == "source")
            .count();
        let excluded_explicit_contexts = crate::view_sources::explicit_contexts().map_err(err)?;
        let mapping = Some(
            json!({"requested":path,"kind":if collection{"virtual_collection_alias"}else{"registered_project_catalogue"},
            "exists":path.exists(),"registered_data_base":registry.as_ref().map(|r|r.data_base.to_string_lossy().to_string()),
            "note":"Catálogo de perfiles ya autorizados; la ruta selecciona contexto, no amplía raíces.",
            "memory_profiles_included":memory_profiles_included,"source_profiles_available":source_profiles_available,
            "excluded_explicit_contexts":excluded_explicit_contexts,"scope":"registered_authorized_memory_and_source_projects"}),
        );
        let mut entropy = [0u8; 32];
        fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut entropy))
            .map_err(|_| err("INTERNAL_ERROR"))?;
        Ok(Self {
            cancelled: AtomicBool::new(false),
            projects,
            selected,
            mapping,
            secret: hex::encode(entropy),
            cached: Mutex::new(BTreeMap::new()),
            tracked: Mutex::new(BTreeMap::new()),
            full: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn check_cancelled(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(err("REQUEST_CANCELLED"))
        } else {
            Ok(())
        }
    }
    pub fn selected_project_id(&self) -> Option<String> {
        self.selected.clone()
    }
    pub fn projects(&self) -> Value {
        json!({"projects":self.projects.iter().filter(|p|load_live(p).is_ok()).map(|p|json!({"id":p.id,"label":p.label,
            "canonical_project_id":p.canonical_root.as_ref().map(|root|digest(&[&root.to_string_lossy()])).unwrap_or_else(||p.id.clone()),
            "canonical_project_label":p.canonical_label,"canonical_project_root":p.canonical_root,
            "profile_kind":p.profile_kind})).collect::<Vec<_>>(),"selected_project_id":self.selected,"collection_mapping":self.mapping})
    }
    fn project(&self, id: &str) -> Result<&Project> {
        self.projects
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| err("PROFILE_NOT_FOUND"))
    }
    fn snapshot(&self, p: &Project) -> Result<Snapshot> {
        self.check_cancelled()?;
        let live = load_live(p)?;
        let db = live.config.data_dir.join("vrg.sqlite3");
        if !db.is_file() {
            return Err(err("STORE_MISSING"));
        }
        if canonical(&live.config.data_dir)? != live.config.data_dir {
            return Err(err("SCOPE_MISMATCH"));
        }
        for name in ["vrg.sqlite3", "vrg.sqlite3-wal", "vrg.sqlite3-shm"] {
            if fs::symlink_metadata(live.config.data_dir.join(name))
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                return Err(err("SCOPE_MISMATCH"));
            }
        }
        let connection = Connection::open_with_flags(
            db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql)?;
        connection
            .busy_timeout(Duration::from_millis(250))
            .map_err(sql)?;
        connection.execute_batch("BEGIN DEFERRED").map_err(sql)?;
        let version: i64 = connection
            .query_row(
                "SELECT version FROM schema_info WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if version != 2 {
            return Err(err("SCHEMA_UNSUPPORTED"));
        }
        let generation: Option<String> = connection
            .query_row(
                "SELECT generation_id FROM active_generation WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql)?;
        let generation = generation.ok_or_else(|| err("EMPTY_INDEX"))?;
        crate::view_sources::check_generation(&live.config, &generation).map_err(err)?;
        let state: Option<String> = connection
            .query_row(
                "SELECT state_json FROM adaptive_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql)?;
        if state.as_ref().is_some_and(|v| v.len() > 256 * 1024) {
            return Err(err("LIMIT_EXCEEDED"));
        }
        let memories: Vec<StoredMemory> = if live.config.adaptive_memory.enabled {
            state
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|_| err("SCHEMA_UNSUPPORTED"))?
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if memories.len() > 256 || memories.iter().map(|m| m.sources.len()).sum::<usize>() > 512 {
            return Err(err("LIMIT_EXCEEDED"));
        }
        let archive_stamp = archive_membership_stamp(&live.config)?;
        let identity = digest(&[
            &p.id,
            &generation,
            &live.policy,
            state.as_deref().unwrap_or(""),
            &archive_stamp,
        ]);
        Ok(Snapshot {
            connection,
            generation,
            identity,
            memories,
            live,
            archive_stamp,
        })
    }
    fn remember(&self, p: &Project, doc: &SourceDocument) -> Result<()> {
        self.tracked
            .lock()
            .map_err(|_| err("INTERNAL_ERROR"))?
            .entry(p.id.clone())
            .or_default()
            .insert(
                doc.source.clone(),
                TrackedSource {
                    revision: doc.revision.clone(),
                    stamp: source_stamp(&load_live(p)?.config, &doc.source),
                },
            );
        Ok(())
    }
    fn remember_scoped(&self, p: &Project, doc: &SourceDocument, config: &VrgConfig) -> Result<()> {
        self.tracked
            .lock()
            .map_err(|_| err("INTERNAL_ERROR"))?
            .entry(p.id.clone())
            .or_default()
            .insert(
                doc.source.clone(),
                TrackedSource {
                    revision: doc.revision.clone(),
                    stamp: source_stamp(config, &doc.source),
                },
            );
        Ok(())
    }
    pub fn status(&self, id: &str) -> Result<Value> {
        let p = self.project(id)?;
        let s = self.snapshot(p)?;
        if let Some(tracked) = self
            .tracked
            .lock()
            .map_err(|_| err("INTERNAL_ERROR"))?
            .get(id)
        {
            for (source, known) in tracked {
                if source_stamp(&s.live.config, source) == known.stamp && known.stamp.is_some() {
                    continue;
                }
                let doc = read_current(&s.live.config, source)?;
                if doc.revision != known.revision {
                    return Ok(
                        json!({"generation_id":s.generation,"policy_fingerprint":s.live.policy,"snapshot_id":s.identity,"changed":true,"reason":"SOURCE_CHANGED","read_only":true}),
                    );
                }
            }
        }
        Ok(
            json!({"generation_id":s.generation,"policy_fingerprint":s.live.policy,"snapshot_id":s.identity,"changed":false,"read_only":true}),
        )
    }
    /// Complete chunk graph, transported in bounded pages, without a sample limit.
    /// The snapshot is built once from the current authorized index, never from
    /// summaries. Each delivered page revalidates its actual source documents.
    fn full_graph(&self, id: &str, query: &GraphQuery) -> Result<Value> {
        if query.limit == 0
            || query.limit > 10000
            || query.query.as_ref().is_some_and(|q| q.len() > 4096)
        {
            return Err(err("LIMIT_EXCEEDED"));
        }
        let p = self.project(id)?;
        let s = self.snapshot(p)?;
        if query.snapshot_id.as_ref().is_some_and(|v| v != &s.identity) {
            return Err(err("SNAPSHOT_EXPIRED"));
        }
        let filter = query.query.as_deref().unwrap_or("");
        // Bind the cursor to full mode as well as snapshot and search.
        let cursor_filter = format!("full:{filter}");
        let (edge_page, offset) = match &query.cursor {
            None => (false, 0),
            Some(cursor) => self.decode_cursor(cursor, &s.identity, &cursor_filter)?,
        };
        let mut full = self.full.lock().map_err(|_| err("INTERNAL_ERROR"))?;
        if !full.get(id).is_some_and(|g| g.snapshot == s.identity) {
            // Bound duplicated server-side graph metadata, never the dataset
            // delivered to the browser. Loaded inspector addresses live in the
            // separate cache and survive graph eviction.
            full.remove(id);
            while full.len() >= 2 {
                let old = full
                    .keys()
                    .next()
                    .cloned()
                    .ok_or_else(|| err("INTERNAL_ERROR"))?;
                full.remove(&old);
            }
            let graph = self.build_full_graph(p, &s)?;
            full.insert(id.into(), graph);
            self.cached
                .lock()
                .map_err(|_| err("INTERNAL_ERROR"))?
                .remove(id);
        }
        let graph = full.get(id).ok_or_else(|| err("INTERNAL_ERROR"))?;
        let search = filter.to_lowercase();
        let selected: Vec<usize> = graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                search.is_empty()
                    || n["label"]
                        .as_str()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&search)
                    || n["source"]["relative_path"]
                        .as_str()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&search)
                    || n["preview"]
                        .as_str()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&search)
            })
            .map(|(i, _)| i)
            .collect();
        let selected_ids: BTreeSet<&str> = if search.is_empty() {
            BTreeSet::new()
        } else {
            selected
                .iter()
                .filter_map(|&i| graph.nodes[i]["id"].as_str())
                .collect()
        };
        let selected_edges: Vec<usize> = if search.is_empty() {
            (0..graph.edges.len()).collect()
        } else {
            graph
                .edges
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e["source"]
                        .as_str()
                        .is_some_and(|n| selected_ids.contains(n))
                        && e["target"]
                            .as_str()
                            .is_some_and(|n| selected_ids.contains(n))
                })
                .map(|(i, _)| i)
                .collect()
        };
        let total = if edge_page {
            selected_edges.len()
        } else {
            selected.len()
        };
        if offset > total {
            return Err(err("SNAPSHOT_EXPIRED"));
        }
        // Edge transport is a distinct cursor phase, so even a very high-degree
        // node cannot overflow a page or force a hidden truncation.
        let mut count = offset;
        let mut bytes = 0usize;
        let mut origin_hierarchies: BTreeMap<&str, &Value> = BTreeMap::new();
        let item_limit = total.min(offset.saturating_add(query.limit));
        while count < item_limit {
            let item = if edge_page {
                &graph.edges[selected_edges[count]]
            } else {
                &graph.nodes[selected[count]]
            };
            let mut size = serde_json::to_vec(item)
                .map_err(|_| err("INTERNAL_ERROR"))?
                .len()
                + 2;
            // A source may have many real worktree owners. Count its dictionary
            // array once per page alongside the first referencing node, before
            // advancing the cursor; later chunks only pay their own node size.
            let new_origin = if edge_page {
                None
            } else {
                item["canonical_source_identity"]
                    .as_str()
                    .filter(|identity| !origin_hierarchies.contains_key(identity))
                    .and_then(|identity| {
                        graph
                            .origin_hierarchies
                            .get(identity)
                            .map(|paths| (identity, paths))
                    })
            };
            if let Some((identity, paths)) = new_origin {
                let key_bytes = serde_json::to_vec(identity)
                    .map_err(|_| err("INTERNAL_ERROR"))?
                    .len();
                let value_bytes = serde_json::to_vec(paths)
                    .map_err(|_| err("INTERNAL_ERROR"))?
                    .len();
                size = size
                    .saturating_add(key_bytes)
                    .saturating_add(value_bytes)
                    .saturating_add(2);
            }
            if bytes.saturating_add(size) > RESPONSE_LIMIT - 64 * 1024 {
                if count == offset {
                    return Err(err("LIMIT_EXCEEDED"));
                }
                break;
            }
            if let Some((identity, paths)) = new_origin {
                origin_hierarchies.insert(identity, paths);
            }
            bytes += size;
            count += 1;
        }
        let page: &[usize] = if edge_page {
            &[]
        } else {
            &selected[offset..count]
        };
        let nodes: Vec<Value> = page.iter().map(|&i| graph.nodes[i].clone()).collect();
        let edges: Vec<Value> = if edge_page {
            selected_edges[offset..count]
                .iter()
                .map(|&i| graph.edges[i].clone())
                .collect()
        } else {
            Vec::new()
        };
        let mut sources = BTreeMap::new();
        for node in &nodes {
            sources.insert(
                SourceRef {
                    root_id: node["source"]["root_id"].as_str().unwrap_or("").into(),
                    relative_path: node["source"]["relative_path"]
                        .as_str()
                        .unwrap_or("")
                        .into(),
                },
                node["source"]["revision"].as_str().unwrap_or("").to_owned(),
            );
        }
        for e in &edges {
            for endpoint in ["source", "target"] {
                if let Some(&index) = e[endpoint].as_str().and_then(|n| graph.node_indices.get(n)) {
                    let node = &graph.nodes[index];
                    sources.insert(
                        SourceRef {
                            root_id: node["source"]["root_id"].as_str().unwrap_or("").into(),
                            relative_path: node["source"]["relative_path"]
                                .as_str()
                                .unwrap_or("")
                                .into(),
                        },
                        node["source"]["revision"].as_str().unwrap_or("").to_owned(),
                    );
                }
            }
        }
        for (source, revision) in &sources {
            self.check_cancelled()?;
            let current = read_current(&s.live.config, source)?;
            if current.revision != *revision {
                return Err(err("SOURCE_CHANGED"));
            }
            self.remember_scoped(p, &current, &s.live.config)?;
        }
        if load_live(p)?.policy != s.live.policy {
            return Err(err("ROOT_REVOKED"));
        }
        if archive_membership_stamp(&s.live.config)? != s.archive_stamp {
            return Err(err("SNAPSHOT_EXPIRED"));
        }
        let next_offset = count;
        let next = if count < total {
            Some(self.encode_cursor(edge_page, count, &s.identity, &cursor_filter))
        } else if !edge_page && !selected_edges.is_empty() {
            Some(self.encode_cursor(true, 0, &s.identity, &cursor_filter))
        } else {
            None
        };
        let mut warnings = Vec::new();
        if graph.stale_chunks > 0 {
            warnings.push(json!({"code":"SOURCE_CHANGED","message":"Se excluyeron chunks cuya revisión indexada no coincide con la fuente actual.","chunks":graph.stale_chunks}));
        }
        if graph.denied_chunks > 0 {
            warnings.push(json!({"code":"NO_AUTHORIZED_RESULTS","message":"Se excluyeron chunks no legibles bajo la autorización vigente.","chunks":graph.denied_chunks}));
        }
        if graph.semantic_count == 0 {
            warnings.push(json!({"code":"NO_VECTORS","message":"No hay embeddings válidos compatibles; se conserva el grafo estructural completo."}));
        }
        let result = json!({
            "api_version":"rag-view-v1", "project_id":id, "snapshot_id":s.identity,
            "generation_id":s.generation, "policy_fingerprint":s.live.policy,
            "sync_state":if graph.stale_chunks>0 {"stale"}else{"ready"},
            "capabilities":{"structure":true,"semantic_vectors":graph.semantic_count>0,"archive":false,"read_only":true,"full_chunks":true},
            "coverage":{"nodes_returned":nodes.len(),"nodes_total_authorized":graph.nodes.len(),
                "chunks_total_indexed":graph.indexed,"chunks_indexed_in_authorized_roots":graph.root_indexed,
                "chunks_total_authorized":graph.nodes.len(),"chunks_matching_filter":selected.len(),
                "chunks_stale":graph.stale_chunks,"chunks_denied":graph.denied_chunks,
                "chunks_outside_authorized_roots":graph.indexed.saturating_sub(graph.root_indexed),
                "edges_returned":edges.len(),"edges_total_authorized":graph.edges.len(),
                "loaded_through":if edge_page {selected.len()}else{next_offset},
                "edges_loaded_through":if edge_page {next_offset}else{0},"phase":if edge_page {"edges"}else{"chunks"},
                "partial":next.is_some()||offset>0,
                "selection":if search.is_empty(){"all_authorized"}else{"search_results"},
                "semantic_candidate_count":graph.semantic_count},
            "projection":{"method":"random_projection_ann","approximate":true,"note":graph.semantic_note},
            "nodes":nodes,"edges":edges,"origin_hierarchies":origin_hierarchies,"next_cursor":next,"warnings":warnings
        });
        if serde_json::to_vec(&result)
            .map_err(|_| err("INTERNAL_ERROR"))?
            .len()
            > RESPONSE_LIMIT
        {
            return Err(err("LIMIT_EXCEEDED"));
        }
        let mut cache = self.cached.lock().map_err(|_| err("INTERNAL_ERROR"))?;
        let project_cache = cache.entry(id.into()).or_default();
        // Retain all loaded pages for inspector access, keyed to this snapshot.
        for &i in page {
            let node = &graph.nodes[i];
            project_cache.insert(
                node["id"].as_str().unwrap_or("").into(),
                CachedNode {
                    snapshot: s.identity.clone(),
                    node: node.clone(),
                    native_id: Some(graph.native_ids[i].clone()),
                },
            );
        }
        Ok(result)
    }

    fn build_full_graph(&self, p: &Project, s: &Snapshot) -> Result<FullGraph> {
        let indexed: usize = s
            .connection
            .query_row(
                "SELECT count(*) FROM chunks WHERE generation_id=?1",
                [&s.generation],
                |r| row_usize(r, 0),
            )
            .map_err(sql)?;
        let roots: Vec<_> = s.live.config.roots.iter().map(|r| &r.id).collect();
        let roots_json = serde_json::to_string(&roots).map_err(|_| err("INTERNAL_ERROR"))?;
        let mut statement = s.connection.prepare(
            "SELECT c.chunk_id,c.root_id,c.relative_path,c.revision,c.ordinal,c.start_byte,c.end_byte,d.title
             FROM chunks c JOIN documents d USING(generation_id,root_id,relative_path)
             WHERE c.generation_id=?1 AND c.root_id IN (SELECT value FROM json_each(?2))
             ORDER BY c.root_id,c.relative_path,c.ordinal,c.chunk_id").map_err(sql)?;
        let rows = statement
            .query_map(params![s.generation, roots_json], |r| {
                Ok(Chunk {
                    native_id: r.get(0)?,
                    source: SourceRef {
                        root_id: r.get(1)?,
                        relative_path: r.get(2)?,
                    },
                    revision: r.get(3)?,
                    ordinal: r.get(4)?,
                    start: row_usize(r, 5)?,
                    end: row_usize(r, 6)?,
                    title: r.get(7)?,
                })
            })
            .map_err(sql)?;
        let memories = authorized_memories(&s.live, &s.memories);
        let mut result = FullGraph {
            snapshot: s.identity.clone(),
            indexed,
            root_indexed: 0,
            stale_chunks: 0,
            denied_chunks: 0,
            nodes: Vec::new(),
            native_ids: Vec::new(),
            edges: Vec::new(),
            node_indices: BTreeMap::new(),
            semantic_count: 0,
            semantic_note: String::new(),
            origin_hierarchies: BTreeMap::new(),
        };
        let mut current_source: Option<SourceRef> = None;
        let mut current_doc: Option<SourceDocument> = None;
        let mut current_origin: Option<Value> = None;
        let mut doc_stale = false;
        let mut document_anchor: BTreeMap<SourceRef, String> = BTreeMap::new();
        let mut previous: Option<String> = None;
        let mut vectors = Vec::new();
        let mut vector_statement = s.connection.prepare(
            "SELECT model_id,dimensions,vector_data FROM vectors WHERE generation_id=?1 AND chunk_id=?2 ORDER BY model_id").map_err(sql)?;
        for row in rows {
            self.check_cancelled()?;
            let c = row.map_err(sql)?;
            result.root_indexed += 1;
            if current_source.as_ref() != Some(&c.source) {
                current_source = Some(c.source.clone());
                current_doc = read_current(&s.live.config, &c.source).ok();
                current_origin = if current_doc.is_some() {
                    crate::view_sources::check_source(&s.live.config, &c.source).map_err(err)?
                } else {
                    None
                };
                // The immutable policy owns the full dependency list. Send its
                // identity and hierarchy with each chunk, and fetch detailed
                // provenance only when the inspector asks for that chunk.
                if let Some(origin) = current_origin.as_mut().and_then(Value::as_object_mut) {
                    origin.remove("origins");
                    if let Some(paths) = origin.remove("hierarchy_paths") {
                        if let Some(identity) = origin
                            .get("canonical_source_identity")
                            .and_then(Value::as_str)
                        {
                            result
                                .origin_hierarchies
                                .entry(identity.to_owned())
                                .or_insert(paths);
                        }
                    }
                    origin.insert("origins_in_inspector".into(), json!(true));
                }
                doc_stale = current_doc
                    .as_ref()
                    .is_some_and(|d| d.revision != c.revision);
                previous = None;
                if let Some(doc) = &current_doc {
                    self.remember_scoped(p, doc, &s.live.config)?;
                }
            }
            let Some(doc) = current_doc.as_ref() else {
                result.denied_chunks += 1;
                continue;
            };
            if doc_stale || doc.revision != c.revision {
                result.stale_chunks += 1;
                continue;
            }
            let Some(text) = doc.body_text.get(c.start..c.end) else {
                result.stale_chunks += 1;
                continue;
            };
            if load_chunk_text(&s.connection, &s.generation, &c.native_id)? != text {
                result.stale_chunks += 1;
                continue;
            }
            let nid = node_id(p, "chunk", &c.source, &c.revision, &c.native_id);
            let mut node = make_node(
                &nid,
                "chunk",
                &format!("{} · {}", c.title, c.ordinal),
                &c.source,
                &c.revision,
                "body_text_utf8",
                Some(c.start),
                Some(c.end),
                tier_for(&c.source, &memories),
                "exact_chunk",
                Some(preview(text)),
                Some(nid.clone()),
            );
            node["native_chunk_id"] = json!(c.native_id);
            let root = s
                .live
                .config
                .roots
                .iter()
                .find(|r| r.id == c.source.root_id)
                .ok_or_else(|| err("ROOT_REVOKED"))?;
            node["canonical_source_identity"] = json!(digest(&[
                &root.path.to_string_lossy(),
                &c.source.relative_path,
                &c.revision
            ]));
            node["project_id"] = json!(p.id);
            node["project_label"] = json!(p.label);
            node["document_title"] = json!(c.title);
            node["ordinal"] = json!(c.ordinal);
            node["folder"] = json!(
                Path::new(&c.source.relative_path)
                    .parent()
                    .map(|v| v.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            if let Some(origin) = &current_origin {
                node["origin"] = origin.clone();
                if let Some(identity) = origin["canonical_source_identity"].as_str() {
                    node["canonical_source_identity"] = json!(identity);
                }
                if let Some(path) = origin["relative_path"].as_str() {
                    node["hierarchy_path"] = json!(path);
                    node["folder"] = json!(
                        Path::new(path)
                            .parent()
                            .map(|v| v.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    );
                }
            }
            result.node_indices.insert(nid.clone(), result.nodes.len());
            result.nodes.push(node);
            result.native_ids.push(c.native_id.clone());
            document_anchor
                .entry(c.source.clone())
                .or_insert_with(|| nid.clone());
            // A sparse chain records shared document provenance, not a claimed
            // semantic relationship or an unverified chunk-level Markdown link.
            if let Some(prior) = previous.replace(nid.clone()) {
                let mut e = edge(
                    &prior,
                    &nid,
                    "contains",
                    "shared_document",
                    None,
                    None,
                    Some("same_source_document"),
                );
                e["evidence"] = json!({"root_id":c.source.root_id,"relative_path":c.source.relative_path,"revision":c.revision,"relation":"successive_indexed_chunks_of_same_document"});
                result.edges.push(e);
            }
            let vector_rows = vector_statement
                .query_map(params![s.generation, c.native_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        row_usize(r, 1)?,
                        r.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .map_err(sql)?;
            for v in vector_rows {
                let (model, dimensions, bytes) = v.map_err(sql)?;
                if dimensions == 0 || dimensions > 4096 || bytes.len() != dimensions * 4 {
                    continue;
                }
                let v = crate::view_semantic::InputVector {
                    id: nid.clone(),
                    model_identity: model,
                    values: bytes
                        .chunks_exact(4)
                        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                        .collect(),
                };
                if valid_vector(&v) {
                    // One deterministic compatible model per chunk; duplicate
                    // node IDs across models must not exclude every embedding.
                    vectors.push(v);
                    break;
                }
            }
        }
        drop(statement);
        let semantic = crate::view_semantic::analyze_full_cancellable(&vectors, 4, 0.75, || {
            self.cancelled.load(Ordering::Acquire)
        })
        .ok_or_else(|| err("REQUEST_CANCELLED"))?;
        result.semantic_count = vectors.len().saturating_sub(semantic.excluded);
        result.semantic_note = semantic.note;
        for node in &mut result.nodes {
            if let Some(position) = node["id"]
                .as_str()
                .and_then(|id| semantic.positions.get(id))
            {
                node["position"] =
                    serde_json::to_value(position).map_err(|_| err("INTERNAL_ERROR"))?;
            }
        }
        for e in semantic.edges {
            result.edges.push(edge(
                &e.source,
                &e.target,
                "semantic_similarity",
                "computed_ann",
                Some(e.score),
                Some(&e.model_identity),
                None,
            ));
        }
        let mut links = s.connection.prepare(
            "SELECT from_root,from_path,to_root,to_path,kind FROM edges WHERE generation_id=?1 ORDER BY from_root,from_path,start_byte").map_err(sql)?;
        let rows = links
            .query_map([&s.generation], |r| {
                Ok((
                    SourceRef {
                        root_id: r.get(0)?,
                        relative_path: r.get(1)?,
                    },
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(sql)?;
        let mut seen = BTreeSet::new();
        for row in rows {
            self.check_cancelled()?;
            let (from, tr, tp, kind) = row.map_err(sql)?;
            let (Some(tr), Some(tp)) = (tr, tp) else {
                continue;
            };
            let to = SourceRef {
                root_id: tr,
                relative_path: tp,
            };
            let (Some(a), Some(b)) = (document_anchor.get(&from), document_anchor.get(&to)) else {
                continue;
            };
            let mut e = edge(a, b, "explicit", "document", None, None, Some(&kind));
            e["evidence"] = json!({"source_document":from,"target_document":to,"mapping":"first_authorized_chunk_anchors_document_level_link"});
            if seen.insert(e["id"].as_str().unwrap_or("").to_owned()) {
                result.edges.push(e);
            }
        }
        Ok(result)
    }

    pub fn graph(&self, id: &str, query: &GraphQuery) -> Result<Value> {
        if query.mode == "full" {
            return self.full_graph(id, query);
        }
        let p = self.project(id)?;
        let s = self.snapshot(p)?;
        if query.limit == 0
            || query.limit > 1000
            || query.query.as_ref().is_some_and(|q| q.len() > 4096)
        {
            return Err(err("LIMIT_EXCEEDED"));
        }
        if !matches!(query.mode.as_str(), "structural" | "semantic") {
            return Err(err("INVALID_QUERY"));
        }
        if query.snapshot_id.as_ref().is_some_and(|v| v != &s.identity) {
            return Err(err("SNAPSHOT_EXPIRED"));
        }
        let filter = query.query.as_deref().unwrap_or("");
        let (auxiliary_page, offset) = match &query.cursor {
            None => (false, 0),
            Some(cursor) => self.decode_cursor(cursor, &s.identity, filter)?,
        };
        let roots: Vec<_> = s.live.config.roots.iter().map(|r| r.id.clone()).collect();
        let roots_json = serde_json::to_string(&roots).map_err(|_| err("INTERNAL_ERROR"))?;
        let phrase = format!("\"{}\"", filter.replace('"', "\"\""));
        let like = format!("%{filter}%");
        let where_search = if filter.is_empty() {
            ""
        } else {
            " AND (c.relative_path LIKE ?5 OR d.title LIKE ?5 OR c.object_rowid IN (SELECT rowid FROM chunk_fts WHERE chunk_fts MATCH ?6))"
        };
        let statement_text = format!(
            "SELECT c.chunk_id,c.root_id,c.relative_path,c.revision,c.ordinal,c.start_byte,c.end_byte,d.title FROM chunks c JOIN documents d USING(generation_id,root_id,relative_path) WHERE c.generation_id=?1 AND c.root_id IN (SELECT value FROM json_each(?2)){where_search} ORDER BY c.root_id,c.relative_path,c.ordinal LIMIT ?3 OFFSET ?4"
        );
        let mut st = s.connection.prepare(&statement_text).map_err(sql)?;
        let mapper = |r: &rusqlite::Row<'_>| {
            Ok(Chunk {
                native_id: r.get(0)?,
                source: SourceRef {
                    root_id: r.get(1)?,
                    relative_path: r.get(2)?,
                },
                revision: r.get(3)?,
                ordinal: r.get(4)?,
                start: row_usize(r, 5)?,
                end: row_usize(r, 6)?,
                title: r.get(7)?,
            })
        };
        let chunks: Vec<Chunk> = if filter.is_empty() {
            st.query_map(
                params![
                    s.generation,
                    roots_json,
                    if auxiliary_page {
                        0
                    } else {
                        query.limit as i64 + 1
                    },
                    offset as i64
                ],
                mapper,
            )
            .map_err(sql)?
            .collect::<std::result::Result<_, _>>()
            .map_err(sql)?
        } else {
            st.query_map(
                params![
                    s.generation,
                    roots_json,
                    if auxiliary_page {
                        0
                    } else {
                        query.limit as i64 + 1
                    },
                    offset as i64,
                    like,
                    phrase
                ],
                mapper,
            )
            .map_err(|_| err("INVALID_QUERY"))?
            .collect::<std::result::Result<_, _>>()
            .map_err(sql)?
        };
        drop(st);
        let more = chunks.len() > query.limit;
        let chunks = &chunks[..chunks.len().min(query.limit)];
        self.tracked
            .lock()
            .map_err(|_| err("INTERNAL_ERROR"))?
            .remove(id);
        let mut docs = BTreeMap::new();
        let mut denied = BTreeSet::new();
        let mut stale = 0;
        let mut source_bytes = 0usize;
        for c in chunks {
            if !docs.contains_key(&c.source) && !denied.contains(&c.source) {
                match read_current(&s.live.config, &c.source) {
                    Ok(doc) if doc.revision == c.revision => {
                        source_bytes = source_bytes
                            .saturating_add(doc.raw_markdown.len() + doc.body_text.len());
                        if source_bytes > 32 * 1024 * 1024 {
                            return Err(err("LIMIT_EXCEEDED"));
                        }
                        self.remember(p, &doc)?;
                        docs.insert(c.source.clone(), doc);
                    }
                    Ok(_) => {
                        stale += 1;
                        denied.insert(c.source.clone());
                    }
                    Err(_) => {
                        denied.insert(c.source.clone());
                    }
                }
            }
        }
        let mut archive_problem = false;
        let mut auxiliary_partial = false;
        let mut auxiliary_next = None;
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut node_by_chunk = BTreeMap::new();
        let mut document_ids = BTreeMap::new();
        let valid_memories = authorized_memories(&s.live, &s.memories);
        for (source, doc) in &docs {
            let did = node_id(p, "document", source, &doc.revision, "");
            document_ids.insert(source.clone(), did.clone());
            nodes.push(make_node(
                &did,
                "document",
                &doc.title,
                source,
                &doc.revision,
                "none",
                None,
                None,
                tier_for(source, &valid_memories),
                "source_metadata",
                None,
                None,
            ));
        }
        for c in chunks {
            let Some(doc) = docs.get(&c.source) else {
                continue;
            };
            let Some(text) = doc.body_text.get(c.start..c.end) else {
                return Err(err("SOURCE_CHANGED"));
            };
            if load_chunk_text(&s.connection, &s.generation, &c.native_id)? != text {
                return Err(err("SOURCE_CHANGED"));
            }
            let nid = node_id(p, "chunk", &c.source, &c.revision, &c.native_id);
            node_by_chunk.insert(c.native_id.clone(), nid.clone());
            nodes.push(make_node(
                &nid,
                "chunk",
                &format!("{} · {}", c.title, c.ordinal),
                &c.source,
                &c.revision,
                "body_text_utf8",
                Some(c.start),
                Some(c.end),
                tier_for(&c.source, &valid_memories),
                "exact_chunk",
                Some(preview(text)),
                Some(nid.clone()),
            ));
            edges.push(edge(
                &document_ids[&c.source],
                &nid,
                "contains",
                "document",
                None,
                None,
                None,
            ));
        }
        // Explicit links remain document-scoped. Raw Markdown body link ranges are not mapped onto chunk/body_text coordinates.
        let selected_docs = serde_json::to_string(
            &document_ids
                .keys()
                .map(|r| (&r.root_id, &r.relative_path))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| err("INTERNAL_ERROR"))?;
        // Materialize the authorized document set once. Row-value IN builds
        // membership indexes instead of reparsing/scanning JSON for every edge.
        let mut st = s
            .connection
            .prepare(
                "WITH selected(root_id,relative_path) AS MATERIALIZED (
                 SELECT json_extract(value,'$[0]'),json_extract(value,'$[1]')
                 FROM json_each(?2)
             )
             SELECT from_root,from_path,to_root,to_path,kind FROM edges
             WHERE generation_id=?1
               AND (from_root,from_path) IN (SELECT root_id,relative_path FROM selected)
               AND (to_root,to_path) IN (SELECT root_id,relative_path FROM selected)
             ORDER BY from_root,from_path,start_byte LIMIT 10001",
            )
            .map_err(sql)?;
        let links = st
            .query_map(params![s.generation, selected_docs], |r| {
                Ok((
                    SourceRef {
                        root_id: r.get(0)?,
                        relative_path: r.get(1)?,
                    },
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(sql)?;
        let mut links_truncated = false;
        let mut explicit_ids = BTreeSet::new();
        for (i, link) in links.enumerate() {
            if i >= 10000 {
                links_truncated = true;
                break;
            }
            let (from, tr, tp, kind) = link.map_err(sql)?;
            let (Some(tr), Some(tp)) = (tr, tp) else {
                continue;
            };
            if let (Some(a), Some(b)) = (
                document_ids.get(&from),
                document_ids.get(&SourceRef {
                    root_id: tr,
                    relative_path: tp,
                }),
            ) {
                if edges.len() < 9000 {
                    let e = edge(a, b, "explicit", "document", None, None, Some(&kind));
                    if explicit_ids.insert(e["id"].as_str().unwrap_or("").to_owned()) {
                        edges.push(e);
                    }
                }
            }
        }
        drop(st);
        if !more && filter.is_empty() {
            // Auxiliary metadata is one stable stream after chunk pages, with its own
            // authenticated phase/offset. Payloads are restored only by detail().
            let mut auxiliary: Vec<(String, SourceRef, Value)> = Vec::new();
            for memory in &valid_memories {
                let Some(span) = memory.sources.first() else {
                    continue;
                };
                let Some(source) = adaptive::parse_source_id(&span.source_id) else {
                    continue;
                };
                let Some(summary) = &memory.summary else {
                    continue;
                };
                let nid = node_id(
                    p,
                    "summary",
                    &source,
                    &span.revision_sha256,
                    &memory.memory_id,
                );
                let mut node = make_node(
                    &nid,
                    "summary",
                    &memory.memory_id,
                    &source,
                    &span.revision_sha256,
                    "raw_markdown_utf8",
                    Some(span.span_start),
                    Some(span.span_end),
                    (tier_str(&memory.tier), "direct"),
                    "derived_summary",
                    Some(preview(summary)),
                    None,
                );
                node["needs_expansion"] = json!(memory.needs_expansion);
                auxiliary.push((format!("summary:{nid}"), source, node));
            }
            if s.live.config.adaptive_memory.enabled {
                if let Ok(archive) = ArchiveStore::open_existing(&s.live.config.data_dir) {
                    match archive.records_existing(4096) {
                        Ok(records) => {
                            for record in records.into_iter().filter(|r| r.scope == s.live.scope) {
                                let Some(source) = adaptive::parse_source_id(&record.source_id)
                                else {
                                    continue;
                                };
                                let nid = node_id(
                                    p,
                                    "source_revision",
                                    &source,
                                    &record.revision,
                                    &record.manifest_id,
                                );
                                let node = make_node(
                                    &nid,
                                    "source_revision",
                                    &format!("{} · archivo", source.relative_path),
                                    &source,
                                    &record.revision,
                                    "raw_markdown_utf8",
                                    None,
                                    None,
                                    ("COLD", "direct"),
                                    "archive_revision",
                                    None,
                                    None,
                                );
                                auxiliary.push((format!("source_revision:{nid}"), source, node));
                            }
                        }
                        Err(_) => archive_problem = true,
                    }
                } else if s.live.config.data_dir.join("cold").exists() {
                    archive_problem = true;
                }
            }
            auxiliary.sort_by(|a, b| a.0.cmp(&b.0));
            let auxiliary_offset = if auxiliary_page { offset } else { 0 };
            let take = 128usize.min(2000usize.saturating_sub(nodes.len()));
            let mut processed = 0;
            for (_, source, node) in auxiliary.iter().skip(auxiliary_offset).take(take) {
                processed += 1;
                let Ok(current) = read_current(&s.live.config, source) else {
                    continue;
                };
                self.remember(p, &current)?;
                if node["kind"] == "summary" {
                    if let Some(did) = document_ids.get(source) {
                        edges.push(edge(
                            node["id"].as_str().unwrap_or(""),
                            did,
                            "derived_from",
                            "summary",
                            None,
                            None,
                            None,
                        ));
                    }
                }
                let mut node = node.clone();
                if node["kind"] == "source_revision" {
                    node["revision_state"] =
                        json!(if node["source"]["revision"] == current.revision {
                            "current"
                        } else {
                            "historical"
                        });
                }
                nodes.push(node);
            }
            let consumed = auxiliary_offset + processed;
            auxiliary_partial = consumed < auxiliary.len();
            if auxiliary_partial {
                auxiliary_next = Some(self.encode_cursor(true, consumed, &s.identity, filter));
            }
        }
        let mut vectors = Vec::new();
        for (native, nid) in &node_by_chunk {
            let mut st=s.connection.prepare("SELECT model_id,dimensions,CASE WHEN length(vector_data)<=32768 THEN vector_data ELSE X'' END FROM vectors WHERE generation_id=?1 AND chunk_id=?2 ORDER BY model_id LIMIT 2").map_err(sql)?;
            let rows = st
                .query_map(params![s.generation, native], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        row_usize(r, 1)?,
                        r.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .map_err(sql)?;
            for row in rows {
                let (model, dimensions, bytes) = row.map_err(sql)?;
                if dimensions == 0
                    || dimensions > 4096
                    || bytes.len() != dimensions * 4
                    || vectors.len() >= 500
                    || vectors.len().saturating_mul(dimensions) > 2_000_000
                {
                    continue;
                }
                let values = bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                vectors.push(crate::view_semantic::InputVector {
                    id: nid.clone(),
                    model_identity: model,
                    values,
                });
            }
        }
        s.connection.execute_batch("COMMIT").map_err(sql)?;
        let model_group = vectors
            .iter()
            .filter(|v| valid_vector(v))
            .map(|v| (v.model_identity.clone(), v.values.len()))
            .min();
        let excluded_models = vectors
            .iter()
            .filter(|v| Some((v.model_identity.clone(), v.values.len())) != model_group)
            .count();
        let vectors: Vec<_> = vectors
            .into_iter()
            .filter(|v| {
                valid_vector(v) && Some((v.model_identity.clone(), v.values.len())) == model_group
            })
            .collect();
        let computation_started = std::time::Instant::now();
        let semantic = crate::view_semantic::analyze_cancellable(
            if query.mode == "semantic" {
                &vectors
            } else {
                &[]
            },
            6,
            0.75,
            || computation_started.elapsed() > Duration::from_millis(1500),
        )
        .ok_or_else(|| err("REQUEST_CANCELLED"))?;
        let has_vectors = if query.mode == "semantic" {
            !semantic.positions.is_empty()
        } else {
            !vectors.is_empty()
        };
        if query.mode == "semantic" {
            for n in &mut nodes {
                if let Some(position) = n["id"].as_str().and_then(|id| semantic.positions.get(id)) {
                    n["position"] =
                        serde_json::to_value(position).map_err(|_| err("INTERNAL_ERROR"))?;
                }
            }
            for e in &semantic.edges {
                if edges.len() >= 10000 {
                    break;
                }
                edges.push(edge(
                    &e.source,
                    &e.target,
                    "semantic_similarity",
                    "computed",
                    Some(e.score),
                    Some(&e.model_identity),
                    None,
                ));
            }
        }
        let next = if more {
            Some(self.encode_cursor(false, offset + chunks.len(), &s.identity, filter))
        } else {
            auxiliary_next
        };
        let mut warnings = Vec::new();
        if nodes.is_empty() && !auxiliary_page && offset == 0 {
            let (code, message) = if !filter.is_empty() {
                (
                    "FILTER_EMPTY",
                    "La búsqueda no produjo chunks; cambie el texto o restablezca filtros.",
                )
            } else if roots.is_empty() || !denied.is_empty() {
                (
                    "NO_AUTHORIZED_RESULTS",
                    "No hay chunks legibles bajo la autorización vigente; no se amplió el alcance.",
                )
            } else {
                (
                    "EMPTY_INDEX",
                    "El índice de este ámbito no contiene chunks; la indexación existente es una acción separada.",
                )
            };
            warnings.push(json!({"code":code,"message":message}));
        }

        if auxiliary_partial {
            warnings.push(json!({"code":"AUXILIARY_LIMIT","message":"Metadatos de resúmenes/archivo paginados a128 por lote; continúe con Siguiente lote."}));
        }
        if excluded_models > 0 {
            warnings.push(json!({"code":"MODEL_MISMATCH","message":"Se proyecta únicamente el primer grupo compatible por identidad completa y dimensión, ordenado de forma estable; los demás modelos no se comparan."}));
        }
        if vectors.len() >= 500 {
            warnings.push(json!({"code":"SEMANTIC_SAMPLE_LIMIT","message":"Semántica limitada a los primeros 500 vectores compatibles del lote; no son vecinos globales."}));
        }
        if archive_problem {
            warnings.push(json!({"code":"COLD_CORRUPT","message":"Catálogo de archivo no disponible o corrupto; no se reconstruyó desde resúmenes."}));
        }
        if !has_vectors {
            warnings.push(json!({"code":"NO_VECTORS","message":"Sin embeddings compatibles: disposición estructural; la distancia no mide similitud."}));
        }
        if stale > 0 {
            warnings.push(json!({"code":"SOURCE_CHANGED","message":"Hay revisiones del índice que ya no corresponden a las fuentes actuales; no se muestran."}));
        }
        if links_truncated {
            warnings.push(json!({"code":"LIMIT_EXCEEDED","message":"Lectura de enlaces limitada a 10000 registros; cobertura parcial."}));
        }
        let model = semantic.groups.first().map(|g| g.model_identity.clone());
        let projection_note = if query.mode == "semantic" && has_vectors {
            let mut note="PCA 2D determinista de embeddings normalizados en este lote; la distancia visual es una aproximación. Coseno top-k=6, umbral=0.75, unión de vecindades. ".to_owned();
            if let Some(group) = semantic.groups.first() {
                note.push_str(&format!("Muestra={}, dimensiones={}, varianza explicada={:?}, convergencia={}, degenerada={}.",group.count,group.dimensions,group.explained_variance,group.converged,group.degenerate));
            }
            note
        } else {
            "Disposición estructural; la distancia no mide similitud.".into()
        };
        let result = json!({"api_version":"rag-view-v1","project_id":id,"snapshot_id":s.identity,"generation_id":s.generation,"policy_fingerprint":s.live.policy,"sync_state":if stale>0{"stale"}else{"ready"},"capabilities":{"structure":true,"semantic_vectors":has_vectors,"archive":s.live.config.adaptive_memory.enabled,"read_only":true},"coverage":{"nodes_returned":nodes.len(),"nodes_total_authorized":if !more&&offset==0&&!auxiliary_partial&&!auxiliary_page {Some(nodes.len())}else{None},"edges_returned":edges.len(),"partial":more||offset>0||links_truncated||auxiliary_partial||auxiliary_page,"selection":if !filter.is_empty(){"search_results"}else if more||offset>0||auxiliary_partial||auxiliary_page{"page"}else{"all_authorized"},"semantic_candidate_count":vectors.len()},"projection":{"method":if query.mode=="semantic"&&has_vectors{"pca_2d"}else{"structural"},"model_identity":model,"approximate":true,"note":projection_note},"nodes":nodes,"edges":edges,"next_cursor":next,"warnings":warnings});
        if serde_json::to_vec(&result)
            .map_err(|_| err("INTERNAL_ERROR"))?
            .len()
            > RESPONSE_LIMIT
        {
            return Err(err("LIMIT_EXCEEDED"));
        }
        if archive_membership_stamp(&s.live.config)? != s.archive_stamp {
            return Err(err("SNAPSHOT_EXPIRED"));
        }
        // Recheck live policy after the bounded read before releasing content.
        if load_live(p)?.policy != s.live.policy {
            return Err(err("ROOT_REVOKED"));
        }
        let mut cache = self.cached.lock().map_err(|_| err("INTERNAL_ERROR"))?;
        let project_cache = cache.entry(id.into()).or_default();
        project_cache.clear();
        for node in result["nodes"]
            .as_array()
            .ok_or_else(|| err("INTERNAL_ERROR"))?
        {
            let nid = node["id"].as_str().ok_or_else(|| err("INTERNAL_ERROR"))?;
            let native_id = node_by_chunk
                .iter()
                .find(|(_, v)| v.as_str() == nid)
                .map(|(native, _)| native.clone());
            project_cache.insert(
                nid.into(),
                CachedNode {
                    snapshot: s.identity.clone(),
                    node: node.clone(),
                    native_id,
                },
            );
        }
        Ok(result)
    }
    pub fn detail(
        &self,
        id: &str,
        node_id: &str,
        snapshot_id: &str,
        offset: usize,
        limit: usize,
        source: bool,
    ) -> Result<Value> {
        if limit == 0 || limit > 128 * 1024 {
            return Err(err("LIMIT_EXCEEDED"));
        }
        let p = self.project(id)?;
        let s = self.snapshot(p)?;
        if s.identity != snapshot_id {
            return Err(err("SNAPSHOT_EXPIRED"));
        }
        let cached = self
            .cached
            .lock()
            .map_err(|_| err("INTERNAL_ERROR"))?
            .get(id)
            .and_then(|m| m.get(node_id))
            .cloned()
            .ok_or_else(|| err("NODE_NOT_FOUND"))?;
        if cached.snapshot != snapshot_id {
            return Err(err("SNAPSHOT_EXPIRED"));
        }
        let mut node = cached.node;
        let reference = SourceRef {
            root_id: node["source"]["root_id"]
                .as_str()
                .ok_or_else(|| err("NODE_NOT_FOUND"))?
                .into(),
            relative_path: node["source"]["relative_path"]
                .as_str()
                .ok_or_else(|| err("NODE_NOT_FOUND"))?
                .into(),
        };
        let current = read_current(&s.live.config, &reference)?;
        if let Some(origin) =
            crate::view_sources::check_source(&s.live.config, &reference).map_err(err)?
        {
            node["origin"] = origin;
        }
        if node["kind"] == "source_revision" {
            node["revision_state"] = json!(if node["source"]["revision"] == current.revision {
                "current"
            } else {
                "historical"
            });
        }

        let revision = node["source"]["revision"]
            .as_str()
            .ok_or_else(|| err("NODE_NOT_FOUND"))?;
        let kind = node["kind"].as_str().unwrap_or("");
        let inherited = authorized_memories(&s.live, &s.memories);
        let inherited = inherited.iter().find(|m| {
            m.fully_covered_sources
                .contains(&adaptive::source_id(&reference))
        });
        let mut pinned = inherited.is_some_and(|m| m.pinned);
        let mut provenance = inherited
            .and_then(|m| serde_json::to_value(m).ok())
            .unwrap_or(Value::Null);
        let (text, content_kind, basis) = match kind {
            "chunk" => {
                if current.revision != revision {
                    return Err(err("SOURCE_CHANGED"));
                }
                let native = cached
                    .native_id
                    .as_deref()
                    .ok_or_else(|| err("NODE_NOT_FOUND"))?;
                let (start,end,body):(usize,usize,Vec<u8>)=s.connection.query_row("SELECT start_byte,end_byte,CASE WHEN length(body)<=4194368 THEN body ELSE NULL END FROM chunks WHERE generation_id=?1 AND chunk_id=?2 AND root_id=?3 AND relative_path=?4 AND revision=?5",params![s.generation,native,reference.root_id,reference.relative_path,revision],|r|{let body=match r.get_ref(2)?{rusqlite::types::ValueRef::Text(b)|rusqlite::types::ValueRef::Blob(b)=>b.to_vec(),_=>return Err(rusqlite::Error::InvalidColumnType(2,"body".into(),rusqlite::types::Type::Null))};Ok((row_usize(r,0)?,row_usize(r,1)?,body))}).map_err(|_|err("SNAPSHOT_EXPIRED"))?;
                if body.len() > SOURCE_LIMIT + 64 {
                    return Err(err("LIMIT_EXCEEDED"));
                }
                if body.starts_with(b"VRGA")
                    && (body.len() < 46
                        || u64::from_le_bytes(body[6..14].try_into().unwrap())
                            > SOURCE_LIMIT as u64)
                {
                    return Err(err("LIMIT_EXCEEDED"));
                }
                let raw = if body.starts_with(b"VRGA") {
                    crate::adaptive_codec::decode_frame(&body).map_err(|_| err("COLD_CORRUPT"))?
                } else {
                    body
                };
                let text = String::from_utf8(raw).map_err(|_| err("SOURCE_CHANGED"))?;
                if current.body_text.get(start..end) != Some(text.as_str()) {
                    return Err(err("SOURCE_CHANGED"));
                }
                if source {
                    (
                        current.raw_markdown.clone(),
                        "exact_source",
                        "raw_markdown_utf8",
                    )
                } else {
                    (text, "exact_chunk", "body_text_utf8")
                }
            }
            "document" => {
                if current.revision != revision {
                    return Err(err("SOURCE_CHANGED"));
                }
                (
                    current.raw_markdown.clone(),
                    "exact_source",
                    "raw_markdown_utf8",
                )
            }
            "summary" => {
                let memories = authorized_memories(&s.live, &s.memories);
                let memory = memories
                    .iter()
                    .find(|m| {
                        m.sources
                            .first()
                            .is_some_and(|sp| node_id_for_memory(p, m, sp) == node_id)
                    })
                    .ok_or_else(|| err("ROOT_REVOKED"))?;
                pinned = memory.pinned;
                provenance = serde_json::to_value(memory).map_err(|_| err("INTERNAL_ERROR"))?;
                if source {
                    if current.revision != revision {
                        return Err(err("SOURCE_CHANGED"));
                    }
                    (
                        current.raw_markdown.clone(),
                        "exact_source",
                        "raw_markdown_utf8",
                    )
                } else {
                    (
                        memory
                            .summary
                            .clone()
                            .ok_or_else(|| err("NODE_NOT_FOUND"))?,
                        "derived_summary",
                        "none",
                    )
                }
            }
            "source_revision" => {
                let archive = ArchiveStore::open_existing(&s.live.config.data_dir)
                    .map_err(|_| err("SOURCE_UNAVAILABLE"))?;
                let bytes = archive
                    .restore_bounded(
                        &s.live.scope,
                        &adaptive::source_id(&reference),
                        revision,
                        8 * 1024 * 1024,
                    )
                    .map_err(|_| err("COLD_CORRUPT"))?;
                (
                    String::from_utf8(bytes).map_err(|_| err("COLD_CORRUPT"))?,
                    "archive_revision",
                    "raw_markdown_utf8",
                )
            }
            _ => return Err(err("NODE_NOT_FOUND")),
        };
        self.remember(p, &current)?;
        let start = boundary(&text, offset.min(text.len()));
        let end = boundary(&text, start.saturating_add(limit).min(text.len()));
        if load_live(p)?.policy != s.live.policy {
            return Err(err("ROOT_REVOKED"));
        }
        // A second capability read prevents metadata/config changes during COLD decode leaking stale access.
        let checked = read_current(&s.live.config, &reference)?;
        if checked.revision != current.revision {
            return Err(err("SOURCE_CHANGED"));
        }
        Ok(
            json!({"node":node,"source":node["source"],"revision":revision,"text":&text[start..end],"content_kind":content_kind,"offset_basis":basis,"offset":start,"next_offset":if end<text.len(){Some(end)}else{None},"total_bytes":text.len(),"pinned":pinned,"provenance":provenance,"snapshot_id":s.identity,"read_only":true}),
        )
    }
    pub fn neighbors(&self, id: &str, node_id: &str, snapshot_id: &str) -> Result<Value> {
        let q = GraphQuery {
            snapshot_id: Some(snapshot_id.into()),
            ..Default::default()
        };
        let mut graph = self.graph(id, &q)?;
        let edges = graph["edges"]
            .as_array()
            .ok_or_else(|| err("INTERNAL_ERROR"))?
            .iter()
            .filter(|e| e["source"] == node_id || e["target"] == node_id)
            .cloned()
            .collect::<Vec<_>>();
        let mut ids = BTreeSet::from([node_id.to_owned()]);
        for e in &edges {
            ids.insert(e["source"].as_str().unwrap_or("").into());
            ids.insert(e["target"].as_str().unwrap_or("").into());
        }
        graph["nodes"] = Value::Array(
            graph["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|n| ids.contains(n["id"].as_str().unwrap_or("")))
                .cloned()
                .collect(),
        );
        graph["edges"] = json!(edges);
        graph["coverage"]["nodes_returned"] = json!(graph["nodes"].as_array().unwrap().len());
        graph["coverage"]["edges_returned"] = json!(edges.len());
        graph["coverage"]["selection"] = json!("neighbor_subset");
        graph["coverage"]["partial"] = json!(true);
        Ok(graph)
    }
    fn encode_cursor(
        &self,
        auxiliary: bool,
        offset: usize,
        snapshot: &str,
        filter: &str,
    ) -> String {
        let phase = if auxiliary { "aux" } else { "chunks" };
        format!(
            "{phase}.{offset}.{}",
            digest(&[&self.secret, snapshot, filter, phase, &offset.to_string()])
        )
    }
    fn decode_cursor(&self, cursor: &str, snapshot: &str, filter: &str) -> Result<(bool, usize)> {
        if cursor.len() > 110 {
            return Err(err("SNAPSHOT_EXPIRED"));
        }
        let mut parts = cursor.split('.');
        let phase = parts.next().ok_or_else(|| err("SNAPSHOT_EXPIRED"))?;
        let offset: usize = parts
            .next()
            .ok_or_else(|| err("SNAPSHOT_EXPIRED"))?
            .parse()
            .map_err(|_| err("SNAPSHOT_EXPIRED"))?;
        let auxiliary = match phase {
            "aux" => true,
            "chunks" => false,
            _ => return Err(err("SNAPSHOT_EXPIRED")),
        };
        if offset > 100_000_000 || self.encode_cursor(auxiliary, offset, snapshot, filter) != cursor
        {
            return Err(err("SNAPSHOT_EXPIRED"));
        }
        Ok((auxiliary, offset))
    }
}
/// Authorized, derived metadata only. Original bodies and vectors are not retained.
struct FullGraph {
    snapshot: String,
    indexed: usize,
    root_indexed: usize,
    stale_chunks: usize,
    denied_chunks: usize,
    nodes: Vec<Value>,
    native_ids: Vec<String>,
    edges: Vec<Value>,
    node_indices: BTreeMap<String, usize>,
    semantic_count: usize,
    semantic_note: String,
    origin_hierarchies: BTreeMap<String, Value>,
}
#[derive(Clone)]
struct TrackedSource {
    revision: String,
    stamp: Option<(u64, u128)>,
}
#[derive(Clone)]
struct CachedNode {
    snapshot: String,
    node: Value,
    native_id: Option<String>,
}
fn matches_path(path: &Path, config: &VrgConfig) -> bool {
    path == config.data_dir || config.roots.iter().any(|r| path == r.path)
}
fn load_live(p: &Project) -> Result<Live> {
    if canonical(&p.profile)? != p.profile {
        return Err(err("ROOT_REVOKED"));
    }
    if let (Some(project), Some(registry_path)) = (&p.registered_project, &p.registry_path) {
        let registry: Registry = serde_json::from_slice(&bounded_file(registry_path, 256 * 1024)?)
            .map_err(|_| err("ROOT_REVOKED"))?;
        if registry.excluded_projects.contains(project)
            || !registry
                .projects
                .iter()
                .any(|r| &r.project == project && r.profile == p.profile)
        {
            return Err(err("ROOT_REVOKED"));
        }
    }
    let raw = bounded_file(&p.profile, 256 * 1024)?;
    let config = crate::view_sources::parse_config(&p.profile, &raw).map_err(err)?;
    // Viewer profiles must have explicit roots/data paths; no cwd-dependent legacy expansion.
    if !config.project_memory.is_empty() || config.data_dir.to_string_lossy().contains("{project}")
    {
        return Err(err("SCOPE_MISMATCH"));
    }
    config.validate().map_err(|_| err("ROOT_REVOKED"))?;
    for r in &config.roots {
        if canonical(&r.path)? != r.path {
            return Err(err("ROOT_REVOKED"));
        }
        if r.path.starts_with(&config.data_dir) || config.data_dir.starts_with(&r.path) {
            return Err(err("SCOPE_MISMATCH"));
        }
    }
    let mut roots = config.roots.clone();
    roots.sort_by(|a, b| a.id.cmp(&b.id));
    let encoded = serde_json::to_vec(&roots).map_err(|_| err("INTERNAL_ERROR"))?;
    let scope = hex::encode(Sha256::digest(&encoded));
    let source_policy = crate::view_sources::load_policy(&p.profile, &config).map_err(err)?;
    let config_policy = hex::encode(Sha256::digest(&raw));
    let policy = source_policy
        .map(|v| digest(&[&config_policy, &v]))
        .unwrap_or(config_policy);
    Ok(Live {
        config,
        policy,
        scope,
    })
}
fn archive_membership_stamp(config: &VrgConfig) -> Result<String> {
    if !config.adaptive_memory.enabled {
        return Ok(String::new());
    }
    let cold = config.data_dir.join("cold");
    let metadata = match fs::symlink_metadata(&cold) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(_) => return Ok("unavailable".into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok("unavailable".into());
    }
    let entries = fs::read_dir(cold).map_err(|_| err("SOURCE_UNAVAILABLE"))?;
    let mut stamps = Vec::new();
    for (i, entry) in entries.enumerate() {
        if i >= 4096 {
            return Err(err("LIMIT_EXCEEDED"));
        }
        let entry = entry.map_err(|_| err("SOURCE_UNAVAILABLE"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|n| n.ends_with(".arc")) else {
            continue;
        };
        let m = entry.metadata().map_err(|_| err("SOURCE_UNAVAILABLE"))?;
        stamps.push((
            name.to_owned(),
            m.len(),
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos()),
        ));
    }
    stamps.sort();
    let encoded = serde_json::to_vec(&stamps).map_err(|_| err("INTERNAL_ERROR"))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}
fn valid_vector(v: &crate::view_semantic::InputVector) -> bool {
    v.model_identity
        .rsplit_once('#')
        .is_some_and(|(_, fp)| fp.len() == 64 && fp.bytes().all(|b| b.is_ascii_hexdigit()))
        && v.values.iter().all(|x| x.is_finite())
        && v.values.iter().any(|x| *x != 0.0)
}
fn load_chunk_text(connection: &Connection, generation: &str, native: &str) -> Result<String> {
    let body:Vec<u8>=connection.query_row("SELECT CASE WHEN length(body)<=4194368 THEN body ELSE NULL END FROM chunks WHERE generation_id=?1 AND chunk_id=?2",params![generation,native],|r|match r.get_ref(0)?{rusqlite::types::ValueRef::Text(b)|rusqlite::types::ValueRef::Blob(b)=>Ok(b.to_vec()),_=>Err(rusqlite::Error::InvalidColumnType(0,"body".into(),rusqlite::types::Type::Null))}).map_err(|_|err("LIMIT_EXCEEDED"))?;
    if body.starts_with(b"VRGA")
        && (body.len() < 46
            || u64::from_le_bytes(body[6..14].try_into().unwrap()) > SOURCE_LIMIT as u64)
    {
        return Err(err("LIMIT_EXCEEDED"));
    }
    let raw = if body.starts_with(b"VRGA") {
        crate::adaptive_codec::decode_frame(&body).map_err(|_| err("COLD_CORRUPT"))?
    } else {
        body
    };
    String::from_utf8(raw).map_err(|_| err("SOURCE_CHANGED"))
}
fn source_stamp(config: &VrgConfig, source: &SourceRef) -> Option<(u64, u128)> {
    let root = config.roots.iter().find(|r| r.id == source.root_id)?;
    let metadata = fs::symlink_metadata(root.path.join(&source.relative_path)).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    Some((
        metadata.len(),
        metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos(),
    ))
}
fn read_current(config: &VrgConfig, source: &SourceRef) -> Result<SourceDocument> {
    crate::view_sources::check_source(config, source).map_err(err)?;
    vault::read_source(config, &source.root_id, &source.relative_path, SOURCE_LIMIT)
        .map_err(|_| err("ROOT_REVOKED"))
}
fn authorized_memories(live: &Live, memories: &[StoredMemory]) -> Vec<StoredMemory> {
    let project = hex::encode(Sha256::digest(
        live.config.data_dir.to_string_lossy().as_bytes(),
    ));
    memories
        .iter()
        .filter(|m| {
            m.privacy_scope_id == live.scope
                && m.project_id == project
                && !m.sources.is_empty()
                && m.sources.iter().all(|span| {
                    adaptive::parse_source_id(&span.source_id).is_some_and(|source| {
                        read_current(&live.config, &source).is_ok_and(|doc| {
                            doc.revision == span.revision_sha256
                                && span.span_start <= span.span_end
                                && doc
                                    .raw_markdown
                                    .get(span.span_start..span.span_end)
                                    .is_some()
                        })
                    })
                })
        })
        .cloned()
        .collect()
}
fn tier_str(tier: &adaptive::MemoryTier) -> &'static str {
    match tier {
        adaptive::MemoryTier::HOT => "HOT",
        adaptive::MemoryTier::WARM => "WARM",
        adaptive::MemoryTier::COLD => "COLD",
    }
}
fn tier_for<'a>(source: &SourceRef, memories: &'a [StoredMemory]) -> (&'a str, &'static str) {
    let sid = adaptive::source_id(source);
    memories
        .iter()
        .find(|m| m.fully_covered_sources.contains(&sid))
        .map(|m| (tier_str(&m.tier), "inherited_from_source"))
        .unwrap_or(("unclassified", "none"))
}
fn node_id(p: &Project, kind: &str, source: &SourceRef, revision: &str, extra: &str) -> String {
    digest(&[
        &p.id,
        kind,
        &source.root_id,
        &source.relative_path,
        revision,
        extra,
    ])
}
fn node_id_for_memory(p: &Project, m: &StoredMemory, sp: &adaptive::SourceSpan) -> String {
    let source = adaptive::parse_source_id(&sp.source_id).unwrap_or(SourceRef {
        root_id: String::new(),
        relative_path: String::new(),
    });
    node_id(p, "summary", &source, &sp.revision_sha256, &m.memory_id)
}
fn boundary(s: &str, mut n: usize) -> usize {
    while n > 0 && !s.is_char_boundary(n) {
        n -= 1;
    }
    n
}
fn preview(s: &str) -> String {
    s[..boundary(s, s.len().min(160))].into()
}
#[allow(clippy::too_many_arguments)]
fn make_node(
    id: &str,
    kind: &str,
    label: &str,
    source: &SourceRef,
    revision: &str,
    basis: &str,
    start: Option<usize>,
    end: Option<usize>,
    tier: (&str, &str),
    content_kind: &str,
    preview: Option<String>,
    chunk_id: Option<String>,
) -> Value {
    json!({"id":id,"kind":kind,"label":label.chars().take(160).collect::<String>(),"source":{"root_id":source.root_id,"relative_path":source.relative_path,"revision":revision,"offset_basis":basis,"start_byte":start,"end_byte":end},"tier":tier.0,"tier_origin":tier.1,"content_kind":content_kind,"revision_state":"current","preview":preview,"chunk_id":chunk_id,"needs_expansion":kind=="summary","position":null})
}
fn edge(
    source: &str,
    target: &str,
    kind: &str,
    scope: &str,
    score: Option<f64>,
    model: Option<&str>,
    original: Option<&str>,
) -> Value {
    json!({"id":digest(&[source,target,kind,original.unwrap_or("")]),"source":source,"target":target,"kind":kind,"score":score,"provenance_scope":scope,"model_identity":model,"original_link_kind":original})
}
