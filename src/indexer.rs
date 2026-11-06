use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
thread_local! { static SOURCE_READS: RefCell<(BTreeSet<PathBuf>, usize)> = RefCell::new((BTreeSet::new(), 0)); }
thread_local! { static METADATA_READS: std::cell::Cell<[usize;4]> = const { std::cell::Cell::new([0;4]) }; }
pub(crate) fn record_metadata_read(policy: bool, hit: bool) {
    METADATA_READS.with(|value| {
        let mut counts = value.get();
        counts[if policy {
            if hit { 1 } else { 0 }
        } else {
            if hit { 3 } else { 2 }
        }] += 1;
        value.set(counts);
    });
}
pub(crate) fn record_source_read(path: &std::path::Path) {
    SOURCE_READS.with(|reads| {
        let mut reads = reads.borrow_mut();
        reads.0.insert(path.to_owned());
        reads.1 += 1;
    });
}
fn source_read_counts() -> (usize, usize) {
    SOURCE_READS.with(|reads| {
        let reads = reads.borrow();
        (reads.0.len(), reads.1)
    })
}

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
    index_scoped(config, store, embedder, None)
}
pub fn index_paths(
    config: &VrgConfig,
    store: &DerivedStore,
    embedder: Option<&LocalEmbedder>,
    paths: &BTreeSet<PathBuf>,
) -> VrgResult<IndexReport> {
    index_scoped(config, store, embedder, Some(paths))
}
fn index_scoped(
    config: &VrgConfig,
    store: &DerivedStore,
    embedder: Option<&LocalEmbedder>,
    paths: Option<&BTreeSet<PathBuf>>,
) -> VrgResult<IndexReport> {
    SOURCE_READS.with(|reads| *reads.borrow_mut() = (BTreeSet::new(), 0));
    METADATA_READS.with(|reads| reads.set([0; 4]));
    config.validate()?;
    if config.roots.is_empty() {
        return Err(VrgError::InvalidConfig(
            "refusing to index with an empty source-root allowlist".into(),
        ));
    }

    let changed_projection = if let Some(paths) = paths {
        crate::projection::updates::reconcile_paths(config, paths)?
    } else {
        crate::projection::reconcile(config)?
    };
    if changed_projection {
        store.mark_owned_projection_dirty()?;
    }
    let projection_head = crate::projection::updates::head_token(config)?;
    let previous = store.load_current_snapshot()?;
    let scan = match crate::projection::updates::scan_authorized(
        config,
        previous.as_ref(),
        paths.is_some(),
    )? {
        Some(scan) => scan,
        None => vault::scan_roots(config)?,
    };
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

    crate::projection::updates::authorize_scan(
        config,
        &scan.documents,
        projection_head.as_deref(),
    )?;
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
    let chunks_by_id = previous
        .as_ref()
        .map(|snapshot| {
            snapshot
                .chunks
                .iter()
                .map(|chunk| (chunk.id.as_str(), chunk.text.as_str()))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let vectors_by_text: HashMap<&str, &VectorEmbedding> = prior_vectors
        .iter()
        .filter(|v| selected_model.is_some_and(|m| v.model_id == m))
        .filter_map(|v| chunks_by_id.get(v.chunk_id.as_str()).map(|text| (*text, v)))
        .collect();
    let mut content_reused = 0usize;
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
            let mut selected_present = false;
            if let Some(prior) = vectors_by_chunk.get(chunk.id.as_str()) {
                for vector in prior.iter().copied() {
                    if selected_model.is_none_or(|model_id| vector.model_id == model_id) {
                        selected_present |=
                            selected_model.is_some_and(|model_id| vector.model_id == model_id);
                        vectors.push(vector.clone());
                    }
                }
            }
            if selected_model.is_some() && !selected_present {
                if let Some(prior) = vectors_by_text.get(chunk.text.as_str()) {
                    let mut reused = (**prior).clone();
                    reused.chunk_id = chunk.id.clone();
                    vectors.push(reused);
                    content_reused += 1;
                } else {
                    chunks_to_embed.push(chunk.clone());
                }
            }
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

    // The selected model/passage pipeline is identical for this entire batch.
    // Cross-source/worktree chunk IDs differ even when their exact input text
    // does not. Submit each distinct text once and fan its vector out to every
    // real chunk identity; no source/chunk metadata is deduplicated away.
    let mut unique_by_text = HashMap::<&str, usize>::new();
    let mut unique_pending = Vec::<TextChunk>::new();
    let mut pending_fanout = Vec::<Vec<String>>::new();
    for chunk in &chunks_to_embed {
        if let Some(index) = unique_by_text.get(chunk.text.as_str()) {
            pending_fanout[*index].push(chunk.id.clone());
        } else {
            unique_by_text.insert(chunk.text.as_str(), unique_pending.len());
            unique_pending.push(chunk.clone());
            pending_fanout.push(Vec::new());
        }
    }
    let (source_files_read, source_read_operations) = source_read_counts();
    let mut work = crate::contracts::IndexWork {
        event_paths: paths.map_or(0, BTreeSet::len),
        full_scans: usize::from(paths.is_none()),
        source_files_read,
        source_read_operations,
        chunks_embedded: 0,
        vectors_reused_by_content: content_reused,
        ..Default::default()
    };
    if let Some(embedder) = embedder {
        if !unique_pending.is_empty() {
            match embedder.embed_chunks(&unique_pending) {
                Ok(generated) => {
                    work.chunks_embedded = generated.len();
                    for (index, vector) in generated.into_iter().enumerate() {
                        for duplicate_id in &pending_fanout[index] {
                            let mut reused = vector.clone();
                            reused.chunk_id = duplicate_id.clone();
                            vectors.push(reused);
                            work.vectors_reused_by_content += 1;
                        }
                        vectors.push(vector);
                    }
                }
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

    // Reused chunks/vectors are already owned by the outgoing snapshot. Once
    // comparison is complete, release the previous generation and borrowed
    // lookup maps before fingerprinting/publication instead of retaining both.
    drop(vectors_by_text);
    drop(chunks_by_id);
    drop(vectors_by_chunk);
    drop(prior_documents);
    drop(prior_chunks);
    drop(previous);

    if !source_or_chunks_changed && unchanged_graph && !vector_set_changed {
        crate::projection::updates::mark_indexed(
            config,
            projection_head.as_deref(),
            expected_parent.as_deref(),
        )?;
        return finish_report(
            config,
            store,
            IndexReport {
                generation_id: expected_parent,
                roots_scanned: config.roots.len(),
                files_seen: scan.files_seen,
                indexed: 0,
                unchanged,
                excluded: scan.excluded,
                failed: 0,
                semantic_status,
                work,
            },
            &scan.documents,
        );
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
    let published_generation = snapshot.generation_id.clone();
    store.commit_generation(snapshot, expected_parent.as_deref())?;
    crate::projection::updates::mark_indexed(
        config,
        projection_head.as_deref(),
        Some(&published_generation),
    )?;

    finish_report(
        config,
        store,
        IndexReport {
            generation_id: Some(published_generation),
            roots_scanned: config.roots.len(),
            files_seen: scan.files_seen,
            indexed: changed,
            unchanged,
            excluded: scan.excluded,
            failed: 0,
            semantic_status,
            work,
        },
        &scan.documents,
    )
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
    use std::io::{BufWriter, Write};
    // digest 0.11 intentionally has no io::Write implementation. This sink is
    // the minimal adapter; serde's identical compact JSON bytes flow into SHA
    // through a bounded native buffer rather than a snapshot-sized Vec<u8>.
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hash = HashWriter(Sha256::new());
    {
        let mut writer = BufWriter::with_capacity(64 * 1024, &mut hash);
        serde_json::to_writer(
            &mut writer,
            &(
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
            ),
        )
        .map_err(|error| {
            VrgError::Storage(format!("cannot fingerprint index generation: {error}"))
        })?;
        writer.flush().map_err(|error| {
            VrgError::Storage(format!("cannot fingerprint index generation: {error}"))
        })?;
    }
    Ok(hex::encode(hash.0.finalize()))
}

fn finish_report(
    config: &VrgConfig,
    store: &DerivedStore,
    mut report: IndexReport,
    documents: &[SourceDocument],
) -> VrgResult<IndexReport> {
    if let Some(generation) = report.generation_id.as_deref() {
        crate::mechanical_memory::apply_under_sync(
            config,
            store,
            generation,
            documents,
            crate::projection::updates::automatic_memory_enabled(config)?,
        )?;
    }
    let (files, operations) = source_read_counts();
    report.work.source_files_read = files;
    report.work.source_read_operations = operations;
    let metadata = METADATA_READS.with(std::cell::Cell::get);
    report.work.policy_manifest_reads = metadata[0];
    report.work.policy_cache_hits = metadata[1];
    report.work.head_reads = metadata[2];
    report.work.head_cache_hits = metadata[3];
    persist_work(config, &report)?;
    Ok(report)
}
fn persist_work(config: &VrgConfig, report: &IndexReport) -> VrgResult<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let value = serde_json::json!({"generation_id":report.generation_id,"unix_ns":timestamp.to_string(),"indexed":report.indexed,"unchanged":report.unchanged,"work":report.work});
    let bytes = serde_json::to_vec(&value).map_err(|error| VrgError::Storage(error.to_string()))?;
    let temporary = config.data_dir.join(format!(
        ".index-work-{}-{timestamp}.tmp",
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, config.data_dir.join("last-index-work.json"))?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    let mut history = options.open(config.data_dir.join("index-work.jsonl"))?;
    history.write_all(&bytes)?;
    history.write_all(b"\n")?;
    Ok(())
}
