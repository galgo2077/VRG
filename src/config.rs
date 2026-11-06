use std::{collections::BTreeSet, env, fs, path::PathBuf};

use sha2::{Digest, Sha256};

use serde::{Deserialize, Serialize};

use crate::error::{VrgError, VrgResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VrgConfig {
    #[serde(default)]
    pub roots: Vec<RootConfig>,
    /// Explicit source authorization scoped to one canonical repository.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_memory: Vec<ProjectMemoryConfig>,
    /// Explicit opt-in; existing profiles retain classic retrieval by default.
    #[serde(default)]
    pub adaptive_memory: AdaptiveMemoryConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_invalidation: Option<crate::projection::ProjectionConfig>,
    pub data_dir: PathBuf,
    #[serde(default)]
    pub semantic_model: Option<SemanticModelConfig>,
    #[serde(default)]
    pub capture_session_events: bool,
    #[serde(default = "default_session_event_retention_days")]
    pub session_event_retention_days: u16,
    #[serde(default = "default_chunk_bytes")]
    pub chunk_bytes: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap_bytes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdaptiveMemoryConfig {
    pub enabled: bool,
    pub max_response_bytes: usize,
    pub max_context_units: usize,
    pub retain_generations: usize,
    pub soft_store_budget_mib: u64,
}

impl Default for AdaptiveMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_response_bytes: 8192,
            max_context_units: 1536,
            retain_generations: 5,
            soft_store_budget_mib: 512,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMemoryConfig {
    pub project: PathBuf,
    pub path: PathBuf,
    pub allowed_files: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootConfig {
    pub id: String,
    pub path: PathBuf,
    #[serde(default)]
    pub exclusions: Vec<String>,
    /// Optional exact top-level Markdown filenames. An empty list admits no files.
    #[serde(default)]
    pub allowed_files: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticModelConfig {
    pub model_id: String,
    pub model_dir: PathBuf,
    #[serde(default = "default_model_threads")]
    pub threads: usize,
    #[serde(default = "default_model_max_length")]
    pub max_length: usize,
}

impl Default for VrgConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            project_memory: Vec::new(),
            adaptive_memory: AdaptiveMemoryConfig::default(),
            projection_invalidation: None,
            data_dir: default_data_dir(),
            semantic_model: None,
            capture_session_events: false,
            session_event_retention_days: default_session_event_retention_days(),
            chunk_bytes: default_chunk_bytes(),
            chunk_overlap_bytes: default_chunk_overlap(),
        }
    }
}

impl VrgConfig {
    pub fn load(path: impl AsRef<std::path::Path>) -> VrgResult<Self> {
        Self::load_with_identity(path).map(|(config, _, _)| config)
    }

    pub fn load_with_identity(
        path: impl AsRef<std::path::Path>,
    ) -> VrgResult<(Self, PathBuf, String)> {
        let path = path.as_ref();
        let canonical_path = path
            .canonicalize()
            .map_err(|_| VrgError::InvalidConfig("cannot canonicalize config path".into()))?;
        let text = fs::read_to_string(&canonical_path).map_err(|error| {
            VrgError::InvalidConfig(format!("cannot read {}: {error}", canonical_path.display()))
        })?;
        let mut config: Self =
            toml::from_str(&text).map_err(|error| VrgError::InvalidConfig(error.to_string()))?;
        let project = canonical_project(&env::current_dir()?)?;
        let mut projects = BTreeSet::new();
        for memory in &config.project_memory {
            if !memory.project.is_absolute()
                || memory.project.canonicalize()? != memory.project
                || !memory.project.is_dir()
                || !projects.insert(&memory.project)
            {
                return Err(VrgError::InvalidConfig(
                    "project memory requires unique canonical project directories".into(),
                ));
            }
            if project == memory.project {
                config.roots.push(RootConfig {
                    id: "vrg-memory".into(),
                    path: memory.path.clone(),
                    exclusions: Vec::new(),
                    allowed_files: Some(memory.allowed_files.clone()),
                });
            }
        }
        if config
            .data_dir
            .components()
            .any(|part| part.as_os_str() == "{project}")
        {
            config.data_dir = project_data_dir(&config.data_dir, &env::current_dir()?)?;
        }
        config.validate()?;
        let digest = Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok((config, canonical_path, digest))
    }

    pub fn validate(&self) -> VrgResult<()> {
        if self.projection_invalidation.is_some()
            && self.adaptive_memory.enabled
            && !crate::projection::updates::automatic_memory_enabled(self)?
        {
            return Err(VrgError::InvalidConfig(
                "projection invalidation does not support adaptive memory".into(),
            ));
        }
        if !self.data_dir.is_absolute() {
            return Err(VrgError::InvalidConfig(
                "data_dir must be an absolute path".into(),
            ));
        }
        if self.chunk_bytes == 0 || self.chunk_overlap_bytes >= self.chunk_bytes {
            return Err(VrgError::InvalidConfig(
                "chunk sizes must be positive and overlap smaller than chunk size".into(),
            ));
        }
        if self.session_event_retention_days == 0 || self.session_event_retention_days > 3650 {
            return Err(VrgError::InvalidConfig(
                "session event retention must be between 1 and 3650 days".into(),
            ));
        }
        let policy = &self.adaptive_memory;
        if !(512..=8192).contains(&policy.max_response_bytes)
            || !(128..=262_144).contains(&policy.max_context_units)
            || !(2..=10_000).contains(&policy.retain_generations)
            || !(1..=1_048_576).contains(&policy.soft_store_budget_mib)
        {
            return Err(VrgError::InvalidConfig(
                "adaptive memory budgets require 512..8192 response bytes, 128..262144 context estimate units, 2..10000 retained generations and 1..1048576 MiB soft store budget".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for root in &self.roots {
            if root.id.trim().is_empty() || !ids.insert(root.id.as_str()) {
                return Err(VrgError::InvalidConfig(
                    "root IDs must be non-empty and unique".into(),
                ));
            }
            if !root.path.is_absolute() {
                return Err(VrgError::InvalidConfig(format!(
                    "root {} must use an absolute path",
                    root.id
                )));
            }
            if let Some(allowed) = &root.allowed_files {
                let mut names = BTreeSet::new();
                for name in allowed {
                    let path = std::path::Path::new(name);
                    let mut parts = path.components();
                    if name.is_empty()
                        || name.contains(['/', '\\', '\0'])
                        || !matches!(parts.next(), Some(std::path::Component::Normal(_)))
                        || parts.next().is_some()
                        || !path
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                        || !names.insert(name)
                    {
                        return Err(VrgError::InvalidConfig(format!(
                            "root {} contains an invalid allowed filename",
                            root.id
                        )));
                    }
                }
            }
        }
        if let Some(model) = &self.semantic_model {
            if model.model_id.trim().is_empty()
                || !model.model_dir.is_absolute()
                || model.threads == 0
                || model.max_length == 0
            {
                return Err(VrgError::InvalidConfig(
                    "semantic model ID/path, thread count, and max length must be valid".into(),
                ));
            }
        }
        Ok(())
    }

    /// Validate derived/model locations before opening storage or reading a source root.
    pub fn validate_layout(&self) -> VrgResult<()> {
        let data_path = comparable_path(&self.data_dir)?;
        for root in &self.roots {
            let root_path = comparable_path(&root.path)?;
            if overlaps(&root_path, &data_path) {
                return Err(VrgError::InvalidConfig(format!(
                    "derived data directory overlaps source root {}",
                    root.id
                )));
            }
            if let Some(model) = &self.semantic_model {
                let model_path = comparable_path(&model.model_dir)?;
                if overlaps(&root_path, &model_path) {
                    return Err(VrgError::InvalidConfig(format!(
                        "semantic model directory overlaps source root {}",
                        root.id
                    )));
                }
            }
        }
        if let Some(model) = &self.semantic_model {
            let model_path = comparable_path(&model.model_dir)?;
            if overlaps(&data_path, &model_path) {
                return Err(VrgError::InvalidConfig(
                    "semantic model directory overlaps derived storage".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn canonical_data_dir(&self) -> VrgResult<PathBuf> {
        comparable_path(&self.data_dir)
    }
}

/// Resolve an optional project directory component using the enclosing repository.
/// Configurations with a literal data_dir retain their existing behavior.
fn project_data_dir(template: &std::path::Path, cwd: &std::path::Path) -> VrgResult<PathBuf> {
    let project = canonical_project(cwd)?;
    let name = project.file_name().ok_or_else(|| {
        VrgError::InvalidConfig("project database requires a named working directory".into())
    })?;
    let mut result = PathBuf::new();
    for component in template.components() {
        if component.as_os_str() == "{project}" {
            result.push(name);
        } else {
            result.push(component.as_os_str());
        }
    }
    Ok(result)
}

fn canonical_project(cwd: &std::path::Path) -> VrgResult<PathBuf> {
    let cwd = cwd.canonicalize()?;
    Ok(cwd
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(&cwd)
        .to_path_buf())
}

fn comparable_path(path: &std::path::Path) -> VrgResult<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .map_err(|_| VrgError::InvalidConfig("cannot canonicalize configured path".into()));
    }
    let mut cursor = path.to_path_buf();
    let mut suffix = Vec::new();
    while !cursor.exists() {
        let name = cursor.file_name().ok_or_else(|| {
            VrgError::InvalidConfig("configured path has no existing ancestor".into())
        })?;
        suffix.push(name.to_os_string());
        if !cursor.pop() {
            return Err(VrgError::InvalidConfig(
                "configured path has no existing ancestor".into(),
            ));
        }
    }
    let mut result = cursor
        .canonicalize()
        .map_err(|_| VrgError::InvalidConfig("cannot canonicalize configured path".into()))?;
    for component in suffix.iter().rev() {
        result.push(component);
    }
    Ok(result)
}

fn overlaps(left: &std::path::Path, right: &std::path::Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn default_data_dir() -> PathBuf {
    if let Some(path) = env::var_os("VRG_DATA_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("vrg");
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/vrg")
}

fn default_chunk_bytes() -> usize {
    2400
}

fn default_chunk_overlap() -> usize {
    240
}

fn default_model_threads() -> usize {
    4
}

fn default_model_max_length() -> usize {
    512
}

fn default_session_event_retention_days() -> u16 {
    90
}

#[cfg(test)]
mod project_storage_tests {
    use super::*;

    #[test]
    fn project_storage_uses_repository_name_from_nested_directory() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp = env::temp_dir().join(format!(
            "vrg-project-storage-{}-{nonce}",
            std::process::id()
        ));
        let project = temp.join("Example Project");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(project.join("src/nested")).unwrap();
        let template = std::path::Path::new("/tmp/VRG database/{project}");
        assert_eq!(
            project_data_dir(template, &project.join("src/nested")).unwrap(),
            PathBuf::from("/tmp/VRG database/Example Project")
        );
        let other = temp.join("Another Project");
        fs::create_dir(&other).unwrap();
        assert_eq!(
            project_data_dir(template, &other).unwrap(),
            PathBuf::from("/tmp/VRG database/Another Project")
        );
        fs::remove_dir_all(temp).unwrap();
    }
}
