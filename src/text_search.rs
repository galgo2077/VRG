use std::collections::{BTreeMap, BTreeSet};

use crate::{
    contracts::{MatchKind, SearchCandidate, SourceRef},
    error::VrgResult,
    storage::DerivedStore,
};

const MAX_RESULTS: usize = 1000;

/// Runs deterministic path/title matching before lexical FTS5 search.
/// Repeated hits retain every match kind while sharing one citation per chunk.
pub fn search(
    store: &DerivedStore,
    query: &str,
    root_ids: Option<&[String]>,
    limit: usize,
) -> VrgResult<Vec<SearchCandidate>> {
    search_scoped(store, query, root_ids, limit, None)
}

pub fn search_scoped(
    store: &DerivedStore, query: &str, root_ids: Option<&[String]>, limit: usize,
    sources: Option<&BTreeSet<SourceRef>>,
) -> VrgResult<Vec<SearchCandidate>> {
    let query = query.trim();
    if query.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let bounded_limit = limit.min(MAX_RESULTS);
    let roots = root_ids.unwrap_or(&[]);
    let exact = store.exact_search_rows_scoped(query, roots, bounded_limit, sources)?;
    let lexical = store.fts_search_rows_scoped(query, roots, bounded_limit, sources)?;
    let mut merged = BTreeMap::<String, SearchCandidate>::new();

    for candidate in exact.into_iter().chain(lexical) {
        let key = candidate.chunk.id.clone();
        match merged.get_mut(&key) {
            Some(existing) => {
                for kind in candidate.kinds {
                    if !existing.kinds.contains(&kind) {
                        existing.kinds.push(kind);
                    }
                }
                existing.score = existing.score.max(candidate.score);
            }
            None => {
                merged.insert(key, candidate);
            }
        }
    }

    let mut candidates: Vec<_> = merged.into_values().collect();
    candidates.sort_by(|left, right| {
        candidate_priority(right)
            .cmp(&candidate_priority(left))
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| left.chunk.source.root_id.cmp(&right.chunk.source.root_id))
            .then_with(|| {
                left.chunk
                    .source
                    .relative_path
                    .cmp(&right.chunk.source.relative_path)
            })
            .then_with(|| left.chunk.ordinal.cmp(&right.chunk.ordinal))
    });
    candidates.truncate(bounded_limit);
    Ok(candidates)
}

fn candidate_priority(candidate: &SearchCandidate) -> u8 {
    if candidate.kinds.contains(&MatchKind::ExactPath) {
        3
    } else if candidate.kinds.contains(&MatchKind::ExactTitle) {
        2
    } else if candidate.kinds.contains(&MatchKind::FullText) {
        1
    } else {
        0
    }
}
