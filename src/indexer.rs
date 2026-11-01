use std::collections::{BTreeSet, HashMap, HashSet};

#[cfg(feature = "f4-instrumentation")]
use std::{
    env,
    fs::OpenOptions,
    io::Write,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

use crate::{
    config::VrgConfig,
    contracts::{
        GraphEdge, IndexReport, IndexSnapshot, ModuleStatus, SourceDocument, SourceRef,
        StoredDocument, TextChunk, VectorEmbedding,
    },
    error::{VrgError, VrgResult},
    graph,
    semantic::LocalEmbedder,
    storage::DerivedStore,
    vault,
};

const GENERATION_SCHEMA: &str = "vrg-index-v2";

/// Scans all configured roots, reuses unchanged derived records, and publishes
/// one complete immutable generation. A partial or ambiguous scan is rejected
/// before storage is changed.
pub fn index(
    config: &VrgConfig,
    store: &DerivedStore,
    embedder: Option<&LocalEmbedder>,
) -> VrgResult<IndexReport> {
    config.validate()?;
    if config.roots.is_empty() {
        return Err(VrgError::InvalidConfig(
            "refusing to index with an empty source-root allowlist".into(),
        ));
    }

    let scan = vault::scan_roots(config)?;
    if !scan.issues.is_empty() {
        let summary = scan
            .issues
            .iter()
            .take(8)
            .map(|issue| {
                format!(
                    "{}:{}:{}",
                    issue.root_id,
                    issue.relative_path.as_deref().unwrap_or("<root>"),
                    issue.code
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(VrgError::AccessDenied(format!(
            "source scan incomplete ({} issue(s)); generation was not published: {summary}",
            scan.issues.len()
        )));
    }

    let previous = store.load_current_snapshot()?;
    let expected_parent = previous
        .as_ref()
        .map(|snapshot| snapshot.generation_id.clone());
    let prior_documents: HashMap<SourceRef, &StoredDocument> = previous
        .as_ref()
        .map(|snapshot| {
            snapshot
                .documents
                .iter()
                .map(|document| (document.source.clone(), document))
                .collect()
        })
        .unwrap_or_default();

    let prior_chunks: HashMap<SourceRef, Vec<TextChunk>> = previous
        .as_ref()
        .map(|snapshot| {
            let mut grouped: HashMap<SourceRef, Vec<TextChunk>> = HashMap::new();
            for chunk in &snapshot.chunks {
                grouped
                    .entry(chunk.source.clone())
                    .or_default()
                    .push(chunk.clone());
            }
            grouped
        })
        .unwrap_or_default();
    let prior_vectors = previous
        .as_ref()
        .map(|snapshot| snapshot.vectors.as_slice())
        .unwrap_or_default();
    let vectors_by_chunk: HashMap<&str, Vec<&VectorEmbedding>> = {
        let mut grouped: HashMap<&str, Vec<&VectorEmbedding>> = HashMap::new();
        for vector in prior_vectors {
            grouped
                .entry(vector.chunk_id.as_str())
                .or_default()
                .push(vector);
        }
        grouped
    };

    let mut documents = Vec::with_capacity(scan.documents.len());
    let mut chunks = Vec::new();
    let mut vectors = Vec::new();
    let mut chunks_to_embed = Vec::new();
    let selected_model = embedder.map(LocalEmbedder::model_id);
    let mut changed = 0usize;
    let mut unchanged = 0usize;
    let current_sources: HashSet<SourceRef> = scan
        .documents
        .iter()
        .map(|document| document.source.clone())
        .collect();

    for document in &scan.documents {
        let stored = StoredDocument {
            source: document.source.clone(),
            revision: document.revision.clone(),
            title: document.title.clone(),
            frontmatter: document.frontmatter.clone(),
            modified_unix_ms: document.modified_unix_ms,
        };
        documents.push(stored.clone());

        let same_revision = prior_documents
            .get(&document.source)
            .is_some_and(|prior| prior.revision == document.revision);
        let prior_for_source = prior_chunks
            .get(&document.source)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let unchanged_source = same_revision && stored_chunks_match(document, prior_for_source);
        let mut document_chunks = if unchanged_source {
            unchanged += 1;
            prior_for_source.to_vec()
        } else {
            changed += 1;
            chunk_document(document, config.chunk_bytes, config.chunk_overlap_bytes)?
        };

        // A malformed old snapshot must not silently lose the current source.
        if unchanged_source
            && document_chunks
                .iter()
                .any(|chunk| chunk.revision != document.revision || chunk.source != document.source)
        {
            return Err(VrgError::Storage(format!(
                "stored chunks do not match source revision for {}",
                document.source.relative_path
            )));
        }

        for chunk in &document_chunks {
            if let Some(prior) = vectors_by_chunk.get(chunk.id.as_str()) {
                for vector in prior.iter().copied() {
                    if selected_model.is_none_or(|model_id| vector.model_id == model_id) {
                        vectors.push(vector.clone());
                    }
                }
            }
        }

        if let Some(model_id) = selected_model {
            let existing_ids: HashSet<&str> = vectors
                .iter()
                .filter(|vector| vector.model_id == model_id)
                .map(|vector| vector.chunk_id.as_str())
                .collect();
            chunks_to_embed.extend(
                document_chunks
                    .iter()
                    .filter(|chunk| !existing_ids.contains(chunk.id.as_str()))
                    .cloned(),
            );
        }
        chunks.append(&mut document_chunks);
    }

    // Chunks for sources removed from the complete manifest are tombstoned by
    // omission. Earlier generations remain available for rollback.
    let removed = prior_documents
        .keys()
        .filter(|source| !current_sources.contains(*source))
        .count();
    let graph_snapshot = graph::derive_graph(&scan.documents);
    let edges = graph_snapshot.edges;

    let mut semantic_status = match (config.semantic_model.as_ref(), embedder) {
        (_, Some(_)) => ModuleStatus::Ready,
        (Some(_), None) => ModuleStatus::Degraded(
            "configured local semantic model is unavailable; lexical indexing remains enabled"
                .into(),
        ),
        (None, None) => ModuleStatus::Degraded(
            "no local semantic model configured; lexical indexing remains enabled".into(),
        ),
    };

    if let Some(embedder) = embedder {
        if !chunks_to_embed.is_empty() {
            match embedder.embed_chunks(&chunks_to_embed) {
                Ok(mut generated) => vectors.append(&mut generated),
                Err(error) => {
                    semantic_status = ModuleStatus::Degraded(error.to_string());
                }
            }
        }
    }

    let unchanged_graph = previous
        .as_ref()
        .is_some_and(|snapshot| same_edges(&snapshot.edges, &edges));
    let source_or_chunks_changed = changed > 0 || removed > 0;
    let vector_set_changed = previous
        .as_ref()
        .is_none_or(|snapshot| !same_vector_identity(&snapshot.vectors, &vectors));

    if !source_or_chunks_changed && unchanged_graph && !vector_set_changed {
        return Ok(IndexReport {
            generation_id: expected_parent,
            roots_scanned: config.roots.len(),
            files_seen: scan.files_seen,
            indexed: 0,
            unchanged,
            excluded: scan.excluded,
            failed: 0,
            semantic_status,
        });
    }

    documents.sort_by(|left, right| left.source.cmp(&right.source));
    chunks.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then(left.ordinal.cmp(&right.ordinal))
    });
    vectors.sort_by(|left, right| {
        left.chunk_id
            .cmp(&right.chunk_id)
            .then(left.model_id.cmp(&right.model_id))
    });
    let edges = sorted_edges(edges);
    let mut snapshot = IndexSnapshot {
        generation_id: String::new(),
        documents,
        chunks,
        edges,
        vectors,
    };
    snapshot.generation_id = generation_id(&snapshot, expected_parent.as_deref())?;
    f4_barrier_before_commit()?;
    store.commit_generation(snapshot.clone(), expected_parent.as_deref())?;

    Ok(IndexReport {
        generation_id: Some(snapshot.generation_id),
        roots_scanned: config.roots.len(),
        files_seen: scan.files_seen,
        indexed: changed,
        unchanged,
        excluded: scan.excluded,
        failed: 0,
        semantic_status,
    })
}

/// Test-only process barrier used by the approved F4-04 two-writer procedure.
/// The hook is compiled out of normal builds and has no effect unless the
/// `f4-instrumentation` feature and `VRG_F4_BARRIER_DIR` are both present.
#[cfg(feature = "f4-instrumentation")]
fn f4_barrier_before_commit() -> VrgResult<()> {
    let Some(directory) = env::var_os("VRG_F4_BARRIER_DIR") else {
        return Ok(());
    };
    let directory = Path::new(&directory);
    if !directory.is_absolute() {
        return Err(VrgError::InvalidConfig(
            "VRG_F4_BARRIER_DIR must be absolute".into(),
        ));
    }
    std::fs::create_dir_all(directory).map_err(|error| VrgError::Io(error))?;
    let marker = directory.join(format!("ready-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .map_err(VrgError::Io)?;
    writeln!(file, "{}", std::process::id()).map_err(VrgError::Io)?;
    file.sync_all().map_err(VrgError::Io)?;

    let release = directory.join("release");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !release.exists() {
        if Instant::now() >= deadline {
            return Err(VrgError::Recovery(
                "F4 barrier timed out waiting for release marker".into(),
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

#[cfg(not(feature = "f4-instrumentation"))]
fn f4_barrier_before_commit() -> VrgResult<()> {
    Ok(())
}

fn chunk_document(
    document: &SourceDocument,
    chunk_bytes: usize,
    overlap_bytes: usize,
) -> VrgResult<Vec<TextChunk>> {
    if chunk_bytes == 0 || overlap_bytes >= chunk_bytes {
        return Err(VrgError::InvalidConfig(
            "chunk overlap must be smaller than a positive chunk size".into(),
        ));
    }
    let body = document.body_text.as_str();
    if body.is_empty() {
        return Ok(Vec::new());
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut ordinal = 0u32;
    while start < body.len() {
        let target = start.saturating_add(chunk_bytes).min(body.len());
        let mut end = floor_char_boundary(body, target);
        if end <= start {
            end = next_char_boundary(body, start);
        }
        if end <= start {
            return Err(VrgError::Markdown(
                "cannot advance while chunking UTF-8 Markdown body".into(),
            ));
        }

        let chunk_text = body[start..end].to_owned();
        let start_byte = u32::try_from(start).map_err(|_| {
            VrgError::Markdown("document exceeds supported 32-bit byte spans".into())
        })?;
        let end_byte = u32::try_from(end).map_err(|_| {
            VrgError::Markdown("document exceeds supported 32-bit byte spans".into())
        })?;
        let id = chunk_id(document, ordinal, start_byte, end_byte, &chunk_text);
        chunks.push(TextChunk {
            id,
            source: document.source.clone(),
            revision: document.revision.clone(),
            ordinal,
            start_byte,
            end_byte,
            text: chunk_text,
        });
        ordinal = ordinal
            .checked_add(1)
            .ok_or_else(|| VrgError::Markdown("document has too many chunks".into()))?;

        if end == body.len() {
            break;
        }
        let desired_next = end.saturating_sub(overlap_bytes);
        let mut next = ceil_char_boundary(body, desired_next);
        if next <= start {
            next = end;
        }
        if next >= end {
            next = end;
        }
        start = next;
    }
    Ok(chunks)
}

fn stored_chunks_match(document: &SourceDocument, chunks: &[TextChunk]) -> bool {
    if document.body_text.is_empty() {
        return chunks.is_empty();
    }
    if chunks.is_empty() {
        return false;
    }
    let mut ordered = chunks.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|chunk| chunk.ordinal);
    if ordered[0].start_byte != 0
        || ordered.last().map(|chunk| chunk.end_byte as usize) != Some(document.body_text.len())
    {
        return false;
    }
    let mut previous_end = 0usize;
    for chunk in ordered {
        let start = chunk.start_byte as usize;
        let end = chunk.end_byte as usize;
        if chunk.source != document.source
            || chunk.revision != document.revision
            || start > end
            || end > document.body_text.len()
            || !document.body_text.is_char_boundary(start)
            || !document.body_text.is_char_boundary(end)
            || start > previous_end
            || document.body_text.get(start..end) != Some(chunk.text.as_str())
        {
            return false;
        }
        previous_end = end;
    }
    previous_end == document.body_text.len()
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn next_char_boundary(text: &str, index: usize) -> usize {
    text[index..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| index + offset)
        .unwrap_or(text.len())
}

fn chunk_id(document: &SourceDocument, ordinal: u32, start: u32, end: u32, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(document.source.root_id.as_bytes());
    hasher.update([0]);
    hasher.update(document.source.relative_path.as_bytes());
    hasher.update([0]);
    hasher.update(document.revision.as_bytes());
    hasher.update(ordinal.to_be_bytes());
    hasher.update(start.to_be_bytes());
    hasher.update(end.to_be_bytes());
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

fn same_edges(left: &[GraphEdge], right: &[GraphEdge]) -> bool {
    edge_keys(left) == edge_keys(right)
}

fn edge_keys(edges: &[GraphEdge]) -> BTreeSet<String> {
    edges
        .iter()
        .map(|edge| {
            format!(
                "{}\0{}\0{}\0{}\0{:?}\0{}\0{}",
                edge.from.root_id,
                edge.from.relative_path,
                edge.to
                    .as_ref()
                    .map(|source| source.root_id.as_str())
                    .unwrap_or(""),
                edge.to
                    .as_ref()
                    .map(|source| source.relative_path.as_str())
                    .or(edge.unresolved_target.as_deref())
                    .unwrap_or(""),
                edge.kind,
                edge.start_byte,
                edge.end_byte
            )
        })
        .collect()
}

fn same_vector_identity(left: &[VectorEmbedding], right: &[VectorEmbedding]) -> bool {
    vector_identity(left) == vector_identity(right)
}

fn vector_identity(vectors: &[VectorEmbedding]) -> Vec<(&str, &str, u32, &[f32])> {
    let mut values = vectors
        .iter()
        .map(|vector| {
            (
                vector.chunk_id.as_str(),
                vector.model_id.as_str(),
                vector.dimensions,
                vector.values.as_slice(),
            )
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left.0
            .cmp(right.0)
            .then(left.1.cmp(right.1))
            .then(left.2.cmp(&right.2))
    });
    values
}

fn sorted_edges(mut edges: Vec<GraphEdge>) -> Vec<GraphEdge> {
    edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then(left.to.cmp(&right.to))
            .then(left.unresolved_target.cmp(&right.unresolved_target))
            .then((left.start_byte, left.end_byte).cmp(&(right.start_byte, right.end_byte)))
    });
    edges
}

fn generation_id(snapshot: &IndexSnapshot, parent_generation: Option<&str>) -> VrgResult<String> {
    let encoded = serde_json::to_vec(&(
        GENERATION_SCHEMA,
        parent_generation,
        &snapshot.documents,
        &snapshot.chunks,
        &snapshot.edges,
        snapshot
            .vectors
            .iter()
            .map(|vector| {
                (
                    &vector.chunk_id,
                    &vector.model_id,
                    vector.dimensions,
                    &vector.values,
                )
            })
            .collect::<Vec<_>>(),
    ))
    .map_err(|error| VrgError::Storage(format!("cannot fingerprint index generation: {error}")))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}
