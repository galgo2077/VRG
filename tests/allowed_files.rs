use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use vrg::{config::VrgConfig, vault};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("vrg-allowed-files-{}-{unique}", std::process::id()));
        fs::create_dir_all(path.join("root")).unwrap();
        Self(path)
    }
    fn config(&self, policy: &str) -> VrgConfig {
        let text = format!(
            "data_dir = {:?}\n[[roots]]\nid = 'memory'\npath = {:?}\n{policy}\n",
            self.0.join("data").to_str().unwrap(),
            self.0.join("root").to_str().unwrap()
        );
        toml::from_str(&text).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn exact_allowlist_is_enforced_for_scan_reads_and_existing_exclusions() {
    let f = Fixture::new();
    let root = f.0.join("root");
    fs::write(root.join("STATUS.md"), "# Status\npermitted sentinel\n").unwrap();
    fs::write(root.join("DECISIONS.md"), "# Excluded\n").unwrap();
    fs::write(root.join("OTHER.md"), "# Unauthorised\n").unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/STATUS.md"), "# Nested\n").unwrap();
    #[cfg(unix)]
    {
        fs::write(f.0.join("external.md"), "# External\n").unwrap();
        std::os::unix::fs::symlink(f.0.join("external.md"), root.join("SOLUTIONS.md")).unwrap();
    }
    let c = f.config("allowed_files = ['STATUS.md', 'DECISIONS.md', 'SOLUTIONS.md']\nexclusions = ['DECISIONS.md']");
    c.validate().unwrap();
    let scan = vault::scan_roots(&c).unwrap();
    assert!(scan.issues.is_empty());
    assert_eq!(scan.documents.len(), 1);
    assert_eq!(scan.documents[0].source.relative_path, "STATUS.md");
    assert!(vault::read_source(&c, "memory", "STATUS.md", 4096).is_ok());
    for denied in [
        "OTHER.md",
        "DECISIONS.md",
        "nested/STATUS.md",
        "../external.md",
        "SOLUTIONS.md",
    ] {
        assert!(
            vault::read_source(&c, "memory", denied, 4096).is_err(),
            "{denied}"
        );
    }
    let empty = f.config("allowed_files = []");
    assert!(vault::scan_roots(&empty).unwrap().documents.is_empty());
    assert!(vault::read_source(&empty, "memory", "STATUS.md", 4096).is_err());
    let legacy = f.config("");
    assert!(vault::read_source(&legacy, "memory", "OTHER.md", 4096).is_ok());
}

#[test]
fn invalid_allowlist_paths_are_rejected() {
    let f = Fixture::new();
    for value in [
        "../secret.md",
        "/absolute.md",
        "nested/file.md",
        "nested\\file.md",
        ".",
        "not-markdown.txt",
    ] {
        let policy = format!("allowed_files = [{}]", toml::Value::String(value.into()));
        assert!(f.config(&policy).validate().is_err(), "{value}");
    }
    assert!(f
        .config("allowed_files = ['STATUS.md', 'STATUS.md']")
        .validate()
        .is_err());
}

#[test]
fn allowlist_prevents_search_and_resolved_graph_exposure() {
    use vrg::{
        contracts::{SearchSyncState, SourceRef},
        SearchRequest, VrgEngine,
    };

    let f = Fixture::new();
    let root = f.0.join("root");
    fs::write(
        root.join("STATUS.md"),
        "# Status\nALLOWEDSIGMA [[SOLUTIONS]] [[OTHER]]\n",
    )
    .unwrap();
    fs::write(root.join("SOLUTIONS.md"), "# Solutions\nAllowed target.\n").unwrap();
    fs::write(root.join("OTHER.md"), "# Other\nUNIQUEFORBIDDENSIGMA\n").unwrap();
    let engine =
        VrgEngine::open(f.config("allowed_files = ['STATUS.md', 'SOLUTIONS.md']")).unwrap();
    engine.index().unwrap();
    let search = |query: &str| {
        engine
            .search(SearchRequest {
                query: query.into(),
                limit: 10,
                root_ids: Some(vec!["memory".into()]),
                use_semantic: false,
                graph_hops: 1,
            })
            .unwrap()
    };
    let visible = search("ALLOWEDSIGMA");
    assert!(matches!(visible.sync_state, SearchSyncState::Ready));
    assert!(visible
        .hits
        .iter()
        .any(|h| h.source.relative_path == "STATUS.md"));
    let hidden = search("UNIQUEFORBIDDENSIGMA");
    assert!(matches!(hidden.sync_state, SearchSyncState::Ready));
    assert!(hidden.hits.is_empty());
    assert!(engine.read_source("memory", "OTHER.md", 4096).is_err());
    let edges = engine
        .neighbors(
            &SourceRef {
                root_id: "memory".into(),
                relative_path: "STATUS.md".into(),
            },
            32,
        )
        .unwrap();
    assert!(edges.iter().any(|e| {
        e.to.as_ref()
            .is_some_and(|s| s.relative_path == "SOLUTIONS.md")
    }));
    assert!(!edges.iter().any(|e| e.from.relative_path == "OTHER.md"
        || e.to.as_ref().is_some_and(|s| s.relative_path == "OTHER.md")));
    // A link written in an allowed note can remain as an unresolved target; the
    // excluded note itself must not be exposed as a resolved graph node.
    assert!(engine
        .neighbors(
            &SourceRef {
                root_id: "memory".into(),
                relative_path: "OTHER.md".into()
            },
            32
        )
        .unwrap()
        .is_empty());
}
