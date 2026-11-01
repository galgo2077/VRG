use std::{cmp::Ordering, fs, path::Path, sync::Mutex};

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
    model: Mutex<TextEmbedding>,
}

#[derive(Clone, Debug)]
pub struct SemanticMatch {
    pub chunk_id: String,
    pub score: f32,
}

/// Runtime path is stable beside the installed executable, independent of the checkout.
/// No Intel macOS runtime is currently distributed or certified by VRG.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub(crate) fn macos_intel_runtime_path() -> VrgResult<std::path::PathBuf> {
    let executable = std::env::current_exe().map_err(|_| {
        VrgError::SemanticUnavailable("cannot identify the installed executable".into())
    })?;
    let directory = executable.parent().ok_or_else(|| {
        VrgError::SemanticUnavailable("installed executable has no parent directory".into())
    })?;
    Ok(directory.join("libonnxruntime.dylib"))
}

/// Verify the installed runtime before any optional model download or setup writes.
/// This only loads the library; it does not download or create files.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub(crate) fn preflight_macos_intel_runtime() -> VrgResult<()> {
    initialize_macos_intel_runtime(&macos_intel_runtime_path()?)
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
    // ort validates its API version while loading. Convert its exceptional initialization
    // paths to an ordinary semantic-unavailable result rather than aborting the caller.
    std::panic::catch_unwind(|| ort::init_from(path).map(|_| ()))
        .map_err(|_| {
            VrgError::SemanticUnavailable(
                "ONNX Runtime initialization failed; verify the library and API compatibility"
                    .into(),
            )
        })?
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
        preflight_macos_intel_runtime()?;

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
        let onnx = read_model_file(&dir, "model.onnx")?;
        let tokenizer = TokenizerFiles {
            tokenizer_file: read_model_file(&dir, "tokenizer.json")?,
            config_file: read_model_file(&dir, "config.json")?,
            special_tokens_map_file: read_model_file(&dir, "special_tokens_map.json")?,
            tokenizer_config_file: read_model_file(&dir, "tokenizer_config.json")?,
        };

        let mut fingerprint = Sha256::new();
        fingerprint.update(&onnx);
        fingerprint.update(&tokenizer.tokenizer_file);
        fingerprint.update(&tokenizer.config_file);
        fingerprint.update(&tokenizer.special_tokens_map_file);
        fingerprint.update(&tokenizer.tokenizer_config_file);
        let model_id = format!(
            "{}#{}",
            config.model_id,
            hex::encode(fingerprint.finalize())
        );

        let model = UserDefinedEmbeddingModel::new(onnx, tokenizer).with_pooling(Pooling::Mean);
        let available_threads = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .max(1);
        let threads = config.threads.max(1).min(available_threads).min(8);
        let options = InitOptionsUserDefined::new()
            .with_intra_threads(threads)
            .with_max_length(config.max_length);
        let model = TextEmbedding::try_new_from_user_defined(model, options).map_err(|error| {
            VrgError::SemanticUnavailable(format!(
                "local model/tokenizer initialization failed: {error}"
            ))
        })?;

        Ok(Self {
            model_id,
            model: Mutex::new(model),
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Embed source chunks using the E5 passage prefix. Work is submitted in
    /// bounded batches so a large index does not allocate all model inputs at once.
    pub fn embed_chunks(&self, chunks: &[TextChunk]) -> VrgResult<Vec<VectorEmbedding>> {
        let mut output = Vec::with_capacity(chunks.len());
        let mut model = self.model.lock().map_err(|_| {
            VrgError::SemanticUnavailable("embedding model lock was poisoned".into())
        })?;

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
        let mut model = self.model.lock().map_err(|_| {
            VrgError::SemanticUnavailable("embedding model lock was poisoned".into())
        })?;
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
    for vector in vectors.iter().filter(|v| v.model_id == model_id) {
        if vector.dimensions as usize != MODEL_DIMENSIONS {
            return Err(VrgError::SemanticUnavailable(format!(
                "stored vector {} reports an incompatible dimension",
                vector.chunk_id
            )));
        }
        validate_vector(&vector.values, MODEL_DIMENSIONS)?;
        let vector_norm = l2_norm(&vector.values);
        if vector_norm <= f32::EPSILON {
            return Err(VrgError::SemanticUnavailable(format!(
                "stored vector {} has zero magnitude",
                vector.chunk_id
            )));
        }
        let dot = query
            .iter()
            .zip(&vector.values)
            .map(|(left, right)| left * right)
            .sum::<f32>();
        let score = (dot / (query_norm * vector_norm)).clamp(-1.0, 1.0);
        if !score.is_finite() {
            return Err(VrgError::SemanticUnavailable(
                "cosine similarity produced a non-finite score".into(),
            ));
        }
        matches.push(SemanticMatch {
            chunk_id: vector.chunk_id.clone(),
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

fn read_model_file(dir: &Dir, name: &str) -> VrgResult<Vec<u8>> {
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
    dir.read(name).map_err(|error| {
        VrgError::SemanticUnavailable(format!("cannot read local model file {name}: {error}"))
    })
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
