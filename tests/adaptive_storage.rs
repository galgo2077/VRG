//! Closed Adaptive Memory catalog: AM-T02, AM-T09, AM-T10, AM-T13.
//! Prepared during AM2; execution is reserved for AM4.
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use vrg::{
    contracts::{IndexSnapshot, SourceRef, StoredDocument, TextChunk, VectorEmbedding},
    storage::DerivedStore,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "vrg-adaptive-storage-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn store(&self) -> DerivedStore {
        DerivedStore::open(self.0.join("derived")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn snapshot(revision: usize) -> IndexSnapshot {
    let mut snapshot = IndexSnapshot {
        generation_id: format!("g{revision:02}"),
        documents: Vec::new(),
        chunks: Vec::new(),
        edges: Vec::new(),
        vectors: Vec::new(),
    };
    for index in 0..30 {
        let domain = if index < 15 { "private-a" } else { "private-b" };
        let text = if index == revision % 30 {
            format!(
                "Decision {index}: Linux x86_64 only; rollback before publication; revision {revision}; rareIdentifierQ{index}."
            )
        } else {
            format!(
                "Decision {index}: Linux x86_64 only; rollback before publication; stable; rareIdentifierQ{index}."
            )
        };
        let source = SourceRef {
            root_id: domain.into(),
            relative_path: format!("decision-{index}.md"),
        };
        let revision = hex::encode(sha2::Sha256::digest(text.as_bytes()));
        let id = format!("{domain}-{index}-{revision}");
        snapshot.documents.push(StoredDocument {
            source: source.clone(),
            revision: revision.clone(),
            title: format!("Decision {index}"),
            frontmatter: BTreeMap::new(),
            modified_unix_ms: None,
        });
        snapshot.chunks.push(TextChunk {
            id: id.clone(),
            source,
            revision,
            ordinal: 0,
            start_byte: 0,
            end_byte: text.len() as u32,
            text,
        });
        for model in [
            "model-a:tokenizer1:max512:passage",
            "model-b:tokenizer2:max256:passage",
        ] {
            snapshot.vectors.push(VectorEmbedding {
                chunk_id: id.clone(),
                model_id: model.into(),
                dimensions: 2,
                values: vec![0.25, 0.75],
            });
        }
    }
    snapshot
}
use sha2::Digest;

#[test]
fn am_t02_dedup_manifests_privacy_and_models() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for revision in 0..12 {
        let parent = (revision > 0).then(|| format!("g{:02}", revision - 1));
        store
            .commit_generation(snapshot(revision), parent.as_deref())
            .unwrap();
    }
    let stats = store.storage_stats().unwrap();
    assert_eq!(stats.generations, 12);
    assert_eq!(stats.chunks, 360);
    assert_eq!(stats.text_objects, 42);
    assert_eq!(stats.vector_objects, 84);
    let connection = rusqlite::Connection::open(fixture.0.join("derived/vrg.sqlite3")).unwrap();
    let duplicated:i64=connection.query_row("SELECT count(*) FROM (SELECT domain,object_id,count(*) n FROM text_objects GROUP BY domain,object_id HAVING n>1)",[],|r|r.get(0)).unwrap();
    assert_eq!(duplicated, 0);
    // Contentless FTS stores postings, not another retrievable copy of bodies.
    let fts_body: Option<String> = connection
        .query_row("SELECT body FROM chunk_fts LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(fts_body.is_none());
    assert!(
        !store
            .fts_search_rows("rareIdentifierQ29", &["private-b".into()], 1)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .vector_catalog_for_model("model-a:tokenizer1:max512:passage", &[])
            .unwrap()
            .iter()
            .all(|v| v.chunk.text.is_empty())
    );
    // Equal exact bytes in distinct privacy roots remain separate payloads.
    let mut same = snapshot(12);
    same.chunks[15].text = same.chunks[0].text.clone();
    same.chunks[15].end_byte = same.chunks[0].end_byte;
    same.chunks[15].revision = same.chunks[0].revision.clone();
    same.documents[15].revision = same.documents[0].revision.clone();
    store.commit_generation(same, Some("g11")).unwrap();
    let distinct:i64=connection.query_row("SELECT count(DISTINCT object_id) FROM chunk_manifests WHERE generation_id='g12' AND relative_path IN ('decision-0.md','decision-15.md')",[],|r|r.get(0)).unwrap();
    assert_eq!(distinct, 2);
    // The frozen AM-T14 runner owns the 100 real no-op indexations.
}

#[test]
fn am_t09_gc_protects_current_previous_pins_readers_pending_and_rollback() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for revision in 0..6 {
        let parent = (revision > 0).then(|| format!("g{:02}", revision - 1));
        store
            .commit_generation(snapshot(revision), parent.as_deref())
            .unwrap();
    }
    store.pin_generation("g00", "backup-in-progress").unwrap();
    let original = snapshot(0).documents[0].clone();
    assert!(
        store
            .retained_source_revisions()
            .unwrap()
            .contains(&(original.source.clone(), original.revision.clone()))
    );
    let connection = rusqlite::Connection::open(fixture.0.join("derived/vrg.sqlite3")).unwrap();
    connection.execute("INSERT INTO journal(operation,state,before_generation,after_generation,created_unix_ms) VALUES('publish','PREPARING','g01','g02',0)",[]).unwrap();
    let plan = store.maintenance_plan(2).unwrap();
    assert_eq!(plan.remove_generations, vec!["g03"]);
    assert!(
        store
            .projected_source_revisions(&plan)
            .unwrap()
            .contains(&(original.source.clone(), original.revision.clone()))
    );
    let reader = store.acquire_reader_lease().unwrap();
    assert!(store.apply_maintenance(&plan).is_err());
    assert!(store.commit_generation(snapshot(6), Some("g05")).is_err());
    drop(reader);
    store
        .pin_generation("g03", "migration-in-progress")
        .unwrap();
    assert!(store.apply_maintenance(&plan).is_err());
    store
        .unpin_generation("g03", "migration-in-progress")
        .unwrap();
    let report = store
        .apply_maintenance(&store.maintenance_plan(2).unwrap())
        .unwrap();
    assert_eq!(report.removed_generations, 1);
    assert_eq!(report.filesystem_bytes_released, 0);
    connection
        .execute(
            "UPDATE journal SET state='ABORTED' WHERE state='PREPARING'",
            [],
        )
        .unwrap();
    store.rollback_generation("g04", "g05").unwrap();
    assert_eq!(
        store
            .load_current_snapshot()
            .unwrap()
            .unwrap()
            .generation_id,
        "g04"
    );
    // Reachable text and vectors survive; only actual unreachable payloads leave.
    let before = store.storage_stats().unwrap();
    assert!(before.text_objects > 0);
    store.unpin_generation("g00", "backup-in-progress").unwrap();
    let report = store
        .apply_maintenance(&store.maintenance_plan(2).unwrap())
        .unwrap();
    assert!(report.removed_generations > 0);
    assert!(report.removed_text_objects > 0);
    assert!(
        !store
            .retained_source_revisions()
            .unwrap()
            .contains(&(original.source, original.revision))
    );
    assert_eq!(
        store.load_current_snapshot().unwrap().unwrap().chunks.len(),
        30
    );
}

fn legacy_database(path: &std::path::Path) {
    fs::create_dir_all(path).unwrap();
    let c = rusqlite::Connection::open(path.join("vrg.sqlite3")).unwrap();
    c.execute_batch(include_str!("fixtures/adaptive/schema1.sql"))
        .unwrap();
    c.execute("INSERT INTO generations VALUES('legacy',NULL,1,1,1,1)", [])
        .unwrap();
    c.execute(
        "INSERT INTO documents VALUES('legacy','project','decision.md','rev','Decision','{}',NULL)",
        [],
    )
    .unwrap();
    c.execute("INSERT INTO chunks VALUES('legacy','chunk','project','decision.md','rev',0,0,42,'Do not publish until rollback is verified.')",[]).unwrap();
    c.execute("INSERT INTO chunk_fts(generation_id,chunk_id,root_id,relative_path,title,body) VALUES('legacy','chunk','project','decision.md','Decision','Do not publish until rollback is verified.')",[]).unwrap();
    c.execute("INSERT INTO active_generation VALUES(1,'legacy')", [])
        .unwrap();
}

#[test]
fn am_t10_explicit_migration_is_idempotent_and_keeps_consistent_rollback() {
    let fixture = Fixture::new();
    let path = fixture.0.join("legacy");
    legacy_database(&path);
    assert!(DerivedStore::open(&path).is_err());
    // A prior interrupted attempt is kept; retry uses fresh private staging.
    fs::create_dir(path.join("schema-migration-00")).unwrap();
    fs::write(
        path.join("schema-migration-00/interrupted"),
        "synthetic interruption",
    )
    .unwrap();
    let copy = fixture.0.join("migrated-copy");
    assert!(DerivedStore::migrate_schema_copy(&path, &copy).unwrap());
    let original = rusqlite::Connection::open(path.join("vrg.sqlite3")).unwrap();
    let version: i64 = original
        .query_row("SELECT version FROM schema_info", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    drop(original);
    assert!(DerivedStore::migrate_schema(&path).unwrap());
    let store = DerivedStore::open(&path).unwrap();
    let snapshot = store.load_current_snapshot().unwrap().unwrap();
    assert_eq!(
        snapshot.chunks[0].text,
        "Do not publish until rollback is verified."
    );
    assert!(
        !store
            .fts_search_rows("rollback", &[], 10)
            .unwrap()
            .is_empty()
    );
    assert!(DerivedStore::migrate_schema(&path).is_err()); // live schema lease
    drop(store);
    assert!(!DerivedStore::migrate_schema(&path).unwrap());
    let backup = path.join("schema-migration-01/schema1.rollback.sqlite3");
    let connection = rusqlite::Connection::open(backup).unwrap();
    let version: i64 = connection
        .query_row("SELECT version FROM schema_info", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    let original: String = connection
        .query_row("SELECT body FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(original, snapshot.chunks[0].text);
    let connection = rusqlite::Connection::open(path.join("vrg.sqlite3")).unwrap();
    assert!(connection.execute("INSERT INTO chunks(generation_id,chunk_id,root_id,relative_path,revision,ordinal,start_byte,end_byte,body) VALUES('legacy','bad','project','decision.md','rev',0,0,1,'x')",[]).is_err());
}

#[test]
fn am_t13_statistics_and_catalog_are_passive_and_bodies_are_lazy() {
    let fixture = Fixture::new();
    let store = fixture.store();
    store.commit_generation(snapshot(0), None).unwrap();
    let before = store.current_generation_id().unwrap();
    for _ in 0..100 {
        store.storage_stats().unwrap();
        let catalog = store.load_current_catalog().unwrap().unwrap();
        assert!(catalog.chunks.is_empty());
        assert!(catalog.vectors.is_empty());
    }
    assert_eq!(store.current_generation_id().unwrap(), before);
    assert_eq!(store.storage_stats().unwrap().generations, 1);
    assert!(
        store
            .apply_adaptive_state(before.as_deref().unwrap(), "plan", "fingerprint", "[]")
            .unwrap()
    );
    assert!(
        !store
            .apply_adaptive_state(before.as_deref().unwrap(), "plan", "fingerprint", "[]")
            .unwrap()
    );
    assert!(
        store
            .apply_adaptive_state(before.as_deref().unwrap(), "plan", "different", "[]")
            .is_err()
    );
}

#[test]
fn am_t09_adaptive_historical_references_use_canonical_escaped_source_identity() {
    use vrg::adaptive::{EpistemicState, MemoryTier, ReasonCode, SourceSpan, StoredMemory};
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut original = None;
    for revision in 0..6 {
        let mut snapshot = snapshot(revision);
        for doc in &mut snapshot.documents {
            if doc.source.relative_path == "decision-0.md" {
                doc.source.root_id = "private:scope".into();
                doc.source.relative_path = "decision:retry.md".into();
            }
        }
        for chunk in &mut snapshot.chunks {
            if chunk.source.relative_path == "decision-0.md" {
                chunk.source.root_id = "private:scope".into();
                chunk.source.relative_path = "decision:retry.md".into();
            }
        }
        if revision == 0 {
            original = Some(snapshot.documents[0].clone());
        }
        let parent = (revision > 0).then(|| format!("g{:02}", revision - 1));
        store
            .commit_generation(snapshot, parent.as_deref())
            .unwrap();
        if revision == 2 {
            let original = original.as_ref().unwrap();
            let memory = StoredMemory {
                memory_id: "historical".into(),
                project_id: "project".into(),
                privacy_scope_id: "private:scope".into(),
                tier: MemoryTier::WARM,
                pinned: false,
                sources: vec![SourceSpan {
                    source_id: vrg::adaptive::source_id(&original.source),
                    revision_sha256: original.revision.clone(),
                    span_start: 0,
                    span_end: 1,
                }],
                fully_covered_sources: Vec::new(),
                summary: None,
                protected_facts: Vec::new(),
                epistemic_state: EpistemicState::Proposed,
                confidence: 0.5,
                needs_expansion: true,
                reason_code: ReasonCode::UncertainKeepDetail,
            };
            store
                .apply_adaptive_state(
                    "g02",
                    "historical",
                    "fingerprint",
                    &serde_json::to_string(&vec![memory]).unwrap(),
                )
                .unwrap();
        }
    }
    let plan = store.maintenance_plan(2).unwrap();
    assert!(!plan.remove_generations.contains(&"g00".into()));
    assert!(!plan.remove_generations.contains(&"g02".into()));
    let historical = original.as_ref().unwrap();
    assert!(
        store
            .projected_source_revisions(&plan)
            .unwrap()
            .contains(&(historical.source.clone(), historical.revision.clone()))
    );
    store.apply_maintenance(&plan).unwrap();
    store.rollback_generation("g00", "g05").unwrap();
    let original = original.unwrap();
    let restored = store.load_current_snapshot().unwrap().unwrap();
    assert_eq!(
        restored
            .documents
            .iter()
            .find(|doc| doc.source == original.source)
            .unwrap()
            .revision,
        original.revision
    );
}
