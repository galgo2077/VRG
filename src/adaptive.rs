//! Structured, local assistant proposals. Source bytes and authorization remain authoritative.
//!
//! This module never writes sources and never executes content returned by retrieval.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    contracts::{SearchResponse, SourceDocument, SourceRef},
    error::{VrgError, VrgResult},
};

const MAX_PROPOSAL_BYTES: usize = 256 * 1024;
const MAX_SNIPPET_BYTES: usize = 512;

fn present_value<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub schema_version: u32,
    pub proposal_id: String,
    pub project_id: String,
    pub privacy_scope_id: String,
    pub expected_generation: String,
    pub created_at: String,
    pub producer: Producer,
    pub operations: Vec<Operation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Producer {
    pub kind: ProducerKind,
    pub identity: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProducerKind {
    Codex,
    Human,
    /// Bounded native structural summaries; never represents an assistant review.
    Mechanical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub operation_id: String,
    pub action: Action,
    pub memory_id: String,
    pub sources: Vec<SourceSpan>,
    pub reason_code: ReasonCode,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub target_tier: Option<MemoryTier>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub summary: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub protected_facts: Option<Vec<ProtectedFact>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub epistemic_state: Option<EpistemicState>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub confidence: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub needs_expansion: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Keep,
    SetTier,
    UpsertSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    CurrentConstraint,
    ReusableSolution,
    ActiveWork,
    InfrequentButValid,
    SupersededHistory,
    DuplicateKnowledge,
    UncertainKeepDetail,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryTier {
    HOT,
    WARM,
    COLD,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicState {
    Proposed,
    Verified,
    Conflicted,
    Superseded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    /// Canonical identifier returned by `source_id`, not a filesystem path.
    pub source_id: String,
    pub revision_sha256: String,
    /// UTF-8 offsets in the exact raw Markdown revision, never in a previous summary.
    pub span_start: usize,
    pub span_end: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedFact {
    pub fact: String,
    pub source_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredMemory {
    pub memory_id: String,
    pub project_id: String,
    pub privacy_scope_id: String,
    pub tier: MemoryTier,
    pub pinned: bool,
    pub sources: Vec<SourceSpan>,
    /// Entire exact revisions covered by proposal spans; partial spans cannot replace other chunks.
    pub fully_covered_sources: Vec<String>,
    pub summary: Option<String>,
    pub protected_facts: Vec<ProtectedFact>,
    pub epistemic_state: EpistemicState,
    pub confidence: f64,
    pub needs_expansion: bool,
    pub reason_code: ReasonCode,
}

/// Callers must read sources through the current authorized vault configuration immediately
/// before validation and hold the publication lock through applying the returned state.
pub struct ValidationContext {
    pub project_id: String,
    pub privacy_scope_id: String,
    pub generation_id: String,
    pub sources: Vec<SourceDocument>,
    pub existing: Vec<StoredMemory>,
}

pub struct ValidatedProposal {
    pub memories: Vec<StoredMemory>,
    pub fingerprint: String,
}

pub fn source_id(source: &SourceRef) -> String {
    fn escape(value: &str) -> String {
        value.replace('%', "%25").replace(':', "%3A")
    }
    format!(
        "{}:{}",
        escape(&source.root_id),
        escape(&source.relative_path)
    )
}

/// Decode only the canonical percent-escaped source identity, never an OS path directly.
pub fn parse_source_id(id: &str) -> Option<SourceRef> {
    fn decode(value: &str) -> Option<String> {
        let mut bytes = Vec::with_capacity(value.len());
        let mut input = value.bytes();
        while let Some(byte) = input.next() {
            if byte == b'%' {
                let high = (input.next()? as char).to_digit(16)?;
                let low = (input.next()? as char).to_digit(16)?;
                bytes.push((high * 16 + low) as u8);
            } else {
                bytes.push(byte);
            }
        }
        String::from_utf8(bytes).ok()
    }
    let (root, path) = id.split_once(':')?;
    let source = SourceRef {
        root_id: decode(root)?,
        relative_path: decode(path)?,
    };
    if source.root_id.is_empty()
        || source.relative_path.is_empty()
        || source.root_id.contains('\0')
        || source.relative_path.contains('\0')
        || source_id(&source) != id
    {
        return None;
    }
    Some(source)
}

pub fn load_memories(state_json: Option<&str>) -> VrgResult<Vec<StoredMemory>> {
    match state_json {
        Some(json) => serde_json::from_str(json).map_err(|_| invalid("invalid adaptive state")),
        None => Ok(Vec::new()),
    }
}

pub fn parse_proposal(bytes: &[u8]) -> VrgResult<Proposal> {
    if bytes.is_empty() || bytes.len() > MAX_PROPOSAL_BYTES {
        return Err(invalid("proposal exceeds the bounded input limit"));
    }
    let proposal: Proposal = serde_json::from_slice(bytes)
        .map_err(|_| invalid("proposal does not match the strict v1 structure"))?;
    validate_shape(&proposal)?;
    Ok(proposal)
}

pub fn validate_proposal(
    proposal: &Proposal,
    context: &ValidationContext,
) -> VrgResult<ValidatedProposal> {
    validate_shape(proposal)?;
    if proposal.project_id != context.project_id
        || proposal.privacy_scope_id != context.privacy_scope_id
    {
        return Err(VrgError::AccessDenied(
            "proposal scope does not match the active project".into(),
        ));
    }
    if proposal.expected_generation != context.generation_id {
        return Err(VrgError::Conflict("proposal generation is obsolete".into()));
    }
    let mut sources = BTreeMap::new();
    for document in &context.sources {
        if sources
            .insert(source_id(&document.source), document)
            .is_some()
        {
            return Err(invalid("source identifiers must be unambiguous"));
        }
    }
    let mut memories: BTreeMap<String, StoredMemory> = context
        .existing
        .iter()
        .cloned()
        .map(|m| (m.memory_id.clone(), m))
        .collect();
    if memories.len() != context.existing.len() {
        return Err(invalid("duplicate memory identity in existing state"));
    }
    if matches!(proposal.producer.kind, ProducerKind::Mechanical) {
        validate_mechanical_proposal(proposal, context, &sources)?;
    }
    for operation in &proposal.operations {
        let mut excerpts = BTreeMap::<String, Vec<&str>>::new();
        for reference in &operation.sources {
            let document = sources.get(&reference.source_id).ok_or_else(|| {
                VrgError::AccessDenied("proposal references an unauthorized source".into())
            })?;
            if document.revision != reference.revision_sha256
                || hex::encode(Sha256::digest(document.raw_markdown.as_bytes()))
                    != document.revision
            {
                return Err(VrgError::Conflict(
                    "proposal source revision is obsolete".into(),
                ));
            }
            let text = document
                .raw_markdown
                .get(reference.span_start..reference.span_end)
                .filter(|text| !text.is_empty())
                .ok_or_else(|| invalid("source span must be nonempty, bounded UTF-8"))?;
            excerpts
                .entry(reference.source_id.clone())
                .or_default()
                .push(text);
        }
        let previous = memories.get(&operation.memory_id);
        if previous.is_some_and(|m| {
            m.project_id != context.project_id || m.privacy_scope_id != context.privacy_scope_id
        }) {
            return Err(VrgError::AccessDenied(
                "memory belongs to another scope".into(),
            ));
        }
        let constraint = operation.reason_code == ReasonCode::CurrentConstraint;
        let pinned = constraint || previous.is_some_and(|m| m.pinned);
        let requested_tier = operation
            .target_tier
            .clone()
            .or_else(|| previous.map(|m| m.tier.clone()))
            .unwrap_or(MemoryTier::WARM);
        if pinned && requested_tier != MemoryTier::HOT && operation.target_tier.is_some() {
            return Err(invalid(
                "a pinned constraint cannot be demoted by a proposal",
            ));
        }
        let tier = if pinned {
            MemoryTier::HOT
        } else {
            requested_tier
        };
        let mut memory = if let Some(previous) = previous {
            previous.clone()
        } else {
            StoredMemory {
                memory_id: operation.memory_id.clone(),
                project_id: context.project_id.clone(),
                privacy_scope_id: context.privacy_scope_id.clone(),
                tier: tier.clone(),
                pinned,
                sources: operation.sources.clone(),
                fully_covered_sources: Vec::new(),
                summary: None,
                protected_facts: Vec::new(),
                epistemic_state: EpistemicState::Proposed,
                confidence: 0.0,
                needs_expansion: true,
                reason_code: operation.reason_code.clone(),
            }
        };
        // Changing source references invalidates an earlier summary even for set_tier/keep.
        if memory.sources != operation.sources {
            memory.summary = None;
            memory.protected_facts.clear();
            memory.needs_expansion = true;
            memory.epistemic_state = EpistemicState::Proposed;
        }
        memory.fully_covered_sources = operation
            .sources
            .iter()
            .filter(|reference| {
                reference.span_start == 0
                    && sources
                        .get(&reference.source_id)
                        .is_some_and(|document| reference.span_end == document.raw_markdown.len())
            })
            .map(|reference| reference.source_id.clone())
            .collect();
        memory.sources = operation.sources.clone();
        memory.tier = tier;
        memory.pinned = pinned;
        memory.reason_code = operation.reason_code.clone();
        if operation.action == Action::UpsertSummary {
            let summary = operation.summary.as_ref().expect("shape validated");
            let facts = operation.protected_facts.as_ref().expect("shape validated");
            for fact in facts {
                let original = excerpts
                    .get(&fact.source_id)
                    .ok_or_else(|| invalid("protected fact has no attributed source span"))?;
                // Fact anchors are exact source fragments. This checks attribution only, never truth.
                if !original.iter().any(|text| text.contains(&fact.fact)) {
                    return Err(invalid(
                        "protected fact must be an exact fragment of its source span",
                    ));
                }
            }
            let critical: Vec<&str> = excerpts
                .values()
                .flatten()
                .flat_map(|text| critical_clauses(text))
                .collect();
            let lower_summary = summary.to_lowercase();
            let covered = !facts.is_empty()
                && facts
                    .iter()
                    .all(|fact| lower_summary.contains(&fact.fact.to_lowercase()))
                && critical
                    .iter()
                    .all(|clause| lower_summary.contains(&clause.to_lowercase()));
            // Semantic confidence is only an assistant assertion. Missing anchors, conflicts, and
            // omitted conditions always require exact-source expansion; hashes cannot certify meaning.
            let epistemic = operation.epistemic_state.clone().expect("shape validated");
            memory.summary = Some(summary.clone());
            memory.protected_facts = facts.clone();
            memory.confidence = operation.confidence.expect("shape validated");
            memory.needs_expansion = operation.needs_expansion.expect("shape validated")
                || !covered
                || matches!(
                    epistemic,
                    EpistemicState::Conflicted | EpistemicState::Superseded
                )
                || operation.reason_code == ReasonCode::UncertainKeepDetail;
            memory.epistemic_state = epistemic;
        }
        memories.insert(memory.memory_id.clone(), memory);
    }
    let fingerprint = hex::encode(Sha256::digest(
        serde_json::to_vec(proposal).map_err(|_| invalid("proposal serialization failed"))?,
    ));
    Ok(ValidatedProposal {
        memories: memories.into_values().collect(),
        fingerprint,
    })
}


/// Native mechanical producers can summarize only their exact, authorized derived
/// memory documents. They cannot create authority, certify truth, modify pins,
/// erase conflicts, or replace assistant/human protected knowledge.
fn validate_mechanical_proposal(
    proposal: &Proposal,
    context: &ValidationContext,
    sources: &BTreeMap<String, &SourceDocument>,
) -> VrgResult<()> {
    if proposal.producer.identity != "vrg-native-code-synthesis-v1" {
        return Err(VrgError::AccessDenied("unknown mechanical producer".into()));
    }
    for op in &proposal.operations {
        if op.action != Action::UpsertSummary || op.sources.len() != 1
            || op.reason_code != ReasonCode::ActiveWork
            || op.target_tier.as_ref().is_some_and(|tier| *tier != MemoryTier::WARM)
            || op.epistemic_state != Some(EpistemicState::Proposed)
            || op.confidence != Some(0.0) || op.needs_expansion != Some(true)
            || op.protected_facts.as_ref().is_none_or(|facts| !facts.is_empty())
        {
            return Err(VrgError::AccessDenied("mechanical summaries require bounded proposed structural evidence".into()));
        }
        let reference = &op.sources[0];
        let doc = sources.get(&reference.source_id).ok_or_else(|| invalid("mechanical source unavailable"))?;
        let expected_id = format!("mechanical:{}", hex::encode(Sha256::digest(reference.source_id.as_bytes())));
        if op.memory_id != expected_id || !doc.source.relative_path.starts_with("incremental-v2/memory/")
            || !doc.raw_markdown.starts_with("# Structural memory\n\nProducer: native_mechanical\nMethod: deterministic_structural_synthesis\n")
            || op.summary.as_deref() != Some(doc.raw_markdown.as_str())
            || reference.span_start != 0 || reference.span_end != doc.raw_markdown.len()
        {
            return Err(VrgError::AccessDenied("mechanical source or summary exceeds its derived namespace".into()));
        }
        if context.existing.iter().find(|m| m.memory_id == op.memory_id).is_some_and(|memory| {
            memory.pinned || memory.epistemic_state != EpistemicState::Proposed
                || !memory.protected_facts.is_empty()
                || memory.summary.as_ref().is_some_and(|text| !text.starts_with("# Structural memory\n\nProducer: native_mechanical\n"))
        }) {
            return Err(VrgError::AccessDenied("mechanical producer cannot replace protected or conflicting memory".into()));
        }
    }
    Ok(())
}

/// Build native proposals only when stable compact-document bytes have changed.
/// Callers still require explicit adaptive opt-in, current authorized source reads,
/// the native generation lock and validate_proposal before applying any proposal.
/// No age-based demotion, deletion, source writes or provider calls occur here.
pub fn mechanical_proposals(
    project_id: &str,
    privacy_scope_id: &str,
    generation_id: &str,
    created_at: &str,
    documents: &[SourceDocument],
    existing: &[StoredMemory],
) -> VrgResult<Vec<Proposal>> {
    let mut operations = Vec::new();
    let mut docs = documents.iter().collect::<Vec<_>>();
    docs.sort_by(|a, b| a.source.cmp(&b.source));
    for doc in docs {
        if !doc.source.relative_path.starts_with("incremental-v2/memory/") { continue; }
        let sid = source_id(&doc.source);
        let memory_id = format!("mechanical:{}", hex::encode(Sha256::digest(sid.as_bytes())));
        if let Some(previous) = existing.iter().find(|m| m.memory_id == memory_id) {
            if previous.pinned || previous.epistemic_state != EpistemicState::Proposed
                || !previous.protected_facts.is_empty()
                || previous.summary.as_ref().is_some_and(|text| !text.starts_with("# Structural memory\n\nProducer: native_mechanical\n"))
            { continue; }
            if previous.summary.as_deref() == Some(doc.raw_markdown.as_str())
                && previous.sources.len() == 1
                && previous.sources[0].revision_sha256 == doc.revision
            { continue; }
        }
        let operation_id = format!("structural-{}", &hex::encode(Sha256::digest(sid.as_bytes()))[..32]);
        operations.push(Operation {
            operation_id, action: Action::UpsertSummary, memory_id,
            sources: vec![SourceSpan { source_id: sid, revision_sha256: doc.revision.clone(), span_start: 0, span_end: doc.raw_markdown.len() }],
            reason_code: ReasonCode::ActiveWork, target_tier: Some(MemoryTier::WARM),
            summary: Some(doc.raw_markdown.clone()), protected_facts: Some(Vec::new()),
            epistemic_state: Some(EpistemicState::Proposed), confidence: Some(0.0), needs_expansion: Some(true),
        });
    }
    let mut proposals = Vec::new();
    for batch in operations.chunks(32) {
        let encoded = serde_json::to_vec(batch).map_err(|_| invalid("mechanical operation encoding failed"))?;
        let mut identity = generation_id.as_bytes().to_vec(); identity.extend_from_slice(created_at.as_bytes()); identity.extend_from_slice(&encoded);
        let digest = hex::encode(Sha256::digest(&identity));
        let proposal = Proposal {
            schema_version: 1, proposal_id: format!("mechanical-{}", digest),
            project_id: project_id.to_owned(), privacy_scope_id: privacy_scope_id.to_owned(),
            expected_generation: generation_id.to_owned(), created_at: created_at.to_owned(),
            producer: Producer { kind: ProducerKind::Mechanical, identity: "vrg-native-code-synthesis-v1".into() },
            operations: batch.to_vec(),
        };
        validate_shape(&proposal)?;
        proposals.push(proposal);
    }
    Ok(proposals)
}

fn validate_shape(proposal: &Proposal) -> VrgResult<()> {
    if proposal.schema_version != 1
        || !valid_hash(&proposal.expected_generation)
        || !valid_timestamp(&proposal.created_at)
        || !(1..=32).contains(&proposal.operations.len())
    {
        return Err(invalid(
            "unsupported proposal version, generation, timestamp, or operation count",
        ));
    }
    check_string(&proposal.proposal_id, 120)?;
    check_string(&proposal.project_id, 120)?;
    check_string(&proposal.privacy_scope_id, 120)?;
    check_string(&proposal.producer.identity, 160)?;
    let mut operations = BTreeSet::new();
    let mut memories = BTreeSet::new();
    for operation in &proposal.operations {
        check_string(&operation.operation_id, 120)?;
        check_string(&operation.memory_id, 160)?;
        if !operations.insert(&operation.operation_id)
            || !memories.insert(&operation.memory_id)
            || !(1..=16).contains(&operation.sources.len())
        {
            return Err(invalid(
                "proposal operations and memory identities must be unique and bounded",
            ));
        }
        let mut references = BTreeSet::new();
        for reference in &operation.sources {
            check_string(&reference.source_id, 240)?;
            if !valid_hash(&reference.revision_sha256)
                || reference.span_start >= reference.span_end
                || !references.insert((
                    &reference.source_id,
                    reference.span_start,
                    reference.span_end,
                ))
            {
                return Err(invalid("invalid or duplicate source reference"));
            }
        }
        if operation.action == Action::SetTier && operation.target_tier.is_none() {
            return Err(invalid("set_tier requires target_tier"));
        }
        if operation.action == Action::UpsertSummary
            && (operation.summary.is_none()
                || operation.protected_facts.is_none()
                || operation.epistemic_state.is_none()
                || operation.confidence.is_none()
                || operation.needs_expansion.is_none())
        {
            return Err(invalid(
                "upsert_summary requires all provenance and quality fields",
            ));
        }
        if let Some(summary) = &operation.summary {
            check_string(summary, 6000)?;
        }
        if let Some(facts) = &operation.protected_facts {
            if facts.len() > 32 {
                return Err(invalid("too many protected facts"));
            }
            for fact in facts {
                check_string(&fact.fact, 800)?;
                check_string(&fact.source_id, 240)?;
            }
        }
        if operation
            .confidence
            .is_some_and(|n| !n.is_finite() || !(0.0..=1.0).contains(&n))
        {
            return Err(invalid(
                "confidence must be finite and between zero and one",
            ));
        }
    }
    Ok(())
}

fn check_string(value: &str, maximum: usize) -> VrgResult<()> {
    if value.trim().is_empty() || value.contains('\0') || value.chars().count() > maximum {
        Err(invalid("proposal string is empty or exceeds its limit"))
    } else {
        Ok(())
    }
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// RFC3339 dates, including offsets and fractional seconds, without a new runtime dependency.
fn valid_timestamp(value: &str) -> bool {
    if !value.is_ascii() || value.len() < 20 {
        return false;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return false;
    }
    let number = |start: usize, end: usize| -> Option<u32> {
        let part = value.get(start..end)?;
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0, 4),
        number(5, 7),
        number(8, 10),
        number(11, 13),
        number(14, 16),
        number(17, 19),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    if day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    let mut position = 19;
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        let start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if position == start {
            return false;
        }
    }
    match value.get(position..) {
        Some("Z" | "z") => true,
        Some(offset)
            if offset.len() == 6
                && matches!(offset.as_bytes()[0], b'+' | b'-')
                && offset.as_bytes()[3] == b':' =>
        {
            number(position + 1, position + 3).is_some_and(|n| n <= 23)
                && number(position + 4, position + 6).is_some_and(|n| n <= 59)
        }
        _ => false,
    }
}

/// Conservative lexical safety gate. Passing it is not proof of semantic completeness;
/// every summary remains labeled derived and the exact source remains recoverable.
fn critical_clauses(text: &str) -> Vec<&str> {
    text.split(['\n', ';'])
        .flat_map(|line| line.split(". "))
        .map(str::trim)
        .filter(|clause| {
            let lower = clause.to_lowercase();
            !clause.is_empty()
                && (clause.chars().any(|c| c.is_ascii_digit())
                    || lower.split(|c: char| !c.is_alphanumeric()).any(|word| {
                        matches!(
                            word,
                            "no" | "not"
                                | "never"
                                | "only"
                                | "solo"
                                | "sólo"
                                | "sin"
                                | "except"
                                | "excepto"
                                | "unless"
                                | "rollback"
                                | "must"
                                | "debe"
                                | "if"
                                | "si"
                                | "cuando"
                                | "prohibido"
                                | "jamás"
                                | "pinned"
                        )
                    }))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub max_response_bytes: usize,
    pub max_context_units: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_response_bytes: 8192,
            max_context_units: 1536,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryHitMetadata {
    pub chunk_id: String,
    pub memory_id: String,
    pub tier: MemoryTier,
    pub representation: String,
    pub derived: bool,
    pub needs_expansion: bool,
    pub epistemic_state: EpistemicState,
    pub sources: Vec<SourceSpan>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BudgetReport {
    pub hard_response_bytes: usize,
    pub max_context_units: usize,
    pub response_bytes: usize,
    pub context_units: usize,
    pub count_mode: String,
    pub needs_expansion: bool,
    pub omitted_hits: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BudgetedResponse {
    #[serde(flatten)]
    pub response: SearchResponse,
    pub memory: Vec<MemoryHitMetadata>,
    pub budget: BudgetReport,
}

/// Adapt an already authorized response. Callers must first filter every summary source by
/// current ACL/revision, including secondary sources. No tier is an authorization decision.
pub fn budget_response(
    response: SearchResponse,
    memories: &[StoredMemory],
    budget: Budget,
) -> VrgResult<BudgetedResponse> {
    budget_response_with_query(response, memories, budget, "")
}

pub fn budget_response_with_query(
    mut response: SearchResponse,
    memories: &[StoredMemory],
    budget: Budget,
    query: &str,
) -> VrgResult<BudgetedResponse> {
    if !(512..=8192).contains(&budget.max_response_bytes) || budget.max_context_units < 128 {
        return Err(invalid(
            "budget requires 512..8192 bytes and at least 128 estimated units",
        ));
    }
    let visible: BTreeSet<(String, String)> = response
        .hits
        .iter()
        .map(|hit| (source_id(&hit.source), hit.revision.clone()))
        .collect();
    let mut metadata = Vec::new();
    let mut truncated_excerpt = false;
    let mut used_memories = BTreeSet::new();
    for hit in &mut response.hits {
        let original_length = hit.snippet.len();
        hit.snippet = bounded_snippet(&hit.snippet, query);
        let excerpt_is_partial = hit.snippet.len() < original_length;
        let source = source_id(&hit.source);
        let selected = memories
            .iter()
            .filter(|memory| {
                memory.sources.iter().any(|reference| {
                    reference.source_id == source && reference.revision_sha256 == hit.revision
                }) && memory.sources.iter().all(|reference| {
                    visible.contains(&(
                        reference.source_id.clone(),
                        reference.revision_sha256.clone(),
                    ))
                })
            })
            .min_by_key(|memory| {
                (
                    memory.needs_expansion,
                    memory.summary.as_ref().map_or(usize::MAX, String::len),
                    &memory.memory_id,
                )
            });
        if let Some(memory) = selected {
            let mut derived = false;
            if !matches!(
                memory.epistemic_state,
                EpistemicState::Conflicted | EpistemicState::Superseded
            ) {
                if let Some(summary) = memory.summary.as_ref().filter(|s| {
                    memory.fully_covered_sources.contains(&source)
                        && s.len() <= MAX_SNIPPET_BYTES
                        && s.len() < hit.snippet.len()
                        && query_covered(query, &hit.snippet, s)
                        && !memory.protected_facts.is_empty()
                        && memory
                            .protected_facts
                            .iter()
                            .all(|fact| s.contains(&fact.fact))
                }) {
                    if used_memories.insert(&memory.memory_id) {
                        hit.snippet = summary.clone();
                        derived = true;
                    }
                }
            }
            if !derived && excerpt_is_partial {
                truncated_excerpt = true;
            }
            metadata.push(MemoryHitMetadata {
                chunk_id: hit.chunk_id.clone(),
                memory_id: memory.memory_id.clone(),
                tier: memory.tier.clone(),
                representation: if derived {
                    "derived_summary"
                } else {
                    "source_excerpt"
                }
                .into(),
                derived,
                needs_expansion: memory.needs_expansion,
                epistemic_state: memory.epistemic_state.clone(),
                sources: memory.sources.clone(),
            });
        } else if excerpt_is_partial {
            truncated_excerpt = true;
        }
    }
    let mut result = BudgetedResponse {
        response,
        memory: metadata,
        budget: BudgetReport {
            hard_response_bytes: budget.max_response_bytes,
            max_context_units: budget.max_context_units,
            response_bytes: 0,
            context_units: 0,
            count_mode: "utf8_bytes_div4_estimate".into(),
            needs_expansion: false,
            omitted_hits: 0,
        },
    };
    result.budget.needs_expansion =
        truncated_excerpt || result.memory.iter().any(|m| m.needs_expansion);
    loop {
        settle_measurement(&mut result)?;
        if result.budget.response_bytes <= budget.max_response_bytes
            && result.budget.context_units <= budget.max_context_units
        {
            return Ok(result);
        }
        let Some(removed) = result.response.hits.pop() else {
            return Err(invalid(
                "requested budget cannot contain even the response metadata",
            ));
        };
        result
            .memory
            .retain(|memory| memory.chunk_id != removed.chunk_id);
        result.budget.omitted_hits += 1;
        result.budget.needs_expansion = true;
    }
}

fn settle_measurement(response: &mut BudgetedResponse) -> VrgResult<()> {
    // The accounting fields themselves count. Decimal digit lengths converge monotonically.
    for _ in 0..16 {
        let bytes = serde_json::to_vec(response)
            .map_err(|_| invalid("response serialization failed"))?
            .len();
        let units = bytes.div_ceil(4);
        if response.budget.response_bytes == bytes && response.budget.context_units == units {
            return Ok(());
        }
        response.budget.response_bytes = bytes;
        response.budget.context_units = units;
    }
    Err(invalid("budget accounting did not converge"))
}

fn invalid(message: &str) -> VrgError {
    VrgError::InvalidConfig(message.into())
}

pub fn bounded_snippet(text: &str, query: &str) -> String {
    if text.len() <= MAX_SNIPPET_BYTES {
        return text.to_owned();
    }

    const MARKER: &str = "…";
    let content_budget = MAX_SNIPPET_BYTES - 2 * MARKER.len();
    let lowered = text.to_ascii_lowercase();
    let mut terms: Vec<_> = query
        .split_ascii_whitespace()
        .filter(|term| term.len() >= 3)
        .take(32)
        .map(str::to_ascii_lowercase)
        .collect();
    terms.sort_unstable();
    terms.dedup();
    let match_start = terms
        .iter()
        .flat_map(|term| lowered.match_indices(term).map(|(index, _)| index))
        .max_by_key(|&index| {
            let mut start = index.saturating_sub(content_budget / 3);
            while !lowered.is_char_boundary(start) {
                start += 1;
            }
            let mut end = (start + content_budget).min(lowered.len());
            while !lowered.is_char_boundary(end) {
                end -= 1;
            }
            terms
                .iter()
                .filter(|term| lowered[start..end].contains(term.as_str()))
                .count()
        })
        .unwrap_or(0);
    let mut start = match_start.saturating_sub(content_budget / 3);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (start + content_budget).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }

    let prefix = if start > 0 { MARKER } else { "" };
    let suffix = if end < text.len() { MARKER } else { "" };
    format!("{prefix}{}{suffix}", &text[start..end])
}

fn query_covered(query: &str, excerpt: &str, summary: &str) -> bool {
    let excerpt = excerpt.to_lowercase();
    let summary = summary.to_lowercase();
    let terms: Vec<String> = query
        .split_whitespace()
        .map(str::to_lowercase)
        .map(|term| {
            term.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
                .to_owned()
        })
        .filter(|term| !term.is_empty() && excerpt.contains(term))
        .collect();
    !terms.is_empty() && terms.iter().all(|term| summary.contains(term))
}
