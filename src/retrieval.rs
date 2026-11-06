use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::{
    contracts::{
        GraphEdge, IndexSnapshot, MatchKind, RetrievalHit, RetrievalMode, SearchCandidate,
        SearchRequest, SearchResponse, SourceRef,
    },
    error::{VrgError, VrgResult},
    semantic::{LocalEmbedder, cosine_top_k_refs},
    storage::DerivedStore,
    text_search,
};

const MAX_LIMIT: usize = 100;
const RRF_OFFSET: f32 = 60.0;

#[derive(Clone)]
struct RankedHit {
    candidate: SearchCandidate,
    score: f32,
    matches: Vec<MatchKind>,
    expanded: bool,
}

/// Return attributed hybrid retrieval results from the active derived generation.
/// A missing, unavailable, or corrupt local semantic model degrades visibly to text.
pub fn retrieve(
    store: &DerivedStore,
    embedder: Option<&LocalEmbedder>,
    request: &SearchRequest,
) -> VrgResult<SearchResponse> {
    retrieve_authorized(store, embedder, request, &|_, _| true)
}

/// Authorize direct candidates and every graph endpoint before ranking or traversal.
/// The callback must check the current source policy and exact source revision; a request's
/// root filter is only a relevance filter. Engine callers recheck results for source races.
pub fn retrieve_authorized(
    store: &DerivedStore,
    embedder: Option<&LocalEmbedder>,
    request: &SearchRequest,
    authorize: &dyn Fn(&SourceRef, &str) -> bool,
) -> VrgResult<SearchResponse> {
    retrieve_authorized_scoped(store, embedder, request, authorize, None)
}

/// A context refines already authorized sources to itself and its direct graph
/// endpoints. It never expands roots and is not a permission grant.
pub fn retrieve_authorized_scoped(
    store: &DerivedStore,
    embedder: Option<&LocalEmbedder>,
    request: &SearchRequest,
    authorize: &dyn Fn(&SourceRef, &str) -> bool,
    context_source: Option<&SourceRef>,
) -> VrgResult<SearchResponse> {
    retrieve_authorized_scoped_with_policy(
        store,
        embedder,
        request,
        authorize,
        context_source,
        false,
    )
}
pub fn retrieve_authorized_scoped_with_policy(
    store: &DerivedStore,
    embedder: Option<&LocalEmbedder>,
    request: &SearchRequest,
    authorize: &dyn Fn(&SourceRef, &str) -> bool,
    context_source: Option<&SourceRef>,
    prefer_implementation: bool,
) -> VrgResult<SearchResponse> {
    let limit = request.limit.clamp(1, MAX_LIMIT);
    if request.query.trim().is_empty() {
        return Err(VrgError::Search("query must not be empty".into()));
    }

    let _reader = store.acquire_reader_lease()?;
    let snapshot = store.load_current_catalog()?;
    let Some(snapshot) = snapshot else {
        if context_source.is_some() {
            return Err(VrgError::AccessDenied(
                "context source is unavailable or not current".into(),
            ));
        }
        let (mode, reason) = if request.use_semantic {
            (
                RetrievalMode::DegradedTextOnly,
                Some("no active index generation exists".to_owned()),
            )
        } else {
            (RetrievalMode::TextOnly, None)
        };
        return Ok(SearchResponse {
            generation_id: None,
            sync_state: crate::contracts::SearchSyncState::Ready,
            sync_reason: None,
            mode,
            degradation_reason: reason,
            hits: Vec::new(),
        });
    };

    let roots = request.root_ids.as_deref();
    if roots.is_some_and(|ids| ids.is_empty()) {
        return Ok(SearchResponse {
            generation_id: Some(snapshot.generation_id),
            sync_state: crate::contracts::SearchSyncState::Ready,
            sync_reason: None,
            mode: if request.use_semantic {
                RetrievalMode::DegradedTextOnly
            } else {
                RetrievalMode::TextOnly
            },
            degradation_reason: request
                .use_semantic
                .then(|| "the requested root filter is empty".to_owned()),
            hits: Vec::new(),
        });
    }
    let revisions: BTreeMap<SourceRef, String> = snapshot
        .documents
        .iter()
        .map(|document| (document.source.clone(), document.revision.clone()))
        .collect();
    let base_allowed = |source: &SourceRef, revision: &str| {
        root_allowed(source, roots)
            && revisions
                .get(source)
                .is_some_and(|current| current == revision)
            && authorize(source, revision)
    };
    let source_scope = if let Some(context) = context_source {
        if !revisions
            .get(context)
            .is_some_and(|revision| base_allowed(context, revision))
        {
            return Err(VrgError::AccessDenied(
                "context source is unavailable or not current".into(),
            ));
        }
        let mut sources = BTreeSet::from([context.clone()]);
        // Exactly one pass over edges incident to the original hub: never
        // traverse an admitted neighbor (in particular the global INDEX).
        for edge in &snapshot.edges {
            let neighbor = if &edge.from == context {
                edge.to.as_ref()
            } else if edge.to.as_ref() == Some(context) {
                Some(&edge.from)
            } else {
                None
            };
            if let Some(neighbor) = neighbor {
                let author_allowed = revisions
                    .get(&edge.from)
                    .is_some_and(|revision| base_allowed(&edge.from, revision));
                let target_allowed = edge.to.as_ref().is_some_and(|target| {
                    revisions
                        .get(target)
                        .is_some_and(|revision| base_allowed(target, revision))
                });
                if author_allowed && target_allowed {
                    sources.insert(neighbor.clone());
                }
            }
        }
        Some(sources)
    } else {
        None
    };
    // Identity has already selected and authorized the context. An explicit
    // leading alias is redundant query metadata, never a candidate grant.
    let query = context_source
        .and_then(|source| {
            snapshot
                .documents
                .iter()
                .find(|document| &document.source == source)
        })
        .map(|document| context_query(&request.query, document.frontmatter.get("aliases")))
        .unwrap_or(&request.query);
    let allowed = |source: &SourceRef, revision: &str| {
        source_scope
            .as_ref()
            .is_none_or(|sources| sources.contains(source))
            && base_allowed(source, revision)
    };
    let implementation_sources = snapshot
        .documents
        .iter()
        .filter(|document| implementation_source(document))
        .map(|document| document.source.clone())
        .collect::<BTreeSet<_>>();
    let mut semantic_reason = None;
    let mut semantic_ready = false;
    let mut semantic_pool = Vec::new();
    if request.use_semantic {
        match semantic_candidates(
            store,
            &snapshot,
            embedder,
            &request.query,
            roots,
            source_scope.as_ref(),
        ) {
            Ok(candidates) if candidates.is_empty() => {
                semantic_reason = Some(
                    "the active generation has no vectors for the configured local model".into(),
                )
            }
            Ok(candidates) => {
                semantic_ready = true;
                semantic_pool = candidates;
            }
            Err(error) => semantic_reason = Some(error.to_string()),
        }
    }
    let mut window = limit.saturating_mul(4).max(4);
    let mut ordered;
    loop {
        let text_candidates = text_search::search_scoped(
            store,
            query,
            roots,
            window.min(1000),
            source_scope.as_ref(),
        )?;
        let text_exhausted = text_candidates.len() < window.min(1000) || window >= 1000;
        let mut ranked = BTreeMap::<String, RankedHit>::new();
        add_text_candidates(&mut ranked, text_candidates);
        if prefer_implementation {
            add_semantic_candidates(
                &mut ranked,
                semantic_pool
                    .iter()
                    .filter(|candidate| implementation_sources.contains(&candidate.chunk.source))
                    .take(window)
                    .cloned()
                    .collect(),
            );
            add_semantic_candidates(
                &mut ranked,
                semantic_pool
                    .iter()
                    .filter(|candidate| !implementation_sources.contains(&candidate.chunk.source))
                    .take(window)
                    .cloned()
                    .collect(),
            );
        } else {
            add_semantic_candidates(
                &mut ranked,
                semantic_pool.iter().take(window).cloned().collect(),
            );
        }
        let mut candidates = ranked.into_values().collect::<Vec<_>>();
        candidates.sort_by(rank_order);
        ordered = select_authorized(
            store,
            candidates,
            limit,
            prefer_implementation,
            &implementation_sources,
            &allowed,
        )?;
        // Ranking is numerical/catalog-only. Refill the bounded initial window
        // after revocation/diversity filtering without authorizing the corpus.
        if ordered.len() >= limit || text_exhausted && window >= semantic_pool.len() {
            break;
        }
        window = window.saturating_mul(2).min(semantic_pool.len().max(1000));
    }
    if request.graph_hops > 0 {
        add_graph_expansion(
            store,
            &snapshot,
            &mut ordered,
            request.graph_hops.min(2),
            limit,
            roots,
            &allowed,
        )?;
        ordered.sort_by(rank_order);
        ordered = select_authorized(
            store,
            ordered,
            limit,
            prefer_implementation,
            &implementation_sources,
            &allowed,
        )?;
    }

    let selected_sources: BTreeSet<_> = ordered
        .iter()
        .take(limit)
        .map(|entry| &entry.candidate.chunk.source)
        .collect();
    let allowed_edges: Vec<GraphEdge> = snapshot
        .edges
        .iter()
        .filter(|_| !prefer_implementation || request.graph_hops > 0)
        .filter(|edge| {
            selected_sources.contains(&edge.from)
                || edge
                    .to
                    .as_ref()
                    .is_some_and(|to| selected_sources.contains(to))
        })
        .filter(|edge| {
            revisions
                .get(&edge.from)
                .is_some_and(|revision| allowed(&edge.from, revision))
                && edge.to.as_ref().is_none_or(|to| {
                    revisions
                        .get(to)
                        .is_some_and(|revision| allowed(to, revision))
                })
        })
        .cloned()
        .collect();
    let edges_by_source = group_edges(&allowed_edges);
    let hits = ordered
        .into_iter()
        .take(limit)
        .map(|entry| {
            let chunk = entry.candidate.chunk;
            let relations = edges_by_source
                .get(&chunk.source)
                .cloned()
                .unwrap_or_default();
            RetrievalHit {
                source: chunk.source,
                revision: chunk.revision,
                chunk_id: chunk.id,
                snippet: chunk.text,
                start_byte: chunk.start_byte,
                end_byte: chunk.end_byte,
                score: entry.score,
                matches: entry.matches,
                relations,
            }
        })
        .collect();

    let (mode, degradation_reason) = if request.use_semantic && semantic_ready {
        (RetrievalMode::Hybrid, None)
    } else if request.use_semantic {
        (RetrievalMode::DegradedTextOnly, semantic_reason)
    } else {
        (RetrievalMode::TextOnly, None)
    };

    if let Some(context) = context_source {
        if !revisions
            .get(context)
            .is_some_and(|revision| base_allowed(context, revision))
        {
            return Err(VrgError::AccessDenied(
                "context source changed during search".into(),
            ));
        }
    }
    Ok(SearchResponse {
        generation_id: Some(snapshot.generation_id),
        sync_state: crate::contracts::SearchSyncState::Ready,
        sync_reason: None,
        mode,
        degradation_reason,
        hits,
    })
}

fn semantic_candidates(
    store: &DerivedStore,
    _snapshot: &IndexSnapshot,
    embedder: Option<&LocalEmbedder>,
    query: &str,
    roots: Option<&[String]>,
    sources: Option<&BTreeSet<SourceRef>>,
) -> VrgResult<Vec<SearchCandidate>> {
    let embedder = embedder.ok_or_else(|| {
        VrgError::SemanticUnavailable("no local embedding model is configured".into())
    })?;
    let root_ids = roots.unwrap_or_default();
    let stored_vectors =
        store.vector_catalog_for_model_scoped(embedder.model_id(), root_ids, sources)?;
    let expected_chunk_ids = store.current_chunk_ids_scoped(root_ids, sources)?;
    let vector_chunk_ids = stored_vectors
        .iter()
        .map(|stored| stored.chunk.id.as_str())
        .collect::<BTreeSet<_>>();
    if expected_chunk_ids
        .iter()
        .any(|chunk_id| !vector_chunk_ids.contains(chunk_id.as_str()))
    {
        return Err(VrgError::SemanticUnavailable(
            "active generation has incomplete vectors for the configured local model".into(),
        ));
    }
    if stored_vectors.is_empty() {
        return Ok(Vec::new());
    }
    let query_vector = embedder.embed_query(query)?;
    let semantic_matches = cosine_top_k_refs(
        &query_vector,
        stored_vectors.iter().map(|v| {
            (
                v.chunk.id.as_str(),
                v.values.len() as u32,
                v.values.as_slice(),
            )
        }),
        embedder.model_id(),
        stored_vectors.len(),
    )?;
    let stored_by_chunk = stored_vectors
        .into_iter()
        .map(|stored| (stored.chunk.id.clone(), stored))
        .collect::<HashMap<_, _>>();
    let mut result = Vec::with_capacity(semantic_matches.len());
    for candidate in semantic_matches {
        let Some(stored) = stored_by_chunk.get(candidate.chunk_id.as_str()) else {
            continue;
        };
        result.push(SearchCandidate {
            chunk: stored.chunk.clone(),
            title: stored.title.clone(),
            score: candidate.score,
            kinds: vec![MatchKind::Semantic],
        });
    }
    Ok(result)
}

/// Removes at most one explicit identity prefix, case-sensitively. Absence,
/// malformed metadata, a middle/subword match, or an empty residual is a no-op.
fn context_query<'a>(query: &'a str, aliases: Option<&serde_json::Value>) -> &'a str {
    let Some(aliases) = aliases.and_then(serde_json::Value::as_array) else {
        return query;
    };
    let mut names = aliases
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter(|alias| !alias.is_empty() && alias.trim() == *alias)
        .collect::<Vec<_>>();
    names.sort_by_key(|alias| std::cmp::Reverse(alias.len()));
    for alias in names {
        if let Some(rest) = query.strip_prefix(alias) {
            if rest.is_empty() || rest.chars().all(char::is_whitespace) {
                return query;
            }
            if rest.starts_with(char::is_whitespace) {
                return rest.trim_start();
            }
        }
    }
    query
}

fn add_text_candidates(ranked: &mut BTreeMap<String, RankedHit>, candidates: Vec<SearchCandidate>) {
    for (rank, candidate) in candidates.into_iter().enumerate() {
        let exact = candidate
            .kinds
            .iter()
            .any(|kind| matches!(kind, MatchKind::ExactPath | MatchKind::ExactTitle));
        let rrf = 1.0 / (RRF_OFFSET + rank as f32 + 1.0);
        let score = rrf + if exact { 1.0 } else { 0.0 };
        insert_ranked(ranked, candidate, score, false);
    }
}

fn add_semantic_candidates(
    ranked: &mut BTreeMap<String, RankedHit>,
    candidates: Vec<SearchCandidate>,
) {
    for (rank, candidate) in candidates.into_iter().enumerate() {
        let score = 1.0 / (RRF_OFFSET + rank as f32 + 1.0);
        insert_ranked(ranked, candidate, score, false);
    }
}

fn insert_ranked(
    ranked: &mut BTreeMap<String, RankedHit>,
    candidate: SearchCandidate,
    score: f32,
    expanded: bool,
) {
    let id = candidate.chunk.id.clone();
    if let Some(existing) = ranked.get_mut(&id) {
        existing.score += score;
        for kind in candidate.kinds {
            if !existing.matches.contains(&kind) {
                existing.matches.push(kind);
            }
        }
        existing.expanded &= expanded;
    } else {
        ranked.insert(
            id,
            RankedHit {
                matches: candidate.kinds.clone(),
                candidate,
                score,
                expanded,
            },
        );
    }
}

fn add_graph_expansion(
    store: &DerivedStore,
    snapshot: &IndexSnapshot,
    ranked: &mut Vec<RankedHit>,
    max_hops: u8,
    limit: usize,
    roots: Option<&[String]>,
    authorize: &dyn Fn(&SourceRef, &str) -> bool,
) -> VrgResult<()> {
    if max_hops == 0 || limit == 0 {
        return Ok(());
    }
    let revisions: BTreeMap<_, _> = snapshot
        .documents
        .iter()
        .map(|document| (&document.source, document.revision.as_str()))
        .collect();
    let node_allowed = |source: &SourceRef| {
        root_allowed(source, roots)
            && revisions
                .get(source)
                .is_some_and(|revision| authorize(source, revision))
    };
    let titles = snapshot
        .documents
        .iter()
        .map(|document| (&document.source, document.title.as_str()))
        .collect::<HashMap<_, _>>();

    let mut known_sources = ranked
        .iter()
        .map(|entry| entry.candidate.chunk.source.clone())
        .collect::<BTreeSet<_>>();
    let mut known_chunks = ranked
        .iter()
        .map(|entry| entry.candidate.chunk.id.clone())
        .collect::<BTreeSet<_>>();
    let seeds = ranked
        .iter()
        .take(limit.min(20))
        .map(|entry| entry.candidate.chunk.source.clone())
        .collect::<Vec<_>>();
    let mut queue = seeds
        .into_iter()
        .map(|source| (source, 0_u8))
        .collect::<VecDeque<_>>();
    let mut expanded_count = 0usize;

    while let Some((source, depth)) = queue.pop_front() {
        if depth >= max_hops || expanded_count >= limit || !node_allowed(&source) {
            continue;
        }
        let mut adjacent = snapshot
            .edges
            .iter()
            .filter(|edge| edge.from == source || edge.to.as_ref() == Some(&source))
            .filter(|edge| node_allowed(&edge.from) && edge.to.as_ref().is_some_and(&node_allowed))
            .filter_map(|edge| {
                if edge.from == source {
                    edge.to.clone().map(|to| (to, edge.clone()))
                } else if edge.to.as_ref() == Some(&source) {
                    Some((edge.from.clone(), edge.clone()))
                } else {
                    None
                }
            })
            .filter(|(next, _)| node_allowed(next))
            .collect::<Vec<_>>();
        adjacent.sort_by(|a, b| a.0.cmp(&b.0));
        adjacent.dedup_by(|a, b| a.0 == b.0);

        for (next, relation) in adjacent {
            if known_sources.insert(next.clone()) {
                queue.push_back((next.clone(), depth + 1));
            }
            if ranked
                .iter()
                .any(|entry| entry.candidate.chunk.source == next)
            {
                continue;
            }
            let chunks = store.chunks_for_source(&next)?;
            let Some(chunk) = chunks.first() else {
                continue;
            };
            if !authorize(&chunk.source, &chunk.revision) {
                continue;
            }
            if !known_chunks.insert(chunk.id.clone()) {
                continue;
            }
            let title = titles
                .get(&chunk.source)
                .copied()
                .unwrap_or_default()
                .to_owned();
            ranked.push(RankedHit {
                candidate: SearchCandidate {
                    chunk: chunk.clone(),
                    title,
                    score: 0.0,
                    kinds: vec![MatchKind::GraphExpansion],
                },
                score: 1.0 / (RRF_OFFSET + 1.0 + depth as f32),
                matches: vec![MatchKind::GraphExpansion],
                expanded: true,
            });
            let _ = relation;
            expanded_count += 1;
            if expanded_count >= limit {
                break;
            }
        }
    }
    Ok(())
}

fn group_edges(edges: &[GraphEdge]) -> HashMap<SourceRef, Vec<GraphEdge>> {
    let mut grouped = HashMap::new();
    for edge in edges {
        grouped
            .entry(edge.from.clone())
            .or_insert_with(Vec::new)
            .push(edge.clone());
        if let Some(to) = &edge.to {
            grouped
                .entry(to.clone())
                .or_insert_with(Vec::new)
                .push(edge.clone());
        }
    }
    grouped
}

fn root_allowed(source: &SourceRef, roots: Option<&[String]>) -> bool {
    roots.is_none_or(|allowed| allowed.iter().any(|root| root == &source.root_id))
}

fn rank_order(left: &RankedHit, right: &RankedHit) -> std::cmp::Ordering {
    let left_exact = left
        .matches
        .iter()
        .any(|kind| matches!(kind, MatchKind::ExactPath | MatchKind::ExactTitle));
    let right_exact = right
        .matches
        .iter()
        .any(|kind| matches!(kind, MatchKind::ExactPath | MatchKind::ExactTitle));
    right_exact
        .cmp(&left_exact)
        .then_with(|| right.score.total_cmp(&left.score))
        .then_with(|| {
            left.candidate
                .chunk
                .source
                .cmp(&right.candidate.chunk.source)
        })
        .then_with(|| {
            left.candidate
                .chunk
                .ordinal
                .cmp(&right.candidate.chunk.ordinal)
        })
        .then_with(|| left.expanded.cmp(&right.expanded))
}

/// Relevance metadata only: root/revision/privacy grants remain independent.
/// Documentation/web markup stays indexed and available in the other bucket.
fn implementation_source(document: &crate::contracts::StoredDocument) -> bool {
    if document
        .frontmatter
        .get("source_format")
        .and_then(serde_json::Value::as_str)
        != Some("original_utf8")
    {
        return false;
    }
    let Some(relative) = document
        .frontmatter
        .get("original_relative_path")
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    match document
        .frontmatter
        .get("source_role")
        .and_then(serde_json::Value::as_str)
    {
        Some("implementation") => return true,
        Some("documentation" | "configuration") => return false,
        _ => (),
    }
    let extension = std::path::Path::new(relative)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "rs" | "py"
            | "pyi"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "c"
            | "cc"
            | "cpp"
            | "cxx"
            | "h"
            | "hpp"
            | "go"
            | "java"
            | "cs"
            | "kt"
            | "kts"
            | "swift"
            | "rb"
            | "php"
            | "lua"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            | "sql"
            | "r"
            | "jl"
            | "ex"
            | "exs"
            | "scala"
            | "hs"
            | "ml"
            | "mli"
            | "clj"
            | "cljs"
            | "cljc"
            | "f"
            | "f90"
            | "cu"
            | "glsl"
            | "hlsl"
            | "wgsl"
            | "vue"
            | "svelte"
            | ""
    )
}
fn select_authorized(
    store: &DerivedStore,
    mut candidates: Vec<RankedHit>,
    limit: usize,
    prefer_implementation: bool,
    implementation_sources: &BTreeSet<SourceRef>,
    authorize: &dyn Fn(&SourceRef, &str) -> bool,
) -> VrgResult<Vec<RankedHit>> {
    let exact = |entry: &RankedHit| {
        entry
            .matches
            .iter()
            .any(|kind| matches!(kind, MatchKind::ExactPath | MatchKind::ExactTitle))
    };
    let mut indices = Vec::with_capacity(candidates.len());
    if prefer_implementation {
        indices.extend(
            candidates
                .iter()
                .enumerate()
                .filter(|(_, entry)| exact(entry))
                .map(|(index, _)| index),
        );
        let mut code = Vec::new();
        let mut other = Vec::new();
        for (index, entry) in candidates
            .iter()
            .enumerate()
            .filter(|(_, entry)| !exact(entry))
        {
            if implementation_sources.contains(&entry.candidate.chunk.source) {
                code.push(index);
            } else {
                other.push(index);
            }
        }
        // First candidate per real source precedes that source's later chunks;
        // preserve the fused rank within each pass rather than inventing scores.
        let diverse = |items: Vec<usize>| {
            let mut sources = BTreeSet::new();
            let mut first = Vec::new();
            let mut later = Vec::new();
            for index in items {
                if sources.insert(candidates[index].candidate.chunk.source.clone()) {
                    first.push(index);
                } else {
                    later.push(index);
                }
            }
            first.extend(later);
            first
        };
        let code = diverse(code);
        let other = diverse(other);
        let mut at_code = 0;
        let mut at_other = 0;
        while at_code < code.len() || at_other < other.len() {
            for _ in 0..4 {
                if let Some(index) = code.get(at_code) {
                    indices.push(*index);
                    at_code += 1;
                }
            }
            if let Some(index) = other.get(at_other) {
                indices.push(*index);
                at_other += 1;
            }
        }
    } else {
        indices.extend(0..candidates.len());
    }
    let mut selected = Vec::new();
    let mut chunks = BTreeSet::new();
    let mut sources = BTreeSet::new();
    let mut texts = BTreeSet::new();
    let passes = if prefer_implementation { 2 } else { 1 };
    for pass in 0..passes {
        for index in &indices {
            let entry = &mut candidates[*index];
            if chunks.contains(&entry.candidate.chunk.id) {
                continue;
            }
            let diverse = prefer_implementation && pass == 0 && !exact(entry);
            if diverse && sources.contains(&entry.candidate.chunk.source) {
                continue;
            }
            // Vector catalogs intentionally omit bodies. Load only an actually
            // proposed result, still internal until the exact fresh grant below.
            if entry.candidate.chunk.text.is_empty() {
                entry.candidate.chunk =
                    store
                        .chunk_by_id(&entry.candidate.chunk.id)?
                        .ok_or_else(|| {
                            VrgError::Storage("vector references a missing active chunk".into())
                        })?;
            }
            if diverse && texts.contains(&entry.candidate.chunk.text) {
                continue;
            }
            if !authorize(
                &entry.candidate.chunk.source,
                &entry.candidate.chunk.revision,
            ) {
                continue;
            }
            chunks.insert(entry.candidate.chunk.id.clone());
            sources.insert(entry.candidate.chunk.source.clone());
            texts.insert(entry.candidate.chunk.text.clone());
            selected.push(entry.clone());
            if selected.len() >= limit {
                return Ok(selected);
            }
        }
    }
    Ok(selected)
}
