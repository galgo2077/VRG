//! Directed GV4 validation. Fixtures are synthetic and isolated; no real profile is mutated.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use vrg::{
    adaptive::{self, EpistemicState, MemoryTier, ReasonCode, SourceSpan, StoredMemory},
    adaptive_codec::ArchiveStore,
    config::{RootConfig, VrgConfig},
    contracts::IndexSnapshot,
    indexer,
    storage::DerivedStore,
    vault,
    view_data::{GraphQuery, ViewCatalog},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    base: PathBuf,
    notes: PathBuf,
    profile: PathBuf,
    config: VrgConfig,
    snapshot: IndexSnapshot,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "vrg-view-data-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let base = base.canonicalize().unwrap();
        let notes = base.join("notas café");
        fs::create_dir(&notes).unwrap();
        for i in 0..12 {
            let link = if i == 0 {
                "[vecino](note-01.md)"
            } else {
                "Aislado"
            };
            let raw = format!(
                "---\ntitle: Documento {i}\n---\n# Título **Unicode café 漢字**\n\n{link}\n\n{}",
                format!("Texto exacto {i}, párrafo de prueba con Unicode.\n\n").repeat(20)
            );
            fs::write(notes.join(format!("note-{i:02}.md")), raw).unwrap();
        }
        let mut config = VrgConfig {
            roots: vec![RootConfig {
                id: "test".into(),
                path: notes.clone(),
                exclusions: vec![],
                allowed_files: None,
            }],
            data_dir: base.join("store"),
            chunk_bytes: 180,
            chunk_overlap_bytes: 20,
            ..Default::default()
        };
        config.adaptive_memory.enabled = true;
        let profile = base.join("perfil.toml");
        fs::write(&profile, toml::to_string(&config).unwrap()).unwrap();
        let store = DerivedStore::open(&config.data_dir).unwrap();
        indexer::index(&config, &store, None).unwrap();
        let snapshot = store.load_current_snapshot().unwrap().unwrap();
        drop(store);
        Self {
            base,
            notes,
            profile,
            config,
            snapshot,
        }
    }
    fn catalog(&self) -> ViewCatalog {
        ViewCatalog::resolve(&self.notes, Some(&self.profile)).unwrap()
    }
    fn source(&self) -> vrg::contracts::SourceDocument {
        vault::read_source(&self.config, "test", "note-00.md", 4 * 1024 * 1024).unwrap()
    }
    fn save_config(&self) {
        fs::write(&self.profile, toml::to_string(&self.config).unwrap()).unwrap();
    }
    fn add_memory_archive(&self) {
        let doc = self.source();
        let scope = hex::encode(Sha256::digest(
            serde_json::to_vec(&self.config.roots).unwrap(),
        ));
        let sid = adaptive::source_id(&doc.source);
        let memory = StoredMemory {
            memory_id: "verified-fixture".into(),
            project_id: hex::encode(Sha256::digest(
                self.config.data_dir.to_string_lossy().as_bytes(),
            )),
            privacy_scope_id: scope.clone(),
            tier: MemoryTier::WARM,
            pinned: true,
            sources: vec![SourceSpan {
                source_id: sid.clone(),
                revision_sha256: doc.revision.clone(),
                span_start: 0,
                span_end: doc.raw_markdown.len(),
            }],
            fully_covered_sources: vec![sid.clone()],
            summary: Some("Resumen derivado de fixture; no equivale al texto original.".into()),
            protected_facts: vec![],
            epistemic_state: EpistemicState::Verified,
            confidence: 1.0,
            needs_expansion: true,
            reason_code: ReasonCode::ReusableSolution,
        };
        let store = DerivedStore::open(&self.config.data_dir).unwrap();
        store
            .apply_adaptive_state(
                &self.snapshot.generation_id,
                "proposal",
                "fingerprint",
                &serde_json::to_string(&vec![memory]).unwrap(),
            )
            .unwrap();
        drop(store);
        let archive = ArchiveStore::open(&self.config.data_dir).unwrap();
        archive
            .archive(&scope, &sid, &doc.revision, doc.raw_markdown.as_bytes())
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::env::var_os("VRG_VIEW_KEEP_FIXTURE").is_some() {
            eprintln!(
                "SYNTHETIC_FIXTURE profile={} notes={}",
                self.profile.display(),
                self.notes.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.base);
        }
    }
}
fn graph(c: &ViewCatalog) -> Value {
    c.graph(&c.selected_project_id().unwrap(), &Default::default())
        .unwrap()
}
fn chunks(g: &Value) -> Vec<&Value> {
    g["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "chunk")
        .collect()
}
fn hashes(base: &std::path::Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(base).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(hashes(&p));
        } else {
            // Read-only SQLite WAL readers may create coordination files. These are
            // not logical index/source writes; keep hashing every persistent input.
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".sqlite3-wal") || n.ends_with(".sqlite3-shm"))
            {
                continue;
            }
            out.insert(p.clone(), hex::encode(Sha256::digest(fs::read(p).unwrap())));
        }
    }
    out
}
#[test]
fn v03_v04_paths_and_scoped_identical_payloads() {
    let a = Fixture::new();
    let b = Fixture::new();
    assert_eq!(a.source().revision, b.source().revision);
    let ca = a.catalog();
    let cb = b.catalog();
    let ga = graph(&ca);
    let gb = graph(&cb);
    let a_id = chunks(&ga)[0]["id"].as_str().unwrap();
    let b_id = chunks(&gb)[0]["id"].as_str().unwrap();
    assert_ne!(a_id, b_id);
    assert_eq!(
        ca.detail(
            &ca.selected_project_id().unwrap(),
            b_id,
            ga["snapshot_id"].as_str().unwrap(),
            0,
            100,
            false
        )
        .unwrap_err()
        .code,
        "NODE_NOT_FOUND"
    );
    assert_eq!(
        ViewCatalog::resolve(&b.notes, Some(&a.profile))
            .err()
            .unwrap()
            .code,
        "SCOPE_MISMATCH"
    );
    assert!(ViewCatalog::resolve(&a.base.join("absent"), Some(&a.profile)).is_err());
    assert!(ViewCatalog::resolve(&a.profile, Some(&a.profile)).is_err());
    #[cfg(unix)]
    {
        let alias = a.base.join("alias");
        std::os::unix::fs::symlink(&a.notes, &alias).unwrap();
        assert_eq!(
            ViewCatalog::resolve(&alias, Some(&a.profile))
                .err()
                .unwrap()
                .code,
            "SCOPE_MISMATCH"
        );
    }
}
#[test]
fn v05_v07_v08_passive_chunks_offsets_and_document_links() {
    let f = Fixture::new();
    let before = hashes(&f.base);
    let c = f.catalog();
    let g = graph(&c);
    assert_eq!(chunks(&g).len(), f.snapshot.chunks.len());
    assert!(chunks(&g).len() >= 48);
    assert_eq!(g["capabilities"]["semantic_vectors"], false);
    let project = c.selected_project_id().unwrap();
    for node in chunks(&g).into_iter().take(8) {
        let d = c
            .detail(
                &project,
                node["id"].as_str().unwrap(),
                g["snapshot_id"].as_str().unwrap(),
                0,
                128 * 1024,
                false,
            )
            .unwrap();
        let start = node["source"]["start_byte"].as_u64().unwrap() as usize;
        let end = node["source"]["end_byte"].as_u64().unwrap() as usize;
        let doc = vault::read_source(
            &f.config,
            node["source"]["root_id"].as_str().unwrap(),
            node["source"]["relative_path"].as_str().unwrap(),
            4 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(d["text"], doc.body_text[start..end]);
        assert_eq!(d["offset_basis"], "body_text_utf8");
        let raw = c
            .detail(
                &project,
                node["id"].as_str().unwrap(),
                g["snapshot_id"].as_str().unwrap(),
                0,
                128 * 1024,
                true,
            )
            .unwrap();
        assert_eq!(raw["text"], doc.raw_markdown);
        assert_ne!(raw["text"], d["text"]);
    }
    let explicit = g["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "explicit")
        .collect::<Vec<_>>();
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0]["provenance_scope"], "document");
    assert_eq!(
        g["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "contains")
            .count(),
        f.snapshot.chunks.len()
    );
    assert_eq!(hashes(&f.base), before);
}
#[test]
fn v05_missing_unknown_schema_never_initialized() {
    let mut f = Fixture::new();
    f.config.data_dir = f.base.join("missing-store");
    f.save_config();
    let c = f.catalog();
    assert_eq!(
        c.graph(&c.selected_project_id().unwrap(), &Default::default())
            .unwrap_err()
            .code,
        "STORE_MISSING"
    );
    assert!(!f.config.data_dir.exists());
    f.config.data_dir = f.base.join("store");
    f.save_config();
    let db = rusqlite::Connection::open(f.config.data_dir.join("vrg.sqlite3")).unwrap();
    db.execute("UPDATE schema_info SET version=999", [])
        .unwrap();
    drop(db);
    let before = hashes(&f.base);
    let c = f.catalog();
    assert_eq!(
        c.graph(&c.selected_project_id().unwrap(), &Default::default())
            .unwrap_err()
            .code,
        "SCHEMA_UNSUPPORTED"
    );
    assert_eq!(before, hashes(&f.base));
}
#[test]
fn v06_pagination_cursor_and_writer_generation() {
    let f = Fixture::new();
    let c = f.catalog();
    let project = c.selected_project_id().unwrap();
    let q = GraphQuery {
        limit: 3,
        ..Default::default()
    };
    let g = c.graph(&project, &q).unwrap();
    assert_eq!(chunks(&g).len(), 3);
    assert!(g["coverage"]["partial"].as_bool().unwrap());
    let cursor = g["next_cursor"].as_str().unwrap().to_owned();
    let next = c
        .graph(
            &project,
            &GraphQuery {
                cursor: Some(cursor.clone()),
                snapshot_id: Some(g["snapshot_id"].as_str().unwrap().into()),
                ..q.clone()
            },
        )
        .unwrap();
    assert_ne!(chunks(&g)[0]["id"], chunks(&next)[0]["id"]);
    assert_eq!(
        c.graph(
            &project,
            &GraphQuery {
                cursor: Some(format!("{cursor}x")),
                ..q.clone()
            }
        )
        .unwrap_err()
        .code,
        "SNAPSHOT_EXPIRED"
    );
    let store = DerivedStore::open(&f.config.data_dir).unwrap();
    let mut newer = f.snapshot.clone();
    newer.generation_id = "new-generation".into();
    store
        .commit_generation(newer, Some(&f.snapshot.generation_id))
        .unwrap();
    let node = chunks(&g)[0];
    assert_eq!(
        c.detail(
            &project,
            node["id"].as_str().unwrap(),
            g["snapshot_id"].as_str().unwrap(),
            0,
            100,
            false
        )
        .unwrap_err()
        .code,
        "SNAPSHOT_EXPIRED"
    );
    assert_eq!(
        c.graph(
            &project,
            &GraphQuery {
                cursor: Some(cursor),
                ..q
            }
        )
        .unwrap_err()
        .code,
        "SNAPSHOT_EXPIRED"
    );
    assert_eq!(
        c.status(&project).unwrap()["generation_id"],
        "new-generation"
    );
}
#[test]
fn v08_v13_revision_exclusion_privacy_and_root_revocation() {
    let mut f = Fixture::new();
    let c = f.catalog();
    let g = graph(&c);
    let project = c.selected_project_id().unwrap();
    let node = chunks(&g)[0];
    let path = f
        .notes
        .join(node["source"]["relative_path"].as_str().unwrap());
    fs::write(&path, "# Changed\nNew exact source\n").unwrap();
    assert_eq!(
        c.detail(
            &project,
            node["id"].as_str().unwrap(),
            g["snapshot_id"].as_str().unwrap(),
            0,
            100,
            false
        )
        .unwrap_err()
        .code,
        "SOURCE_CHANGED"
    );
    assert_eq!(c.status(&project).unwrap()["changed"], true);
    fs::write(&path, "---\nprivate: true\n---\nSecret\n").unwrap();
    assert_eq!(
        c.detail(
            &project,
            node["id"].as_str().unwrap(),
            g["snapshot_id"].as_str().unwrap(),
            0,
            100,
            false
        )
        .unwrap_err()
        .code,
        "ROOT_REVOKED"
    );
    assert_eq!(c.status(&project).unwrap_err().code, "ROOT_REVOKED");
    f.config.roots[0]
        .exclusions
        .push(node["source"]["relative_path"].as_str().unwrap().into());
    f.save_config();
    assert_eq!(
        c.detail(
            &project,
            node["id"].as_str().unwrap(),
            g["snapshot_id"].as_str().unwrap(),
            0,
            100,
            false
        )
        .unwrap_err()
        .code,
        "SNAPSHOT_EXPIRED"
    );
    f.config.roots.clear();
    f.save_config();
    let revoked = graph(&c);
    assert!(revoked["nodes"].as_array().unwrap().is_empty());
}
#[test]
fn v11_summary_pin_tier_archive_corruption_and_bounded_restore() {
    let f = Fixture::new();
    f.add_memory_archive();
    let before = hashes(&f.base);
    let c = f.catalog();
    let g = graph(&c);
    let project = c.selected_project_id().unwrap();
    let nodes = g["nodes"].as_array().unwrap();
    let summary = nodes.iter().find(|n| n["kind"] == "summary").unwrap();
    let detail = c
        .detail(
            &project,
            summary["id"].as_str().unwrap(),
            g["snapshot_id"].as_str().unwrap(),
            0,
            10000,
            false,
        )
        .unwrap();
    assert_eq!(detail["pinned"], true);
    assert_eq!(detail["content_kind"], "derived_summary");
    assert_eq!(summary["revision_state"], "current");
    assert!(
        g["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["kind"] != "source_revision")
            .all(|n| n["revision_state"] == "current")
    );
    let exact = c
        .detail(
            &project,
            summary["id"].as_str().unwrap(),
            g["snapshot_id"].as_str().unwrap(),
            0,
            10000,
            true,
        )
        .unwrap();
    assert_eq!(exact["text"], f.source().raw_markdown);
    assert_ne!(detail["text"], exact["text"]);
    let warm = chunks(&g)
        .into_iter()
        .find(|n| n["source"]["relative_path"] == "note-00.md")
        .unwrap();
    assert_eq!(warm["tier"], "WARM");
    assert_eq!(warm["tier_origin"], "inherited_from_source");
    let cold = nodes
        .iter()
        .find(|n| n["kind"] == "source_revision")
        .unwrap();
    let restored = c
        .detail(
            &project,
            cold["id"].as_str().unwrap(),
            g["snapshot_id"].as_str().unwrap(),
            0,
            10000,
            false,
        )
        .unwrap();
    assert_eq!(restored["text"], f.source().raw_markdown);
    assert_eq!(cold["revision_state"], "current");
    assert_eq!(restored["node"]["revision_state"], "current");
    assert_eq!(before, hashes(&f.base));
    let scope = hex::encode(Sha256::digest(serde_json::to_vec(&f.config.roots).unwrap()));
    let archive = ArchiveStore::open_existing(&f.config.data_dir).unwrap();
    assert!(
        archive
            .restore_bounded(
                &scope,
                &adaptive::source_id(&f.source().source),
                &f.source().revision,
                4
            )
            .is_err()
    );
    let blob = fs::read_dir(f.config.data_dir.join("cold"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "blob"))
        .unwrap();
    fs::write(blob, b"truncated").unwrap();
    assert_eq!(
        c.detail(
            &project,
            cold["id"].as_str().unwrap(),
            g["snapshot_id"].as_str().unwrap(),
            0,
            10000,
            false
        )
        .unwrap_err()
        .code,
        "COLD_CORRUPT"
    );
}
#[test]
fn v11_historical_cold_is_exact_and_not_current_source() {
    let f = Fixture::new();
    f.add_memory_archive();
    let original = f.source();
    fs::write(
        f.notes.join("note-00.md"),
        "# Current revision\nThis is newer.\n",
    )
    .unwrap();
    let store = DerivedStore::open(&f.config.data_dir).unwrap();
    indexer::index(&f.config, &store, None).unwrap();
    drop(store);
    let c = f.catalog();
    let g = graph(&c);
    let cold = g["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "source_revision")
        .unwrap();
    let detail = c
        .detail(
            &c.selected_project_id().unwrap(),
            cold["id"].as_str().unwrap(),
            g["snapshot_id"].as_str().unwrap(),
            0,
            10000,
            false,
        )
        .unwrap();
    assert_eq!(detail["text"], original.raw_markdown);
    assert_eq!(cold["revision_state"], "historical");
    assert_eq!(detail["node"]["revision_state"], "historical");
    assert_eq!(detail["revision"], original.revision);
    assert_ne!(detail["revision"], f.source().revision);
}
#[test]
fn v17_bounded_search_page_and_response() {
    let f = Fixture::new();
    let c = f.catalog();
    let p = c.selected_project_id().unwrap();
    let g = c
        .graph(
            &p,
            &GraphQuery {
                query: Some("Unicode".into()),
                limit: 5,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(chunks(&g).len() <= 5);
    assert!(serde_json::to_vec(&g).unwrap().len() < 2 * 1024 * 1024);
    assert_eq!(g["coverage"]["selection"], "search_results");
    assert_eq!(
        c.graph(
            &p,
            &GraphQuery {
                limit: 1001,
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        "LIMIT_EXCEEDED"
    );
    assert_eq!(
        c.graph(
            &p,
            &GraphQuery {
                query: Some("x".repeat(4097)),
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        "LIMIT_EXCEEDED"
    );
    assert_eq!(
        c.graph(
            &p,
            &GraphQuery {
                cursor: Some("0.invalid".into()),
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        "SNAPSHOT_EXPIRED"
    );
}
#[test]
fn v15_native_rich_synthetic_browser_fixture() {
    let f = Fixture::new();
    f.add_memory_archive();
    let mut snapshot = f.snapshot.clone();
    snapshot.generation_id = "synthetic-browser-generation".into();
    let identity = format!(
        "synthetic-visual-fixture#{}",
        hex::encode(Sha256::digest(b"explicit synthetic visual vectors v1"))
    );
    snapshot.vectors = snapshot
        .chunks
        .iter()
        .enumerate()
        .map(|(i, c)| vrg::contracts::VectorEmbedding {
            chunk_id: c.id.clone(),
            model_id: identity.clone(),
            dimensions: 4,
            values: vec![1.0, (i % 5) as f32, (i % 7) as f32, (i % 3) as f32],
        })
        .collect();
    let store = DerivedStore::open(&f.config.data_dir).unwrap();
    store
        .commit_generation(snapshot, Some(&f.snapshot.generation_id))
        .unwrap();
    drop(store);
    let c = f.catalog();
    let g = graph(&c);
    assert!(
        g["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["kind"] == "summary")
    );
    assert!(
        g["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["kind"] == "source_revision")
    );
    assert!(chunks(&g).len() >= 48);
    let p = c.selected_project_id().unwrap();
    let semantic = c
        .graph(
            &p,
            &GraphQuery {
                mode: "semantic".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(semantic["projection"]["method"], "pca_2d");
    assert_eq!(semantic["capabilities"]["semantic_vectors"], true);
}
#[test]
fn v11_v17_auxiliary_metadata_pagination_and_membership_change() {
    let f = Fixture::new();
    f.add_memory_archive();
    let store = DerivedStore::open(&f.config.data_dir).unwrap();
    let original: Vec<StoredMemory> =
        serde_json::from_str(&store.load_adaptive_state().unwrap().unwrap()).unwrap();
    let memories: Vec<_> = (0..129)
        .map(|i| {
            let mut m = original[0].clone();
            m.memory_id = format!("summary-{i:03}");
            m
        })
        .collect();
    store
        .apply_adaptive_state(
            &f.snapshot.generation_id,
            "many-summaries",
            "many-fingerprint",
            &serde_json::to_string(&memories).unwrap(),
        )
        .unwrap();
    drop(store);
    let c = f.catalog();
    let p = c.selected_project_id().unwrap();
    let first = graph(&c);
    assert!(first["coverage"]["partial"].as_bool().unwrap());
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    assert!(cursor.starts_with("aux."));
    let second = c
        .graph(
            &p,
            &GraphQuery {
                cursor: Some(cursor.clone()),
                snapshot_id: Some(first["snapshot_id"].as_str().unwrap().into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(second["next_cursor"].is_null());
    assert!(chunks(&second).is_empty());
    let mut summaries = std::collections::BTreeSet::new();
    let mut archive_count = 0;
    for g in [&first, &second] {
        assert!(g["nodes"].as_array().unwrap().len() <= 2000);
        for n in g["nodes"].as_array().unwrap() {
            match n["kind"].as_str().unwrap() {
                "summary" => {
                    assert!(summaries.insert(n["id"].as_str().unwrap().to_owned()));
                }
                "source_revision" => archive_count += 1,
                _ => {}
            }
        }
    }
    assert_eq!(summaries.len(), 129);
    assert_eq!(archive_count, 1);
    assert_eq!(
        c.graph(
            &p,
            &GraphQuery {
                cursor: Some(cursor.replacen("aux.", "chunks.", 1)),
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        "SNAPSHOT_EXPIRED"
    );
    let doc = vault::read_source(&f.config, "test", "note-01.md", 4 * 1024 * 1024).unwrap();
    let scope = hex::encode(Sha256::digest(serde_json::to_vec(&f.config.roots).unwrap()));
    ArchiveStore::open(&f.config.data_dir)
        .unwrap()
        .archive(
            &scope,
            &adaptive::source_id(&doc.source),
            &doc.revision,
            doc.raw_markdown.as_bytes(),
        )
        .unwrap();
    assert_eq!(
        c.graph(
            &p,
            &GraphQuery {
                cursor: Some(cursor),
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        "SNAPSHOT_EXPIRED"
    );
}
#[test]
fn v07_native_payload_corruption_never_becomes_exact_preview() {
    let f = Fixture::new();
    let db = rusqlite::Connection::open(f.config.data_dir.join("vrg.sqlite3")).unwrap();
    db.execute("UPDATE text_objects SET payload=?1 WHERE object_id=(SELECT object_id FROM chunk_manifests ORDER BY chunk_id LIMIT 1)",[b"fabricated native bytes".as_slice()]).unwrap();
    drop(db);
    let c = f.catalog();
    assert_eq!(
        c.graph(&c.selected_project_id().unwrap(), &Default::default())
            .unwrap_err()
            .code,
        "SOURCE_CHANGED"
    );
}
#[test]
fn v05_empty_index_authorized_empty_and_filter_empty_are_distinct() {
    let mut f = Fixture::new();
    let c = f.catalog();
    let p = c.selected_project_id().unwrap();
    let filtered = c
        .graph(
            &p,
            &GraphQuery {
                query: Some("zzunfindablefixturezz".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        filtered["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "FILTER_EMPTY")
    );
    f.config.roots[0].allowed_files = Some(vec![]);
    f.save_config();
    let denied = graph(&c);
    assert!(
        denied["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "NO_AUTHORIZED_RESULTS")
    );
    f.config.roots[0].allowed_files = None;
    f.save_config();
    let store = DerivedStore::open(&f.config.data_dir).unwrap();
    store
        .commit_generation(
            IndexSnapshot {
                generation_id: "empty-generation".into(),
                documents: vec![],
                chunks: vec![],
                edges: vec![],
                vectors: vec![],
            },
            Some(&f.snapshot.generation_id),
        )
        .unwrap();
    drop(store);
    let empty = graph(&c);
    assert!(
        empty["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "EMPTY_INDEX")
    );
}

#[test]
fn full_graph_pages_every_indexed_chunk_and_retains_inspector() {
    let f = Fixture::new();
    let c = f.catalog();
    let project = c.selected_project_id().unwrap();
    let mut query = GraphQuery {
        mode: "full".into(),
        limit: 7,
        ..Default::default()
    };
    let mut ids = std::collections::BTreeSet::new();
    let mut edges = Vec::new();
    let mut first = None;
    let mut snapshot = String::new();
    let mut saw_edge_page = false;
    loop {
        let graph = c.graph(&project, &query).unwrap();
        assert_eq!(
            graph["coverage"]["chunks_total_indexed"],
            f.snapshot.chunks.len()
        );
        assert_eq!(
            graph["coverage"]["chunks_total_authorized"],
            f.snapshot.chunks.len()
        );
        snapshot = graph["snapshot_id"].as_str().unwrap().into();
        for n in graph["nodes"].as_array().unwrap() {
            assert_eq!(n["kind"], "chunk");
            assert!(ids.insert(n["id"].as_str().unwrap().to_owned()));
            first.get_or_insert_with(|| n["id"].as_str().unwrap().to_owned());
        }
        if graph["coverage"]["phase"] == "edges" {
            saw_edge_page = true;
        }
        edges.extend(graph["edges"].as_array().unwrap().iter().cloned());
        let Some(cursor) = graph["next_cursor"].as_str() else {
            break;
        };
        query.cursor = Some(cursor.into());
        query.snapshot_id = Some(snapshot.clone());
    }
    assert_eq!(ids.len(), f.snapshot.chunks.len());
    assert!(saw_edge_page);
    assert!(!edges.is_empty());
    for e in &edges {
        assert!(ids.contains(e["source"].as_str().unwrap()));
        assert!(ids.contains(e["target"].as_str().unwrap()));
    }
    let detail = c
        .detail(&project, &first.unwrap(), &snapshot, 0, 1024, false)
        .unwrap();
    assert!(!detail["text"].as_str().unwrap().is_empty());
}

#[test]
fn full_graph_counts_excluded_sources_and_rechecks_cached_pages() {
    let mut f = Fixture::new();
    f.config.roots[0].exclusions.push("note-00.md".into());
    f.save_config();
    let c = f.catalog();
    let id = c.selected_project_id().unwrap();
    let query = GraphQuery {
        mode: "full".into(),
        limit: 1,
        ..Default::default()
    };
    let graph = c.graph(&id, &query).unwrap();
    let denied = f
        .snapshot
        .chunks
        .iter()
        .filter(|c| c.source.relative_path == "note-00.md")
        .count();
    assert_eq!(
        graph["coverage"]["chunks_total_indexed"],
        f.snapshot.chunks.len()
    );
    assert_eq!(graph["coverage"]["chunks_denied"], denied);
    assert_eq!(
        graph["coverage"]["chunks_total_authorized"],
        f.snapshot.chunks.len() - denied
    );
    let node = &graph["nodes"][0];
    fs::write(
        f.notes
            .join(node["source"]["relative_path"].as_str().unwrap()),
        "---\nprivate: true\n---\nsecret",
    )
    .unwrap();
    assert_eq!(c.graph(&id, &query).unwrap_err().code, "ROOT_REVOKED");
}

#[test]
fn full_graph_native_chunk_ids_do_not_merge_distinct_canonical_sources() {
    let mut f = Fixture::new();
    let query = GraphQuery {
        mode: "full".into(),
        limit: 1,
        ..Default::default()
    };
    let first_catalog = f.catalog();
    let first = first_catalog
        .graph(&first_catalog.selected_project_id().unwrap(), &query)
        .unwrap();
    let other_notes = f.base.join("independent-notes");
    fs::create_dir(&other_notes).unwrap();
    for entry in fs::read_dir(&f.notes).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), other_notes.join(entry.file_name())).unwrap();
    }
    f.config.roots[0].path = other_notes.clone();
    f.save_config();
    let other_catalog = ViewCatalog::resolve(&other_notes, Some(&f.profile)).unwrap();
    let other = other_catalog
        .graph(&other_catalog.selected_project_id().unwrap(), &query)
        .unwrap();
    assert_eq!(
        first["nodes"][0]["native_chunk_id"],
        other["nodes"][0]["native_chunk_id"]
    );
    assert_ne!(
        first["nodes"][0]["canonical_source_identity"],
        other["nodes"][0]["canonical_source_identity"]
    );
    assert!(
        first["nodes"][0]["canonical_source_identity"]
            .as_str()
            .is_some_and(|s| s.len() == 64)
    );
}
