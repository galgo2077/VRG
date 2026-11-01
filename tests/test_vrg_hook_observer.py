import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import subprocess
import threading
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("vrg_hook_observer", Path(__file__).parents[1] / "scripts/vrg_hook_observer.py")
observer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(observer)


class Stream:
    def __init__(self, data=b""):
        self.buffer = io.BytesIO(data)
    def write(self, value):
        return len(value)
    def flush(self):
        pass


class ObserverTests(unittest.TestCase):
    def test_portable_paths_are_explicit_canonical_and_private(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            binary = root / "vrg"
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o700)
            env = {"VRG_HOOK_EXECUTABLE": str(binary), "VRG_HOOK_EVIDENCE_DIR": str(root)}
            with patch.dict(os.environ, env, clear=True):
                self.assertEqual(observer.vrg_executable(), str(binary))
                self.assertEqual(observer.audit_dir(), root / "hook-observations")
            with patch.dict(os.environ, {}, clear=True):
                with self.assertRaises(ValueError): observer.vrg_executable()
                with self.assertRaises(ValueError): observer.audit_dir()
            link = root / "linked"
            link.symlink_to(binary)
            with patch.dict(os.environ, {"VRG_HOOK_EXECUTABLE": str(link)}, clear=True):
                with self.assertRaises(ValueError): observer.vrg_executable()
            root.chmod(0o755)
            with patch.dict(os.environ, {"VRG_HOOK_EVIDENCE_DIR": str(root)}, clear=True):
                with self.assertRaises(ValueError): observer.audit_dir()

    def test_outer_observes_supervisor_exit_without_retaining_payload(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            payload = {"hook_event_name": "Stop", "session_id": "s1", "turn_id": "t1", "stop_hook_active": False, "prompt": "PRIVATE_SENTINEL"}
            stdin, stdout, stderr = Stream(json.dumps(payload).encode()), Stream(), Stream()
            result = type("Result", (), {"stdout": b'{"decision":"block","reason":"private"}\n', "stderr": b"", "returncode": 0})()
            with patch.object(observer.sys, "argv", ["observer", "codex-stop"]), patch.object(observer.sys, "stdin", stdin), patch.object(observer.sys, "stdout", stdout), patch.object(observer.sys, "stderr", stderr), patch.object(observer, "audit_dir", return_value=root), patch.object(observer, "vrg_executable", return_value="/mock/vrg"), patch.object(observer.subprocess, "run", return_value=result):
                self.assertEqual(observer.observe_supervisor(), 0)
            self.assertEqual(stdout.buffer.getvalue(), result.stdout)
            data = (root / "vrg-supervisor-events.jsonl").read_text()
            self.assertNotIn("PRIVATE_SENTINEL", data)
            self.assertNotIn("private", data)
            event = json.loads(data)
            self.assertEqual(event["supervisor_exit_code"], 0)
            self.assertEqual(event["protocol_shape"], "json_object")
            self.assertTrue(event["continuation_requested"])

    def test_protocol_shape_does_not_require_nonempty_output(self):
        self.assertEqual(observer.protocol_shape("codex-stop", b""), "empty")
        self.assertEqual(observer.protocol_shape("codex-turn", b"context"), "plain_text")
        self.assertEqual(observer.protocol_shape("codex-stop", b"not json"), "invalid_json")
        self.assertEqual(observer.protocol_shape("codex-turn", b'{"hookSpecificOutput":{"hookEventName":"Stop"}}'), "invalid_event_output")

    def test_forwards_output_and_records_only_allowed_metadata(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            os.chmod(root, 0o700)
            cfg = root / "config.toml"
            cfg.write_text(f'data_dir = "{tmp}"\n')
            payload = {"hook_event_name": "Stop", "session_id": "session-1", "turn_id": "turn-2", "last_assistant_message": "PRIVATE_SENTINEL", "prompt": "PROMPT_SENTINEL"}
            stdin, stdout, stderr = Stream(json.dumps(payload).encode()), Stream(), Stream()
            result = type("Result", (), {"stdout": b'{"decision":"block","reason":"sensitive"}\n', "stderr": b"", "returncode": 0})()
            with patch.object(observer.sys, "argv", ["observer", "codex-stop"]), patch.object(observer.sys, "stdin", stdin), patch.object(observer.sys, "stdout", stdout), patch.object(observer.sys, "stderr", stderr), patch.object(observer, "audit_dir", return_value=root), patch.object(observer, "vrg_executable", return_value="/mock/vrg"), patch.object(observer.subprocess, "run", return_value=result):
                code = observer.main()
            self.assertEqual(code, 0)
            self.assertEqual(stdout.buffer.getvalue(), result.stdout)
            log = (root / "vrg-hook-events.jsonl").read_text()
            self.assertNotIn("SENTINEL", log)
            self.assertNotIn("sensitive", log)
            row = json.loads(log)
            self.assertEqual(row["result"], "continuation_requested")
            self.assertEqual(row["child_exit_code"], 0)
            self.assertEqual(set(row), {"event", "session_id", "turn_id", "started_unix_ms", "finished_unix_ms", "child_exit_code", "result"})
            self.assertEqual(os.stat(root / "vrg-hook-events.jsonl").st_mode & 0o777, 0o600)

    def test_timeout_records_no_invented_child_exit(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            payload = {"hook_event_name": "Stop", "session_id": "s1", "turn_id": "t1"}
            stdin, stdout, stderr = Stream(json.dumps(payload).encode()), Stream(), Stream()
            error = subprocess.TimeoutExpired(cmd="vrg", timeout=590)
            with patch.object(observer.sys, "argv", ["observer", "codex-stop"]), patch.object(observer.sys, "stdin", stdin), patch.object(observer.sys, "stdout", stdout), patch.object(observer.sys, "stderr", stderr), patch.object(observer, "audit_dir", return_value=root), patch.object(observer, "vrg_executable", return_value="/mock/vrg"), patch.object(observer.subprocess, "run", side_effect=error):
                self.assertEqual(observer.main(), 124)
            row = json.loads((root / "vrg-hook-events.jsonl").read_text())
            self.assertIsNone(row["child_exit_code"])
            self.assertEqual(row["result"], "timeout")

    def test_compaction_and_concurrent_appends_remain_bounded_and_parseable(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "vrg-hook-events.jsonl"
            path.write_bytes(b'{"event":"old"}\n' * (observer.MAX_LOG // 16 + 5))
            workers = [threading.Thread(target=observer.audit, args=(root, {"event": "Stop", "turn_id": str(i)})) for i in range(8)]
            for worker in workers:
                worker.start()
            for worker in workers:
                worker.join()
            lines = path.read_bytes().splitlines()
            records = [json.loads(line) for line in lines]
            self.assertLessEqual(path.stat().st_size, observer.MAX_LOG)
            self.assertEqual(sum(r.get("turn_id") is not None for r in records), 8)

    def test_rejects_hardlinked_audit(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            external = root / "existing"
            external.write_text("preserve")
            os.link(external, root / "vrg-hook-events.jsonl")
            with self.assertRaises(ValueError):
                observer.audit(root, {"event": "Stop"})
            self.assertEqual(external.read_text(), "preserve")

    def test_refuses_symlink_log_and_reports_invalid_response(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            outside = root / "outside"
            outside.write_text("keep")
            (root / "vrg-hook-events.jsonl").symlink_to(outside)
            with self.assertRaises(OSError):
                observer.audit(root, {"event": "Stop"})
            self.assertEqual(outside.read_text(), "keep")
        self.assertEqual(observer.category(b"not-json", 0), "invalid_response")
        self.assertEqual(observer.category(b"{}", 7), "process_failed")
        self.assertEqual(observer.category(b'{"systemMessage":"VRG documentation check could not initialize for this turn."}', 0), "reported_failure_or_skip")


if __name__ == "__main__":
    unittest.main()
