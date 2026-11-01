use std::{collections::BTreeSet, env, fs, path::PathBuf};

use sha2::{Digest, Sha256};

use serde::{Deserialize, Serialize};

use crate::error::{VrgError, VrgResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VrgConfig {
    #[serde(default)]
    pub roots: Vec<RootConfig>,
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
        let config: Self =
            toml::from_str(&text).map_err(|error| VrgError::InvalidConfig(error.to_string()))?;
        config.validate()?;
        let digest = Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok((config, canonical_path, digest))
    }

    pub fn validate(&self) -> VrgResult<()> {
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
