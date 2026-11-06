//! Native automatic structural memory using the existing adaptive validation/store.
//! The caller owns the native publication lock; this module never writes source notes.
use crate::{
    adaptive,
    config::VrgConfig,
    contracts::SourceDocument,
    error::{VrgError, VrgResult},
    storage::DerivedStore,
    vault,
};
use sha2::{Digest, Sha256};

/// Invoke only after native projection/index publication under its sync lock.
/// `automatic_authorized` must come from the pinned project discovery policy.
/// Derived compact documents are re-read through the authorized vault before and
/// after validation. Generation, exact revisions, scope, pins and conflicts are
/// checked by the same adaptive contract used for explicit proposals.
pub fn apply_under_sync(
    config: &VrgConfig,
    store: &DerivedStore,
    generation: &str,
    documents: &[SourceDocument],
    automatic_authorized: bool,
) -> VrgResult<usize> {
    if !automatic_authorized || !config.adaptive_memory.enabled {
        return Ok(0);
    }
    if store.current_generation_id()?.as_deref() != Some(generation) {
        return Err(VrgError::Conflict(
            "automatic memory generation changed".into(),
        ));
    }
    let (project, scope) = crate::engine::adaptive_scope(config)?;
    let existing = adaptive::load_memories(store.load_adaptive_state()?.as_deref())?;
    let created_at = utc_now()?;
    // Unchanged snapshot bodies contain normalized Markdown text, not the
    // original compact document markup. Its exact revision plus the hash of a
    // previously validated full summary proves identity without rereading every
    // unrelated module's code dependencies on each no-op event.
    let pending = documents
        .iter()
        .filter(|document| {
            if !document
                .source
                .relative_path
                .starts_with("incremental-v2/memory/")
            {
                return false;
            }
            let sid = adaptive::source_id(&document.source);
            !existing.iter().any(|memory| {
                memory.project_id == project
                    && memory.privacy_scope_id == scope
                    && memory.memory_id
                        == format!("mechanical:{}", hex::encode(Sha256::digest(sid.as_bytes())))
                    && memory.sources.len() == 1
                    && memory.sources[0].source_id == sid
                    && memory.sources[0].revision_sha256 == document.revision
                    && memory.sources[0].span_start == 0
                    && memory.summary.as_ref().is_some_and(|summary| {
                        memory.sources[0].span_end == summary.len()
                            && hex::encode(Sha256::digest(summary.as_bytes())) == document.revision
                    })
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let planned = adaptive::mechanical_proposals(
        &project,
        &scope,
        generation,
        &created_at,
        &pending,
        &existing,
    )?;
    let requested = planned
        .iter()
        .flat_map(|proposal| &proposal.operations)
        .flat_map(|operation| &operation.sources)
        .map(|reference| reference.source_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if requested.is_empty() {
        return Ok(0);
    }
    let mut current = Vec::new();
    for document in documents {
        if !requested.contains(adaptive::source_id(&document.source).as_str()) {
            continue;
        }
        let authorized = vault::read_source(
            config,
            &document.source.root_id,
            &document.source.relative_path,
            4 * 1024 * 1024,
        )?;
        if authorized.revision != document.revision {
            return Err(VrgError::Conflict(
                "automatic memory source revision changed".into(),
            ));
        }
        current.push(authorized);
    }
    let proposals = adaptive::mechanical_proposals(
        &project,
        &scope,
        generation,
        &created_at,
        &current,
        &existing,
    )?;
    let mut applied = 0;
    let mut latest = existing;
    for proposal in proposals {
        let context = adaptive::ValidationContext {
            project_id: project.clone(),
            privacy_scope_id: scope.clone(),
            generation_id: generation.to_owned(),
            sources: current.clone(),
            existing: latest,
        };
        let validated = adaptive::validate_proposal(&proposal, &context)?;
        for reference in proposal.operations.iter().flat_map(|op| &op.sources) {
            let source = adaptive::parse_source_id(&reference.source_id).ok_or_else(|| {
                VrgError::AccessDenied("invalid mechanical source identity".into())
            })?;
            let fresh = vault::read_source(
                config,
                &source.root_id,
                &source.relative_path,
                4 * 1024 * 1024,
            )?;
            if fresh.revision != reference.revision_sha256 {
                return Err(VrgError::Conflict(
                    "mechanical source changed during validation".into(),
                ));
            }
        }
        let state = serde_json::to_string(&validated.memories)
            .map_err(|e| VrgError::Storage(e.to_string()))?;
        if store.apply_adaptive_state(
            generation,
            &proposal.proposal_id,
            &validated.fingerprint,
            &state,
        )? {
            applied += proposal.operations.len();
        }
        latest = validated.memories;
    }
    Ok(applied)
}

fn utc_now() -> VrgResult<String> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| VrgError::Storage(e.to_string()))?
        .as_secs();
    let mut days = seconds / 86400;
    let mut year = 1970u64;
    loop {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let length = if leap { 366 } else { 365 };
        if days < length {
            break;
        }
        days -= length;
        year += 1;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0usize;
    while month < 11 && days >= months[month] {
        days -= months[month];
        month += 1;
    }
    let daytime = seconds % 86400;
    Ok(format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        month + 1,
        days + 1,
        daytime / 3600,
        daytime / 60 % 60,
        daytime % 60
    ))
}
