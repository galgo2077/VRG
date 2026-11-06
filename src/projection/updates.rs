//! Schema2: exact known-source mechanical updates, never AI reading credit.
//! All state is private and derived. Original sources and canonical notes stay read-only.
use super::*;
use rustix::fs::{Mode, OFlags, RenameFlags, openat, renameat, renameat_with};
use std::io::{Cursor, Seek, SeekFrom};

const POLICY: &str = "vrg-known-utf8-v2";
const NAMESPACE: &str = "incremental-v2";
const MAX_VIEW: usize = 32768;
const MAX_STATE: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateManifest {
    pub schema: u32,
    pub data_dir: PathBuf,
    pub corpus_root_id: String,
    pub corpus_path: PathBuf,
    pub original_roots: Vec<PathBuf>,
    pub note_roots: Vec<PathBuf>,
    pub policy_probes: Vec<PolicyProbe>,
    pub policy_version: String,
    pub max_source_bytes: usize,
    pub part_bytes: usize,
    pub sources: Vec<KnownSource>,
    pub notes: Vec<NoteOrigin>,
    pub hubs: Vec<Hub>,
    pub passthrough_views: Vec<ProjectionView>,
    #[serde(default)]
    pub discovery: Option<DiscoveryPolicy>,
}
/// Explicit extension of admission inside existing canonical original roots only.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryPolicy {
    pub enabled: bool,
    pub extensions: Vec<String>,
    pub excluded_components: Vec<String>,
    pub excluded_paths: Vec<ExcludedPath>,
    #[serde(default)]
    pub extensionless_shebang: bool,
    #[serde(default)]
    pub filenames: Vec<String>,
    #[serde(default)]
    pub module_docs: bool,
    #[serde(default)]
    pub direct_sources: bool,
    #[serde(default)]
    pub automatic_memory: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExcludedPath {
    pub root: PathBuf,
    pub relative_path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiscoveredSource {
    pub original_root: PathBuf,
    pub relative_path: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceOutline {
    pub symbols: Vec<String>,
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub facts: crate::code_synthesis::SourceFacts,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModuleDocument {
    pub sha256: String,
    pub source_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub significance_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_text: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyProbe {
    pub root: PathBuf,
    pub relative_path: String,
    pub expected_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Utf8Text,
    FilteredSnapshot,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InitialReadScope {
    #[default]
    FullSource,
    AdmittedViewOnly,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownSource {
    pub source_id: String,
    pub original_root: PathBuf,
    pub relative_path: String,
    pub extension: String,
    pub kind: SourceKind,
    pub initial_sha256: Option<String>,
    pub initial_parts: Vec<ProjectionView>,
    pub historical_read_evidence: Option<String>,
    #[serde(default)]
    pub initial_read_scope: InitialReadScope,
    pub dependent_notes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteOrigin {
    pub original_root: PathBuf,
    pub relative_path: String,
    pub expected_sha256: String,
    pub view: ProjectionView,
    #[serde(default)]
    pub source_dependencies: Vec<SourceDependency>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDependency {
    pub original_root: PathBuf,
    pub relative_path: String,
    pub expected_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hub {
    pub relative_path: String,
    pub initial_sha256: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub source_ids: Vec<String>,
    pub note_paths: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub relative_path: String,
    pub sha256: String,
    /// None for approved baseline parts: exact ranges remain in their original ledger.
    pub source_byte_range: Option<[usize; 2]>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceHead {
    pub observed_sha256: Option<String>,
    pub state: String,
    pub read_status: String,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_document: Option<Part>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Head {
    pub schema: u32,
    pub authorization_sha256: String,
    pub sources: BTreeMap<String, SourceHead>,
    pub retired_notes: BTreeSet<String>,
    pub outputs: BTreeMap<String, String>,
    /// Mechanical projection and semantic reading are independent dimensions.
    pub indexed_generation: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub discovered_sources: BTreeMap<String, DiscoveredSource>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source_outlines: BTreeMap<String, SourceOutline>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub module_documents: BTreeMap<String, ModuleDocument>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub memory_documents: BTreeMap<String, ModuleDocument>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub synthesis_version: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    path: String,
    old_sha256: Option<String>,
    new_sha256: Option<String>,
    stage: Option<String>,
    backup: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    authorization_sha256: String,
    prior_head_sha256: Option<String>,
    previous_head: Head,
    next_head: Head,
    operations: Vec<Operation>,
    #[serde(default, skip_serializing_if = "is_false")]
    incremental: bool,
}
fn is_zero(value: &u32) -> bool {
    *value == 0
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn json<T: Serialize>(value: &T) -> VrgResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| denied())
}
fn safe_relative(value: &str) -> VrgResult<()> {
    if value.contains('\\')
        || value.chars().any(char::is_control)
        || value
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(denied());
    }
    parts(Path::new(value))?;
    Ok(())
}
fn safe_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn view_hash(view: &ProjectionView, manifest: &UpdateManifest) -> VrgResult<()> {
    safe_relative(&view.relative_path)?;
    if view.root_id != manifest.corpus_root_id
        || !valid_hash(&view.sha256)
        || !view.relative_path.ends_with(".md")
    {
        return Err(denied());
    }
    Ok(())
}
fn disjoint(a: &Path, b: &Path) -> bool {
    !a.starts_with(b) && !b.starts_with(a)
}
fn canonical_directory(path: &Path) -> VrgResult<File> {
    if path.canonicalize().map_err(|_| denied())? != path {
        return Err(denied());
    }
    open_dir(path)
}
fn validate(manifest: &UpdateManifest, config: &VrgConfig) -> VrgResult<()> {
    if manifest.schema != 2
        || manifest.policy_version != POLICY
        || manifest.data_dir != config.data_dir
        || manifest.max_source_bytes == 0
        || manifest.max_source_bytes > 64 * 1024 * 1024
        || !(1..=16384).contains(&manifest.part_bytes)
        || config.roots.len() != 1
        || config.roots[0].id != manifest.corpus_root_id
        || config.roots[0].path != manifest.corpus_path
        || !disjoint(&manifest.data_dir, &manifest.corpus_path)
    {
        return Err(denied());
    }
    canonical_directory(&manifest.data_dir)?;
    canonical_directory(&manifest.corpus_path)?;
    let original: BTreeSet<_> = manifest.original_roots.iter().collect();
    let notes: BTreeSet<_> = manifest.note_roots.iter().collect();
    if original.len() != manifest.original_roots.len() || notes.len() != manifest.note_roots.len() {
        return Err(denied());
    }
    for root in original.iter().chain(notes.iter()) {
        if root.to_string_lossy().chars().any(char::is_control) {
            return Err(denied());
        }
        canonical_directory(root)?;
        if !disjoint(root, &manifest.data_dir) || !disjoint(root, &manifest.corpus_path) {
            return Err(denied());
        }
    }
    if original
        .iter()
        .any(|root| notes.iter().any(|note| !disjoint(root, note)))
    {
        return Err(denied());
    }
    let mut ids = BTreeSet::new();
    let mut source_paths = BTreeSet::new();
    let mut initial = BTreeMap::new();
    let mut note_paths = BTreeSet::new();
    for note in &manifest.notes {
        if !notes.contains(&&note.original_root) || !valid_hash(&note.expected_sha256) {
            return Err(denied());
        }
        safe_relative(&note.relative_path)?;
        for dependency in &note.source_dependencies {
            safe_relative(&dependency.relative_path)?;
            if !original.contains(&&dependency.original_root)
                || !valid_hash(&dependency.expected_sha256)
                || forbidden_path(&dependency.relative_path)
            {
                return Err(denied());
            }
        }
        view_hash(&note.view, manifest)?;
        if !note_paths.insert(note.view.relative_path.as_str()) {
            return Err(denied());
        }
        initial.insert(note.view.relative_path.as_str(), note.view.sha256.as_str());
    }
    for source in &manifest.sources {
        safe_relative(&source.relative_path)?;
        let extension = Path::new(&source.relative_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if !valid_hash(&source.source_id)
            || !ids.insert(source.source_id.as_str())
            || !original.contains(&&source.original_root)
            || extension != source.extension
            || source
                .initial_sha256
                .as_ref()
                .is_some_and(|s| !valid_hash(s))
            || !source_paths.insert((&source.original_root, &source.relative_path))
            || forbidden_path(&source.relative_path)
            || source.initial_sha256.is_none() && !source.initial_parts.is_empty()
        {
            return Err(denied());
        }
        // Filtered Linux observations are captured files, never live device/proc/sys inputs.
        if ["/proc", "/sys", "/dev", "/etc", "/run"]
            .iter()
            .any(|p| source.original_root.starts_with(p))
        {
            return Err(denied());
        }
        for view in &source.initial_parts {
            view_hash(view, manifest)?;
            if note_paths.contains(view.relative_path.as_str()) {
                return Err(denied());
            }
            if initial
                .insert(view.relative_path.as_str(), view.sha256.as_str())
                .is_some_and(|s| s != view.sha256)
            {
                return Err(denied());
            }
        }
        if source
            .dependent_notes
            .iter()
            .any(|s| !note_paths.contains(s.as_str()))
        {
            return Err(denied());
        }
    }
    for view in &manifest.passthrough_views {
        view_hash(view, manifest)?;
        if initial
            .insert(view.relative_path.as_str(), view.sha256.as_str())
            .is_some()
        {
            return Err(denied());
        }
    }
    let mut hub_paths = BTreeSet::new();
    for hub in &manifest.hubs {
        safe_relative(&hub.relative_path)?;
        if !valid_hash(&hub.initial_sha256)
            || !safe_label(&hub.title)
            || hub.aliases.iter().any(|s| !safe_label(s))
            || !hub_paths.insert(&hub.relative_path)
            || initial.contains_key(hub.relative_path.as_str())
            || hub.source_ids.iter().any(|s| !ids.contains(s.as_str()))
            || hub
                .note_paths
                .iter()
                .any(|s| !note_paths.contains(s.as_str()))
        {
            return Err(denied());
        }
        if hub.source_ids.iter().collect::<BTreeSet<_>>().len() != hub.source_ids.len() {
            return Err(denied());
        }
    }
    let linked_sources = manifest
        .hubs
        .iter()
        .flat_map(|h| h.source_ids.iter())
        .collect::<BTreeSet<_>>();
    if manifest
        .sources
        .iter()
        .any(|s| !linked_sources.contains(&s.source_id))
    {
        return Err(denied());
    }
    for probe in &manifest.policy_probes {
        safe_relative(&probe.relative_path)?;
        if !original.contains(&&probe.root)
            || Path::new(&probe.relative_path)
                .file_name()
                .and_then(|s| s.to_str())
                != Some(".codexignore")
            || probe
                .expected_sha256
                .as_ref()
                .is_some_and(|s| !valid_hash(s))
        {
            return Err(denied());
        }
    }
    let probe_paths = manifest
        .policy_probes
        .iter()
        .map(|p| (&p.root, p.relative_path.as_str()))
        .collect::<BTreeSet<_>>();
    if probe_paths.len() != manifest.policy_probes.len() {
        return Err(denied());
    }
    // Each ordinary source needs exact exclusion-policy probes for every ancestor.
    for source in &manifest.sources {
        if source.kind == SourceKind::FilteredSnapshot {
            continue;
        }
        let mut directory = Path::new(&source.relative_path)
            .parent()
            .unwrap_or(Path::new(""));
        loop {
            let relative = directory
                .join(".codexignore")
                .to_string_lossy()
                .into_owned();
            if !probe_paths.contains(&(&source.original_root, relative.as_str())) {
                return Err(denied());
            }
            let Some(parent) = directory.parent() else {
                break;
            };
            directory = parent;
        }
    }
    if let Some(policy) = &manifest.discovery {
        if !policy.enabled
            || policy.extensions.is_empty()
            || policy
                .extensions
                .iter()
                .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric()))
            || policy
                .excluded_components
                .iter()
                .any(|s| s.is_empty() || s.contains('/') || s.contains('\\'))
            || policy
                .filenames
                .iter()
                .any(|s| s.is_empty() || s.contains('/') || s.contains('\\'))
        {
            return Err(denied());
        }
        for item in &policy.excluded_paths {
            safe_relative(&item.relative_path)?;
            if !original.contains(&&item.root) {
                return Err(denied());
            }
        }
    }
    Ok(())
}
fn discovery_id(root: &Path, relative: &str) -> String {
    sha(format!("{}\n{relative}", root.to_string_lossy()).as_bytes())
}
fn discovery_excluded(policy: &DiscoveryPolicy, root: &Path, relative: &str) -> bool {
    forbidden_path(relative)
        || Path::new(relative).components().any(|p| {
            let name = p.as_os_str().to_string_lossy().to_ascii_lowercase();
            name.starts_with('.')
                || policy
                    .excluded_components
                    .iter()
                    .any(|v| v.eq_ignore_ascii_case(&name))
                || name.ends_with('~')
                || name.ends_with(".bak")
                || name.ends_with(".backup")
        })
        || policy.excluded_paths.iter().any(|p| {
            p.root == root
                && (relative == p.relative_path
                    || relative.starts_with(&format!("{}/", p.relative_path)))
        })
}
fn discovery_candidate(policy: &DiscoveryPolicy, relative: &str) -> bool {
    let path = Path::new(relative);
    let extension = path.extension().and_then(|p| p.to_str()).unwrap_or("");
    policy.extensions.iter().any(|p| p == extension)
        || path
            .file_name()
            .and_then(|p| p.to_str())
            .is_some_and(|p| policy.filenames.iter().any(|name| name == p))
        || extension.is_empty() && policy.extensionless_shebang
}
fn eligible_code_bytes(policy: &DiscoveryPolicy, relative: &str, bytes: &[u8]) -> bool {
    let path = Path::new(relative);
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| policy.filenames.iter().any(|allowed| allowed == name))
    {
        return true;
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    if !extension.is_empty() {
        return policy.extensions.iter().any(|allowed| allowed == extension);
    }
    policy.extensionless_shebang && bytes.starts_with(b"#!")
}
fn discovered_source(id: &str, source: &DiscoveredSource) -> KnownSource {
    KnownSource {
        source_id: id.to_owned(),
        original_root: source.original_root.clone(),
        relative_path: source.relative_path.clone(),
        extension: Path::new(&source.relative_path)
            .extension()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_owned(),
        kind: SourceKind::Utf8Text,
        initial_sha256: None,
        initial_parts: vec![],
        historical_read_evidence: None,
        initial_read_scope: InitialReadScope::FullSource,
        dependent_notes: vec![],
    }
}
fn effective_manifest(
    baseline: &UpdateManifest,
    discovered: &BTreeMap<String, DiscoveredSource>,
) -> VrgResult<UpdateManifest> {
    let mut manifest = baseline.clone();
    if discovered.is_empty() {
        return Ok(manifest);
    }
    let policy = baseline
        .discovery
        .as_ref()
        .filter(|p| p.enabled)
        .ok_or_else(denied)?;
    let mut paths = baseline
        .sources
        .iter()
        .map(|s| (s.original_root.clone(), s.relative_path.clone()))
        .collect::<BTreeSet<_>>();
    let mut ids = baseline
        .sources
        .iter()
        .map(|source| source.source_id.clone())
        .collect::<BTreeSet<_>>();
    let roots_by_id = baseline
        .sources
        .iter()
        .map(|source| (source.source_id.as_str(), &source.original_root))
        .collect::<BTreeMap<_, _>>();
    let mut hub_by_root = BTreeMap::new();
    for (index, hub) in baseline.hubs.iter().enumerate() {
        for id in &hub.source_ids {
            if let Some(root) = roots_by_id.get(id.as_str()) {
                hub_by_root.entry((*root).clone()).or_insert(index);
            }
        }
    }
    for (id, source) in discovered {
        safe_relative(&source.relative_path)?;
        if !baseline.original_roots.contains(&source.original_root)
            || !valid_hash(id)
            || discovery_id(&source.original_root, &source.relative_path) != *id
            || discovery_excluded(policy, &source.original_root, &source.relative_path)
            || !discovery_candidate(policy, &source.relative_path)
            || !paths.insert((source.original_root.clone(), source.relative_path.clone()))
            || !ids.insert(id.clone())
        {
            return Err(denied());
        }
        // Hubs retain real context membership; no unrelated origins or fake nodes.
        let index = hub_by_root
            .get(&source.original_root)
            .copied()
            .or_else(|| (!baseline.hubs.is_empty()).then_some(0))
            .ok_or_else(denied)?;
        manifest.hubs[index].source_ids.push(id.clone());
        manifest.sources.push(discovered_source(id, source));
    }
    Ok(manifest)
}
fn discover_directory(
    manifest: &UpdateManifest,
    policy: &DiscoveryPolicy,
    original_root: &Path,
    directory: &File,
    prefix: &str,
    known: &mut BTreeSet<(PathBuf, String)>,
    head: &mut Head,
) -> VrgResult<()> {
    let dir = cap_std::fs::Dir::from_std_file(directory.try_clone()?);
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let relative = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if safe_relative(&relative).is_err() || discovery_excluded(policy, original_root, &relative)
        {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            let full = original_root.join(&relative);
            // Nested explicitly authorized roots own their paths exactly once.
            if manifest
                .original_roots
                .iter()
                .any(|root| root != original_root && *root == full)
            {
                continue;
            }
            let child = File::from(
                openat(
                    directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| denied())?,
            );
            // A newly introduced ignore policy cannot silently broaden admission.
            let ignore_path = if relative.is_empty() {
                ".codexignore".to_owned()
            } else {
                format!("{relative}/.codexignore")
            };
            if let Some(bytes) = read_exact(&child, ".codexignore", 65536)? {
                if !bytes.is_empty()
                    && !manifest.policy_probes.iter().any(|p| {
                        p.root == original_root
                            && p.relative_path == ignore_path
                            && p.expected_sha256.as_ref() == Some(&sha(&bytes))
                    })
                {
                    continue;
                }
            }
            discover_directory(
                manifest,
                policy,
                original_root,
                &child,
                &relative,
                known,
                head,
            )?;
        } else if kind.is_file()
            && discovery_candidate(policy, &relative)
            && known.insert((original_root.to_owned(), relative.clone()))
        {
            let extension = Path::new(&relative)
                .extension()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            let filename_admitted = policy.filenames.iter().any(|p| p == name);
            if extension.is_empty() && !filename_admitted {
                let Some(mut file) = open_file(directory, std::ffi::OsStr::new(name))? else {
                    continue;
                };
                let mut prefix_bytes = [0u8; 2];
                if file.read(&mut prefix_bytes).ok() != Some(2) || &prefix_bytes != b"#!" {
                    continue;
                }
            }
            let id = discovery_id(original_root, &relative);
            head.discovered_sources.insert(
                id.clone(),
                DiscoveredSource {
                    original_root: original_root.to_owned(),
                    relative_path: relative,
                },
            );
            head.sources.insert(
                id,
                SourceHead {
                    observed_sha256: None,
                    state: "missing".into(),
                    read_status: "pending_review".into(),
                    parts: vec![],
                    direct_document: None,
                },
            );
        }
    }
    Ok(())
}
fn discover_sources(
    manifest: &UpdateManifest,
    roots: &BTreeMap<PathBuf, File>,
    head: &mut Head,
) -> VrgResult<()> {
    let Some(policy) = manifest.discovery.as_ref().filter(|p| p.enabled) else {
        return Ok(());
    };
    let mut known = manifest
        .sources
        .iter()
        .map(|s| (s.original_root.clone(), s.relative_path.clone()))
        .chain(
            head.discovered_sources
                .values()
                .map(|s| (s.original_root.clone(), s.relative_path.clone())),
        )
        .collect::<BTreeSet<_>>();
    for (root, directory) in roots {
        discover_directory(manifest, policy, root, directory, "", &mut known, head)?;
    }
    Ok(())
}
fn outline(bytes: &[u8], relative_path: &str) -> SourceOutline {
    let text = std::str::from_utf8(bytes).unwrap_or("");
    let mut result = SourceOutline {
        facts: crate::code_synthesis::analyze_source_for_path(bytes, relative_path),
        ..SourceOutline::default()
    };
    let tokens = |line: &str| {
        line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':' || c == '.'))
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    for line in text.lines() {
        let line = line.trim();
        let words = tokens(line);
        for (index, word) in words.iter().enumerate() {
            if [
                "fn",
                "def",
                "class",
                "struct",
                "enum",
                "trait",
                "interface",
                "function",
                "type",
                "mod",
            ]
            .contains(&word.as_str())
            {
                if let Some(name) = words.get(index + 1) {
                    if name.len() <= 160 {
                        result.symbols.push(format!("{word} {name}"));
                    }
                }
                break;
            }
        }
        if line.starts_with("use ")
            || line.starts_with("import ")
            || line.starts_with("from ")
            || line.starts_with("#include ")
            || line.starts_with("require(")
        {
            result
                .dependencies
                .push(words.iter().take(12).cloned().collect::<Vec<_>>().join(" "));
        }
    }
    result.symbols.sort();
    result.symbols.dedup();
    result.dependencies.sort();
    result.dependencies.dedup();
    result
}
fn render_module_documents(
    manifest: &UpdateManifest,
    head: &Head,
) -> VrgResult<BTreeMap<String, (ModuleDocument, Vec<u8>)>> {
    let mut groups = BTreeMap::<(PathBuf, String), Vec<&KnownSource>>::new();
    let Some(policy) = manifest
        .discovery
        .as_ref()
        .filter(|p| p.enabled && p.module_docs)
    else {
        return Ok(BTreeMap::new());
    };
    for source in &manifest.sources {
        if head.source_outlines.contains_key(&source.source_id)
            && discovery_candidate(policy, &source.relative_path)
        {
            let parent = Path::new(&source.relative_path)
                .parent()
                .unwrap_or(Path::new(""))
                .to_string_lossy()
                .into_owned();
            groups
                .entry((source.original_root.clone(), parent))
                .or_default()
                .push(source);
        }
    }
    let mut result = BTreeMap::new();
    for ((root, module), mut sources) in groups {
        sources.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        let path = format!("{NAMESPACE}/modules/{}.md", discovery_id(&root, &module));
        let mut text = format!(
            "# Module {}\n\nOrigin root: {}\n\nThis mechanically derived module reference records active source revisions, lexical declaration candidates and import evidence. It does not claim reviewed runtime behavior or inferred call relationships. Source files remain authoritative.\n\n",
            inline_code(if module.is_empty() { "." } else { &module }),
            inline_code(&root.to_string_lossy())
        );
        if head.synthesis_version > 0 {
            text.push_str(
                &crate::code_synthesis::synthesize_module(
                    &module,
                    &synthesis_sources(head, &sources)?,
                )
                .markdown,
            );
            text.push_str("\n\n");
        }
        let mut source_ids = Vec::new();
        for source in sources {
            let state = head.sources.get(&source.source_id).ok_or_else(denied)?;
            let outline = head
                .source_outlines
                .get(&source.source_id)
                .ok_or_else(denied)?;
            source_ids.push(source.source_id.clone());
            text.push_str(&format!(
                "## {}\n\nSource ID: {}\n\nRevision SHA256: {}\n\n",
                escaped_markdown(&source.relative_path),
                source.source_id,
                state.observed_sha256.as_deref().unwrap_or("unavailable")
            ));
            if !outline.symbols.is_empty() {
                text.push_str("Declaration candidates:\n\n");
                for symbol in &outline.symbols {
                    text.push_str(&format!("- {}\n", inline_code(symbol)));
                }
                text.push('\n');
            }
            if !outline.dependencies.is_empty() {
                text.push_str("Import evidence:\n\n");
                for dependency in &outline.dependencies {
                    text.push_str(&format!("- {}\n", inline_code(dependency)));
                }
                text.push('\n');
            }
            text.push_str("Attributed source fragments:\n\n");
            for part in state.parts.iter().chain(state.direct_document.iter()) {
                text.push_str(&format!(
                    "- [Source view](<{}>)\n",
                    relative_link(&path, &part.relative_path)?
                ));
            }
            text.push('\n');
        }
        // Existing transaction/view reader permits bounded 4 MiB module documents.
        if text.len() > 4 * 1024 * 1024 {
            return Err(denied());
        }
        let bytes = text.into_bytes();
        result.insert(
            path,
            (
                ModuleDocument {
                    sha256: sha(&bytes),
                    source_ids,
                    significance_sha256: None,
                    stable_text: None,
                },
                bytes,
            ),
        );
    }
    Ok(result)
}
/// Existing native watcher also observes approved original files. No additional service.
pub(crate) fn watch_sources(
    config: &VrgConfig,
) -> VrgResult<Vec<(PathBuf, Option<DiscoveryPolicy>)>> {
    let Some((manifest, _)) = load_manifest(config)? else {
        return Ok(Vec::new());
    };
    Ok(manifest
        .original_roots
        .iter()
        .cloned()
        .map(|root| (root, manifest.discovery.clone()))
        .chain(manifest.note_roots.iter().cloned().map(|root| (root, None)))
        .collect())
}
pub(crate) fn source_event_relevant(
    sources: &[(PathBuf, Option<DiscoveryPolicy>)],
    path: &Path,
) -> bool {
    sources.iter().any(|(root, policy)| {
        if path.starts_with(root) && path.file_name().is_some_and(|name| name == ".codexignore") {
            return true;
        }
        path.strip_prefix(root).ok().is_some_and(|relative| {
            let Some(relative) = relative.to_str() else {
                return false;
            };
            !forbidden_path(relative)
                && policy
                    .as_ref()
                    .is_none_or(|p| !discovery_excluded(p, root, relative))
                && (path.is_dir()
                    || !path.exists()
                    || policy
                        .as_ref()
                        .is_none_or(|p| discovery_candidate(p, relative))
                    || path.extension().is_some_and(|p| p == "md")
                    || path.file_name().is_some_and(|p| p == ".codexignore"))
        })
    })
}

fn forbidden_path(relative: &str) -> bool {
    Path::new(relative).components().any(|part| {
        let s = part.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(
            s.as_str(),
            ".git"
                | ".codex"
                | ".ssh"
                | ".aws"
                | ".gnupg"
                | "node_modules"
                | "target"
                | "__pycache__"
                | ".venv"
                | ".obsidian"
                | "auth.json"
                | ".env"
                | ".bash_history"
                | ".zsh_history"
                | ".python_history"
                | ".node_repl_history"
                | "transcripts"
                | "sessions"
        ) || s.ends_with(".sqlite")
            || s.ends_with(".sqlite3")
            || s.ends_with(".zip")
            || s.ends_with(".onnx")
            || s.ends_with(".ipynb")
            || s.ends_with(".pem")
            || s.ends_with(".key")
    })
}
// Same frontmatter privacy parser as initial VRG ingestion; additional known-pattern
// gate is conservative and does not assert universal secret detection.
/// A Rust signature's primitive pointer/array type is not a credential value.
/// This deliberately recognizes no strings, references, identifiers-as-values,
/// initializers, or plaintext declarations in other file formats.
fn rust_credential_type_declaration(relative: &str, prefix: &str, line: &str, rhs: &str) -> bool {
    if !relative.ends_with(".rs") || line.contains('=') {
        return false;
    }
    let rhs = rhs.trim().trim_end_matches(',').trim();
    let pointer = rhs
        .strip_prefix("*const ")
        .or_else(|| rhs.strip_prefix("*mut "))
        .is_some_and(|element| matches!(element.trim(), "u8" | "u16"));
    let array = rhs
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .and_then(|inner| inner.split_once(';'))
        .is_some_and(|(element, length)| {
            matches!(element.trim(), "u8" | "u16")
                && !length.trim().is_empty()
                && length.trim().bytes().all(|byte| byte.is_ascii_digit())
        });
    if !pointer && !array {
        return false;
    }
    let function = ["fn ", "fn(", "fn ("]
        .iter()
        .filter_map(|marker| prefix.rfind(marker))
        .max()
        .is_some_and(|at| {
            let signature = &prefix[at..];
            signature.contains('(') && !signature.contains(['{', '}', ';'])
        });
    let structure = prefix.rfind("struct ").is_some_and(|at| {
        let declaration = &prefix[at..];
        declaration.contains('{') && !declaration.contains('}') && !declaration.contains('=')
    });
    function || structure
}

fn private_key_material(text: &str) -> bool {
    for (start, _) in text.match_indices("-----BEGIN ") {
        let label_start = start + 11;
        let Some(end) = text[label_start..].find("-----") else {
            continue;
        };
        let label = &text[label_start..label_start + end];
        if !matches!(
            label,
            "PRIVATE KEY"
                | "RSA PRIVATE KEY"
                | "EC PRIVATE KEY"
                | "DSA PRIVATE KEY"
                | "OPENSSH PRIVATE KEY"
                | "ENCRYPTED PRIVATE KEY"
        ) {
            continue;
        }
        let tail = &text[label_start + end + 5..];
        let end_marker = format!("-----END {label}-----");
        let encoded_body = tail.find(&end_marker).map(|end| &tail[..end]);
        let first_body = tail
            .trim_start()
            .strip_prefix("\\n")
            .or_else(|| tail.trim_start().strip_prefix("\\r\\n"))
            .unwrap_or_else(|| tail.trim_start());
        let material = |body: &str| {
            body.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=')))
                .any(|run| run.len() >= 48)
        };
        if encoded_body.is_some_and(material)
            || first_body
                .bytes()
                .take_while(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
                })
                .count()
                >= 48
        {
            return true;
        }
    }
    false
}
fn rust_empty_credential_detector_literal(relative: &str, line: &str) -> bool {
    if !relative.ends_with(".rs") {
        return false;
    }
    let text = line.trim().trim_end_matches(',').trim();
    let Some(inner) = text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
    else {
        return false;
    };
    let Some(key) = inner.strip_suffix(':').or_else(|| inner.strip_suffix('=')) else {
        return false;
    };
    [
        "password",
        "passwd",
        "api_key",
        "access_token",
        "secret_key",
        "client_secret",
    ]
    .contains(&key.trim())
}
fn python_privacy_evidence_lines(relative: &str, text: &str, detectors: bool) -> BTreeSet<usize> {
    if !relative.ends_with(".py")
        || text.len() > 2 * 1024 * 1024
        || if detectors {
            !text.contains("compile") || !text.contains("PRIVATE KEY-----")
        } else {
            !text.contains("write_text") || !text.contains("test_") || !text.contains("assert")
        }
    {
        return BTreeSet::new();
    }
    static CACHE: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, BTreeSet<usize>>>> =
        std::sync::OnceLock::new();
    let fingerprint = format!(
        "{}:{}",
        if detectors { "detectors" } else { "fixtures" },
        sha(text.as_bytes())
    );
    if let Ok(cache) = CACHE.get_or_init(Default::default).lock() {
        if let Some(lines) = cache.get(&fingerprint) {
            return lines.clone();
        }
    }
    let result = (|| -> Option<BTreeSet<usize>> {
        use std::process::{Command, Stdio};
        let mut child = Command::new("/usr/bin/python3")
            .args([
                "-I",
                "-S",
                "-c",
                include_str!("python_fixture_ast.py"),
                if detectors { "detectors" } else { "fixtures" },
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(text.as_bytes()).ok()?;
        let output = child.wait_with_output().ok()?;
        if !output.status.success() || output.stdout.len() > 16384 {
            return None;
        }
        let lines: Vec<usize> = serde_json::from_slice(&output.stdout).ok()?;
        (lines.len() <= 1000
            && lines
                .iter()
                .all(|line| *line > 0 && *line <= text.lines().count()))
        .then(|| lines.into_iter().collect())
    })()
    .unwrap_or_default();
    if let Ok(mut cache) = CACHE.get_or_init(Default::default).lock() {
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(fingerprint, result.clone());
    }
    result
}

fn hold_reason(relative: &str, bytes: &[u8]) -> Option<&'static str> {
    if forbidden_path(relative) {
        return Some("excluded_path");
    }
    if bytes.contains(&0) {
        return Some("nul_requires_review");
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Some("non_utf8_requires_review");
    };
    match crate::vault::read_private_frontmatter(&mut Cursor::new(bytes)) {
        Ok(true) => return Some("private_metadata"),
        Err(_) => return Some("ambiguous_metadata"),
        Ok(false) => (),
    }
    let lower = text.to_ascii_lowercase();
    if private_key_material(text) {
        return Some("private_key_marker");
    }
    if lower.contains("\"role\":\"assistant\"")
        || lower.contains("\"role\": \"assistant\"")
        || lower.contains("\"role\":\"user\"")
        || lower.contains("<|im_start|>")
    {
        return Some("transcript_requires_review");
    }
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        if word
            .strip_prefix("sk-")
            .is_some_and(|v| v.len() >= 25 && v.bytes().all(|c| c.is_ascii_alphanumeric()))
            || ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"].iter().any(|p| {
                word.strip_prefix(p)
                    .is_some_and(|v| v.len() >= 25 && v.bytes().all(|c| c.is_ascii_alphanumeric()))
            })
        {
            return Some("credential_pattern_requires_review");
        }
    }
    if text.contains("PRIVATE KEY-----") {
        let detector_lines = python_privacy_evidence_lines(relative, text, true);
        if text.match_indices("PRIVATE KEY-----").any(|(offset, _)| {
            !detector_lines
                .contains(&(text[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1))
        }) {
            return Some("private_key_marker");
        }
    }
    for (line_start, line) in lower.split_inclusive('\n').scan(0usize, |offset, line| {
        let start = *offset;
        *offset += line.len();
        Some((start, line))
    }) {
        let Some((left, right)) = line.split_once('=').or_else(|| line.split_once(':')) else {
            continue;
        };
        let key = left.trim().trim_matches(|c: char| c == '\'' || c == '"');
        if [
            "password",
            "passwd",
            "api_key",
            "access_token",
            "secret_key",
            "client_secret",
        ]
        .contains(&key)
            && !right.trim().is_empty()
        {
            if rust_credential_type_declaration(relative, &lower[..line_start], line, right)
                || rust_empty_credential_detector_literal(relative, line)
            {
                continue;
            }
            let line_number = lower[..line_start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            if python_privacy_evidence_lines(relative, text, false).contains(&line_number) {
                continue;
            }
            return Some("credential_assignment_requires_review");
        }
    }
    None
}
fn read_bounded(mut file: File, limit: usize) -> VrgResult<Vec<u8>> {
    let before = file.metadata()?;
    if !before.is_file() || before.len() > limit as u64 {
        return Err(denied());
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if bytes.len() > limit
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || bytes.len() as u64 != after.len()
    {
        return Err(denied());
    }
    Ok(bytes)
}
fn read_exact(root: &File, relative: &str, limit: usize) -> VrgResult<Option<Vec<u8>>> {
    let (dir, name) = parent(root, relative)?;
    open_file(&dir, &name)?
        .map(|f| read_bounded(f, limit))
        .transpose()
}
fn read_state<T: for<'de> Deserialize<'de>>(work: &File, name: &str) -> VrgResult<Option<T>> {
    let Some(file) = open_file(work, std::ffi::OsStr::new(name))? else {
        return Ok(None);
    };
    check_private_file(&file)?;
    let bytes = read_bounded(file, MAX_STATE as usize)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| denied())
}
fn unique() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
fn create_private(dir: &File, name: &str, bytes: &[u8]) -> VrgResult<()> {
    let mut file = File::from(
        openat(
            dir,
            name,
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| denied())?,
    );
    check_private_file(&file)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn atomic_state<T: Serialize>(work: &File, name: &str, value: &T) -> VrgResult<()> {
    let temp = format!("{name}.{}", unique());
    create_private(work, &temp, &json(value)?)?;
    renameat(work, temp.as_str(), work, name).map_err(|_| denied())?;
    work.sync_all()?;
    Ok(())
}
fn append_event(work: &File, event: serde_json::Value) -> VrgResult<()> {
    let mut file = File::from(
        openat(
            work,
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
    check_private_file(&file)?;
    writeln!(file, "{event}")?;
    file.sync_all()?;
    Ok(())
}
fn initial_head(manifest: &UpdateManifest, pin: &ProjectionConfig) -> VrgResult<Head> {
    let mut outputs = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for source in &manifest.sources {
        let mut parts = Vec::new();
        for view in &source.initial_parts {
            outputs.insert(view.relative_path.clone(), view.sha256.clone());
            parts.push(Part {
                relative_path: view.relative_path.clone(),
                sha256: view.sha256.clone(),
                source_byte_range: None,
            });
        }
        sources.insert(
            source.source_id.clone(),
            SourceHead {
                observed_sha256: source.initial_sha256.clone(),
                state: "baseline".into(),
                read_status: if source.historical_read_evidence.is_some() {
                    if source.initial_read_scope == InitialReadScope::AdmittedViewOnly {
                        "historical_view_only".into()
                    } else {
                        "historical_evidence_unchanged".into()
                    }
                } else {
                    "pending_review".into()
                },
                parts,
                direct_document: None,
            },
        );
    }
    for note in &manifest.notes {
        outputs.insert(note.view.relative_path.clone(), note.view.sha256.clone());
    }
    for hub in &manifest.hubs {
        outputs.insert(hub.relative_path.clone(), hub.initial_sha256.clone());
    }
    for view in &manifest.passthrough_views {
        outputs.insert(view.relative_path.clone(), view.sha256.clone());
    }
    Ok(Head {
        schema: 2,
        authorization_sha256: pin.manifest_sha256.clone(),
        sources,
        retired_notes: BTreeSet::new(),
        outputs,
        indexed_generation: None,
        discovered_sources: BTreeMap::new(),
        source_outlines: BTreeMap::new(),
        module_documents: BTreeMap::new(),
        memory_documents: BTreeMap::new(),
        synthesis_version: 0,
    })
}
fn validate_head(head: &Head, baseline: &UpdateManifest, pin: &ProjectionConfig) -> VrgResult<()> {
    let manifest = effective_manifest(baseline, &head.discovered_sources)?;
    validate_head_effective(head, &manifest, pin)
}
fn validate_head_effective(
    head: &Head,
    manifest: &UpdateManifest,
    pin: &ProjectionConfig,
) -> VrgResult<()> {
    if head.schema != 2
        || head.authorization_sha256 != pin.manifest_sha256
        || head.sources.len() != manifest.sources.len()
    {
        return Err(denied());
    }
    let initial = initial_head(manifest, pin)?;
    let note_paths = manifest
        .notes
        .iter()
        .map(|n| n.view.relative_path.as_str())
        .collect::<BTreeSet<_>>();
    if head
        .retired_notes
        .iter()
        .any(|s| !note_paths.contains(s.as_str()))
    {
        return Err(denied());
    }
    let mut expected_paths = BTreeSet::new();
    for source in &manifest.sources {
        let state = head.sources.get(&source.source_id).ok_or_else(denied)?;
        if state
            .observed_sha256
            .as_ref()
            .is_some_and(|h| !valid_hash(h))
            || ![
                "historical_evidence_unchanged",
                "historical_view_only",
                "pending_review",
            ]
            .contains(&state.read_status.as_str())
        {
            return Err(denied());
        }
        if state.state != "baseline"
            && state.state != "mechanical_projected"
            && state.state != "missing"
            && !state.state.starts_with("held_")
        {
            return Err(denied());
        }
        if state.state == "baseline"
            && (state.observed_sha256 != source.initial_sha256
                || state.parts != initial.sources[&source.source_id].parts)
        {
            return Err(denied());
        }
        if state.state != "baseline" && state.read_status != "pending_review" {
            return Err(denied());
        }
        if state.state != "baseline"
            && state.state != "mechanical_projected"
            && !state.parts.is_empty()
        {
            return Err(denied());
        }
        if let Some(direct) = &state.direct_document {
            let expected = direct_path(source);
            if !manifest
                .discovery
                .as_ref()
                .is_some_and(|p| p.direct_sources && discovery_candidate(p, &source.relative_path))
                || state.state != "mechanical_projected"
                || !state.parts.is_empty()
                || direct.relative_path != expected
                || state.observed_sha256.as_ref() != Some(&direct.sha256)
                || !valid_hash(&direct.sha256)
                || !matches!(direct.source_byte_range, Some([0, end]) if end <= manifest.max_source_bytes)
            {
                return Err(denied());
            }
        }
        for part in &state.parts {
            safe_relative(&part.relative_path)?;
            if !valid_hash(&part.sha256)
                || head.outputs.get(&part.relative_path) != Some(&part.sha256)
            {
                return Err(denied());
            }
            if state.state != "baseline" {
                let hash = state.observed_sha256.as_ref().ok_or_else(denied)?;
                let [start, end] = part.source_byte_range.ok_or_else(denied)?;
                let expected = format!(
                    "{NAMESPACE}/s-{}-{}-{:012}.md",
                    source.source_id, hash, start
                );
                if part.relative_path != expected
                    || end < start
                    || end == start && (start != 0 || *hash != sha(b""))
                    || end - start > manifest.part_bytes
                {
                    return Err(denied());
                }
            }
            expected_paths.insert(part.relative_path.clone());
        }
    }
    for note in &manifest.notes {
        if !head.retired_notes.contains(&note.view.relative_path) {
            if head.outputs.get(&note.view.relative_path) != Some(&note.view.sha256) {
                return Err(denied());
            }
            expected_paths.insert(note.view.relative_path.clone());
        }
    }
    for hub in &manifest.hubs {
        let expected = if hub_dirty(hub, head) {
            sha(&render_hub(hub, head)?)
        } else {
            hub.initial_sha256.clone()
        };
        if head.outputs.get(&hub.relative_path) != Some(&expected) {
            return Err(denied());
        }
        expected_paths.insert(hub.relative_path.clone());
    }
    for view in &manifest.passthrough_views {
        if head.outputs.get(&view.relative_path) != Some(&view.sha256) {
            return Err(denied());
        }
        expected_paths.insert(view.relative_path.clone());
    }
    if head
        .source_outlines
        .keys()
        .any(|id| !head.sources.contains_key(id))
    {
        return Err(denied());
    }
    let documents = render_module_documents(manifest, head)?;
    if documents.len() != head.module_documents.len() {
        return Err(denied());
    }
    for (path, (metadata, bytes)) in documents {
        if head.module_documents.get(&path) != Some(&metadata)
            || head.outputs.get(&path) != Some(&sha(&bytes))
        {
            return Err(denied());
        }
        expected_paths.insert(path);
    }
    let memories = render_memory_documents(manifest, head)?;
    if memories.len() != head.memory_documents.len() {
        return Err(denied());
    }
    for (path, (metadata, bytes)) in memories {
        if head.memory_documents.get(&path) != Some(&metadata)
            || head.outputs.get(&path) != Some(&sha(&bytes))
        {
            return Err(denied());
        }
        expected_paths.insert(path);
    }
    if head.outputs.keys().cloned().collect::<BTreeSet<_>>() != expected_paths {
        return Err(denied());
    }
    Ok(())
}
fn check_outputs(corpus: &File, head: &Head) -> VrgResult<()> {
    for (path, expected) in &head.outputs {
        let bytes = read_exact(corpus, path, MAX_VIEW.max(4 * 1024 * 1024))?.ok_or_else(denied)?;
        if sha(&bytes) != *expected {
            return Err(denied());
        }
    }
    Ok(())
}
fn hub_dirty(hub: &Hub, head: &Head) -> bool {
    hub.source_ids
        .iter()
        .any(|id| head.sources.get(id).is_some_and(|s| s.state != "baseline"))
        || hub
            .note_paths
            .iter()
            .any(|s| head.retired_notes.contains(s))
}
fn relative_link(from: &str, to: &str) -> VrgResult<String> {
    safe_relative(from)?;
    safe_relative(to)?;
    let a = Path::new(from)
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .map(|p| p.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let b = Path::new(to)
        .components()
        .map(|p| p.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut out = vec!["..".to_string(); a.len() - common];
    out.extend(b[common..].iter().cloned());
    // Angle-bracket Markdown destinations handle spaces and parentheses safely.
    Ok(out.join("/").replace('<', "\\<").replace('>', "\\>"))
}
fn escaped_markdown(text: &str) -> String {
    let mut result = String::new();
    for ch in text.chars() {
        if "\\[]()*_!#`~>|".contains(ch) {
            result.push('\\');
        }
        result.push(ch);
    }
    result
}
fn inline_code(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    format!("{fence} {text} {fence}")
}
fn render_hub(hub: &Hub, head: &Head) -> VrgResult<Vec<u8>> {
    let title = serde_json::to_string(&hub.title).map_err(|_| denied())?;
    let aliases = serde_json::to_string(&hub.aliases).map_err(|_| denied())?;
    let mut text = format!(
        "---\ntitle: {title}\naliases: {aliases}\n---\n# {}\n\nDerived provenance hub; source updates are mechanical and require real review.\n\n",
        escaped_markdown(&hub.title)
    );
    let mut linked = BTreeSet::new();
    let mut pending = 0usize;
    for id in &hub.source_ids {
        let source = head.sources.get(id).ok_or_else(denied)?;
        if source.read_status == "pending_review" {
            pending += 1;
        }
        for part in source.parts.iter().chain(source.direct_document.iter()) {
            if linked.insert(&part.relative_path) {
                text.push_str(&format!(
                    "- [Source view](<{}>)\n",
                    relative_link(&hub.relative_path, &part.relative_path)?
                ));
            }
        }
    }
    for path in &hub.note_paths {
        if !head.retired_notes.contains(path) {
            text.push_str(&format!(
                "- [Reviewed note view](<{}>)\n",
                relative_link(&hub.relative_path, path)?
            ));
        }
    }
    text.push_str(&format!("\nPending source reviews: {pending}. Dependent synthesis remains withdrawn until coordinated readmission.\n"));
    if text.len() > 4 * 1024 * 1024 {
        return Err(denied());
    }
    Ok(text.into_bytes())
}
fn render_parts(
    source: &KnownSource,
    bytes: &[u8],
    part_bytes: usize,
) -> VrgResult<(Vec<Part>, BTreeMap<String, Vec<u8>>)> {
    let text = std::str::from_utf8(bytes).map_err(|_| denied())?;
    let hash = sha(bytes);
    let mut parts = Vec::new();
    let mut rendered = BTreeMap::new();
    let mut start = 0;
    while start < bytes.len() || bytes.is_empty() && parts.is_empty() {
        let mut end = (start + part_bytes).min(bytes.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        loop {
            if end < start || end == start && !bytes.is_empty() {
                return Err(denied());
            }
            let body = &text[start..end];
            let mut run = 0;
            let mut longest = 0;
            for ch in body.chars() {
                if ch == '`' {
                    run += 1;
                    longest = longest.max(run);
                } else {
                    run = 0;
                }
            }
            let fence = "`".repeat(longest.max(2) + 1);
            let title = serde_json::to_string(&source.relative_path).map_err(|_| denied())?;
            let view=format!("---\ntitle: {title}\n---\nOrigin: {}\nSHA256: {hash}\nUTF-8 byte range [inclusive,exclusive): {start},{end}\nTransformer: {POLICY}; derived source view.\nread_status: pending_review\nMechanical projection; no AI reading or synthesis credit.\n\n{fence}\n{body}\n{fence}\n",inline_code(&source.original_root.join(&source.relative_path).to_string_lossy())).into_bytes();
            if view.len() > MAX_VIEW {
                end = start + (end - start) / 2;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                continue;
            }
            let relative_path = format!("{NAMESPACE}/s-{}-{hash}-{start:012}.md", source.source_id);
            let part = Part {
                relative_path: relative_path.clone(),
                sha256: sha(&view),
                source_byte_range: Some([start, end]),
            };
            rendered.insert(relative_path, view);
            parts.push(part);
            break;
        }
        start = end;
        if bytes.is_empty() {
            break;
        }
    }
    Ok((parts, rendered))
}
enum Observation {
    Bytes(Vec<u8>),
    Held(&'static str),
    Missing,
}
fn observe(root: &File, relative: &str, limit: usize) -> Observation {
    let Ok((directory, name)) = parent(root, relative) else {
        return Observation::Held("path_unavailable");
    };
    let file = match open_file(&directory, &name) {
        Ok(Some(f)) => f,
        Ok(None) => return Observation::Missing,
        Err(_) => return Observation::Held("path_unavailable"),
    };
    let mut file = file;
    match crate::vault::read_private_frontmatter(&mut file) {
        Ok(true) => return Observation::Held("private_metadata"),
        Err(_) => return Observation::Held("ambiguous_metadata"),
        Ok(false) => (),
    }
    if file.seek(SeekFrom::Start(0)).is_err() {
        return Observation::Held("unreadable");
    }
    match read_bounded(file, limit) {
        Ok(bytes) => Observation::Bytes(bytes),
        Err(_) => Observation::Held("unreadable_or_unstable"),
    }
}
fn probe_states(
    manifest: &UpdateManifest,
    roots: &BTreeMap<PathBuf, File>,
    changes: Option<&BTreeSet<PathBuf>>,
) -> BTreeMap<PathBuf, bool> {
    manifest
        .policy_probes
        .iter()
        .filter(|probe| {
            changes.is_none_or(|changes| {
                changes.iter().any(|changed| {
                    probe
                        .root
                        .join(&probe.relative_path)
                        .parent()
                        .is_some_and(|parent| changed.starts_with(parent))
                })
            })
        })
        .map(|probe| {
            let valid = roots.get(&probe.root).is_some_and(|root| {
                match read_exact(root, &probe.relative_path, 65536) {
                    Ok(Some(bytes)) => probe.expected_sha256.as_ref() == Some(&sha(&bytes)),
                    Ok(None) => probe.expected_sha256.is_none(),
                    Err(_) => false,
                }
            });
            (probe.root.join(&probe.relative_path), valid)
        })
        .collect()
}
fn probes_current(source: &KnownSource, states: &BTreeMap<PathBuf, bool>) -> bool {
    if source.kind == SourceKind::FilteredSnapshot {
        return true;
    }
    let full = source.original_root.join(&source.relative_path);
    let mut directory = full.parent();
    while let Some(path) = directory {
        if states.get(&path.join(".codexignore")) == Some(&false) {
            return false;
        }
        directory = path.parent();
    }
    true
}

fn desired(
    manifest: &UpdateManifest,
    old: &Head,
    roots: &BTreeMap<PathBuf, File>,
    note_roots: &BTreeMap<PathBuf, File>,
    changes: Option<&BTreeSet<PathBuf>>,
) -> VrgResult<(Head, BTreeMap<String, Vec<u8>>)> {
    let mut next = old.clone();
    let policies = probe_states(manifest, roots, changes);
    let mut bytes_by_path = BTreeMap::new();
    for source in &manifest.sources {
        let previous = old.sources.get(&source.source_id).ok_or_else(denied)?;
        let full = source.original_root.join(&source.relative_path);
        if changes.is_some_and(|changes| !path_affected(&full, changes)) {
            continue;
        }
        crate::indexer::record_source_read(&full);
        let observation = if probes_current(source, &policies) {
            observe(
                roots.get(&source.original_root).ok_or_else(denied)?,
                &source.relative_path,
                manifest.max_source_bytes,
            )
        } else {
            Observation::Held("policy_changed")
        };
        let current = match observation {
            Observation::Bytes(bytes) => {
                let hash = sha(&bytes);
                let eligible = manifest.discovery.as_ref().is_some_and(|policy| {
                    eligible_code_bytes(policy, &source.relative_path, &bytes)
                });
                if !eligible {
                    next.source_outlines.remove(&source.source_id);
                }
                if manifest
                    .discovery
                    .as_ref()
                    .is_some_and(|p| p.module_docs && eligible)
                    && (previous.observed_sha256.as_ref() != Some(&hash)
                        || next
                            .source_outlines
                            .get(&source.source_id)
                            .is_none_or(|outline| outline.facts.behavioral_sha256.is_empty()))
                    && hold_reason(&source.relative_path, &bytes).is_none()
                {
                    next.source_outlines.insert(
                        source.source_id.clone(),
                        outline(&bytes, &source.relative_path),
                    );
                }
                if manifest
                    .discovery
                    .as_ref()
                    .is_some_and(|p| p.direct_sources && eligible)
                {
                    if let Some(reason) = hold_reason(&source.relative_path, &bytes) {
                        SourceHead {
                            observed_sha256: None,
                            state: format!("held_{reason}"),
                            read_status: "pending_review".into(),
                            parts: vec![],
                            direct_document: None,
                        }
                    } else {
                        SourceHead {
                            observed_sha256: Some(hash.clone()),
                            state: "mechanical_projected".into(),
                            read_status: "pending_review".into(),
                            parts: vec![],
                            direct_document: Some(Part {
                                relative_path: direct_path(source),
                                sha256: hash,
                                source_byte_range: Some([0, bytes.len()]),
                            }),
                        }
                    }
                } else if previous.state == "baseline"
                    && source.initial_sha256.as_ref() == Some(&hash)
                    && !previous.parts.is_empty()
                {
                    // A pinned sanitized baseline can have different raw and view
                    // hashes. Retain only its approved views and declared read scope.
                    previous.clone()
                } else if previous.state == "mechanical_projected"
                    && previous.direct_document.is_none()
                    && previous.observed_sha256.as_ref() == Some(&hash)
                {
                    // Same admitted bytes and fixed transformer/policy: reuse only
                    // the exact SHA-checked owned output objects, without rendering.
                    previous.clone()
                } else if let Some(reason) = hold_reason(&source.relative_path, &bytes) {
                    SourceHead {
                        observed_sha256: None,
                        state: format!("held_{reason}"),
                        read_status: "pending_review".into(),
                        parts: vec![],
                        direct_document: None,
                    }
                } else {
                    let (parts, rendered) = render_parts(source, &bytes, manifest.part_bytes)?;
                    bytes_by_path.extend(rendered);
                    SourceHead {
                        observed_sha256: Some(hash),
                        state: "mechanical_projected".into(),
                        read_status: "pending_review".into(),
                        parts,
                        direct_document: None,
                    }
                }
            }
            Observation::Missing => SourceHead {
                observed_sha256: None,
                state: "missing".into(),
                read_status: "pending_review".into(),
                parts: vec![],
                direct_document: None,
            },
            Observation::Held(reason) => SourceHead {
                observed_sha256: None,
                state: format!("held_{reason}"),
                read_status: "pending_review".into(),
                parts: vec![],
                direct_document: None,
            },
        };
        if current != *previous {
            next.retired_notes
                .extend(source.dependent_notes.iter().cloned());
        }
        next.sources.insert(source.source_id.clone(), current);
    }
    // Outlines never outlive the active source revision.
    next.source_outlines.retain(|id, _| {
        next.sources
            .get(id)
            .is_some_and(|s| s.state == "baseline" || s.state == "mechanical_projected")
    });
    if !next.source_outlines.is_empty() {
        next.synthesis_version = 1;
    }
    // Canonical-note origins have independent frozen revisions; never auto-project
    // their text and never resurrect an already retired view after original reversion.
    for note in &manifest.notes {
        if next.retired_notes.contains(&note.view.relative_path) {
            continue;
        }
        if changes.is_some_and(|changes| {
            !path_affected(&note.original_root.join(&note.relative_path), changes)
                && !note.source_dependencies.iter().any(|dependency| {
                    path_affected(
                        &dependency.original_root.join(&dependency.relative_path),
                        changes,
                    )
                })
        }) {
            continue;
        }
        crate::indexer::record_source_read(&note.original_root.join(&note.relative_path));
        let unchanged = match observe(
            note_roots.get(&note.original_root).ok_or_else(denied)?,
            &note.relative_path,
            manifest.max_source_bytes,
        ) {
            Observation::Bytes(bytes) => sha(&bytes) == note.expected_sha256,
            _ => false,
        };
        let dependencies_current = note.source_dependencies.iter().all(|dependency| {
            manifest
                .sources
                .iter()
                .find(|source| {
                    source.original_root == dependency.original_root
                        && source.relative_path == dependency.relative_path
                })
                .and_then(|source| next.sources.get(&source.source_id))
                .is_some_and(|state| {
                    (state.state == "baseline" || state.state == "mechanical_projected")
                        && state.observed_sha256.as_ref() == Some(&dependency.expected_sha256)
                })
        });
        if !unchanged || !dependencies_current {
            next.retired_notes.insert(note.view.relative_path.clone());
        }
    }
    let mut outputs = BTreeMap::new();
    for source in next.sources.values() {
        for part in &source.parts {
            if outputs
                .insert(part.relative_path.clone(), part.sha256.clone())
                .is_some_and(|old| old != part.sha256)
            {
                return Err(denied());
            }
        }
    }
    for note in &manifest.notes {
        if !next.retired_notes.contains(&note.view.relative_path) {
            outputs.insert(note.view.relative_path.clone(), note.view.sha256.clone());
        }
    }
    for view in &manifest.passthrough_views {
        outputs.insert(view.relative_path.clone(), view.sha256.clone());
    }
    next.outputs = outputs;
    for hub in &manifest.hubs {
        let (hash, rendered) = if hub_dirty(hub, &next) {
            let bytes = render_hub(hub, &next)?;
            (sha(&bytes), Some(bytes))
        } else {
            (hub.initial_sha256.clone(), None)
        };
        next.outputs.insert(hub.relative_path.clone(), hash);
        if let Some(bytes) = rendered {
            bytes_by_path.insert(hub.relative_path.clone(), bytes);
        }
    }
    next.module_documents.clear();
    for (path, (metadata, bytes)) in render_module_documents(manifest, &next)? {
        next.outputs.insert(path.clone(), metadata.sha256.clone());
        next.module_documents.insert(path.clone(), metadata);
        if old.outputs.get(&path) != next.outputs.get(&path) {
            bytes_by_path.insert(path, bytes);
        }
    }
    let memories = render_memory_documents(manifest, &next)?;
    next.memory_documents.clear();
    for (path, (metadata, bytes)) in memories {
        next.outputs.insert(path.clone(), metadata.sha256.clone());
        next.memory_documents.insert(path.clone(), metadata);
        if old.outputs.get(&path) != next.outputs.get(&path) {
            bytes_by_path.insert(path, bytes);
        }
    }
    if next.sources != old.sources
        || next.discovered_sources != old.discovered_sources
        || next.source_outlines != old.source_outlines
        || next.retired_notes != old.retired_notes
        || next.outputs != old.outputs
    {
        next.indexed_generation = None;
    }
    Ok((next, bytes_by_path))
}
fn walk_namespace(
    directory: &File,
    prefix: &str,
    expected: &BTreeMap<String, String>,
    seen: &mut BTreeSet<String>,
) -> VrgResult<()> {
    let dir = cap_std::fs::Dir::from_std_file(directory.try_clone()?);
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(denied)?;
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        safe_relative(&path)?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(denied());
        }
        if kind.is_dir() {
            let child = File::from(
                openat(
                    directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| denied())?,
            );
            walk_namespace(&child, &path, expected, seen)?;
        } else if kind.is_file() && expected.contains_key(&path) {
            seen.insert(path);
        } else {
            return Err(denied());
        }
    }
    Ok(())
}
fn check_namespace(corpus: &File, head: &Head) -> VrgResult<()> {
    let mut seen = BTreeSet::new();
    walk_namespace(corpus, "", &head.outputs, &mut seen)?;
    if seen != head.outputs.keys().cloned().collect::<BTreeSet<_>>() {
        return Err(denied());
    }
    Ok(())
}
fn file_hash(root: &File, path: &str) -> VrgResult<Option<String>> {
    Ok(read_exact(root, path, 4 * 1024 * 1024)?.map(|bytes| sha(&bytes)))
}
fn open_work(config: &VrgConfig) -> VrgResult<(File, File)> {
    let data = canonical_directory(&config.data_dir)?;
    let work = private_dir(&data, "projection-updates-v2")?;
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
    Ok((work, lock))
}
fn checked_transaction(
    transaction: &Transaction,
    current: Option<&Head>,
    manifest: &UpdateManifest,
    pin: &ProjectionConfig,
) -> VrgResult<()> {
    if transaction.authorization_sha256 != pin.manifest_sha256 {
        return Err(denied());
    }
    if current != Some(&transaction.next_head) {
        validate_head(&transaction.next_head, manifest, pin)?;
    }
    let initial = initial_head(manifest, pin)?;
    if current != Some(&transaction.previous_head) {
        validate_head(&transaction.previous_head, manifest, pin)?;
    }
    let old = &transaction.previous_head;
    let previous_hash = sha(&json(old)?);
    if transaction
        .prior_head_sha256
        .as_ref()
        .is_some_and(|h| h != &previous_hash)
    {
        return Err(denied());
    }
    match current {
        Some(actual) if actual != old && actual != &transaction.next_head => return Err(denied()),
        None if old != &initial || transaction.prior_head_sha256.is_some() => return Err(denied()),
        _ => (),
    }
    let old_hash = current.map(json).transpose()?.map(|bytes| sha(&bytes));
    if old_hash != transaction.prior_head_sha256 && current != Some(&transaction.next_head) {
        return Err(denied());
    }
    let mut seen = BTreeSet::new();
    for op in &transaction.operations {
        safe_relative(&op.path)?;
        safe_relative(&op.backup)?;
        if op.backup.contains('/')
            || !seen.insert(&op.path)
            || op.old_sha256.as_ref().is_some_and(|h| !valid_hash(h))
            || op.new_sha256.as_ref().is_some_and(|h| !valid_hash(h))
            || op.old_sha256 == op.new_sha256
            || op.new_sha256 != transaction.next_head.outputs.get(&op.path).cloned()
        {
            return Err(denied());
        }
        if op.old_sha256 != old.outputs.get(&op.path).cloned() {
            return Err(denied());
        }
        // Every old target must be authorized by immutable baseline or this exact
        // validated head; every new target belongs to next_head's deterministic set.
        if op.old_sha256.is_some()
            && !old.outputs.contains_key(&op.path)
            && !initial.outputs.contains_key(&op.path)
        {
            return Err(denied());
        }
        match (&op.new_sha256, &op.stage) {
            (Some(_), Some(name)) => {
                safe_relative(name)?;
                if name.contains('/') {
                    return Err(denied());
                }
            }
            (None, None) => (),
            _ => return Err(denied()),
        }
    }
    let expected = old
        .outputs
        .keys()
        .chain(transaction.next_head.outputs.keys())
        .filter(|p| old.outputs.get(*p) != transaction.next_head.outputs.get(*p))
        .collect::<BTreeSet<_>>();
    if seen != expected {
        return Err(denied());
    }
    Ok(())
}
fn apply_transaction(
    work: &File,
    corpus: &File,
    manifest: &UpdateManifest,
    pin: &ProjectionConfig,
    transaction: &Transaction,
) -> VrgResult<()> {
    let current = read_cached_head(work, manifest, pin)?.map(|head| (*head).clone());
    checked_transaction(transaction, current.as_ref(), manifest, pin)?;
    let backup = private_dir(work, "backup")?;
    let stage = private_dir(work, "stage")?;
    // Validate *all* extant paths before the first move, including recovery.
    let mut permitted = transaction.previous_head.outputs.clone();
    permitted.extend(transaction.next_head.outputs.clone());
    if !transaction.incremental {
        walk_namespace(corpus, "", &permitted, &mut BTreeSet::new())?;
    }
    for (path, hash) in &transaction.previous_head.outputs {
        if !transaction.incremental
            && transaction.next_head.outputs.get(path) == Some(hash)
            && file_hash(corpus, path)?.as_ref() != Some(hash)
        {
            return Err(denied());
        }
    }
    for op in &transaction.operations {
        let now = file_hash(corpus, &op.path)?;
        let saved = open_file(&backup, std::ffi::OsStr::new(&op.backup))?
            .map(digest)
            .transpose()?;
        if saved
            .as_ref()
            .is_some_and(|h| Some(h) != op.old_sha256.as_ref())
        {
            return Err(denied());
        }
        let original_present = now == op.old_sha256 && now.is_some();
        let new_present = now == op.new_sha256 && now.is_some();
        if !original_present && !new_present && now.is_some() {
            return Err(denied());
        }
        if op.old_sha256.is_some() && !original_present && saved != op.old_sha256 {
            return Err(denied());
        }
        if original_present && saved.is_some() {
            return Err(denied());
        }
        if let Some(name) = &op.stage {
            let staged = open_file(&stage, std::ffi::OsStr::new(name))?
                .map(|f| {
                    check_private_file(&f)?;
                    digest(f)
                })
                .transpose()?;
            if (!new_present && staged != op.new_sha256)
                || staged
                    .as_ref()
                    .is_some_and(|h| Some(h) != op.new_sha256.as_ref())
            {
                return Err(denied());
            }
        }
    }
    for op in &transaction.operations {
        let (parent_dir, name) = parent(corpus, &op.path)?;
        let now = file_hash(corpus, &op.path)?;
        if now == op.old_sha256 && op.old_sha256.is_some() {
            renameat_with(
                &parent_dir,
                &name,
                &backup,
                op.backup.as_str(),
                RenameFlags::NOREPLACE,
            )
            .map_err(|_| denied())?;
            parent_dir.sync_all()?;
            backup.sync_all()?;
            if open_file(&backup, std::ffi::OsStr::new(&op.backup))?
                .map(digest)
                .transpose()?
                != op.old_sha256
            {
                return Err(denied());
            }
        }
        if let (Some(hash), Some(staged)) = (&op.new_sha256, &op.stage) {
            if file_hash(corpus, &op.path)?.as_ref() != Some(hash) {
                renameat_with(
                    &stage,
                    staged.as_str(),
                    &parent_dir,
                    &name,
                    RenameFlags::NOREPLACE,
                )
                .map_err(|_| denied())?;
                parent_dir.sync_all()?;
                stage.sync_all()?;
                if file_hash(corpus, &op.path)?.as_ref() != Some(hash) {
                    return Err(denied());
                }
            }
        }
    }
    if !transaction.incremental {
        check_outputs(corpus, &transaction.next_head)?;
        check_namespace(corpus, &transaction.next_head)?;
    }
    atomic_state(work, "HEAD.json", &transaction.next_head)?;
    cache_committed_head(work, manifest, pin, &transaction.next_head)?;
    append_event(
        work,
        serde_json::json!({"event":"projection_committed","transaction_sha256":sha(&json(transaction)?),"authorization_sha256":pin.manifest_sha256,"head_sha256":sha(&json(&transaction.next_head)?),"mechanical_indexed":transaction.next_head.indexed_generation.is_some()}),
    )?;
    let committed = private_dir(work, "transactions")?;
    let name = format!("{}.json", sha(&json(transaction)?));
    renameat_with(
        work,
        "TXN.json",
        &committed,
        name.as_str(),
        RenameFlags::NOREPLACE,
    )
    .map_err(|_| denied())?;
    committed.sync_all()?;
    work.sync_all()?;
    Ok(())
}
fn recover(
    work: &File,
    corpus: &File,
    manifest: &UpdateManifest,
    pin: &ProjectionConfig,
) -> VrgResult<bool> {
    if let Some(transaction) = read_state::<Transaction>(work, "TXN.json")? {
        apply_transaction(work, corpus, manifest, pin, &transaction)?;
        Ok(true)
    } else {
        Ok(false)
    }
}
fn publish(
    work: &File,
    corpus: &File,
    manifest: &UpdateManifest,
    pin: &ProjectionConfig,
    old: Option<&Head>,
    next: Head,
    rendered: BTreeMap<String, Vec<u8>>,
    incremental: bool,
) -> VrgResult<bool> {
    let baseline = if old.is_none() {
        Some(initial_head(manifest, pin)?)
    } else {
        None
    };
    let previous = old.or(baseline.as_ref()).ok_or_else(denied)?;
    if next == *previous && old.is_some() {
        return Ok(false);
    }
    if next == *previous && old.is_none() {
        // One explicit installation checkpoint. Subsequent reads do zero writes.
        atomic_state(work, "HEAD.json", &next)?;
        cache_committed_head(work, manifest, pin, &next)?;
        append_event(
            work,
            serde_json::json!({"event":"projection_initialized","head_sha256":sha(&json(&next)?),"authorization_sha256":pin.manifest_sha256}),
        )?;
        return Ok(false);
    }
    let stage = private_dir(work, "stage")?;
    private_dir(work, "backup")?;
    let paths = previous
        .outputs
        .keys()
        .chain(next.outputs.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let id = unique();
    let mut operations = Vec::new();
    for path in paths {
        let before = previous.outputs.get(&path).cloned();
        let after = next.outputs.get(&path).cloned();
        if before == after {
            continue;
        }
        let number = operations.len();
        let staged = if let Some(hash) = &after {
            let bytes = rendered.get(&path).ok_or_else(denied)?;
            if sha(bytes) != *hash {
                return Err(denied());
            }
            let name = format!("{id}-{number}.md");
            create_private(&stage, &name, bytes)?;
            Some(name)
        } else {
            None
        };
        operations.push(Operation {
            path,
            old_sha256: before,
            new_sha256: after,
            stage: staged,
            backup: format!("{id}-{number}.md"),
        });
    }
    stage.sync_all()?;
    let transaction = Transaction {
        authorization_sha256: pin.manifest_sha256.clone(),
        prior_head_sha256: old.map(json).transpose()?.map(|b| sha(&b)),
        previous_head: previous.clone(),
        next_head: next,
        operations,
        incremental,
    };
    atomic_state(work, "TXN.json", &transaction)?;
    append_event(
        work,
        serde_json::json!({"event":"projection_prepared","transaction_sha256":sha(&json(&transaction)?),"pending_review":transaction.next_head.sources.iter().filter(|(_,s)|s.read_status=="pending_review").map(|(id,_)|id).collect::<Vec<_>>(),"retired_note_views":transaction.next_head.retired_notes}),
    )?;
    apply_transaction(work, corpus, manifest, pin, &transaction)?;
    Ok(true)
}
/// Main worker/indexer guard. A held source retires views, without emitting its
/// values. Incomplete transactions recover before observation or fail closed.
pub(super) fn reconcile(
    config: &VrgConfig,
    pin: &ProjectionConfig,
    bytes: &[u8],
) -> VrgResult<bool> {
    let manifest: UpdateManifest = serde_json::from_slice(bytes).map_err(|_| denied())?;
    validate(&manifest, config)?;
    reconcile_manifest(config, pin, std::sync::Arc::new(manifest), None)
}
fn reconcile_manifest(
    config: &VrgConfig,
    pin: &ProjectionConfig,
    manifest: std::sync::Arc<UpdateManifest>,
    changes: Option<&BTreeSet<PathBuf>>,
) -> VrgResult<bool> {
    // Caller supplied either a freshly validated immutable policy or a stamp-checked Arc cache hit.
    let (work, _lock) = open_work(config)?;
    let corpus = canonical_directory(&manifest.corpus_path)?;
    // The only generated corpus directory. mkdir on an existing directory is a no-op.
    let namespace = private_dir(&corpus, NAMESPACE)?;
    private_dir(&namespace, "modules")?;
    private_dir(&namespace, "memory")?;
    let recovered = recover(&work, &corpus, &manifest, pin)?;
    let saved = read_cached_head(&work, &manifest, pin)?.map(|head| (*head).clone());
    let baseline = if saved.is_none() {
        Some(initial_head(&manifest, pin)?)
    } else {
        None
    };
    let old = saved.as_ref().or(baseline.as_ref()).ok_or_else(denied)?;
    if saved.is_none() {
        validate_head(old, &manifest, pin)?;
    }
    let incremental = changes.is_some() && saved.is_some();
    if !incremental {
        check_outputs(&corpus, old)?;
        check_namespace(&corpus, old)?;
    } else if let Some(changes) = changes {
        for changed in changes {
            if let Ok(relative) = changed.strip_prefix(&manifest.corpus_path) {
                let path = relative.to_str().ok_or_else(denied)?;
                if path.is_empty() || changed.is_dir() {
                    continue;
                }
                if old.outputs.get(path) != file_hash(&corpus, path)?.as_ref() {
                    return Err(denied());
                }
            }
        }
    }
    let roots = manifest
        .original_roots
        .iter()
        .map(|p| Ok((p.clone(), canonical_directory(p)?)))
        .collect::<VrgResult<BTreeMap<_, _>>>()?;
    let notes = manifest
        .note_roots
        .iter()
        .map(|p| Ok((p.clone(), canonical_directory(p)?)))
        .collect::<VrgResult<BTreeMap<_, _>>>()?;
    let mut admitted = old.clone();
    if let Some(changes) = changes.filter(|_| incremental) {
        discover_affected(&manifest, &roots, &mut admitted, changes)?;
    } else {
        discover_sources(&manifest, &roots, &mut admitted)?;
    }
    let effective = effective_manifest(&manifest, &admitted.discovered_sources)?;
    let (next, rendered) = desired(
        &effective,
        &admitted,
        &roots,
        &notes,
        changes.filter(|_| incremental),
    )?;
    let changed = publish(
        &work,
        &corpus,
        &manifest,
        pin,
        saved.as_ref(),
        next,
        rendered,
        incremental,
    )?;
    Ok(changed || recovered)
}
fn load_manifest(
    config: &VrgConfig,
) -> VrgResult<Option<(std::sync::Arc<UpdateManifest>, ProjectionConfig)>> {
    let Some(pin) = &config.projection_invalidation else {
        return Ok(None);
    };
    let dir = open_dir(pin.manifest_path.parent().ok_or_else(denied)?)?;
    let file =
        open_file(&dir, pin.manifest_path.file_name().ok_or_else(denied)?)?.ok_or_else(denied)?;
    check_private_file(&file)?;
    let stamp = file_stamp(&file)?;
    let key = (
        pin.manifest_path.clone(),
        pin.manifest_sha256.clone(),
        config.data_dir.clone(),
    );
    if let Some((cached_stamp, manifest)) = MANIFEST_CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| denied())?
        .get(&key)
    {
        if *cached_stamp == stamp {
            if manifest.data_dir != config.data_dir
                || config.roots.len() != 1
                || config.roots[0].id != manifest.corpus_root_id
                || config.roots[0].path != manifest.corpus_path
            {
                return Err(denied());
            }
            crate::indexer::record_metadata_read(true, true);
            return Ok(Some((std::sync::Arc::clone(manifest), pin.clone())));
        }
    }
    crate::indexer::record_metadata_read(true, false);
    let bytes = read_bounded(file, 64 * 1024 * 1024)?;
    if sha(&bytes) != pin.manifest_sha256 {
        return Err(denied());
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| denied())?;
    if value.get("schema").and_then(serde_json::Value::as_u64) != Some(2) {
        return Ok(None);
    }
    let manifest: UpdateManifest = serde_json::from_value(value).map_err(|_| denied())?;
    validate(&manifest, config)?;
    let manifest = std::sync::Arc::new(manifest);
    let mut cache = MANIFEST_CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| denied())?;
    if cache.len() >= 32 && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(key, (stamp, std::sync::Arc::clone(&manifest)));
    Ok(Some((manifest, pin.clone())))
}
/// Capture after reconcile, before scanning. Other publishers cannot silently
/// mark a newer head indexed by this scan's generation.
pub(crate) fn head_token(config: &VrgConfig) -> VrgResult<Option<String>> {
    let Some((manifest, pin)) = load_manifest(config)? else {
        return Ok(None);
    };
    let (work, _lock) = open_work(config)?;
    if read_state::<Transaction>(&work, "TXN.json")?.is_some() {
        return Err(denied());
    }
    let head = read_cached_head(&work, &manifest, &pin)?.ok_or_else(denied)?;
    Ok(Some(sha(&json(&*head)?)))
}
/// Reject any unapproved Markdown observed between initial preflight and scan,
/// before chunking, embedding or generation publication.
pub(crate) fn authorize_scan(
    config: &VrgConfig,
    documents: &[crate::contracts::SourceDocument],
    token: Option<&str>,
) -> VrgResult<()> {
    let Some((manifest, pin)) = load_manifest(config)? else {
        return Ok(());
    };
    let (work, _lock) = open_work(config)?;
    if read_state::<Transaction>(&work, "TXN.json")?.is_some() {
        return Err(denied());
    }
    let head = read_cached_head(&work, &manifest, &pin)?.ok_or_else(denied)?;
    if token != Some(sha(&json(&*head)?).as_str()) {
        return Err(denied());
    }
    let authorized = authorized_documents(&head);
    let mut seen = BTreeSet::new();
    for doc in documents {
        if doc.source.root_id != manifest.corpus_root_id
            || authorized.get(&doc.source.relative_path) != Some(&doc.revision)
            || !seen.insert(doc.source.relative_path.clone())
        {
            return Err(denied());
        }
    }
    if seen != authorized.keys().cloned().collect::<BTreeSet<_>>() {
        return Err(denied());
    }
    Ok(())
}
pub(crate) fn mark_indexed(
    config: &VrgConfig,
    token: Option<&str>,
    generation: Option<&str>,
) -> VrgResult<()> {
    let Some((manifest, pin)) = load_manifest(config)? else {
        return Ok(());
    };
    let (work, _lock) = open_work(config)?;
    if read_state::<Transaction>(&work, "TXN.json")?.is_some() {
        return Err(denied());
    }
    let head = read_cached_head(&work, &manifest, &pin)?.ok_or_else(denied)?;
    let actual = sha(&json(&*head)?);
    if token != Some(actual.as_str()) {
        return Err(denied());
    }
    if head.indexed_generation.as_deref() == generation {
        return Ok(());
    }
    let mut next = (*head).clone();
    next.indexed_generation = generation.map(str::to_owned);
    let corpus = canonical_directory(&manifest.corpus_path)?;
    publish(
        &work,
        &corpus,
        &manifest,
        &pin,
        Some(&head),
        next,
        BTreeMap::new(),
        true,
    )?;
    Ok(())
}
/// Optional operational facts, separate from the historical reading ledger.
/// reconcile initializes HEAD; this accessor itself writes nothing.
pub fn pending_review(config: &VrgConfig) -> VrgResult<Option<serde_json::Value>> {
    let Some((manifest, pin)) = load_manifest(config)? else {
        return Ok(None);
    };
    let root = open_dir(&config.data_dir)?;
    let work_fd = match rustix::fs::openat(
        &root,
        "projection-updates-v2",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => {
            return Ok(Some(serde_json::json!({"state":"not_initialized"})));
        }
        Err(_) => return Err(denied()),
    };
    let work = File::from(work_fd);
    if read_state::<Transaction>(&work, "TXN.json")?.is_some() {
        return Err(denied());
    }
    let Some(head) = read_cached_head(&work, &manifest, &pin)? else {
        return Ok(Some(serde_json::json!({"state":"not_initialized"})));
    };
    let effective = effective_manifest(&manifest, &head.discovered_sources)?;
    let sources=effective.sources.iter().filter_map(|source| {
        let state=head.sources.get(&source.source_id)?;
        (state.read_status=="pending_review").then(||serde_json::json!({"source_id":source.source_id,"original_root":source.original_root,"relative_path":source.relative_path,"observed_sha256":state.observed_sha256,"projection_state":state.state,"read_status":"pending_review","mechanical_indexed":state.state=="mechanical_projected" && head.indexed_generation.is_some(),"withdrawal_indexed":state.state!="baseline" && state.state!="mechanical_projected" && head.indexed_generation.is_some(),"generation_id":head.indexed_generation,"historical_read_evidence":source.historical_read_evidence,"new_ai_reading_credit":false}))
    }).collect::<Vec<_>>();
    Ok(Some(
        serde_json::json!({"schema":2,"observation_basis":"last_reconciled_head","authorization_sha256":pin.manifest_sha256,"generation_id":head.indexed_generation,"sources":sources,"retired_note_views":head.retired_notes,"new_ai_reading_credit":false}),
    ))
}

fn direct_path(source: &KnownSource) -> String {
    format!("originals/{}/{}", source.source_id, source.relative_path)
}
fn authorized_documents(head: &Head) -> BTreeMap<String, String> {
    let mut authorized = head.outputs.clone();
    for state in head.sources.values() {
        if let Some(document) = &state.direct_document {
            authorized.insert(document.relative_path.clone(), document.sha256.clone());
        }
    }
    authorized
}
fn checked_head(
    config: &VrgConfig,
) -> VrgResult<
    Option<(
        std::sync::Arc<UpdateManifest>,
        ProjectionConfig,
        std::sync::Arc<Head>,
    )>,
> {
    let Some((manifest, pin)) = load_manifest(config)? else {
        return Ok(None);
    };
    let (work, _lock) = open_work(config)?;
    if read_state::<Transaction>(&work, "TXN.json")?.is_some() {
        return Err(denied());
    }
    let head = read_cached_head(&work, &manifest, &pin)?.ok_or_else(denied)?;
    if head.authorization_sha256 != pin.manifest_sha256 || head.schema != 2 {
        return Err(denied());
    }
    Ok(Some((manifest, pin, head)))
}
fn head_source(manifest: &UpdateManifest, head: &Head, id: &str) -> VrgResult<KnownSource> {
    if let Some(source) = manifest
        .sources
        .iter()
        .find(|source| source.source_id == id)
    {
        return Ok(source.clone());
    }
    let discovered = head.discovered_sources.get(id).ok_or_else(denied)?;
    let policy = manifest
        .discovery
        .as_ref()
        .filter(|p| p.enabled)
        .ok_or_else(denied)?;
    safe_relative(&discovered.relative_path)?;
    if !manifest.original_roots.contains(&discovered.original_root)
        || discovery_id(&discovered.original_root, &discovered.relative_path) != id
        || discovery_excluded(policy, &discovered.original_root, &discovered.relative_path)
        || !discovery_candidate(policy, &discovered.relative_path)
    {
        return Err(denied());
    }
    Ok(discovered_source(id, discovered))
}
fn exact_source_bytes(
    manifest: &UpdateManifest,
    source: &KnownSource,
    expected: &str,
    limit: usize,
) -> VrgResult<Vec<u8>> {
    let root = canonical_directory(&source.original_root)?;
    crate::indexer::record_source_read(&source.original_root.join(&source.relative_path));
    let bytes = match observe(
        &root,
        &source.relative_path,
        limit.min(manifest.max_source_bytes),
    ) {
        Observation::Bytes(bytes) => bytes,
        _ => return Err(denied()),
    };
    if sha(&bytes) != expected || hold_reason(&source.relative_path, &bytes).is_some() {
        return Err(denied());
    }
    // A source's current ancestor policy is checked independently of watcher timing.
    let mut directory = Path::new(&source.relative_path).parent();
    while let Some(parent) = directory {
        let path = parent.join(".codexignore").to_string_lossy().into_owned();
        let actual = read_exact(&root, &path, 65536)?.map(|bytes| sha(&bytes));
        if let Some(probe) = manifest
            .policy_probes
            .iter()
            .find(|probe| probe.root == source.original_root && probe.relative_path == path)
        {
            if actual != probe.expected_sha256 {
                return Err(denied());
            }
        } else if actual.is_some() {
            return Err(denied());
        }
        directory = parent.parent();
    }
    Ok(bytes)
}
fn raw_document(
    manifest: &UpdateManifest,
    source: &KnownSource,
    direct: &Part,
    bytes: Vec<u8>,
) -> VrgResult<crate::contracts::SourceDocument> {
    if direct.relative_path != direct_path(source)
        || direct.source_byte_range != Some([0, bytes.len()])
    {
        return Err(denied());
    }
    if !manifest.discovery.as_ref().is_some_and(|policy| {
        policy.direct_sources && eligible_code_bytes(policy, &source.relative_path, &bytes)
    }) {
        return Err(denied());
    }
    let text = String::from_utf8(bytes).map_err(|_| denied())?;
    let mut metadata = BTreeMap::new();
    metadata.insert("source_format".into(), serde_json::json!("original_utf8"));
    metadata.insert("vrg_source_format".into(), serde_json::json!("source_code"));
    metadata.insert(
        "original_root".into(),
        serde_json::json!(source.original_root),
    );
    metadata.insert(
        "original_relative_path".into(),
        serde_json::json!(source.relative_path),
    );
    metadata.insert("source_id".into(), serde_json::json!(source.source_id));
    metadata.insert("byte_offsets".into(), serde_json::json!("source_raw_utf8"));
    Ok(crate::contracts::SourceDocument {
        source: crate::contracts::SourceRef {
            root_id: manifest.corpus_root_id.clone(),
            relative_path: direct.relative_path.clone(),
        },
        revision: direct.sha256.clone(),
        title: source.relative_path.clone(),
        raw_markdown: text.clone(),
        body_text: text,
        frontmatter: metadata,
        links: vec![],
        modified_unix_ms: None,
    })
}
/// Virtual original-format sources are opened capability-relative and never copied to Markdown.
pub(crate) fn read_direct_source(
    config: &VrgConfig,
    root_id: &str,
    relative: &str,
    limit: usize,
) -> VrgResult<Option<crate::contracts::SourceDocument>> {
    if !relative.starts_with("originals/") {
        return Ok(None);
    }
    safe_relative(relative)?;
    let Some((manifest, _, head)) = checked_head(config)? else {
        return Err(denied());
    };
    if manifest.corpus_root_id != root_id {
        return Err(denied());
    }
    let id = relative.split('/').nth(1).ok_or_else(denied)?;
    let source = head_source(&manifest, &head, id)?;
    let state = head.sources.get(id).ok_or_else(denied)?;
    let direct = state.direct_document.as_ref().ok_or_else(denied)?;
    if direct.relative_path != relative
        || state.state != "mechanical_projected"
        || state.observed_sha256.as_ref() != Some(&direct.sha256)
    {
        return Err(denied());
    }
    let bytes = exact_source_bytes(&manifest, &source, &direct.sha256, limit)?;
    raw_document(&manifest, &source, direct, bytes).map(Some)
}
/// Read the complete authorized document set after reconciliation, reusing exact
/// prior snapshot bodies for unchanged entries instead of reopening whole roots.
pub(crate) fn scan_authorized(
    config: &VrgConfig,
    previous: Option<&crate::contracts::IndexSnapshot>,
    incremental: bool,
) -> VrgResult<Option<crate::vault::ScanBatch>> {
    let Some((manifest, _, head)) = checked_head(config)? else {
        return Ok(None);
    };
    let expected = authorized_documents(&head);
    let mut prior = BTreeMap::new();
    if incremental {
        if let Some(snapshot) = previous {
            for document in &snapshot.documents {
                prior.insert(document.source.relative_path.as_str(), document);
            }
        }
    }
    let mut prior_chunks = BTreeMap::<&str, Vec<&crate::contracts::TextChunk>>::new();
    let mut prior_edges = BTreeMap::<&str, Vec<&crate::contracts::GraphEdge>>::new();
    if let Some(snapshot) = previous.filter(|_| incremental) {
        for chunk in &snapshot.chunks {
            prior_chunks
                .entry(&chunk.source.relative_path)
                .or_default()
                .push(chunk);
        }
        for edge in &snapshot.edges {
            prior_edges
                .entry(&edge.from.relative_path)
                .or_default()
                .push(edge);
        }
    }
    let mut documents = Vec::with_capacity(expected.len());
    let mut files_read = 0;
    for (path, revision) in &expected {
        if let Some(stored) = prior.get(path.as_str()).filter(|stored| {
            stored.revision == *revision && stored.source.root_id == manifest.corpus_root_id
        }) {
            let mut chunks = prior_chunks.get(path.as_str()).cloned().unwrap_or_default();
            chunks.sort_by_key(|chunk| chunk.ordinal);
            let mut body = String::new();
            for chunk in chunks {
                let start = chunk.start_byte as usize;
                let end = chunk.end_byte as usize;
                if start > body.len() || end < start || end - start != chunk.text.len() {
                    return Err(denied());
                }
                let overlap = body.len().saturating_sub(start).min(chunk.text.len());
                if !chunk.text.is_char_boundary(overlap)
                    || body.get(start..start + overlap) != chunk.text.get(..overlap)
                {
                    return Err(denied());
                }
                body.push_str(&chunk.text[overlap..]);
            }
            let links = prior_edges
                .get(path.as_str())
                .into_iter()
                .flat_map(|edges| edges.iter())
                .map(|edge| crate::contracts::LinkRef {
                    target: edge
                        .to
                        .as_ref()
                        .map(|to| to.relative_path.clone())
                        .or_else(|| edge.unresolved_target.clone())
                        .unwrap_or_default(),
                    alias: None,
                    heading: None,
                    kind: edge.kind,
                    start_byte: edge.start_byte,
                    end_byte: edge.end_byte,
                })
                .collect();
            documents.push(crate::contracts::SourceDocument {
                source: stored.source.clone(),
                revision: stored.revision.clone(),
                title: stored.title.clone(),
                raw_markdown: body.clone(),
                body_text: body,
                frontmatter: stored.frontmatter.clone(),
                links,
                modified_unix_ms: stored.modified_unix_ms,
            });
            continue;
        }
        let doc = if path.starts_with("originals/") {
            let id = path.split('/').nth(1).ok_or_else(denied)?;
            let source = head_source(&manifest, &head, id)?;
            let direct = head
                .sources
                .get(id)
                .and_then(|s| s.direct_document.as_ref())
                .ok_or_else(denied)?;
            let bytes =
                exact_source_bytes(&manifest, &source, revision, manifest.max_source_bytes)?;
            raw_document(&manifest, &source, direct, bytes)?
        } else {
            crate::vault::read_index_source(
                config,
                &manifest.corpus_root_id,
                path,
                4 * 1024 * 1024,
            )?
        };
        if doc.revision != *revision {
            return Err(denied());
        }
        files_read += 1;
        documents.push(doc);
    }
    Ok(Some(crate::vault::ScanBatch {
        documents,
        files_seen: files_read,
        excluded: 0,
        issues: vec![],
    }))
}

fn path_affected(path: &Path, changes: &BTreeSet<PathBuf>) -> bool {
    changes.iter().any(|changed| {
        path == changed
            || path.starts_with(changed)
            || changed
                .file_name()
                .is_some_and(|name| name == ".codexignore")
                && changed
                    .parent()
                    .is_some_and(|parent| path.starts_with(parent))
    })
}
fn discover_affected(
    manifest: &UpdateManifest,
    roots: &BTreeMap<PathBuf, File>,
    head: &mut Head,
    changes: &BTreeSet<PathBuf>,
) -> VrgResult<()> {
    let Some(policy) = manifest.discovery.as_ref().filter(|p| p.enabled) else {
        return Ok(());
    };
    let mut known = manifest
        .sources
        .iter()
        .map(|s| (s.original_root.clone(), s.relative_path.clone()))
        .chain(
            head.discovered_sources
                .values()
                .map(|s| (s.original_root.clone(), s.relative_path.clone())),
        )
        .collect::<BTreeSet<_>>();
    for changed in changes {
        let Some((root, directory)) = roots
            .iter()
            .filter(|(root, _)| changed.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
        else {
            continue;
        };
        let relative = changed
            .strip_prefix(root)
            .map_err(|_| denied())?
            .to_str()
            .ok_or_else(denied)?;
        if relative.is_empty() {
            discover_directory(manifest, policy, root, directory, "", &mut known, head)?;
            continue;
        }
        if discovery_excluded(policy, root, relative) {
            continue;
        }
        // Never follow symlinks, including intermediate components.
        let metadata = match std::fs::symlink_metadata(changed) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(denied()),
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            let child = open_dir(changed)?;
            discover_directory(manifest, policy, root, &child, relative, &mut known, head)?;
        } else if metadata.is_file()
            && discovery_candidate(policy, relative)
            && known.insert((root.clone(), relative.to_owned()))
        {
            let (parent, name) = parent(directory, relative)?;
            let Some(mut file) = open_file(&parent, &name)? else {
                continue;
            };
            let extension = Path::new(relative)
                .extension()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            if extension.is_empty()
                && !policy.filenames.iter().any(|p| {
                    Path::new(relative)
                        .file_name()
                        .is_some_and(|name| name == p.as_str())
                })
            {
                let mut prefix = [0; 2];
                if file.read(&mut prefix).ok() != Some(2) || prefix != *b"#!" {
                    continue;
                }
            }
            let id = discovery_id(root, relative);
            head.discovered_sources.insert(
                id.clone(),
                DiscoveredSource {
                    original_root: root.clone(),
                    relative_path: relative.to_owned(),
                },
            );
            head.sources.insert(
                id,
                SourceHead {
                    observed_sha256: None,
                    state: "missing".into(),
                    read_status: "pending_review".into(),
                    parts: vec![],
                    direct_document: None,
                },
            );
        }
    }
    Ok(())
}
/// Filesystem-event reconciliation. Only affected originals are read; immutable
/// snapshot objects for unrelated paths remain available to the indexer.
pub(crate) fn reconcile_paths(config: &VrgConfig, changes: &BTreeSet<PathBuf>) -> VrgResult<bool> {
    let Some((manifest, pin)) = load_manifest(config)? else {
        return crate::projection::reconcile(config);
    };
    reconcile_manifest(config, &pin, manifest, Some(changes))
}

fn synthesis_sources(
    head: &Head,
    sources: &[&KnownSource],
) -> VrgResult<Vec<crate::code_synthesis::SynthesisSource>> {
    sources
        .iter()
        .map(|source| {
            let state = head.sources.get(&source.source_id).ok_or_else(denied)?;
            let outline = head
                .source_outlines
                .get(&source.source_id)
                .ok_or_else(denied)?;
            Ok(crate::code_synthesis::SynthesisSource {
                source_id: source.source_id.clone(),
                relative_path: source.relative_path.clone(),
                revision: state.observed_sha256.clone().ok_or_else(denied)?,
                facts: outline.facts.clone(),
            })
        })
        .collect()
}
fn render_memory_documents(
    manifest: &UpdateManifest,
    head: &Head,
) -> VrgResult<BTreeMap<String, (ModuleDocument, Vec<u8>)>> {
    if head.synthesis_version == 0
        || !manifest
            .discovery
            .as_ref()
            .is_some_and(|p| p.automatic_memory && p.module_docs)
    {
        return Ok(BTreeMap::new());
    }
    let mut result = BTreeMap::new();
    for (module_path, metadata) in &head.module_documents {
        let sources = metadata
            .source_ids
            .iter()
            .map(|id| head_source(manifest, head, id))
            .collect::<VrgResult<Vec<_>>>()?;
        let refs = sources.iter().collect::<Vec<_>>();
        let first = sources.first().ok_or_else(denied)?;
        let module = Path::new(&first.relative_path)
            .parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy();
        let synthesis =
            crate::code_synthesis::synthesize_module(&module, &synthesis_sources(head, &refs)?);
        let path = module_path.replacen("/modules/", "/memory/", 1);
        let compact = head
            .memory_documents
            .get(&path)
            .and_then(|previous| {
                previous.stable_text.as_ref().filter(|text| {
                    previous.significance_sha256.as_ref() == Some(&synthesis.significance_sha256)
                        && text.len() <= 16384
                        && sha(text.as_bytes()) == previous.sha256
                })
            })
            .cloned()
            .unwrap_or(synthesis.compact_memory);
        let bytes = compact.as_bytes().to_vec();
        result.insert(
            path,
            (
                ModuleDocument {
                    sha256: sha(&bytes),
                    source_ids: metadata.source_ids.clone(),
                    significance_sha256: Some(synthesis.significance_sha256),
                    stable_text: Some(compact),
                },
                bytes,
            ),
        );
    }
    Ok(result)
}
pub(crate) fn automatic_memory_enabled(config: &VrgConfig) -> VrgResult<bool> {
    Ok(load_manifest(config)?.is_some_and(|(manifest, _)| {
        manifest
            .discovery
            .as_ref()
            .is_some_and(|p| p.enabled && p.direct_sources && p.automatic_memory)
    }))
}

/// Validate only dependencies of the exact document being served. A healthy
/// monitor never needs a full corpus scan to authorize an individual response.
pub(crate) fn authorize_output(
    config: &VrgConfig,
    root_id: &str,
    relative: &str,
    revision: &str,
) -> VrgResult<()> {
    let Some((manifest, _, head)) = checked_head(config)? else {
        return Ok(());
    };
    if root_id != manifest.corpus_root_id
        || head.outputs.get(relative).map(String::as_str) != Some(revision)
    {
        return Err(denied());
    }
    let mut source_ids = BTreeSet::new();
    for (id, state) in &head.sources {
        if state
            .parts
            .iter()
            .any(|part| part.relative_path == relative)
        {
            source_ids.insert(id.clone());
        }
    }
    if let Some(module) = head
        .module_documents
        .get(relative)
        .or_else(|| head.memory_documents.get(relative))
    {
        source_ids.extend(module.source_ids.iter().cloned());
    }
    if let Some(note) = manifest
        .notes
        .iter()
        .find(|note| note.view.relative_path == relative)
    {
        if head.retired_notes.contains(relative) {
            return Err(denied());
        }
        let root = canonical_directory(&note.original_root)?;
        let actual = read_exact(&root, &note.relative_path, manifest.max_source_bytes)?
            .ok_or_else(denied)?;
        if sha(&actual) != note.expected_sha256 {
            return Err(denied());
        }
        for dependency in &note.source_dependencies {
            let source = manifest
                .sources
                .iter()
                .find(|source| {
                    source.original_root == dependency.original_root
                        && source.relative_path == dependency.relative_path
                })
                .cloned()
                .or_else(|| {
                    head.discovered_sources
                        .iter()
                        .find(|(_, source)| {
                            source.original_root == dependency.original_root
                                && source.relative_path == dependency.relative_path
                        })
                        .map(|(id, source)| discovered_source(id, source))
                })
                .ok_or_else(denied)?;
            exact_source_bytes(
                &manifest,
                &source,
                &dependency.expected_sha256,
                manifest.max_source_bytes,
            )?;
        }
        for source in &manifest.sources {
            if source.dependent_notes.iter().any(|path| path == relative) {
                source_ids.insert(source.source_id.clone());
            }
        }
    }
    for id in source_ids {
        let source = head_source(&manifest, &head, &id)?;
        let state = head.sources.get(&id).ok_or_else(denied)?;
        let expected = state.observed_sha256.as_ref().ok_or_else(denied)?;
        exact_source_bytes(&manifest, &source, expected, manifest.max_source_bytes)?;
    }
    Ok(())
}
pub(crate) fn direct_revision(
    config: &VrgConfig,
    root_id: &str,
    relative: &str,
) -> VrgResult<Option<String>> {
    if !relative.starts_with("originals/") {
        return Ok(None);
    }
    let Some((manifest, _, head)) = checked_head(config)? else {
        return Err(denied());
    };
    if root_id != manifest.corpus_root_id {
        return Err(denied());
    }
    let id = relative.split('/').nth(1).ok_or_else(denied)?;
    let source = head_source(&manifest, &head, id)?;
    let state = head.sources.get(id).ok_or_else(denied)?;
    let direct = state.direct_document.as_ref().ok_or_else(denied)?;
    if direct.relative_path != relative
        || direct_path(&source) != relative
        || state.state != "mechanical_projected"
    {
        return Err(denied());
    }
    exact_source_bytes(
        &manifest,
        &source,
        &direct.sha256,
        manifest.max_source_bytes,
    )?;
    Ok(Some(direct.sha256.clone()))
}

#[cfg(unix)]
type FileStamp = (u64, u64, u64, i64, i64, i64, i64);
#[cfg(unix)]
fn file_stamp(file: &File) -> VrgResult<FileStamp> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    ))
}
type ManifestCache =
    BTreeMap<(PathBuf, String, PathBuf), (FileStamp, std::sync::Arc<UpdateManifest>)>;
type HeadCache = BTreeMap<(PathBuf, String), (FileStamp, std::sync::Arc<Head>)>;
static MANIFEST_CACHE: std::sync::OnceLock<std::sync::Mutex<ManifestCache>> =
    std::sync::OnceLock::new();
static HEAD_CACHE: std::sync::OnceLock<std::sync::Mutex<HeadCache>> = std::sync::OnceLock::new();
fn read_cached_head(
    work: &File,
    manifest: &UpdateManifest,
    pin: &ProjectionConfig,
) -> VrgResult<Option<std::sync::Arc<Head>>> {
    let Some(file) = open_file(work, std::ffi::OsStr::new("HEAD.json"))? else {
        return Ok(None);
    };
    check_private_file(&file)?;
    let stamp = file_stamp(&file)?;
    let key = (manifest.data_dir.clone(), pin.manifest_sha256.clone());
    if let Some((cached_stamp, head)) = HEAD_CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| denied())?
        .get(&key)
    {
        if *cached_stamp == stamp {
            crate::indexer::record_metadata_read(false, true);
            return Ok(Some(std::sync::Arc::clone(head)));
        }
    }
    crate::indexer::record_metadata_read(false, false);
    let bytes = read_bounded(file, MAX_STATE as usize)?;
    let head: Head = serde_json::from_slice(&bytes).map_err(|_| denied())?;
    validate_head(&head, manifest, pin)?;
    let head = std::sync::Arc::new(head);
    let mut cache = HEAD_CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| denied())?;
    if cache.len() >= 32 && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(key, (stamp, std::sync::Arc::clone(&head)));
    Ok(Some(head))
}
fn cache_committed_head(
    work: &File,
    manifest: &UpdateManifest,
    pin: &ProjectionConfig,
    head: &Head,
) -> VrgResult<()> {
    let file = open_file(work, std::ffi::OsStr::new("HEAD.json"))?.ok_or_else(denied)?;
    check_private_file(&file)?;
    HEAD_CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| denied())?
        .insert(
            (manifest.data_dir.clone(), pin.manifest_sha256.clone()),
            (file_stamp(&file)?, std::sync::Arc::new(head.clone())),
        );
    Ok(())
}

pub(crate) fn direct_sources_enabled(config: &VrgConfig) -> VrgResult<bool> {
    Ok(load_manifest(config)?.is_some_and(|(manifest, _)| {
        manifest
            .discovery
            .as_ref()
            .is_some_and(|policy| policy.enabled && policy.direct_sources)
    }))
}
