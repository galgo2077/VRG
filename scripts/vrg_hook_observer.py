#!/usr/bin/env python3
"""Supervise VRG hooks and retain only bounded, non-content event metadata."""
import fcntl
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time

EVENTS = {"codex-start": "SessionStart", "codex-turn": "UserPromptSubmit", "codex-stop": "Stop"}
MAX_INPUT = 64 * 1024
MAX_LOG = 1024 * 1024
ID = re.compile(r"^[A-Za-z0-9_.:-]{1,128}$")


def configured_path(name):
    value = os.environ.get(name)
    if not value:
        raise ValueError(f"{name} is required")
    path = Path(value)
    if not path.is_absolute() or path.resolve(strict=True) != path:
        raise ValueError(f"{name} must be an existing canonical absolute path")
    return path


def vrg_executable():
    path = configured_path("VRG_HOOK_EXECUTABLE")
    if not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError("VRG_HOOK_EXECUTABLE must be an executable regular file")
    return str(path)


def audit_dir():
    parent = configured_path("VRG_HOOK_EVIDENCE_DIR")
    if parent.resolve(strict=True) != parent:
        raise ValueError("audit parent is not canonical")
    parent_meta = parent.stat()
    if not stat.S_ISDIR(parent_meta.st_mode) or parent_meta.st_uid != os.getuid() or stat.S_IMODE(parent_meta.st_mode) & 0o077:
        raise ValueError("audit parent must be an owned private directory")
    path = parent / "hook-observations"
    path.mkdir(mode=0o700, exist_ok=True)
    if path.resolve(strict=True) != path:
        raise ValueError("audit directory is not canonical")
    meta = path.stat()
    if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != os.getuid() or stat.S_IMODE(meta.st_mode) & 0o077:
        raise ValueError("audit directory is not private")
    return path


def audit(directory, event, filename="vrg-hook-events.jsonl"):
    line = (json.dumps(event, separators=(",", ":"), ensure_ascii=True) + "\n").encode()
    path = directory / filename
    flags = os.O_RDWR | os.O_CREAT | os.O_APPEND | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        meta = os.fstat(fd)
        if not stat.S_ISREG(meta.st_mode) or meta.st_uid != os.getuid() or meta.st_nlink != 1:
            raise ValueError("audit file is not an owned regular file")
        os.fchmod(fd, 0o600)
        if meta.st_size + len(line) > MAX_LOG:
            # Keep complete recent records; a crash during compaction is detectable
            # as missing evidence, never interpreted as a successful hook.
            os.lseek(fd, max(0, meta.st_size - MAX_LOG // 2), os.SEEK_SET)
            tail = os.read(fd, MAX_LOG // 2)
            first = tail.find(b"\n")
            tail = tail[first + 1:] if first >= 0 else b""
            os.ftruncate(fd, 0)
            pending = memoryview(tail)
            while pending:
                count = os.write(fd, pending)
                if count <= 0:
                    raise OSError("incomplete audit compaction")
                pending = pending[count:]
        remaining = memoryview(line)
        while remaining:
            count = os.write(fd, remaining)
            if count <= 0:
                raise OSError("incomplete audit write")
            remaining = remaining[count:]
        os.fsync(fd)
    finally:
        os.close(fd)


def category(stdout, code):
    if code != 0:
        return "process_failed"
    try:
        response = json.loads(stdout)
    except (ValueError, UnicodeDecodeError):
        return "invalid_response"
    if not isinstance(response, dict):
        return "invalid_response"
    if response.get("decision") == "block":
        return "continuation_requested"
    message = response.get("systemMessage")
    if isinstance(message, str) and any(w in message.lower() for w in ("failed", "skipped", "omitted", "error", "not verified", "could not", "unavailable", "unverified")):
        return "reported_failure_or_skip"
    if isinstance(message, str) and ("index updated" in message.lower() or "reconciliation updated" in message.lower()):
        return "reconciled"
    return "response_ok_unclassified"


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in EVENTS:
        return 64
    mode = sys.argv[1]
    raw = sys.stdin.buffer.read(MAX_INPUT + 1)
    if len(raw) > MAX_INPUT:
        print("VRG hook input too large", file=sys.stderr)
        return 65
    try:
        payload = json.loads(raw)
        if not isinstance(payload, dict) or payload.get("hook_event_name") != EVENTS[mode]:
            raise ValueError("unexpected hook event")
        ids = {}
        for key in ("session_id", "turn_id"):
            v = payload.get(key)
            if v is not None and (not isinstance(v, str) or not ID.fullmatch(v)):
                raise ValueError("invalid identifier")
            ids[key] = v
        directory = audit_dir()
    except (OSError, ValueError, KeyError, TypeError):
        print("VRG hook observer refused invalid event or audit destination", file=sys.stderr)
        return 65
    env = os.environ.copy()
    env["VRG_CLOSURE_ENABLED"] = "1"
    started = time.time_ns() // 1_000_000
    try:
        result = subprocess.run([vrg_executable(), mode], input=raw, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, timeout=590, check=False)
        stdout, stderr, code = result.stdout, result.stderr, result.returncode
        child_exit = code if code >= 0 else None
        termination_signal = -code if code < 0 else None
        supervisor_status = "exited" if code >= 0 else "signalled"
        if len(stdout) > MAX_INPUT or len(stderr) > MAX_INPUT:
            stdout, stderr, code, supervisor_status = b"", b"VRG hook output exceeded limit\n", 74, "output_oversize"
    except subprocess.TimeoutExpired as error:
        stdout, stderr, code = error.stdout or b"", (error.stderr or b"") + b"VRG hook timed out\n", 124
        child_exit, termination_signal, supervisor_status = None, None, "timeout"
    except (OSError, ValueError):
        stdout, stderr, code = b"", b"VRG hook could not start\n", 127
        child_exit, termination_signal, supervisor_status = None, None, "start_failed"
    finished = time.time_ns() // 1_000_000
    outcome = category(stdout, code) if supervisor_status == "exited" else (f"signalled:{termination_signal}" if supervisor_status == "signalled" else supervisor_status)
    event = {"event": EVENTS[mode], **ids, "started_unix_ms": started, "finished_unix_ms": finished, "child_exit_code": child_exit, "result": outcome}
    try:
        audit(directory, event)
    except (OSError, ValueError):
        print("VRG hook audit failed", file=sys.stderr)
        return 74
    sys.stdout.buffer.write(stdout)
    sys.stderr.buffer.write(stderr)
    return code if code >= 0 else 128 - code


def protocol_shape(mode, stdout):
    """Classify output without retaining its contents; Codex remains authoritative."""
    try:
        value = stdout.decode("utf-8").strip()
    except UnicodeDecodeError:
        return "invalid_utf8"
    if not value:
        return "empty"
    if not value.startswith("{") and mode != "codex-stop":
        return "plain_text"
    try:
        result = json.loads(value)
    except ValueError:
        return "invalid_json"
    if not isinstance(result, dict):
        return "invalid_json_object"
    specific = result.get("hookSpecificOutput")
    if specific is not None and (not isinstance(specific, dict) or specific.get("hookEventName") != EVENTS[mode]):
        return "invalid_event_output"
    return "json_object"


def observe_supervisor():
    """Observe a child supervisor's actual wait status and forward its output."""
    if len(sys.argv) != 2 or sys.argv[1] not in EVENTS:
        return 64
    mode = sys.argv[1]
    raw = sys.stdin.buffer.read(MAX_INPUT + 1)
    if len(raw) > MAX_INPUT:
        print("VRG hook input too large", file=sys.stderr)
        return 65
    try:
        payload = json.loads(raw)
        if not isinstance(payload, dict) or payload.get("hook_event_name") != EVENTS[mode]:
            raise ValueError("unexpected hook event")
        ids = {}
        for key in ("session_id", "turn_id"):
            value = payload.get(key)
            if value is not None and (not isinstance(value, str) or not ID.fullmatch(value)):
                raise ValueError("invalid identifier")
            ids[key] = value
        active = payload.get("stop_hook_active", False)
        if not isinstance(active, bool):
            raise ValueError("invalid continuation state")
        directory = audit_dir()
    except (OSError, ValueError, TypeError):
        print("VRG hook observer refused invalid event or audit destination", file=sys.stderr)
        return 65
    started = time.time_ns() // 1_000_000
    try:
        result = subprocess.run([sys.executable, __file__, "--supervisor", mode], input=raw,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=597, check=False)
        stdout, stderr, code = result.stdout, result.stderr, result.returncode
        status = "exited" if code >= 0 else "signalled"
        supervisor_exit = code if code >= 0 else None
        if len(stdout) > MAX_INPUT or len(stderr) > MAX_INPUT:
            stdout, stderr, code, status = b"", b"VRG supervisor output exceeded limit\n", 74, "output_oversize"
    except subprocess.TimeoutExpired:
        stdout, stderr, code, status, supervisor_exit = b"", b"VRG supervisor timed out\n", 124, "timeout", None
    except OSError:
        stdout, stderr, code, status, supervisor_exit = b"", b"VRG supervisor could not start\n", 127, "start_failed", None
    shape = protocol_shape(mode, stdout) if status == "exited" else "unavailable"
    continuation = False
    if shape == "json_object":
        continuation = json.loads(stdout).get("decision") == "block"
    event = {"event": EVENTS[mode], **ids, "started_unix_ms": started,
             "finished_unix_ms": time.time_ns() // 1_000_000,
             "supervisor_exit_code": supervisor_exit, "observer_status": status,
             "protocol_shape": shape, "stop_hook_active": active,
             "continuation_requested": continuation}
    try:
        audit(directory, event, "vrg-supervisor-events.jsonl")
    except (OSError, ValueError):
        print("VRG supervisor audit failed", file=sys.stderr)
        return 74
    sys.stdout.buffer.write(stdout)
    sys.stderr.buffer.write(stderr)
    return code if code >= 0 else 128 - code


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--supervisor":
        sys.argv = [sys.argv[0], sys.argv[2]]
        sys.exit(main())
    sys.exit(observe_supervisor())
