//! Closed Adaptive Memory catalog: AM-T01, AM-T05/06, AM-T07/08.
//! Fixtures are synthetic; no profile, source vault, model, or remote service is used.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use vrg::{
    adaptive::{
        self, Action, Budget, EpistemicState, MemoryTier, Operation, Producer, ProducerKind,
        Proposal, ProtectedFact, ReasonCode, SourceSpan, ValidationContext,
    },
    contracts::{
        MatchKind, RetrievalHit, RetrievalMode, SearchResponse, SearchSyncState, SourceDocument,
        SourceRef,
    },
};

fn source(text: &str) -> SourceDocument {
    SourceDocument {
        source: SourceRef {
            root_id: "synthetic".into(),
            relative_path: "decision.md".into(),
        },
        revision: hex::encode(Sha256::digest(text.as_bytes())),
        title: "Decision".into(),
        raw_markdown: text.into(),
        body_text: text.into(),
        frontmatter: BTreeMap::new(),
        links: Vec::new(),
        modified_unix_ms: Some(0),
    }
}

fn fixture(text: &str, summary: &str, fact: &str) -> (Proposal, ValidationContext) {
    let document = source(text);
    let id = adaptive::source_id(&document.source);
    let proposal = Proposal {
        schema_version: 1,
        proposal_id: "proposal-1".into(),
        project_id: "project".into(),
        privacy_scope_id: "synthetic-only".into(),
        expected_generation: "a".repeat(64),
        created_at: "2026-11-01T00:00:00Z".into(),
        producer: Producer {
            kind: ProducerKind::Codex,
            identity: "closed-fixture".into(),
        },
        operations: vec![Operation {
            operation_id: "op-1".into(),
            action: Action::UpsertSummary,
            memory_id: "decision".into(),
            sources: vec![SourceSpan {
                source_id: id.clone(),
                revision_sha256: document.revision.clone(),
                span_start: 0,
                span_end: text.len(),
            }],
            reason_code: ReasonCode::CurrentConstraint,
            target_tier: Some(MemoryTier::HOT),
            summary: Some(summary.into()),
            protected_facts: Some(vec![ProtectedFact {
                fact: fact.into(),
                source_id: id,
            }]),
            epistemic_state: Some(EpistemicState::Proposed),
            confidence: Some(0.99),
            needs_expansion: Some(false),
        }],
    };
    let context = ValidationContext {
        project_id: "project".into(),
        privacy_scope_id: "synthetic-only".into(),
        generation_id: "a".repeat(64),
        sources: vec![document],
        existing: Vec::new(),
    };
    (proposal, context)
}

fn response(document: &SourceDocument, text: &str) -> SearchResponse {
    SearchResponse {
        generation_id: Some("a".repeat(64)),
        sync_state: SearchSyncState::Ready,
        sync_reason: None,
        mode: RetrievalMode::TextOnly,
        degradation_reason: None,
        hits: vec![RetrievalHit {
            source: document.source.clone(),
            revision: document.revision.clone(),
            chunk_id: "chunk-1".into(),
            snippet: text.into(),
            start_byte: 0,
            end_byte: text.len() as u32,
            score: 1.0,
            matches: vec![MatchKind::FullText],
            relations: Vec::new(),
        }],
    }
}

#[test]
fn am_t01_strict_schema_scope_spans_and_revisions() {
    let (proposal, context) = fixture(
        "No borrar originales.\n",
        "No borrar originales.",
        "No borrar originales.",
    );
    let json = serde_json::to_vec(&proposal).unwrap();
    assert!(adaptive::parse_proposal(&json).is_ok());
    assert!(adaptive::parse_proposal(b"{broken").is_err());
    let mut value = serde_json::to_value(&proposal).unwrap();
    value["delete_source"] = true.into();
    assert!(adaptive::parse_proposal(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::to_value(&proposal).unwrap();
    value["operations"][0]["summary"] = serde_json::Value::Null;
    assert!(adaptive::parse_proposal(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut altered = proposal.clone();
    altered.project_id = "foreign".into();
    assert!(adaptive::validate_proposal(&altered, &context).is_err());
    altered = proposal.clone();
    altered.expected_generation = "b".repeat(64);
    assert!(adaptive::validate_proposal(&altered, &context).is_err());
    altered = proposal.clone();
    altered.operations[0].sources[0].source_id = "foreign:secret.md".into();
    assert!(adaptive::validate_proposal(&altered, &context).is_err());
    altered = proposal.clone();
    altered.operations[0].sources[0].revision_sha256 = "c".repeat(64);
    assert!(adaptive::validate_proposal(&altered, &context).is_err());
    altered = proposal.clone();
    altered.operations[0].sources[0].span_end = usize::MAX;
    assert!(adaptive::validate_proposal(&altered, &context).is_err());
    altered = proposal.clone();
    altered.created_at = "2026-02-30T00:00:00Z".into();
    assert!(adaptive::validate_proposal(&altered, &context).is_err());
    altered = proposal.clone();
    altered.operations[0].action = Action::SetTier;
    altered.operations[0].target_tier = None;
    assert!(adaptive::validate_proposal(&altered, &context).is_err());
    let first = adaptive::validate_proposal(&proposal, &context).unwrap();
    let second = adaptive::validate_proposal(&proposal, &context).unwrap();
    assert_eq!(first.fingerprint, second.fingerprint);
}

#[test]
fn am_t05_pins_age_conflict_and_no_new() {
    let (mut proposal, mut context) = fixture(
        "No borrar originales.",
        "No borrar originales.",
        "No borrar originales.",
    );
    let validated = adaptive::validate_proposal(&proposal, &context).unwrap();
    assert!(validated.memories[0].pinned);
    context.existing = validated.memories;
    proposal.proposal_id = "demotion".into();
    proposal.operations[0].reason_code = ReasonCode::SupersededHistory;
    proposal.operations[0].target_tier = Some(MemoryTier::COLD);
    assert!(adaptive::validate_proposal(&proposal, &context).is_err());
    proposal.operations[0].target_tier = Some(MemoryTier::HOT);
    proposal.operations[0].epistemic_state = Some(EpistemicState::Conflicted);
    let conflict = adaptive::validate_proposal(&proposal, &context).unwrap();
    assert!(conflict.memories[0].needs_expansion);
    assert_eq!(conflict.memories[0].tier, MemoryTier::HOT);
    proposal.operations[0].action = Action::Keep;
    proposal.operations[0].reason_code = ReasonCode::CurrentConstraint;
    let no_new = adaptive::validate_proposal(&proposal, &context).unwrap();
    assert_eq!(
        serde_json::to_value(&no_new.memories).unwrap(),
        serde_json::to_value(&context.existing).unwrap()
    );
    // Artificial recency is never consulted by the classifier's invariants.
    context.sources[0].modified_unix_ms = Some(u64::MAX);
    let still_pinned = adaptive::validate_proposal(&proposal, &context).unwrap();
    assert!(still_pinned.memories[0].pinned);
}

#[test]
fn am_t06_omitted_negation_condition_unit_and_version_need_expansion() {
    let text = "No borrar originales.\nRollback solo si checksum coincide.\nEsperar 17 segundos.\nUsar version 2.4.\n";
    let (mut proposal, context) = fixture(text, "Borrar originales.", "No borrar originales.");
    for defective in [
        "Borrar originales. Rollback solo si checksum coincide. Esperar 17 segundos. Usar version 2.4.",
        "No borrar originales. Rollback disponible. Esperar 17 segundos. Usar version 2.4.",
        "No borrar originales. Rollback solo si checksum coincide. Esperar 17. Usar version 2.4.",
        "No borrar originales. Rollback solo si checksum coincide. Esperar 17 segundos. Usar version 3.0.",
    ] {
        proposal.operations[0].summary = Some(defective.into());
        let validated = adaptive::validate_proposal(&proposal, &context).unwrap();
        assert!(
            validated.memories[0].needs_expansion,
            "defective summary accepted: {defective}"
        );
    }
    proposal.operations[0].summary = Some(text.into());
    assert!(
        !adaptive::validate_proposal(&proposal, &context)
            .unwrap()
            .memories[0]
            .needs_expansion
    );
    // A proposal cannot turn an unsupported fact into a source-verified anchor.
    proposal.operations[0].protected_facts.as_mut().unwrap()[0].fact = "Borrar originales.".into();
    assert!(adaptive::validate_proposal(&proposal, &context).is_err());
    // Identical text with a forged revision still fails before any application.
    let mut changed = context;
    changed.sources[0].raw_markdown.push('x');
    assert!(adaptive::validate_proposal(&proposal, &changed).is_err());
}

#[test]
fn am_t07_summary_coverage_query_and_stale_reference_fallback() {
    let text = "No borrar originales.\nEl identificador NERITA_RARE vive en el archivo.\n"
        .to_owned()
        + &"Detalle tecnico pertinente para recuperar contexto. ".repeat(18);
    let (proposal, context) = fixture(&text, "No borrar originales.", "No borrar originales.");
    let memories = adaptive::validate_proposal(&proposal, &context)
        .unwrap()
        .memories;
    let raw = response(&context.sources[0], &text);
    let rare = adaptive::budget_response_with_query(
        raw.clone(),
        &memories,
        Budget::default(),
        "NERITA_RARE",
    )
    .unwrap();
    assert!(rare.response.hits[0].snippet.contains("NERITA_RARE"));
    assert!(!rare.memory[0].derived);
    let short = adaptive::budget_response_with_query(
        raw.clone(),
        &memories,
        Budget::default(),
        "originales",
    )
    .unwrap();
    assert!(short.memory[0].derived);
    assert_eq!(short.response.hits[0].snippet, "No borrar originales.");
    let mut stale = raw.clone();
    stale.hits[0].revision = "d".repeat(64);
    assert!(
        adaptive::budget_response_with_query(stale, &memories, Budget::default(), "originales")
            .unwrap()
            .memory
            .is_empty()
    );
    let mut partial = memories.clone();
    partial[0].fully_covered_sources.clear();
    assert!(
        !adaptive::budget_response_with_query(raw, &partial, Budget::default(), "originales")
            .unwrap()
            .memory[0]
            .derived
    );
}

#[test]
fn am_t08_aggregate_serialization_budget_includes_all_metadata() {
    let document = source("áéíóú漢字 exact.");
    let mut raw = response(&document, &"áéíóú漢字 ".repeat(200));
    for index in 1..20 {
        let mut hit = raw.hits[0].clone();
        hit.chunk_id = format!("chunk-{index}");
        raw.hits.push(hit);
    }
    let budget = Budget {
        max_response_bytes: 1500,
        max_context_units: 300,
    };
    let result = adaptive::budget_response(raw, &[], budget).unwrap();
    let bytes = serde_json::to_vec(&result).unwrap();
    assert_eq!(bytes.len(), result.budget.response_bytes);
    assert!(bytes.len() <= 1200);
    assert_eq!(result.budget.context_units, bytes.len().div_ceil(4));
    assert_eq!(result.budget.count_mode, "utf8_bytes_div4_estimate");
    assert!(result.budget.needs_expansion && result.budget.omitted_hits > 0);
    assert!(
        result
            .response
            .hits
            .iter()
            .all(|hit| hit.snippet.len() <= 512)
    );
    assert!(
        adaptive::budget_response(
            result.response,
            &[],
            Budget {
                max_response_bytes: 100,
                max_context_units: 10
            }
        )
        .is_err()
    );
    let bounded =
        adaptive::bounded_snippet(&("á漢 ".repeat(400) + "NERITA_RARE exact"), "NERITA_RARE");
    assert!(bounded.len() <= 512 && bounded.contains("NERITA_RARE"));
}

#[test]
fn am_t01_source_identity_has_no_separator_collisions_and_optional_null_is_rejected() {
    let a = SourceRef {
        root_id: "a:b".into(),
        relative_path: "c.md".into(),
    };
    let b = SourceRef {
        root_id: "a".into(),
        relative_path: "b:c.md".into(),
    };
    assert_ne!(adaptive::source_id(&a), adaptive::source_id(&b));
    for source in [
        a,
        b,
        SourceRef {
            root_id: "percent%".into(),
            relative_path: "á%:漢.md".into(),
        },
    ] {
        assert_eq!(
            adaptive::parse_source_id(&adaptive::source_id(&source)),
            Some(source)
        );
    }
    assert!(adaptive::parse_source_id("a:b:c.md").is_none());
    assert!(adaptive::parse_source_id("a:%3a.md").is_none());
    let (mut proposal, _) = fixture(
        "No borrar originales.",
        "No borrar originales.",
        "No borrar originales.",
    );
    proposal.operations[0].action = Action::Keep;
    proposal.operations[0].target_tier = None;
    let mut value = serde_json::to_value(proposal).unwrap();
    value["operations"][0]["target_tier"] = serde_json::Value::Null;
    assert!(adaptive::parse_proposal(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn am_t07_graph_does_not_traverse_a_revoked_bridge_or_expose_its_relations() {
    use vrg::contracts::{
        GraphEdge, IndexSnapshot, LinkKind, SearchRequest, StoredDocument, TextChunk,
    };
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        Directory(std::env::temp_dir().join(format!("vrg-am-t07-{}-{nonce}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let path = directory.0.canonicalize().unwrap();
    let store = vrg::storage::DerivedStore::open(path.join("derived")).unwrap();
    let mut snapshot = IndexSnapshot {
        generation_id: "e".repeat(64),
        documents: Vec::new(),
        chunks: Vec::new(),
        edges: Vec::new(),
        vectors: Vec::new(),
    };
    let refs: Vec<_> = ["PublicSeed.md", "RevokedBridge.md", "PublicTarget.md"]
        .iter()
        .map(|path| SourceRef {
            root_id: "synthetic".into(),
            relative_path: (*path).into(),
        })
        .collect();
    for (index, reference) in refs.iter().enumerate() {
        let text = if index == 0 {
            "AM07_SEED source"
        } else {
            "Unrelated context"
        };
        let revision = hex::encode(Sha256::digest(text.as_bytes()));
        snapshot.documents.push(StoredDocument {
            source: reference.clone(),
            revision: revision.clone(),
            title: reference.relative_path.clone(),
            frontmatter: BTreeMap::new(),
            modified_unix_ms: None,
        });
        snapshot.chunks.push(TextChunk {
            id: format!("chunk-{index}"),
            source: reference.clone(),
            revision,
            ordinal: 0,
            start_byte: 0,
            end_byte: text.len() as u32,
            text: text.into(),
        });
    }
    for index in 0..2 {
        snapshot.edges.push(GraphEdge {
            from: refs[index].clone(),
            to: Some(refs[index + 1].clone()),
            unresolved_target: None,
            kind: LinkKind::Wiki,
            start_byte: 0,
            end_byte: 1,
        });
    }
    store.commit_generation(snapshot, None).unwrap();
    let request = SearchRequest {
        query: "AM07_SEED".into(),
        limit: 10,
        root_ids: None,
        use_semantic: false,
        graph_hops: 2,
    };
    let allowed = vrg::retrieval::retrieve_authorized(&store, None, &request, &|source, _| {
        source.relative_path != "RevokedBridge.md"
    })
    .unwrap();
    assert_eq!(allowed.hits.len(), 1);
    assert_eq!(allowed.hits[0].source, refs[0]);
    assert!(allowed.hits[0].relations.is_empty());
    // The same graph really can reach both other nodes when the bridge is authorized.
    let unrestricted = vrg::retrieval::retrieve(&store, None, &request).unwrap();
    assert_eq!(unrestricted.hits.len(), 3);
}
