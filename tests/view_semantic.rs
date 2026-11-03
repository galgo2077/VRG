use vrg::view_semantic::{InputVector, analyze, analyze_full_cancellable};

fn model(suffix: char) -> String {
    format!("fixture/model#{}", suffix.to_string().repeat(64))
}
fn vector(id: &str, values: &[f32]) -> InputVector {
    InputVector {
        id: id.into(),
        model_identity: model('a'),
        values: values.to_vec(),
    }
}

#[test]
fn cosine_top_k_is_exact_bounded_deterministic_union() {
    let input = vec![
        vector("b", &[1., 0.]),
        vector("a", &[1., 0.]),
        vector("c", &[0.8, 0.6]),
        vector("d", &[-1., 0.]),
    ];
    let output = analyze(&input, 1, 0.75);
    assert_eq!(output.edges.len(), 2); // a-b plus c-a (stable tie)
    assert!(
        output
            .edges
            .iter()
            .any(|e| e.source == "a" && e.target == "b" && (e.score - 1.).abs() < 1e-12)
    );
    assert!(
        output
            .edges
            .iter()
            .any(|e| e.source == "a" && e.target == "c" && (e.score - 0.8).abs() < 1e-6)
    );
    assert_eq!(output.components["a"], output.components["c"]);
    assert_ne!(output.components["a"], output.components["d"]);
    assert!(analyze(&input, 0, -1.).edges.is_empty());
}

#[test]
fn invalid_and_incompatible_vectors_never_mix() {
    let mut input = vec![
        vector("a", &[1., 0.]),
        vector("nan", &[f32::NAN, 1.]),
        vector("zero", &[0., 0.]),
        vector("inf", &[f32::INFINITY, 0.]),
        vector("empty", &[]),
    ];
    let mut other_model = vector("other", &[1., 0.]);
    other_model.model_identity = model('b');
    input.push(other_model);
    input.push(vector("dimension", &[1., 0., 0.]));
    let mut bare = vector("bare", &[1., 0.]);
    bare.model_identity = "commercial/model".into();
    input.push(bare);
    input.push(vector("duplicate", &[1., 0.]));
    input.push(vector("duplicate", &[1., 0.]));
    let output = analyze(&input, 50, -1.);
    assert_eq!(output.excluded, 7);
    assert_eq!(output.groups.len(), 3);
    assert!(output.edges.is_empty());
    assert_eq!(output.positions.len(), 3);
    assert!(
        output
            .positions
            .values()
            .all(|p| p.x.is_finite() && p.y.is_finite())
    );
}

#[test]
fn pca_centered_deterministic_with_degenerate_cases() {
    assert!(analyze(&[], 6, 0.75).groups.is_empty());
    let one = analyze(&[vector("a", &[2., 4.])], 6, 0.75);
    assert_eq!(one.positions["a"].x, 0.);
    assert_eq!(one.positions["a"].y, 0.);
    assert!(one.groups[0].degenerate);
    let two_input = [vector("a", &[1., 0.]), vector("b", &[-1., 0.])];
    let two = analyze(&two_input, 6, -1.);
    assert!((two.positions["a"].x - 1.).abs() < 1e-9);
    assert!((two.positions["b"].x + 1.).abs() < 1e-9);
    assert_eq!(two.positions["a"].y, 0.);
    assert!((two.groups[0].explained_variance[0] - 1.).abs() < 1e-9);
    let identical = analyze(&[vector("a", &[1., 3.]), vector("b", &[1., 3.])], 6, 0.75);
    assert!(identical.groups[0].degenerate);
    assert!(
        identical
            .positions
            .values()
            .all(|p| p.x.abs() < 1e-9 && p.y.abs() < 1e-9)
    );
    let input = [
        vector("a", &[1., 0., 0.]),
        vector("b", &[0., 1., 0.]),
        vector("c", &[-1., 0., 0.]),
        vector("d", &[0., -1., 0.]),
    ];
    let a = analyze(&input, 6, 0.75);
    let mut reverse = input.to_vec();
    reverse.reverse();
    let b = analyze(&reverse, 6, 0.75);
    for id in ["a", "b", "c", "d"] {
        assert!((a.positions[id].x - b.positions[id].x).abs() < 1e-9);
        assert!((a.positions[id].y - b.positions[id].y).abs() < 1e-9);
    }
    assert!((a.groups[0].explained_variance.iter().sum::<f64>() - 1.).abs() < 1e-9);
}

#[test]
#[ignore = "GV4 explicit real local model integration; requires VRG_VIEW_TEST_MODEL"]
fn real_local_embeddings_integration() {
    use std::path::PathBuf;
    use vrg::{
        config::SemanticModelConfig,
        contracts::{SourceRef, TextChunk},
        semantic::LocalEmbedder,
    };
    let path = PathBuf::from(
        std::env::var_os("VRG_VIEW_TEST_MODEL")
            .expect("explicit local model fixture path required"),
    );
    let embedder = LocalEmbedder::load(&SemanticModelConfig {
        model_id: "intfloat/multilingual-e5-small".into(),
        model_dir: path,
        threads: 2,
        max_length: 512,
    })
    .unwrap();
    let texts = [
        "Synthetic fixture: SQLite snapshots isolate readers from writers.",
        "Synthetic fixture: SQLite transactions provide consistent database snapshots.",
        "Synthetic fixture: a red apple grows on a tree.",
    ];
    let chunks: Vec<_> = texts
        .iter()
        .enumerate()
        .map(|(ordinal, text)| TextChunk {
            id: format!("chunk-{ordinal}"),
            source: SourceRef {
                root_id: "fixture".into(),
                relative_path: format!("synthetic-{ordinal}.md"),
            },
            revision: "a".repeat(64),
            ordinal: ordinal as u32,
            start_byte: 0,
            end_byte: text.len() as u32,
            text: (*text).into(),
        })
        .collect();
    let mut chunks = chunks;
    use sha2::{Digest, Sha256};
    use std::{
        collections::BTreeMap,
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    use vrg::{
        contracts::{IndexSnapshot, StoredDocument},
        storage::DerivedStore,
        view_data::{GraphQuery, ViewCatalog},
    };
    let fixture = std::env::temp_dir().join(format!(
        "vrg-view-real-semantic-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if std::env::var_os("VRG_VIEW_KEEP_FIXTURE").as_deref()
                == Some(std::ffi::OsStr::new("1"))
            {
                println!(
                    "real_semantic_fixture={} profile={} notes={}",
                    self.0.display(),
                    self.0.join("profile.toml").display(),
                    self.0.join("notes").display()
                );
            } else {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    fs::create_dir(&fixture).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let _cleanup = Cleanup(fixture.clone());
    let notes = fixture.join("notes");
    fs::create_dir(&notes).unwrap();
    let mut documents = Vec::new();
    for chunk in &mut chunks {
        fs::write(notes.join(&chunk.source.relative_path), &chunk.text).unwrap();
        chunk.revision = hex::encode(Sha256::digest(chunk.text.as_bytes()));
        documents.push(StoredDocument {
            source: chunk.source.clone(),
            revision: chunk.revision.clone(),
            title: chunk.source.relative_path.clone(),
            frontmatter: BTreeMap::new(),
            modified_unix_ms: None,
        });
    }
    let embeddings = embedder.embed_chunks(&chunks).unwrap();
    assert_eq!(embeddings.len(), texts.len());
    let vectors: Vec<_> = embeddings
        .iter()
        .map(|v| InputVector {
            id: v.chunk_id.clone(),
            model_identity: v.model_id.clone(),
            values: v.values.clone(),
        })
        .collect();
    let analysis = analyze(&vectors, 2, -1.);
    assert_eq!(analysis.excluded, 0);
    assert_eq!(analysis.positions.len(), 3);
    assert_eq!(analysis.edges.len(), 3);
    assert_eq!(analysis.groups[0].dimensions, 384);
    let related = analysis
        .edges
        .iter()
        .find(|e| e.source == "chunk-0" && e.target == "chunk-1")
        .unwrap()
        .score;
    let unrelated = analysis
        .edges
        .iter()
        .find(|e| e.source == "chunk-0" && e.target == "chunk-2")
        .unwrap()
        .score;
    assert!(
        related > unrelated,
        "fixture expected related passages to outrank unrelated passage"
    );
    // Publish only the synthetic fixture, then prove that the same production
    // read-only adapter reads real stored embeddings with inference absent.
    let store_path = fixture.join("store");
    let store = DerivedStore::open(&store_path).unwrap();
    store
        .commit_generation(
            IndexSnapshot {
                generation_id: "real-semantic-fixture".into(),
                documents,
                chunks,
                edges: Vec::new(),
                vectors: embeddings,
            },
            None,
        )
        .unwrap();
    drop(store);
    let config_path = fixture.join("profile.toml");
    fs::write(
        &config_path,
        format!(
            "data_dir={}\n[[roots]]\nid=\"fixture\"\npath={}\n",
            serde_json::to_string(&store_path).unwrap(),
            serde_json::to_string(&notes).unwrap()
        ),
    )
    .unwrap();
    let hash_before = Sha256::digest(fs::read(store_path.join("vrg.sqlite3")).unwrap());
    let catalog = ViewCatalog::resolve(&notes, Some(&config_path)).unwrap();
    let project = catalog
        .selected_project_id()
        .expect("explicit fixture profile must resolve");
    let graph = catalog
        .graph(
            &project,
            &GraphQuery {
                snapshot_id: None,
                cursor: None,
                query: None,
                limit: 500,
                mode: "semantic".into(),
            },
        )
        .unwrap();
    assert_eq!(graph["capabilities"]["semantic_vectors"], true);
    assert_eq!(graph["projection"]["method"], "pca_2d");
    let projected = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "chunk" && n["position"].is_object())
        .count();
    assert_eq!(projected, 3);
    assert_eq!(
        Sha256::digest(fs::read(store_path.join("vrg.sqlite3")).unwrap()),
        hash_before
    );
    println!(
        "real_store_adapter=PASS stored_vectors=3 projected_chunks={} readonly_database_hash=unchanged",
        projected
    );
    println!(
        "real_model={} vectors={} dimensions={} related_cosine={:.6} unrelated_cosine={:.6} pca_explained={:?}",
        embedder.model_id(),
        vectors.len(),
        analysis.groups[0].dimensions,
        related,
        unrelated,
        analysis.groups[0].explained_variance
    );
}

#[test]
fn cancelled_calculation_discards_partial_result() {
    use std::cell::Cell;
    let input: Vec<_> = (0..100)
        .map(|i| vector(&format!("n-{i:03}"), &[1., 0.]))
        .collect();
    let checks = Cell::new(0);
    let output = vrg::view_semantic::analyze_cancellable(&input, 6, 0.75, || {
        checks.set(checks.get() + 1);
        checks.get() > 120
    });
    assert!(output.is_none());
    assert!(vrg::view_semantic::analyze_cancellable(&input, 6, 0.75, || true).is_none());
    assert!(vrg::view_semantic::analyze_cancellable(&input, 6, 0.75, || false).is_some());
}

#[test]
fn full_ann_retains_entire_index_and_is_permutation_stable() {
    // Cross the old 2,000-vector sample boundary with real, distinct fixtures.
    // This checks coverage and verified cosine, not an exact-neighbor recall
    // promise: the full path explicitly uses approximate candidate retrieval.
    let mut input: Vec<_> = (0..2_053)
        .map(|index| {
            let angle = index as f32 * 0.013;
            vector(
                &format!("full-{index:05}"),
                &[
                    1.0,
                    angle.sin() * 0.1,
                    angle.cos() * 0.1,
                    (angle * 2.3).sin() * 0.1,
                    (angle * 1.7).cos() * 0.1,
                    (index % 17) as f32 * 0.001,
                    (index % 23) as f32 * 0.001,
                    0.04,
                ],
            )
        })
        .collect();
    let output = analyze_full_cancellable(&input, 4, 0.75, || false).unwrap();
    assert_eq!(output.positions.len(), input.len());
    assert_eq!(output.components.len(), input.len());
    assert_eq!(
        output.groups.iter().map(|g| g.count).sum::<usize>(),
        input.len()
    );
    assert_eq!(output.excluded, 0);
    assert!(!output.truncated);
    assert!(!output.edges.is_empty());
    assert!(output.edges.len() <= input.len() * 4);
    assert!(output.note.contains("ANN"));
    assert!(
        output
            .positions
            .values()
            .all(|p| p.x.is_finite() && p.y.is_finite())
    );
    let by_id: std::collections::BTreeMap<_, _> =
        input.iter().map(|v| (v.id.as_str(), v)).collect();
    for edge in &output.edges {
        let left = by_id[edge.source.as_str()];
        let right = by_id[edge.target.as_str()];
        assert_ne!(left.id, right.id);
        assert_eq!(edge.model_identity, left.model_identity);
        assert_eq!(left.model_identity, right.model_identity);
        let dot: f64 = left
            .values
            .iter()
            .zip(&right.values)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum();
        let norm = |values: &[f32]| {
            values
                .iter()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
                .sqrt()
        };
        let exact = (dot / (norm(&left.values) * norm(&right.values))).clamp(-1., 1.);
        assert!(edge.score.is_finite() && edge.score >= output.threshold);
        assert!((edge.score - exact).abs() < 1e-12);
    }
    input.reverse();
    let reverse = analyze_full_cancellable(&input, 4, 0.75, || false).unwrap();
    assert_eq!(
        serde_json::to_value(output).unwrap(),
        serde_json::to_value(reverse).unwrap()
    );
}

#[test]
fn full_ann_excludes_invalid_vectors_and_separates_compatible_domains() {
    let mut input = vec![
        vector("a", &[1., 0.]),
        vector("b", &[1., 0.]),
        vector("c", &[1., 0.]),
        vector("d", &[1., 0.]),
        vector("e", &[1., 0., 0.]),
        vector("f", &[1., 0., 0.]),
        vector("nan", &[f32::NAN, 1.]),
        vector("zero", &[0., 0.]),
        vector("inf", &[f32::INFINITY, 0.]),
        vector("empty", &[]),
        vector("duplicate", &[1., 0.]),
        vector("duplicate", &[1., 0.]),
        vector("bare", &[1., 0.]),
    ];
    input[2].model_identity = model('b');
    input[3].model_identity = model('b');
    input[12].model_identity = "unfingerprinted/model".into();
    let output = analyze_full_cancellable(&input, 6, 0.75, || false).unwrap();
    assert_eq!(output.excluded, 7);
    assert_eq!(output.groups.len(), 3);
    assert_eq!(output.positions.len(), 6);
    assert_eq!(output.edges.len(), 3);
    let pairs: std::collections::BTreeSet<_> = output
        .edges
        .iter()
        .map(|e| (e.source.as_str(), e.target.as_str()))
        .collect();
    assert_eq!(pairs, [("a", "b"), ("c", "d"), ("e", "f")].into());
    assert!(output.edges.iter().all(|e| (e.score - 1.).abs() < 1e-12));
    assert_ne!(output.components["a"], output.components["c"]);
    assert_ne!(output.components["a"], output.components["e"]);
    assert!(
        output
            .positions
            .values()
            .all(|p| p.x.is_finite() && p.y.is_finite())
    );
    assert!(
        analyze_full_cancellable(&[], 4, 0.75, || false)
            .unwrap()
            .groups
            .is_empty()
    );
    assert!(
        analyze_full_cancellable(&input, 0, 0.75, || false)
            .unwrap()
            .edges
            .is_empty()
    );
}

#[test]
fn full_ann_cancellation_discards_work_during_index_construction() {
    use std::cell::Cell;
    let input: Vec<_> = (0..128)
        .map(|i| vector(&format!("cancel-{i:03}"), &[1., i as f32 * 0.001]))
        .collect();
    assert!(analyze_full_cancellable(&input, 4, 0.75, || true).is_none());
    let checks = Cell::new(0);
    let stop_after = input.len() + 100;
    let result = analyze_full_cancellable(&input, 4, 0.75, || {
        checks.set(checks.get() + 1);
        checks.get() > stop_after
    });
    assert!(checks.get() > stop_after);
    assert!(
        result.is_none(),
        "a partial forest or graph must never be returned"
    );
}
