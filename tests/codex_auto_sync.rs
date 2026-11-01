use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("vrg-auto-sync-{}-{stamp}-{id}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn config(&self, roots: &[(&str, &Path)]) -> PathBuf {
        let mut text = format!(
            "data_dir = {:?}\nchunk_bytes = 128\nchunk_overlap_bytes = 16\n",
            self.0.join("derived")
        );
        for (id, path) in roots {
            text.push_str(&format!("\n[[roots]]\nid = {:?}\npath = {:?}\n", id, path));
        }
        let path = self.0.join("config.toml");
        fs::write(&path, text).unwrap();
        path
    }

    fn config_with_memory(&self, roots: &[(&str, &Path)]) -> PathBuf {
        let path = self.config(roots);
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str("\ncapture_session_events = true\nsession_event_retention_days = 30\n");
        fs::write(&path, text).unwrap();
        path
    }

    fn config_with_model(&self, roots: &[(&str, &Path)], model_dir: &Path) -> PathBuf {
        let path = self.config(roots);
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str(&format!(
            "\n[semantic_model]\nmodel_id = \"intfloat/multilingual-e5-small\"\nmodel_dir = {:?}\nthreads = 2\nmax_length = 512\n",
            model_dir
        ));
        fs::write(&path, text).unwrap();
        path
    }

    fn run(
        &self,
        config: &Path,
        session: &str,
        turn: &str,
        concurrent_start: bool,
    ) -> std::process::Child {
        self.run_with_env(config, session, turn, concurrent_start, &[])
    }

    fn run_with_env(
        &self,
        config: &Path,
        session: &str,
        turn: &str,
        concurrent_start: bool,
        envs: &[(&str, &str)],
    ) -> std::process::Child {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vrg"))
            .arg("--config")
            .arg(config)
            .arg("codex-stop")
            .envs(envs.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let payload = format!(
            "{{\"hook_event_name\":\"Stop\",\"session_id\":{session:?},\"turn_id\":{turn:?},\"cwd\":{cwd:?},\"stop_hook_active\":false}}",
            cwd = self.0,
        );
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(payload.as_bytes()).unwrap();
        drop(stdin);
        if concurrent_start {
            thread::sleep(Duration::from_millis(10));
        }
        child
    }

    fn start_session(&self, config: &Path, session: &str) -> std::process::Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vrg"))
            .arg("--config")
            .arg(config)
            .arg("codex-start")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let payload = serde_json::json!({
            "hook_event_name":"SessionStart", "session_id":session,
            "source":"resume", "cwd":self.0.to_string_lossy(),
        });
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn wait_for_barrier(&self, directory: &Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let ready = fs::read_dir(directory)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .any(|entry| entry.file_name().to_string_lossy().starts_with("ready-"))
                })
                .unwrap_or(false);
            if ready {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "indexer did not reach the F4 barrier"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn status(&self, config: &Path) -> serde_json::Value {
        let result = Command::new(env!("CARGO_BIN_EXE_vrg"))
            .arg("--config")
            .arg(config)
            .arg("status")
            .output()
            .unwrap();
        assert!(result.status.success(), "vrg status failed");
        serde_json::from_slice(&result.stdout).unwrap()
    }
}

struct McpClient {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<std::process::ChildStdout>,
}

impl McpClient {
    fn new(config: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vrg"))
            .arg("--config")
            .arg(config)
            .args(["mcp", "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self {
            input: Some(child.stdin.take().unwrap()),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }

    fn request(&mut self, id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
        let input = self.input.as_mut().unwrap();
        writeln!(
            input,
            "{}",
            serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        input.flush().unwrap();
        loop {
            let mut line = String::new();
            self.output.read_line(&mut line).unwrap();
            let response: serde_json::Value = serde_json::from_str(&line).unwrap();
            if response.get("id").and_then(serde_json::Value::as_u64) == Some(id) {
                return response;
            }
        }
    }

    fn request_with_timeout(
        &mut self,
        id: u64,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> serde_json::Value {
        let input = self.input.as_mut().unwrap();
        writeln!(
            input,
            "{}",
            serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        input.flush().unwrap();

        let (sender, receiver) = std::sync::mpsc::channel::<Result<serde_json::Value, String>>();
        let output = &mut self.output;
        let child = &mut self.child;
        let response = std::thread::scope(|scope| {
            scope.spawn(move || {
                loop {
                    let mut line = String::new();
                    let response = match output.read_line(&mut line) {
                        Ok(0) => Err("MCP stdio closed before responding".to_owned()),
                        Ok(_) => serde_json::from_str::<serde_json::Value>(&line)
                            .map_err(|error| format!("MCP returned invalid JSON: {error}")),
                        Err(error) => Err(format!("MCP stdio read failed: {error}")),
                    };
                    match response {
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            break;
                        }
                        Ok(response)
                            if response.get("id").and_then(serde_json::Value::as_u64)
                                == Some(id) =>
                        {
                            let _ = sender.send(Ok(response));
                            break;
                        }
                        Ok(_) => {}
                    }
                }
            });

            match receiver.recv_timeout(timeout) {
                Ok(response) => response,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let _ = child.kill();
                    Err(format!(
                        "MCP request {id} ({method}) exceeded readiness timeout {timeout:?}"
                    ))
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(format!(
                    "MCP request {id} ({method}) response reader stopped"
                )),
            }
        });
        response.unwrap_or_else(|error| panic!("{error}"))
    }

    fn notify(&mut self, method: &str) {
        let input = self.input.as_mut().unwrap();
        writeln!(
            input,
            "{}",
            serde_json::json!({"jsonrpc":"2.0","method":method})
        )
        .unwrap();
        input.flush().unwrap();
    }

    fn shutdown(&mut self) -> std::process::ExitStatus {
        self.input.take();
        self.child.wait().unwrap()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn empty_roots_skip_without_creating_derived_storage() {
    let fixture = Fixture::new();
    let config = fixture.config(&[]);
    let output = fixture
        .run(&config, "session-empty", "turn-empty", false)
        .wait_with_output()
        .unwrap();
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        response["systemMessage"]
            .as_str()
            .unwrap()
            .contains("no authorized source roots")
    );
    assert!(!fixture.0.join("derived").exists());
}

#[test]
fn stop_sync_is_incremental_deduplicated_and_preserves_last_generation_on_failure() {
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir_all(&root).unwrap();
    let first = root.join("first.md");
    let second = root.join("second.md");
    fs::write(&first, "# Alpha\noriginal exact token\nRelated: [[Beta]]\n").unwrap();
    fs::write(&second, "# Beta\nremove this note\n").unwrap();
    let first_bytes = fs::read(&first).unwrap();
    let config = fixture.config(&[("synthetic", &root)]);

    let initial = fixture
        .run(&config, "session-main", "turn-1", false)
        .wait_with_output()
        .unwrap();
    assert!(initial.status.success());
    assert_eq!(fs::read(&first).unwrap(), first_bytes);
    let generation_1 = fixture.status(&config)["generation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(fixture.status(&config)["index_sync"]["state"], "up_to_date");

    fs::write(&first, "# Alpha\nchanged exact token\nRelated: [[Beta]]\n").unwrap();
    let duplicate = fixture
        .run(&config, "session-main", "turn-1", false)
        .wait_with_output()
        .unwrap();
    assert!(duplicate.status.success());
    assert_eq!(fixture.status(&config)["generation_id"], generation_1);

    let renamed = root.join("renamed.md");
    fs::rename(&first, &renamed).unwrap();
    fs::remove_file(&second).unwrap();
    let updated = fixture
        .run(&config, "session-main", "turn-2", false)
        .wait_with_output()
        .unwrap();
    assert!(updated.status.success());
    let generation_2 = fixture.status(&config)["generation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(generation_2, generation_1);
    assert_eq!(fixture.status(&config)["index_sync"]["state"], "up_to_date");
    let search = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .args(["search", "renamed", "--text-only"])
        .output()
        .unwrap();
    assert!(search.status.success());
    let search_json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(
        search_json["hits"][0]["source"]["relative_path"],
        "renamed.md"
    );
    assert_eq!(
        fs::read(&renamed).unwrap(),
        b"# Alpha\nchanged exact token\nRelated: [[Beta]]\n"
    );
    assert!(!root.join("second.md").exists());
    assert_eq!(
        fs::read(&renamed).unwrap(),
        b"# Alpha\nchanged exact token\nRelated: [[Beta]]\n"
    );
    let graph = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .args(["graph", "synthetic", "renamed.md"])
        .output()
        .unwrap();
    let graph: serde_json::Value = serde_json::from_slice(&graph.stdout).unwrap();
    assert_eq!(graph[0]["unresolved_target"], "Beta");

    let invalid_utf8 = root.join("invalid.md");
    fs::write(&invalid_utf8, [0xff, 0xfe, 0xfd]).unwrap();
    #[cfg(unix)]
    {
        let failed = fixture
            .run(&config, "session-main", "turn-3", false)
            .wait_with_output()
            .unwrap();
        assert!(failed.status.success());
        let failed_status = fixture.status(&config);
        assert_eq!(failed_status["generation_id"], generation_2);
        assert_eq!(failed_status["index_sync"]["state"], "failed");
        assert_eq!(
            fs::read(&renamed).unwrap(),
            b"# Alpha\nchanged exact token\nRelated: [[Beta]]\n"
        );
        assert_eq!(fs::read(&invalid_utf8).unwrap(), [0xff, 0xfe, 0xfd]);
        let no_stale = Command::new(env!("CARGO_BIN_EXE_vrg"))
            .arg("--config")
            .arg(&config)
            .args(["search", "changed exact token", "--text-only"])
            .output()
            .unwrap();
        let no_stale: serde_json::Value = serde_json::from_slice(&no_stale.stdout).unwrap();
        assert_eq!(no_stale["sync_state"], "error");
        assert!(no_stale["hits"].as_array().unwrap().is_empty());
        let stale = Command::new(env!("CARGO_BIN_EXE_vrg"))
            .arg("--config")
            .arg(&config)
            .args([
                "search",
                "changed exact token",
                "--text-only",
                "--allow-stale",
            ])
            .output()
            .unwrap();
        let stale: serde_json::Value = serde_json::from_slice(&stale.stdout).unwrap();
        assert_eq!(stale["sync_state"], "degraded");
        assert_eq!(stale["generation_id"], generation_2);
        assert!(!stale["hits"].as_array().unwrap().is_empty());
    }
}

#[test]
fn concurrent_distinct_stop_events_preserve_a_single_consistent_generation() {
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("large.md"),
        format!("# Concurrent\n{}\n", "stable evidence ".repeat(200_000)),
    )
    .unwrap();
    let config = fixture.config(&[("synthetic", &root)]);
    let first = fixture.run(&config, "session-a", "turn-a", true);
    let second = fixture.run(&config, "session-b", "turn-b", false);
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert!(first.status.success() && second.status.success());
    assert!(serde_json::from_slice::<serde_json::Value>(&first.stdout).is_ok());
    assert!(serde_json::from_slice::<serde_json::Value>(&second.stdout).is_ok());
    let status = fixture.status(&config);
    assert_eq!(status["index_sync"]["state"], "up_to_date");
    assert!(status["generation_id"].as_str().is_some());
}

#[test]
fn filesystem_notifications_reconcile_external_edits_and_deletions() {
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir_all(&root).unwrap();
    let config_path = fixture.config(&[("synthetic", &root)]);
    let config = vrg::config::VrgConfig::load(&config_path).unwrap();
    let engine = vrg::VrgEngine::open(config).unwrap();
    engine.start_change_monitor().unwrap();
    engine.reconcile_at_startup().unwrap();

    let note = root.join("external.md");
    let original = b"# External update\nwatcher sentinel phrase\n";
    fs::write(&note, original).unwrap();
    let request = || vrg::contracts::SearchRequest {
        query: "watcher sentinel".into(),
        limit: 5,
        root_ids: None,
        use_semantic: false,
        graph_hops: 0,
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        let result = engine.search(request()).unwrap();
        if result
            .hits
            .iter()
            .any(|hit| hit.source.relative_path == "external.md")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "external filesystem change was not reconciled"
        );
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(fs::read(&note).unwrap(), original);
    fs::remove_file(&note).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        let result = engine.search(request()).unwrap();
        if result.hits.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "external deletion was not reconciled"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn mcp_search_caps_long_returned_snippets() {
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir_all(&root).unwrap();
    let source = format!(
        "{} TARGETSNIPPETMARKER {}\n",
        "a".repeat(1_200),
        "b".repeat(1_200)
    );
    fs::write(root.join("long.md"), &source).unwrap();
    let config = fixture.0.join("config.toml");
    fs::write(
        &config,
        format!(
            "data_dir = {:?}\nchunk_bytes = 4096\nchunk_overlap_bytes = 0\n\n[[roots]]\nid = {:?}\npath = {:?}\n",
            fixture.0.join("derived"),
            "synthetic",
            root
        ),
    )
    .unwrap();

    let mut client = McpClient::new(&config);
    let init = client.request(
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion":"2025-03-26",
            "capabilities":{},
            "clientInfo":{"name":"vrg-snippet-bound","version":"1"}
        }),
    );
    assert!(init.get("result").is_some());
    client.notify("notifications/initialized");
    let response = client.request(
        2,
        "tools/call",
        serde_json::json!({
            "name":"vrg_search",
            "arguments":{"query":"TARGETSNIPPETMARKER","limit":1,"semantic":false}
        }),
    );
    let response_text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("MCP search must return JSON text");
    let result: serde_json::Value = serde_json::from_str(response_text).unwrap();
    assert_eq!(result["sync_state"], "ready");
    let hits = result["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    let snippet = hits[0]["snippet"].as_str().unwrap();
    assert!(snippet.len() <= 512);
    assert!(snippet.contains("TARGETSNIPPETMARKER"));
    assert_eq!(fs::read_to_string(root.join("long.md")).unwrap(), source);
    assert!(client.shutdown().success());
}

#[test]
fn mcp_query_reconciles_after_start_and_rejects_out_of_root_reads() {
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir_all(&root).unwrap();
    let config = fixture.config(&[("synthetic", &root)]);
    let mut client = McpClient::new(&config);
    let init = client.request(1, "initialize", serde_json::json!({
        "protocolVersion":"2025-03-26", "capabilities":{}, "clientInfo":{"name":"vrg-test","version":"0.1"}
    }));
    assert!(
        init.get("result").is_some(),
        "MCP initialize failed: {init}"
    );
    client.notify("notifications/initialized");
    let tools = client.request(2, "tools/list", serde_json::json!({}));
    let names = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for expected in [
        "vrg_status",
        "vrg_search",
        "vrg_read_source",
        "vrg_graph_neighbors",
    ] {
        assert!(names.contains(&expected), "missing MCP tool {expected}");
    }

    let note = root.join("after-start.md");
    fs::write(&note, "# Fresh MCP\nquery freshness sentinel\n").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let result =
        loop {
            let search = client.request(3, "tools/call", serde_json::json!({
            "name":"vrg_search", "arguments":{"query":"freshness sentinel", "semantic":false}
        }));
            let result_text = search["result"]["content"][0]["text"].as_str().unwrap();
            let result: serde_json::Value = serde_json::from_str(result_text).unwrap();
            if result["sync_state"] == "ready" {
                break result;
            }
            assert_eq!(result["sync_state"], "sync_pending");
            assert!(
                result["hits"].as_array().unwrap().is_empty(),
                "stale MCP results were returned without opt-in"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "MCP freshness reconciliation did not finish"
            );
            thread::sleep(Duration::from_millis(50));
        };
    assert_eq!(
        result["hits"][0]["source"]["relative_path"],
        "after-start.md"
    );

    let denied = client.request(4, "tools/call", serde_json::json!({
        "name":"vrg_read_source", "arguments":{"root_id":"synthetic", "relative_path":"../outside.md"}
    }));
    assert!(
        denied.get("error").is_some(),
        "out-of-root read was not rejected"
    );
    assert!(
        client.shutdown().success(),
        "MCP server did not stop cleanly after stdio disconnect"
    );
}

#[test]
fn opt_in_session_memory_persists_only_observed_event_metadata() {
    let fixture = Fixture::new();
    let config = fixture.config_with_memory(&[]);
    let event = serde_json::json!({
        "hook_event_name":"Stop", "session_id":"session-memory", "turn_id":"turn-memory",
        "cwd":fixture.0, "stop_hook_active":false, "last_assistant_message":"must not be captured"
    });
    let output = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .arg("codex-stop")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = output;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());

    let status = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .args(["memory", "status"])
        .output()
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["capture_enabled"], true);
    assert_eq!(status["stored_events"], 1);
    let events = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .args(["memory", "events"])
        .current_dir(&fixture.0)
        .output()
        .unwrap();
    let events: serde_json::Value = serde_json::from_slice(&events.stdout).unwrap();
    assert_eq!(events[0]["session_id"], "session-memory");
    assert_eq!(events[0]["turn_id"], "turn-memory");
    assert_eq!(events[0]["evidence_state"], "observed");
    assert!(events[0].get("last_assistant_message").is_none());

    let cleared = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .args(["memory", "clear", "--confirm"])
        .output()
        .unwrap();
    let cleared: serde_json::Value = serde_json::from_slice(&cleared.stdout).unwrap();
    assert_eq!(cleared["deleted_events"], 1);
}

#[test]
fn startup_hook_recovers_an_interrupted_sync_and_degraded_text_search_remains_available() {
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("entry.md"),
        "# Recovery\nrecovery lexical sentinel\n",
    )
    .unwrap();
    let config = fixture.config(&[("synthetic", &root)]);
    let first = fixture
        .run(&config, "session-recovery", "turn-1", false)
        .wait_with_output()
        .unwrap();
    assert!(first.status.success());
    let db = fixture.0.join("derived/vrg.sqlite3");
    let connection = rusqlite::Connection::open(db).unwrap();
    connection
        .execute(
            "UPDATE index_sync_status SET state='indexing' WHERE singleton=1",
            [],
        )
        .unwrap();
    drop(connection);
    fs::write(
        root.join("entry.md"),
        "# Recovery\nupdated during downtime sentinel\n",
    )
    .unwrap();

    let event = serde_json::json!({"hook_event_name":"SessionStart", "session_id":"session-recovery", "source":"resume", "cwd":fixture.0});
    let mut child = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .arg("codex-start")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());

    let status = fixture.status(&config);
    assert_eq!(status["index_sync"]["state"], "up_to_date");
    assert!(
        status["semantic"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("no local semantic model")
    );
    let search = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .args(["search", "recovery lexical sentinel"])
        .output()
        .unwrap();
    let search: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(search["sync_state"], "ready");
    assert_eq!(search["hits"][0]["source"]["relative_path"], "entry.md");
    assert!(
        search["hits"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("updated during downtime")
    );
    assert_eq!(search["mode"], "degraded_text_only");
}

#[test]
fn f4_01_private_metadata_and_external_paths_are_excluded() {
    use sha2::Digest;

    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    let outside = fixture.0.join("outside");
    let excluded_dir = root.join("excluded");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(&excluded_dir).unwrap();

    let allowed = root.join("allowed.md");
    let private_flag = root.join("private-flag.md");
    let private_tag = root.join("private-tag.md");
    let private_classification = root.join("private-classification.md");
    let private_vrg = root.join("private-vrg.md");
    let excluded_note = excluded_dir.join("secret.md");
    let outside_note = outside.join("outside.md");
    fs::write(&allowed, "---\ntitle: Public note\nprivate: false\nprivacy: public\ntags: [reference]\n---\nPublic fixture content.\n").unwrap();
    fs::write(
        &private_flag,
        "---\nprivate: true\n---\nprivate marker body sentinel\n",
    )
    .unwrap();
    fs::write(
        &private_tag,
        "---\ntags: [project, confidential-note]\n---\nconfidential tag body sentinel\n",
    )
    .unwrap();
    fs::write(
        &private_classification,
        "---\nclassification: personal\n---\npersonal body sentinel\n",
    )
    .unwrap();
    fs::write(
        &private_vrg,
        "---\nvrg:\n  exclude: yes\n---\nvrg excluded body sentinel\n",
    )
    .unwrap();
    fs::write(
        &excluded_note,
        "# Excluded directory\nexcluded path sentinel\n",
    )
    .unwrap();
    fs::write(&outside_note, "# Outside\nexternal-only sentinel\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("external-link")).unwrap();

    let source_bytes_before = [
        fs::read(&allowed).unwrap(),
        fs::read(&private_flag).unwrap(),
        fs::read(&private_tag).unwrap(),
        fs::read(&private_classification).unwrap(),
        fs::read(&private_vrg).unwrap(),
        fs::read(&excluded_note).unwrap(),
    ];
    let outside_before = fs::read(&outside_note).unwrap();
    for (path, bytes) in [
        ("allowed.md", &source_bytes_before[0]),
        ("private-flag.md", &source_bytes_before[1]),
        ("private-tag.md", &source_bytes_before[2]),
        ("private-classification.md", &source_bytes_before[3]),
        ("private-vrg.md", &source_bytes_before[4]),
        ("excluded/secret.md", &source_bytes_before[5]),
        ("outside/outside.md", &outside_before),
    ] {
        eprintln!(
            "fixture-sha256 {path} {}",
            hex::encode(sha2::Sha256::digest(bytes))
        );
    }
    let config_path = fixture.config(&[("synthetic", &root)]);
    let mut config_text = fs::read_to_string(&config_path).unwrap();
    config_text.push_str("exclusions = [\"excluded\"]\n");
    fs::write(&config_path, config_text).unwrap();
    let config = vrg::config::VrgConfig::load(&config_path).unwrap();
    let scan = vrg::vault::scan_roots(&config).unwrap();

    assert_eq!(scan.documents.len(), 1);
    assert_eq!(scan.documents[0].source.relative_path, "allowed.md");
    assert!(scan.excluded >= 6);
    for private_path in [
        "private-flag.md",
        "private-tag.md",
        "private-classification.md",
        "private-vrg.md",
    ] {
        assert!(vrg::vault::read_source(&config, "synthetic", private_path, 4096).is_err());
    }
    assert!(vrg::vault::read_source(&config, "synthetic", "excluded/secret.md", 4096).is_err());
    assert!(vrg::vault::read_source(&config, "synthetic", "../outside/outside.md", 4096).is_err());
    #[cfg(unix)]
    assert!(
        vrg::vault::read_source(&config, "synthetic", "external-link/outside.md", 4096).is_err()
    );
    assert!(
        !scan
            .documents
            .iter()
            .any(|document| document.body_text.contains("sentinel"))
    );
    assert_eq!(fs::read(&allowed).unwrap(), source_bytes_before[0]);
    assert_eq!(fs::read(&private_flag).unwrap(), source_bytes_before[1]);
    assert_eq!(fs::read(&private_tag).unwrap(), source_bytes_before[2]);
    assert_eq!(
        fs::read(&private_classification).unwrap(),
        source_bytes_before[3]
    );
    assert_eq!(fs::read(&private_vrg).unwrap(), source_bytes_before[4]);
    assert_eq!(fs::read(&excluded_note).unwrap(), source_bytes_before[5]);
    assert_eq!(fs::read(&outside_note).unwrap(), outside_before);
}

#[test]
fn f4_03_local_hybrid_search_returns_attributed_semantic_and_exact_hits() {
    use vrg::contracts::{MatchKind, ModuleStatus, RetrievalMode};

    let model_dir = std::env::var_os("VRG_F4_MODEL_DIR")
        .map(PathBuf::from)
        .expect("F4-03 requires the approved local model directory in VRG_F4_MODEL_DIR");
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir_all(&root).unwrap();
    let atlas_bytes = b"---\ntitle: Graph Atlas\n---\nThe exact title and symbol ORCHID_XYLOPHONE_731 identify this note. Related: [[map-detail]].\n";
    fs::write(root.join("graph-atlas.md"), atlas_bytes).unwrap();
    fs::write(
        root.join("map-detail.md"),
        "# Map Detail\nGraph Atlas is a referenced page. A quiet sentinel synchronizes the knowledge garden whenever an editor saves a page.\n",
    )
    .unwrap();
    let source_bytes = fs::read(root.join("map-detail.md")).unwrap();
    let config_path = fixture.config_with_model(&[("synthetic", &root)], &model_dir);
    let config = vrg::config::VrgConfig::load(&config_path).unwrap();
    let engine = vrg::VrgEngine::open(config).unwrap();
    let report = engine.index().unwrap();
    assert!(matches!(report.semantic_status, ModuleStatus::Ready));

    let exact = engine
        .search(vrg::contracts::SearchRequest {
            query: "Graph Atlas".into(),
            limit: 10,
            root_ids: Some(vec!["synthetic".into()]),
            use_semantic: true,
            graph_hops: 0,
        })
        .unwrap();
    assert!(matches!(exact.mode, RetrievalMode::Hybrid));
    assert_eq!(exact.hits[0].source.relative_path, "graph-atlas.md");
    assert!(exact.hits[0].matches.contains(&MatchKind::ExactTitle));
    assert_eq!(exact.hits[0].revision.len(), 64);
    assert!(!exact.hits[0].snippet.is_empty());
    assert!(exact.hits[0].relations.iter().any(|edge| {
        edge.to
            .as_ref()
            .is_some_and(|target| target.relative_path == "map-detail.md")
    }));

    let exact_path = engine
        .search(vrg::contracts::SearchRequest {
            query: "graph-atlas.md".into(),
            limit: 10,
            root_ids: Some(vec!["synthetic".into()]),
            use_semantic: false,
            graph_hops: 0,
        })
        .unwrap();
    assert_eq!(exact_path.hits[0].source.relative_path, "graph-atlas.md");
    assert!(exact_path.hits[0].matches.contains(&MatchKind::ExactPath));
    assert_eq!(exact_path.hits[0].revision.len(), 64);
    assert!(!exact_path.hits[0].snippet.is_empty());

    let lexical = engine
        .search(vrg::contracts::SearchRequest {
            query: "ORCHID_XYLOPHONE_731".into(),
            limit: 10,
            root_ids: Some(vec!["synthetic".into()]),
            use_semantic: false,
            graph_hops: 0,
        })
        .unwrap();
    assert_eq!(lexical.hits[0].source.relative_path, "graph-atlas.md");
    assert!(lexical.hits[0].matches.contains(&MatchKind::FullText));
    assert_eq!(lexical.hits[0].revision.len(), 64);
    assert!(!lexical.hits[0].snippet.is_empty());

    let semantic = engine
        .search(vrg::contracts::SearchRequest {
            query: "What keeps a collection of notes current after another program edits a page?"
                .into(),
            limit: 10,
            root_ids: Some(vec!["synthetic".into()]),
            use_semantic: true,
            graph_hops: 0,
        })
        .unwrap();
    assert!(matches!(semantic.mode, RetrievalMode::Hybrid));
    assert!(semantic.hits.iter().any(|hit| {
        hit.source.relative_path == "map-detail.md"
            && hit.matches.contains(&MatchKind::Semantic)
            && hit.revision.len() == 64
            && !hit.snippet.is_empty()
    }));
    let neighbors = engine
        .neighbors(
            &vrg::contracts::SourceRef {
                root_id: "synthetic".into(),
                relative_path: "graph-atlas.md".into(),
            },
            10,
        )
        .unwrap();
    assert!(!neighbors.is_empty());
    assert!(neighbors.iter().any(|edge| {
        edge.to
            .as_ref()
            .is_some_and(|target| target.relative_path == "map-detail.md")
    }));
    assert_eq!(fs::read(root.join("graph-atlas.md")).unwrap(), atlas_bytes);
    assert_eq!(fs::read(root.join("map-detail.md")).unwrap(), source_bytes);
}

#[test]
fn f4_04_concurrent_publication_and_journal_recovery_are_deterministic() {
    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir_all(&root).unwrap();
    let note = root.join("entry.md");
    fs::write(&note, "# Initial\ninitial generation sentinel\n").unwrap();
    let config = fixture.config(&[("synthetic", &root)]);
    let initial = fixture
        .run(&config, "f4-04", "initial", false)
        .wait_with_output()
        .unwrap();
    assert!(initial.status.success());
    let original_generation = fixture.status(&config)["generation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let unchanged = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .arg("index")
        .output()
        .unwrap();
    assert!(unchanged.status.success());
    assert_eq!(
        fixture.status(&config)["generation_id"].as_str(),
        Some(original_generation.as_str())
    );

    let updated_bytes = b"# Updated\nconcurrent publication sentinel\n";
    fs::write(&note, updated_bytes).unwrap();
    let barrier = fixture.0.join("barrier");
    let barrier_text = barrier.to_str().unwrap();
    let writer_one = fixture.run_with_env(
        &config,
        "f4-04-writer-one",
        "update-one",
        false,
        &[("VRG_F4_BARRIER_DIR", barrier_text)],
    );
    fixture.wait_for_barrier(&barrier);
    let writer_two = fixture.run(&config, "f4-04-writer-two", "update-two", false);
    let writer_two_output = writer_two.wait_with_output().unwrap();
    assert!(writer_two_output.status.success());
    fs::write(barrier.join("release"), b"release\n").unwrap();
    let writer_one_output = writer_one.wait_with_output().unwrap();
    assert!(writer_one_output.status.success());
    let status = fixture.status(&config);
    assert_eq!(status["index_sync"]["state"], "up_to_date");
    assert_ne!(status["generation_id"], original_generation);
    assert_eq!(fs::read(&note).unwrap(), updated_bytes);

    let crash_bytes = b"# Recovered\nafter journal interruption sentinel\n";
    fs::write(&note, crash_bytes).unwrap();
    let interrupted = fixture
        .run_with_env(
            &config,
            "f4-04-crash",
            "crash-after-journal",
            false,
            &[("VRG_F4_CRASH_AFTER_JOURNAL", "1")],
        )
        .wait_with_output()
        .unwrap();
    assert_eq!(interrupted.status.code(), Some(86));
    let status_before_resume = fixture.status(&config);
    assert_eq!(
        status_before_resume["generation_id"],
        status["generation_id"]
    );
    let resumed = fixture.start_session(&config, "f4-04-resume");
    assert!(resumed.status.success());
    let recovered = fixture.status(&config);
    assert_eq!(recovered["index_sync"]["state"], "up_to_date");
    assert_ne!(
        recovered["generation_id"],
        status_before_resume["generation_id"]
    );
    let database = fixture.0.join("derived/vrg.sqlite3");
    let connection = rusqlite::Connection::open(database).unwrap();
    let pending_journals: i64 = connection
        .query_row(
            "SELECT count(*) FROM journal WHERE state = 'PREPARING'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_journals, 0);
    assert_eq!(fs::read(&note).unwrap(), crash_bytes);
    let search = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .args([
            "search",
            "after journal interruption sentinel",
            "--text-only",
        ])
        .output()
        .unwrap();
    assert!(search.status.success());
    let response: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(response["hits"][0]["source"]["relative_path"], "entry.md");

    let rollback = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .args(["rollback", original_generation.as_str()])
        .output()
        .unwrap();
    assert!(rollback.status.success());
    assert_eq!(
        fixture.status(&config)["generation_id"].as_str(),
        Some(original_generation.as_str())
    );
    assert_eq!(fixture.status(&config)["index_sync"]["state"], "stale");
    assert_eq!(fs::read(&note).unwrap(), crash_bytes);
}

#[test]
fn f4_05_mcp_exposes_read_only_tools_and_enforces_root_boundaries() {
    use sha2::Digest;

    let fixture = Fixture::new();
    let root = fixture.0.join("source");
    fs::create_dir_all(&root).unwrap();
    let note = root.join("entry.md");
    let note_bytes = b"# MCP Entry\nMCP exact freshness sentinel. Related: [[linked]] [[private]] [[history-private]].\n";
    fs::write(&note, note_bytes).unwrap();
    fs::write(root.join("linked.md"), "# Linked\nGraph target.\n").unwrap();
    fs::write(
        root.join("private.md"),
        "---\nprivate: true\n---\nprivate body sentinel\n",
    )
    .unwrap();
    let historical_note = root.join("history-private.md");
    fs::write(
        &historical_note,
        "# Historical public note\nVRG_PRIVACY_ROLLBACK_PROBE_76321\n",
    )
    .unwrap();
    let outside = fixture.0.join("outside.md");
    fs::write(&outside, "# Outside\nnever read through MCP.\n").unwrap();
    let config = fixture.config(&[("synthetic", &root)]);
    let mut client = McpClient::new(&config);
    let init = client.request(1, "initialize", serde_json::json!({
        "protocolVersion":"2025-03-26", "capabilities":{}, "clientInfo":{"name":"vrg-f4-05","version":"1"}
    }));
    assert!(init.get("result").is_some());
    client.notify("notifications/initialized");
    let listed = client.request(2, "tools/list", serde_json::json!({}));
    let names = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for required in [
        "vrg_status",
        "vrg_search",
        "vrg_read_source",
        "vrg_graph_neighbors",
    ] {
        assert!(names.contains(&required));
    }
    assert!(
        !names
            .iter()
            .any(|name| name.contains("write") || name.contains("mutate"))
    );

    let status = client.request(
        3,
        "tools/call",
        serde_json::json!({
            "name":"vrg_status", "arguments":{}
        }),
    );
    assert!(status["result"]["content"][0]["text"].as_str().is_some());
    let readiness_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut readiness_attempt = 0_u64;
    let search_result = loop {
        let request_id = if readiness_attempt == 0 {
            4
        } else {
            1_000 + readiness_attempt
        };
        let remaining = readiness_deadline.saturating_duration_since(std::time::Instant::now());
        let search = client.request_with_timeout(request_id, "tools/call", serde_json::json!({
            "name":"vrg_search", "arguments":{"query":"MCP exact freshness sentinel", "semantic":true}
        }), remaining);
        let search_text = search["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("initial MCP search returned no text: {search}"));
        let result: serde_json::Value = serde_json::from_str(search_text).unwrap_or_else(|error| {
            panic!("initial MCP search returned invalid JSON: {error}; response={search_text}")
        });
        assert!(
            std::time::Instant::now() < readiness_deadline,
            "timed out waiting for initial MCP search readiness after {readiness_attempt} retries; response={result}"
        );
        if result["sync_state"] == "ready" {
            break result;
        }
        assert_eq!(
            result["sync_state"], "sync_pending",
            "initial MCP search reached a non-ready terminal state: {result}"
        );
        readiness_attempt += 1;
        let remaining = readiness_deadline.saturating_duration_since(std::time::Instant::now());
        thread::sleep(remaining.min(Duration::from_millis(25)));
    };
    assert_eq!(search_result["mode"], "degraded_text_only");
    assert_eq!(
        search_result["hits"][0]["source"]["relative_path"],
        "entry.md"
    );
    assert_eq!(
        search_result["hits"][0]["revision"].as_str().unwrap().len(),
        64
    );
    let historical_public = client.request(15, "tools/call", serde_json::json!({
        "name":"vrg_search", "arguments":{"query":"VRG_PRIVACY_ROLLBACK_PROBE_76321", "semantic":false}
    }));
    let historical_public_text = historical_public["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let historical_public_result: serde_json::Value =
        serde_json::from_str(historical_public_text).unwrap();
    assert_eq!(
        historical_public_result["hits"][0]["source"]["relative_path"],
        "history-private.md"
    );
    let historical_generation = historical_public_result["generation_id"]
        .as_str()
        .expect("historical public result must identify its generation")
        .to_owned();
    let private_search = client.request(
        10,
        "tools/call",
        serde_json::json!({
            "name":"vrg_search", "arguments":{"query":"private body sentinel", "semantic":false}
        }),
    );
    let private_search_text = private_search["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let private_search_result: serde_json::Value =
        serde_json::from_str(private_search_text).unwrap();
    let privacy_hits = private_search_result["hits"].as_array().unwrap();
    let hit_metadata = privacy_hits.iter().map(|hit| {
        let source = hit["source"].as_object().expect("search hit must include source attribution");
        serde_json::json!({
            "root_id": source["root_id"].as_str().expect("source root_id must be a string"),
            "relative_path": source["relative_path"].as_str().expect("source relative_path must be a string"),
            "revision": hit["revision"].as_str().expect("hit revision must be a string"),
            "matches": hit["matches"],
        })
    }).collect::<Vec<_>>();
    eprintln!(
        "F4-05 synthetic privacy-query hit metadata: {}",
        serde_json::to_string(&hit_metadata).unwrap()
    );
    // FTS5 query terms are ORed; `sentinel` also matches the public entry.md fixture.
    // Privacy requires that no hit attributes or returns the private document's content.
    assert!(
        hit_metadata
            .iter()
            .any(|hit| hit["relative_path"] == "entry.md")
    );
    for hit in privacy_hits {
        assert_ne!(hit["source"]["relative_path"].as_str(), Some("private.md"));
        let snippet = hit["snippet"]
            .as_str()
            .expect("search hit snippet must be a string");
        assert!(!snippet.contains("private body sentinel"));
    }

    let read = client.request(5, "tools/call", serde_json::json!({
        "name":"vrg_read_source", "arguments":{"root_id":"synthetic", "relative_path":"entry.md"}
    }));
    let read_text = read["result"]["content"][0]["text"].as_str().unwrap();
    let document: serde_json::Value = serde_json::from_str(read_text).unwrap();
    assert_eq!(
        document["raw_markdown"],
        String::from_utf8_lossy(note_bytes).as_ref()
    );

    let updated_bytes = b"# MCP Entry\nMCP updated freshness sentinel. Related: [[linked]] [[private]] [[history-private]].\n";
    fs::write(&note, updated_bytes).unwrap();
    let fresh = client.request(11, "tools/call", serde_json::json!({
        "name":"vrg_search", "arguments":{"query":"MCP updated freshness sentinel", "semantic":false}
    }));
    let fresh_text = fresh["result"]["content"][0]["text"].as_str().unwrap();
    let fresh_result: serde_json::Value = serde_json::from_str(fresh_text).unwrap();
    assert_eq!(fresh_result["sync_state"], "ready");
    assert_eq!(
        fresh_result["hits"][0]["revision"],
        hex::encode(sha2::Sha256::digest(updated_bytes))
    );

    let linked = root.join("linked.md");
    let linked_bytes = fs::read(&linked).unwrap();
    fs::write(&linked, "---\ninvalid: [frontmatter\n---\nmalformed\n").unwrap();
    let failed = client.request(12, "tools/call", serde_json::json!({
        "name":"vrg_search", "arguments":{"query":"MCP updated freshness sentinel", "semantic":false}
    }));
    if failed.get("error").is_none() {
        let failed_text = failed["result"]["content"][0]["text"].as_str().unwrap();
        let failed_result: serde_json::Value = serde_json::from_str(failed_text).unwrap();
        assert!(matches!(
            failed_result["sync_state"].as_str(),
            Some("stale" | "sync_pending" | "error" | "degraded")
        ));
        assert!(failed_result["hits"].as_array().unwrap().is_empty());
    }
    let failed_status = client.request(
        13,
        "tools/call",
        serde_json::json!({
            "name":"vrg_status", "arguments":{}
        }),
    );
    let failed_status_text = failed_status["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let failed_status: serde_json::Value = serde_json::from_str(failed_status_text).unwrap();
    assert_ne!(failed_status["index_sync"]["state"], "up_to_date");
    fs::write(&linked, &linked_bytes).unwrap();
    let recovered = client.request(14, "tools/call", serde_json::json!({
        "name":"vrg_search", "arguments":{"query":"MCP updated freshness sentinel", "semantic":false}
    }));
    let recovered_text = recovered["result"]["content"][0]["text"].as_str().unwrap();
    let recovered_result: serde_json::Value = serde_json::from_str(recovered_text).unwrap();
    assert_eq!(recovered_result["sync_state"], "ready");

    let graph = client.request(6, "tools/call", serde_json::json!({
        "name":"vrg_graph_neighbors", "arguments":{"root_id":"synthetic", "relative_path":"entry.md"}
    }));
    let graph_text = graph["result"]["content"][0]["text"].as_str().unwrap();
    let graph_result: serde_json::Value = serde_json::from_str(graph_text).unwrap();
    assert!(!graph_result.as_array().unwrap().is_empty());
    assert!(!graph_result.as_array().unwrap().iter().any(|value| {
        let edge: vrg::contracts::GraphEdge = serde_json::from_value(value.clone())
            .expect("graph edge must match the GraphEdge contract");
        edge.from.relative_path == "private.md"
            || edge
                .to
                .as_ref()
                .is_some_and(|target| target.relative_path == "private.md")
    }));

    let denied = client.request(7, "tools/call", serde_json::json!({
        "name":"vrg_read_source", "arguments":{"root_id":"synthetic", "relative_path":"../outside.md"}
    }));
    assert!(denied.get("error").is_some());
    let denied_private = client.request(8, "tools/call", serde_json::json!({
        "name":"vrg_read_source", "arguments":{"root_id":"synthetic", "relative_path":"private.md"}
    }));
    assert!(denied_private.get("error").is_some());
    let denied_write = client.request(9, "tools/call", serde_json::json!({
        "name":"vrg_write_source", "arguments":{"root_id":"synthetic", "relative_path":"entry.md"}
    }));
    assert!(denied_write.get("error").is_some());
    assert_eq!(fs::read(&note).unwrap(), updated_bytes);
    assert_eq!(
        fs::read(root.join("private.md")).unwrap(),
        b"---\nprivate: true\n---\nprivate body sentinel\n"
    );
    assert_eq!(
        fs::read(&outside).unwrap(),
        b"# Outside\nnever read through MCP.\n"
    );
    assert!(client.shutdown().success());

    // A retained generation may contain a note that is private in today's source.
    // Rollback must mark that pointer stale so MCP reconciles before exposing it.
    fs::write(
        &historical_note,
        "---\nprivate: true\n---\nVRG_PRIVACY_ROLLBACK_PROBE_76321\n",
    )
    .unwrap();
    let indexed = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .arg("index")
        .output()
        .unwrap();
    assert!(
        indexed.status.success(),
        "index after privacy marker failed: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );
    let rollback = Command::new(env!("CARGO_BIN_EXE_vrg"))
        .arg("--config")
        .arg(&config)
        .args(["rollback", historical_generation.as_str()])
        .output()
        .unwrap();
    assert!(
        rollback.status.success(),
        "rollback failed: {}",
        String::from_utf8_lossy(&rollback.stderr)
    );
    assert_eq!(fixture.status(&config)["index_sync"]["state"], "stale");

    let mut restarted = McpClient::new(&config);
    let restarted_init = restarted.request(30, "initialize", serde_json::json!({
        "protocolVersion":"2025-03-26", "capabilities":{}, "clientInfo":{"name":"vrg-f4-05-recovery","version":"1"}
    }));
    assert!(restarted_init.get("result").is_some());
    restarted.notify("notifications/initialized");
    let historical_search = restarted.request_with_timeout(31, "tools/call", serde_json::json!({
        "name":"vrg_search", "arguments":{"query":"VRG_PRIVACY_ROLLBACK_PROBE_76321", "semantic":false}
    }), Duration::from_secs(10));
    let historical_text = historical_search["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let historical_result: serde_json::Value = serde_json::from_str(historical_text).unwrap();
    assert_eq!(
        historical_result["sync_state"], "ready",
        "historical generation search response: {historical_result}"
    );
    assert!(historical_result["hits"].as_array().unwrap().is_empty());
    let historical_graph = restarted.request(32, "tools/call", serde_json::json!({
        "name":"vrg_graph_neighbors", "arguments":{"root_id":"synthetic", "relative_path":"entry.md"}
    }));
    let historical_graph_text = historical_graph["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let historical_graph_result: serde_json::Value =
        serde_json::from_str(historical_graph_text).unwrap();
    assert!(
        !historical_graph_result
            .as_array()
            .unwrap()
            .iter()
            .any(|value| {
                let edge: vrg::contracts::GraphEdge = serde_json::from_value(value.clone())
                    .expect("historical graph edge must match the GraphEdge contract");
                edge.from.relative_path == "history-private.md"
                    || edge
                        .to
                        .as_ref()
                        .is_some_and(|target| target.relative_path == "history-private.md")
            })
    );
    assert_eq!(
        fs::read(&historical_note).unwrap(),
        b"---\nprivate: true\n---\nVRG_PRIVACY_ROLLBACK_PROBE_76321\n"
    );
    assert!(restarted.shutdown().success());
}
