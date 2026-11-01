use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("vrg-closure-{}-{nonce}", std::process::id()));
        fs::create_dir_all(dir.join("root")).unwrap();
        for n in ["STATUS.md", "DECISIONS.md", "SOLUTIONS.md"] {
            fs::write(dir.join("root").join(n), format!("# {n}\n")).unwrap();
        }
        let cfg = format!(
            "data_dir = {:?}\n[[roots]]\nid = 'vrg-memory'\npath = {:?}\nallowed_files = ['STATUS.md','DECISIONS.md','SOLUTIONS.md']\n",
            dir.join("data"),
            dir.join("root")
        );
        fs::write(dir.join("config.toml"), cfg).unwrap();
        let config = vrg::config::VrgConfig::load(dir.join("config.toml")).unwrap();
        drop(vrg::VrgEngine::open(config).unwrap());
        Self(dir)
    }
    fn invoke(
        &self,
        args: &[&str],
        payload: Option<&serde_json::Value>,
        session: Option<&str>,
    ) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vrg"));
        cmd.arg("--config")
            .arg(self.0.join("config.toml"))
            .args(args)
            .env("VRG_CLOSURE_ENABLED", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(session) = session {
            cmd.env("CODEX_SESSION_ID", session);
        } else {
            cmd.env_remove("CODEX_SESSION_ID");
        }
        if payload.is_some() {
            cmd.stdin(Stdio::piped());
        }
        let mut child = cmd.spawn().unwrap();
        if let Some(payload) = payload {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(payload.to_string().as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    }
    fn event(&self, kind: &str, session: &str, turn: &str, active: bool) -> serde_json::Value {
        serde_json::json!({"hook_event_name":kind,"session_id":session,"turn_id":turn,"cwd":self.0,"stop_hook_active":active,"prompt":"PRIVATE_PROMPT_SENTINEL_MUST_NOT_PERSIST","last_assistant_message":"PRIVATE_RESPONSE_SENTINEL_MUST_NOT_PERSIST"})
    }
    fn turn(&self, session: &str, turn: &str) -> String {
        let out = self.invoke(
            &["codex-turn"],
            Some(&self.event("UserPromptSubmit", session, turn, false)),
            None,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let context = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        context
            .split("--token ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    }
    fn attest(&self, session: &str, token: &str, result: &str) -> Output {
        let mut args = vec!["codex-close", "--token", token, "--result", result];
        if result == "no-new" {
            args.extend(["--reason", "no-change"]);
        }
        self.invoke(&args, None, Some(session))
    }
    fn stop(&self, session: &str, turn: &str, active: bool) -> serde_json::Value {
        let out = self.invoke(
            &["codex-stop"],
            Some(&self.event("Stop", session, turn, active)),
            None,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn sqlite(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.0.join("data/vrg-closure.sqlite3")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn explicit_no_new_and_documented_receipts_are_turn_bound_and_content_free() {
    let f = Fixture::new();
    let token = f.turn("s1", "t1");
    assert!(
        !f.invoke(
            &["codex-close", "--token", &token, "--result", "no-new"],
            None,
            Some("s1")
        )
        .status
        .success()
    );
    assert!(f.attest("s1", &token, "no-new").status.success());
    assert!(f.stop("s1", "t1", false).get("decision").is_none());
    let next = f.turn("s1", "t2");
    assert_ne!(token, next);
    assert!(!f.attest("s1", &token, "no-new").status.success());
    assert!(!f.attest("other-session", &next, "no-new").status.success());
    assert!(!f.attest("s1", &next, "documented").status.success());
    fs::write(
        f.0.join("root/SOLUTIONS.md"),
        "# Synthetic\nVerified solution.\n",
    )
    .unwrap();
    assert!(!f.attest("s1", &next, "no-new").status.success());
    assert!(f.attest("s1", &next, "documented").status.success());
    assert!(f.stop("s1", "t2", false).get("decision").is_none());
    let conn = f.sqlite();
    let outcomes: Vec<String> = {
        let mut q = conn
            .prepare("select outcome from events order by id")
            .unwrap();
        q.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(
        outcomes
            .iter()
            .any(|x| x == "declared_no_new_hash_unchanged:no-new:no-change")
    );
    assert!(
        outcomes
            .iter()
            .any(|x| x == "declared_documented_hash_changed")
    );
    drop(conn);
    let data = fs::read(f.0.join("data/vrg-closure.sqlite3")).unwrap();
    for secret in [
        b"PRIVATE_PROMPT_SENTINEL_MUST_NOT_PERSIST".as_slice(),
        b"PRIVATE_RESPONSE_SENTINEL_MUST_NOT_PERSIST".as_slice(),
    ] {
        assert!(!data.windows(secret.len()).any(|w| w == secret));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(f.0.join("data/vrg-closure.sqlite3"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn missing_or_stale_receipt_continues_only_once() {
    let f = Fixture::new();
    let token = f.turn("s2", "t1");
    let first = f.stop("s2", "t1", false);
    assert_eq!(first["decision"], "block");
    let second = f.stop("s2", "t1", true);
    assert!(second.get("decision").is_none());
    assert!(
        second["systemMessage"]
            .as_str()
            .unwrap()
            .contains("not verified")
    );
    assert!(f.attest("s2", &token, "no-new").status.success());
    let done = f.stop("s2", "t1", true);
    assert!(done.get("decision").is_none());
    // Duplicate delivery of the same Stop is idempotent.
    assert!(f.stop("s2", "t1", false).get("decision").is_none());
    // A new turn rotates the token and cannot reuse the previous receipt.
    let _next = f.turn("s2", "t2");
    assert_eq!(f.stop("s2", "t2", false)["decision"], "block");
    let conn = f.sqlite();
    let n: i64 = conn
        .query_row(
            "select count(*) from events where outcome='closure_unverified_after_continuation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(n >= 1);
}

#[test]
fn continuation_reconciles_documentation_written_after_first_stop() {
    let f = Fixture::new();
    let token = f.turn("s4", "t1");
    assert_eq!(f.stop("s4", "t1", false)["decision"], "block");
    let updated = b"# Synthetic\nDocumented after continuation.\n";
    fs::write(f.0.join("root/SOLUTIONS.md"), updated).unwrap();
    assert!(f.attest("s4", &token, "documented").status.success());
    assert!(f.stop("s4", "t1", true).get("decision").is_none());
    let conn = rusqlite::Connection::open(f.0.join("data/vrg.sqlite3")).unwrap();
    let revision: String = conn.query_row(
        "SELECT d.revision FROM documents d JOIN active_generation a ON d.generation_id=a.generation_id WHERE d.root_id='vrg-memory' AND d.relative_path='SOLUTIONS.md'",
        [], |r| r.get(0),
    ).unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(revision, hex::encode(Sha256::digest(updated)));
}

#[test]
fn duplicate_user_prompt_does_not_reset_the_turn_baseline() {
    let f = Fixture::new();
    let token = f.turn("s6", "t1");
    fs::write(
        f.0.join("root/SOLUTIONS.md"),
        "# Newly documented synthetic result\n",
    )
    .unwrap();
    assert_eq!(f.turn("s6", "t1"), token);
    assert!(!f.attest("s6", &token, "no-new").status.success());
    assert!(f.attest("s6", &token, "documented").status.success());
    assert!(f.stop("s6", "t1", true).get("decision").is_none());
}

#[test]
fn hardlinked_closure_database_is_not_opened_or_chmodded() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let protected = f.0.join("protected");
        fs::write(&protected, b"protected sentinel").unwrap();
        fs::set_permissions(&protected, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&protected, f.0.join("data/vrg-closure.sqlite3")).unwrap();
        let out = f.invoke(
            &["codex-turn"],
            Some(&f.event("UserPromptSubmit", "s5", "t1", false)),
            None,
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(v.get("hookSpecificOutput").is_none());
        assert_eq!(fs::read(protected).unwrap(), b"protected sentinel");
    }
}

#[test]
fn symlinked_memory_source_is_not_read() {
    #[cfg(unix)]
    {
        let f = Fixture::new();
        let target = f.0.join("outside.md");
        fs::write(&target, "# protected\n").unwrap();
        fs::remove_file(f.0.join("root/SOLUTIONS.md")).unwrap();
        std::os::unix::fs::symlink(&target, f.0.join("root/SOLUTIONS.md")).unwrap();
        let out = f.invoke(
            &["codex-turn"],
            Some(&f.event("UserPromptSubmit", "s3", "t1", false)),
            None,
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(v.get("hookSpecificOutput").is_none());
        assert_eq!(fs::read_to_string(target).unwrap(), "# protected\n");
    }
}
