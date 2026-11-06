use std::{
    cmp::Ordering,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, Arc, Weak, OnceLock},
    collections::HashMap,
    time::{Duration, Instant},
};

use cap_std::{ambient_authority, fs::Dir};
use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};
use sha2::{Digest, Sha256};

use crate::{
    config::SemanticModelConfig,
    contracts::{TextChunk, VectorEmbedding},
    error::{VrgError, VrgResult},
};

const SUPPORTED_MODEL_ID: &str = "intfloat/multilingual-e5-small";
const MODEL_DIMENSIONS: usize = 384;
const MODEL_MAX_BYTES: u64 = 1_500_000_000;
const EMBED_BATCH_SIZE: usize = 16;

/// Local-only embedding model. Construction reads a flat set of explicitly
/// configured model files; no FastEmbed downloader or cache path is used.
pub struct LocalEmbedder {
    model_id: String,
    config: SemanticModelConfig,
    data_dir: Option<PathBuf>,
    model: Arc<Mutex<ModelCache>>,
}

struct ModelCache {
    model: Option<TextEmbedding>,
    last_used: Instant,
}

#[derive(Clone, Debug)]
pub struct SemanticMatch {
    pub chunk_id: String,
    pub score: f32,
}

/// Resolve the packaged runtime first, then a verified resource in the active profile.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub(crate) fn macos_intel_runtime_path(data_dir: Option<&Path>) -> VrgResult<std::path::PathBuf> {
    let executable = std::env::current_exe().map_err(|_| {
        VrgError::SemanticUnavailable("cannot identify the installed executable".into())
    })?;
    let directory = executable.parent().ok_or_else(|| {
        VrgError::SemanticUnavailable("installed executable has no parent directory".into())
    })?;
    let packaged = directory.join("libonnxruntime.dylib");
    if fs::symlink_metadata(&packaged)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    {
        return Ok(packaged);
    }
    if let Some(root) = data_dir {
        let cache = root.join("resources/onnxruntime");
        if let Some(runtime) = crate::onnx_source_setup::find_cached(&cache)? {
            return Ok(runtime.path);
        }
    }
    Ok(packaged)
}

/// Verify the installed runtime before any optional model download or setup writes.
/// This only loads the library; it does not download or create files.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub(crate) fn preflight_macos_intel_runtime(data_dir: Option<&Path>) -> VrgResult<()> {
    initialize_macos_intel_runtime(&macos_intel_runtime_path(data_dir)?)
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn initialize_macos_intel_runtime(path: &Path) -> VrgResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        VrgError::SemanticUnavailable(format!(
            "ONNX Runtime is unavailable at {}; macOS Intel runtime is not yet distributed by VRG",
            path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(VrgError::SemanticUnavailable(
            "ONNX Runtime must be a regular non-symlink library beside the executable".into(),
        ));
    }
    // Inspect the public ONNX Runtime C API before initializing ort: merely loading
    // a library and reading its version string does not prove GetApi(24) exists.
    // SAFETY: the library is explicitly selected by the user/distribution and the
    // symbol signature and API-base layout are defined by ONNX Runtime's C ABI.
    let library = unsafe { libloading::Library::new(path) }.map_err(|error| {
        VrgError::SemanticUnavailable(format!("ONNX Runtime could not be loaded: {error}"))
    })?;
    unsafe {
        let getter = library
            .get::<unsafe extern "C" fn() -> *const ort::sys::OrtApiBase>(b"OrtGetApiBase\0")
            .map_err(|_| {
                VrgError::SemanticUnavailable("ONNX Runtime does not export OrtGetApiBase".into())
            })?;
        let base = getter();
        if base.is_null() || ((*base).GetApi)(24).is_null() {
            return Err(VrgError::SemanticUnavailable(
                "ONNX Runtime does not implement API 24".into(),
            ));
        }
    }
    // ort performs its own version validation and holds its own library handle.
    std::panic::catch_unwind(|| ort::init_from(path).map(|_| ()))
        .map_err(|_| VrgError::SemanticUnavailable("ONNX Runtime initialization failed".into()))?
        .map_err(|error| {
            VrgError::SemanticUnavailable(format!(
                "ONNX Runtime is incompatible or could not be loaded: {error}"
            ))
        })
}

impl LocalEmbedder {
    /// Load a local multilingual E5 Small model from:
    /// `model.onnx`, `tokenizer.json`, `config.json`,
    /// `special_tokens_map.json`, and `tokenizer_config.json`.
    ///
    /// The model directory and each file must be regular non-symlink entries.
    /// No network lookup, implicit cache fallback, or model download occurs.
    pub fn load(config: &SemanticModelConfig) -> VrgResult<Self> {
        Self::load_with_data_dir(config, None)
    }

    /// Use the active profile to locate its private runtime on macOS Intel.
    pub(crate) fn load_with_data_dir(
        config: &SemanticModelConfig,
        data_dir: Option<&Path>,
    ) -> VrgResult<Self> {
        static MODELS: OnceLock<Mutex<HashMap<String, Weak<Mutex<ModelCache>>>>> = OnceLock::new();
        let mut prepared = Self::prepare(config, data_dir, false)?;
        let key = serde_json::to_string(config).map_err(|_| VrgError::SemanticUnavailable("model key".into()))?;
        let mut models = MODELS.get_or_init(Default::default).lock()
            .map_err(|_| VrgError::SemanticUnavailable("model registry lock".into()))?;
        if let Some(shared) = models.get(&key).and_then(Weak::upgrade) {
            prepared.model = shared;
        } else {
            models.insert(key, Arc::downgrade(&prepared.model));
        }
        Ok(prepared)
    }

    fn prepare(
        config: &SemanticModelConfig,
        data_dir: Option<&Path>,
        eager: bool,
    ) -> VrgResult<Self> {
        #[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
        let _ = data_dir;
        if config.model_id != SUPPORTED_MODEL_ID {
            return Err(VrgError::SemanticUnavailable(format!(
                "unsupported local model {}; supported model is {SUPPORTED_MODEL_ID}",
                config.model_id
            )));
        }
        if !config.model_dir.is_absolute() {
            return Err(VrgError::SemanticUnavailable(
                "model_dir must be an absolute path".into(),
            ));
        }
        if config.max_length > 512 || config.max_length == 0 {
            return Err(VrgError::SemanticUnavailable(
                "multilingual-e5-small max_length must be between 1 and 512".into(),
            ));
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        preflight_macos_intel_runtime(data_dir)?;

        let root_metadata = fs::symlink_metadata(&config.model_dir).map_err(|error| {
            VrgError::SemanticUnavailable(format!(
                "cannot inspect configured local model directory: {error}"
            ))
        })?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(VrgError::SemanticUnavailable(
                "model_dir must be a regular directory, not a symbolic link".into(),
            ));
        }

        let dir =
            Dir::open_ambient_dir(&config.model_dir, ambient_authority()).map_err(|error| {
                VrgError::SemanticUnavailable(format!(
                    "cannot open configured local model directory: {error}"
                ))
            })?;
        // Validate and hash in bounded pages; metadata-only MCP startup never
        // allocates the 450 MB ONNX buffer or initializes an inference session.
        let mut fingerprint = Sha256::new();
        let onnx = if eager {
            let bytes = read_model_file(&dir, "model.onnx")?;
            fingerprint.update(&bytes);
            Some(bytes)
        } else {
            let mut file = open_model_file(&dir, "model.onnx")?;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|error| VrgError::SemanticUnavailable(error.to_string()))?;
                if count == 0 {
                    break;
                }
                fingerprint.update(&buffer[..count]);
            }
            None
        };
        let tokenizer = TokenizerFiles {
            tokenizer_file: read_model_file(&dir, "tokenizer.json")?,
            config_file: read_model_file(&dir, "config.json")?,
            special_tokens_map_file: read_model_file(&dir, "special_tokens_map.json")?,
            tokenizer_config_file: read_model_file(&dir, "tokenizer_config.json")?,
        };

        fingerprint.update(&tokenizer.tokenizer_file);
        fingerprint.update(&tokenizer.config_file);
        fingerprint.update(&tokenizer.special_tokens_map_file);
        fingerprint.update(&tokenizer.tokenizer_config_file);
        fingerprint.update(b"vrg-e5-pipeline-v1:mean:passage-prefix:query-prefix");
        fingerprint.update(config.max_length.to_le_bytes());
        let model_id = format!(
            "{}#{}",
            config.model_id,
            hex::encode(fingerprint.finalize())
        );

        let model = if let Some(onnx) = onnx {
            let model = UserDefinedEmbeddingModel::new(onnx, tokenizer).with_pooling(Pooling::Mean);
            let available_threads = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1)
                .max(1);
            let threads = config.threads.max(1).min(available_threads).min(8);
            let options = InitOptionsUserDefined::new()
                .with_intra_threads(threads)
                .with_max_length(config.max_length);
            let model =
                TextEmbedding::try_new_from_user_defined(model, options).map_err(|error| {
                    VrgError::SemanticUnavailable(format!(
                        "local model/tokenizer initialization failed: {error}"
                    ))
                })?;

            Some(model)
        } else {
            None
        };
        Ok(Self {
            model_id,
            config: config.clone(),
            data_dir: data_dir.map(Path::to_path_buf),
            model: Arc::new(Mutex::new(ModelCache {
                model,
                last_used: Instant::now(),
            })),
        })
    }

    fn ensure_model<'a>(&self, cache: &'a mut ModelCache) -> VrgResult<&'a mut TextEmbedding> {
        if cache.model.is_none() {
            let loaded = Self::prepare(&self.config, self.data_dir.as_deref(), true)?;
            if loaded.model_id != self.model_id {
                return Err(VrgError::SemanticUnavailable(
                    "model identity changed since startup".into(),
                ));
            }
            cache.model = Arc::try_unwrap(loaded.model)
                .map_err(|_| VrgError::SemanticUnavailable("unexpected shared eager model".into()))?
                .into_inner()
                .map_err(|_| VrgError::SemanticUnavailable("model lock poisoned".into()))?
                .model;
        }
        cache.last_used = Instant::now();
        Ok(cache
            .model
            .as_mut()
            .expect("eager preparation initializes model"))
    }

    /// A busy embedding call holds this same mutex, so eviction cannot interrupt it.
    pub fn release_idle(&self, idle: Duration) -> bool {
        let Ok(mut cache) = self.model.try_lock() else {
            return false;
        };
        if cache.last_used.elapsed() >= idle {
            let retired = cache.model.take();
            let released = retired.is_some();
            drop(retired);
            // Returning Rust/ORT objects to malloc need not release their pages.
            // This native glibc operation only returns already-free heap pages;
            // it does not touch live allocations, file caches or other processes.
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            if released {
                unsafe extern "C" {
                    fn malloc_trim(pad: usize) -> i32;
                }
                // SAFETY: glibc's documented MT-Safe API, no pointer arguments.
                unsafe {
                    malloc_trim(0);
                }
            }
            return released;
        }
        false
    }

    pub fn is_resident(&self) -> bool {
        self.model
            .lock()
            .map(|cache| cache.model.is_some())
            .unwrap_or(false)
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Embed source chunks using the E5 passage prefix. Work is submitted in
    /// bounded batches so a large index does not allocate all model inputs at once.
    pub fn embed_chunks(&self, chunks: &[TextChunk]) -> VrgResult<Vec<VectorEmbedding>> {
        let mut output = Vec::with_capacity(chunks.len());
        let mut cache = self.model.lock().map_err(|_| {
            VrgError::SemanticUnavailable("embedding model lock was poisoned".into())
        })?;

        if chunks.is_empty() {
            return Ok(output);
        }
        let model = self.ensure_model(&mut cache)?;
        for batch in chunks.chunks(EMBED_BATCH_SIZE) {
            let input: Vec<String> = batch
                .iter()
                .map(|chunk| format!("passage: {}", chunk.text))
                .collect();
            let refs: Vec<&str> = input.iter().map(String::as_str).collect();
            let vectors = model.embed(refs, Some(EMBED_BATCH_SIZE)).map_err(|error| {
                VrgError::SemanticUnavailable(format!("local embedding failed: {error}"))
            })?;
            if vectors.len() != batch.len() {
                return Err(VrgError::SemanticUnavailable(
                    "embedding model returned a different number of vectors than inputs".into(),
                ));
            }
            for (chunk, values) in batch.iter().zip(vectors) {
                validate_vector(&values, MODEL_DIMENSIONS)?;
                output.push(VectorEmbedding {
                    chunk_id: chunk.id.clone(),
                    model_id: self.model_id.clone(),
                    dimensions: MODEL_DIMENSIONS as u32,
                    values,
                });
            }
        }
        cache.last_used = Instant::now();
        Ok(output)
    }

    /// Embed a query with the E5 query prefix. No fallback vector is returned
    /// if the model is unavailable or produces an invalid result.
    pub fn embed_query(&self, query: &str) -> VrgResult<Vec<f32>> {
        if query.trim().is_empty() {
            return Err(VrgError::SemanticUnavailable(
                "semantic query must not be empty".into(),
            ));
        }
        let mut cache = self.model.lock().map_err(|_| {
            VrgError::SemanticUnavailable("embedding model lock was poisoned".into())
        })?;
        let model = self.ensure_model(&mut cache)?;
        let input = format!("query: {query}");
        let vectors = model
            .embed(vec![input.as_str()], Some(1))
            .map_err(|error| {
                VrgError::SemanticUnavailable(format!("local query embedding failed: {error}"))
            })?;
        let vector = vectors.into_iter().next().ok_or_else(|| {
            VrgError::SemanticUnavailable("embedding model returned no query vector".into())
        })?;
        validate_vector(&vector, MODEL_DIMENSIONS)?;
        cache.last_used = Instant::now();
        Ok(vector)
    }
}

/// Rank stored vectors by cosine similarity. Vectors belonging to other model
/// identities are ignored; malformed vectors for the selected model fail closed.
pub fn cosine_top_k(
    query: &[f32],
    vectors: &[VectorEmbedding],
    model_id: &str,
    limit: usize,
) -> VrgResult<Vec<SemanticMatch>> {
    cosine_top_k_refs(
        query,
        vectors
            .iter()
            .filter(|v| v.model_id == model_id)
            .map(|v| (v.chunk_id.as_str(), v.dimensions, v.values.as_slice())),
        model_id,
        limit,
    )
}

pub fn cosine_top_k_refs<'a>(
    query: &[f32],
    vectors: impl IntoIterator<Item = (&'a str, u32, &'a [f32])>,
    model_id: &str,
    limit: usize,
) -> VrgResult<Vec<SemanticMatch>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    validate_vector(query, MODEL_DIMENSIONS)?;
    if !model_id.starts_with(&format!("{SUPPORTED_MODEL_ID}#")) {
        return Err(VrgError::SemanticUnavailable(format!(
            "unsupported query model {model_id}"
        )));
    }
    let query_norm = l2_norm(query);
    if query_norm <= f32::EPSILON {
        return Err(VrgError::SemanticUnavailable(
            "query embedding has zero magnitude".into(),
        ));
    }

    let mut matches = Vec::new();
    for (chunk_id, dimensions, values) in vectors {
        if dimensions as usize != MODEL_DIMENSIONS {
            return Err(VrgError::SemanticUnavailable(format!(
                "stored vector {} reports an incompatible dimension",
                chunk_id
            )));
        }
        validate_vector(values, MODEL_DIMENSIONS)?;
        let vector_norm = l2_norm(values);
        if vector_norm <= f32::EPSILON {
            return Err(VrgError::SemanticUnavailable(format!(
                "stored vector {} has zero magnitude",
                chunk_id
            )));
        }
        let dot = query
            .iter()
            .zip(values)
            .map(|(left, right)| left * right)
            .sum::<f32>();
        let score = (dot / (query_norm * vector_norm)).clamp(-1.0, 1.0);
        if !score.is_finite() {
            return Err(VrgError::SemanticUnavailable(
                "cosine similarity produced a non-finite score".into(),
            ));
        }
        matches.push(SemanticMatch {
            chunk_id: chunk_id.to_owned(),
            score,
        });
    }
    matches.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    matches.truncate(limit);
    Ok(matches)
}

fn open_model_file(dir: &Dir, name: &str) -> VrgResult<cap_std::fs::File> {
    let metadata = dir.symlink_metadata(Path::new(name)).map_err(|error| {
        VrgError::SemanticUnavailable(format!(
            "required local model file {name} is unavailable: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(VrgError::SemanticUnavailable(format!(
            "required model entry {name} must be a regular non-symlink file"
        )));
    }
    if metadata.len() == 0 || metadata.len() > MODEL_MAX_BYTES {
        return Err(VrgError::SemanticUnavailable(format!(
            "required model file {name} has an invalid size"
        )));
    }
    dir.open(name).map_err(|error| {
        VrgError::SemanticUnavailable(format!("cannot read local model file {name}: {error}"))
    })
}

fn read_model_file(dir: &Dir, name: &str) -> VrgResult<Vec<u8>> {
    let mut bytes = Vec::new();
    open_model_file(dir, name)?
        .read_to_end(&mut bytes)
        .map_err(|error| VrgError::SemanticUnavailable(error.to_string()))?;
    Ok(bytes)
}

fn validate_vector(vector: &[f32], expected_dimensions: usize) -> VrgResult<()> {
    if vector.len() != expected_dimensions {
        return Err(VrgError::SemanticUnavailable(format!(
            "expected {expected_dimensions} embedding dimensions, received {}",
            vector.len()
        )));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(VrgError::SemanticUnavailable(
            "embedding contains a non-finite value".into(),
        ));
    }
    Ok(())
}

fn l2_norm(vector: &[f32]) -> f32 {
    vector.iter().map(|value| value * value).sum::<f32>().sqrt()
}

#[cfg(all(test, target_os = "macos", target_arch = "x86_64"))]
mod macos_intel_runtime_tests {
    use super::*;

    #[test]
    fn missing_or_invalid_runtime_is_semantic_unavailable() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("vrg-onnx-missing-{}-{nonce}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let lib = dir.join("libonnxruntime.dylib");
        assert!(matches!(
            initialize_macos_intel_runtime(&lib),
            Err(VrgError::SemanticUnavailable(_))
        ));
        fs::write(&lib, b"not a Mach-O library").unwrap();
        assert!(matches!(
            initialize_macos_intel_runtime(&lib),
            Err(VrgError::SemanticUnavailable(_))
        ));
        fs::remove_dir_all(dir).unwrap();
    }
}
