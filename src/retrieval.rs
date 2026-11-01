use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::{
    contracts::{
        GraphEdge, IndexSnapshot, MatchKind, RetrievalHit, RetrievalMode, SearchCandidate,
        SearchRequest, SearchResponse, SourceRef,
    },
    error::{VrgError, VrgResult},
    semantic::{LocalEmbedder, cosine_top_k},
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
    let limit = request.limit.clamp(1, MAX_LIMIT);
    if request.query.trim().is_empty() {
        return Err(VrgError::Search("query must not be empty".into()));
    }

    let snapshot = store.load_current_snapshot()?;
    let Some(snapshot) = snapshot else {
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
    let text_candidates =
        text_search::search(store, &request.query, roots, limit.saturating_mul(4))?;
    let mut ranked = BTreeMap::<String, RankedHit>::new();
    add_text_candidates(&mut ranked, text_candidates);

    let mut semantic_reason = None;
    let mut semantic_ready = false;
    if request.use_semantic {
        match semantic_candidates(
            store,
            &snapshot,
            embedder,
            &request.query,
            roots,
            limit.saturating_mul(4),
        ) {
            Ok(candidates) if candidates.is_empty() => {
                semantic_reason = Some(
                    "the active generation has no vectors for the configured local model".into(),
                );
            }
            Ok(candidates) => {
                add_semantic_candidates(&mut ranked, candidates);
                semantic_ready = true;
            }
            Err(error) => semantic_reason = Some(error.to_string()),
        }
    }

    let mut ordered = ranked.into_values().collect::<Vec<_>>();
    ordered.sort_by(rank_order);

    if request.graph_hops > 0 {
        add_graph_expansion(
            &snapshot,
            &mut ordered,
            request.graph_hops.min(2),
            limit,
            roots,
        );
        ordered.sort_by(rank_order);
    }

    let edges_by_source = group_edges(&snapshot.edges);
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
    snapshot: &IndexSnapshot,
    embedder: Option<&LocalEmbedder>,
    query: &str,
    roots: Option<&[String]>,
    limit: usize,
) -> VrgResult<Vec<SearchCandidate>> {
    let embedder = embedder.ok_or_else(|| {
        VrgError::SemanticUnavailable("no local embedding model is configured".into())
    })?;
    let root_ids = roots.unwrap_or_default();
    let stored_vectors = store.vectors_for_model(embedder.model_id(), root_ids)?;
    let expected_chunk_ids = snapshot
        .chunks
        .iter()
        .filter(|chunk| {
            root_ids.is_empty()
                || root_ids
                    .iter()
                    .any(|root_id| root_id == &chunk.source.root_id)
        })
        .map(|chunk| chunk.id.as_str())
        .collect::<BTreeSet<_>>();
    let vector_chunk_ids = stored_vectors
        .iter()
        .map(|stored| stored.chunk.id.as_str())
        .collect::<BTreeSet<_>>();
    if expected_chunk_ids
        .iter()
        .any(|chunk_id| !vector_chunk_ids.contains(chunk_id))
    {
        return Err(VrgError::SemanticUnavailable(
            "active generation has incomplete vectors for the configured local model".into(),
        ));
    }
    let vectors = stored_vectors
        .iter()
        .map(|stored| crate::contracts::VectorEmbedding {
            chunk_id: stored.chunk.id.clone(),
            model_id: stored.model_id.clone(),
            dimensions: stored.values.len() as u32,
            values: stored.values.clone(),
        })
        .collect::<Vec<_>>();
    if vectors.is_empty() {
        return Ok(Vec::new());
    }

    let query_vector = embedder.embed_query(query)?;
    let semantic_matches = cosine_top_k(&query_vector, &vectors, embedder.model_id(), limit)?;
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
    snapshot: &IndexSnapshot,
    ranked: &mut Vec<RankedHit>,
    max_hops: u8,
    limit: usize,
    roots: Option<&[String]>,
) {
    if max_hops == 0 || limit == 0 {
        return;
    }
    let mut chunks_by_source = HashMap::<SourceRef, Vec<_>>::new();
    for chunk in &snapshot.chunks {
        if root_allowed(&chunk.source, roots) {
            chunks_by_source
                .entry(chunk.source.clone())
                .or_default()
                .push(chunk);
        }
    }
    for chunks in chunks_by_source.values_mut() {
        chunks.sort_by_key(|chunk| chunk.ordinal);
    }
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
        if depth >= max_hops || expanded_count >= limit {
            continue;
        }
        let mut adjacent = snapshot
            .edges
            .iter()
            .filter_map(|edge| {
                if edge.from == source {
                    edge.to.clone().map(|to| (to, edge.clone()))
                } else if edge.to.as_ref() == Some(&source) {
                    Some((edge.from.clone(), edge.clone()))
                } else {
                    None
                }
            })
            .filter(|(next, _)| root_allowed(next, roots))
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
            let Some(chunk) = chunks_by_source.get(&next).and_then(|items| items.first()) else {
                continue;
            };
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
                    chunk: (*chunk).clone(),
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
