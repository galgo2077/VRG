use crate::{
    config::VrgConfig,
    contracts::WritePlan,
    error::{VrgError, VrgResult},
    vault,
};

const MAX_PROPOSED_BYTES: usize = 1_000_000;

pub fn prepare_plan(
    config: &VrgConfig,
    root_id: &str,
    relative_path: &str,
    proposed_markdown: String,
    reason: String,
) -> VrgResult<WritePlan> {
    if proposed_markdown.len() > MAX_PROPOSED_BYTES {
        return Err(VrgError::InvalidConfig(
            "proposed note exceeds the review-plan size limit".into(),
        ));
    }
    if reason.trim().is_empty() || reason.len() > 2048 {
        return Err(VrgError::InvalidConfig(
            "write plan requires a bounded non-empty reason".into(),
        ));
    }
    let source = vault::read_source(config, root_id, relative_path, MAX_PROPOSED_BYTES)?;
    Ok(WritePlan {
        source: source.source,
        expected_revision: source.revision,
        proposed_markdown,
        reason,
    })
}

/// Source-note application is deliberately unavailable until separately
/// authorized and certified; rollback/journaling of derived state is separate.
pub fn apply_disabled(_plan: &WritePlan) -> VrgResult<()> {
    Err(VrgError::WritesDisabled(
        "source-note writes require separate authorization and certification".into(),
    ))
}
