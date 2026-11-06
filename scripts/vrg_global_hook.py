#!/usr/bin/env python3
"""Forward a global Codex hook and record bounded, content-free exit evidence."""
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import time
from vrg_hook_observer import audit, category
from vrg_global import select_hook, private_file, load_registry, maybe_enroll, resolve, atomic_json

EVIDENCE = Path.home() / '.local/state/vrg-auto/hook-observations'
SOURCE_LAUNCHER = Path(__file__).with_name('vrg_sources.py').resolve()
GLOBAL_LAUNCHER = Path(__file__).with_name('vrg_global.py').resolve()
SNAPSHOTS = EVIDENCE / "project-discovery"

EVENTS = {"SessionStart": "codex-start", "UserPromptSubmit": "codex-turn", "Stop": "codex-stop"}
# Native envelopes may carry long prompts/messages; VRG needs metadata only.
MAX_ENVELOPE_BYTES = 8 * 1024 * 1024
MAX_EVENT_BYTES = 64 * 1024


def discovery_root(cwd):
    roots = load_registry().get('discovery_roots', [])
    here = Path(cwd).resolve(strict=True)
    return here if str(here) in roots else None


def child_inventory(root):
    if root is None:
        return {}
    result = {}
    for child in root.iterdir():
        if child.is_symlink() or not child.is_dir() or child.resolve(strict=True) != child:
            continue
        info = child.stat()
        if info.st_uid == os.getuid():
            result[child.name] = info.st_ino
    return result


def snapshot_path(session):
    return SNAPSHOTS / (hashlib.sha256(session.encode()).hexdigest() + '.json')


def save_discovery_snapshot(session, cwd):
    root = discovery_root(cwd)
    if root is None:
        return
    SNAPSHOTS.mkdir(mode=0o700, exist_ok=True)
    payload = {'root': str(root), 'children': child_inventory(root)}
    atomic_json(snapshot_path(session), payload)


def new_project_children(session, cwd):
    root = discovery_root(cwd)
    path = snapshot_path(session)
    if root is None or not path.exists():
        return []
    before = json.loads(private_file(path).read_text())
    if before.get('root') != str(root):
        raise ValueError('project discovery snapshot scope changed')
    current = child_inventory(root)
    selected = []
    for name, inode in current.items():
        if name in before['children']:
            continue
        child = root / name
        readme = child / 'README.md'
        if (readme.is_file() and not readme.is_symlink()
                and readme.stat().st_uid == os.getuid()):
            selected.append(child)
    return sorted(selected)


def record(entry):
    directory = EVIDENCE.resolve(strict=True)
    info = directory.stat()
    if directory != EVIDENCE or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
        raise OSError("unsafe evidence directory")
    audit(directory, entry, "vrg-global-events.jsonl")



def source_ready():
    registry = load_registry()
    if hashlib.sha256(private_file(SOURCE_LAUNCHER).read_bytes()).hexdigest() != registry.get('sources_launcher_sha256'):
        raise ValueError('source launcher pin changed')
    if hashlib.sha256(Path(registry['binary']).read_bytes()).hexdigest() != registry.get('binary_sha256'):
        raise ValueError('runtime binary pin changed')
    return True


def source_sync(event, cwd, started, selected):
    if not selected.get("memory"):
        return {"state": "unconfigured_scope"}
    if event not in ("SessionStart", "Stop"):
        return {"state": "not_applicable"}
    if not source_ready():
        return {"state": "not_active"}
    remaining = 590 - (time.monotonic_ns() - started) / 1_000_000_000
    if remaining < 1:
        return {"state": "deadline_exhausted", "exit_code": 124}
    sync_started = time.monotonic_ns()
    failure_reason = None
    try:
        # The dispatcher owns the shared source/index lock. Never forward prompt
        # payloads or retain source output in hook evidence.
        child = subprocess.run([sys.executable, str(SOURCE_LAUNCHER), "sync"],
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.PIPE, cwd=cwd,
                               timeout=min(240, remaining), check=False)
        code = child.returncode
        # The global memory registry can authorize projects that do not have a
        # source-corpus profile. The source dispatcher validates its runtime
        # before producing this exact selection error. Preserve all other 78s
        # and never claim that a nonexistent source corpus was synchronized.
        if (code == 78 and child.stderr ==
                b"VRG sources selection failed: cwd has no authorized canonical source profile\n"):
            code, state = 0, "unconfigured_source_scope"
        else:
            state = "ready" if code == 0 else "failed"
            if code == 2 and child.stderr.strip() == (
                    b"vrg: index generation conflict: an index synchronization is already in progress"):
                failure_reason = "native_writer_busy"
    except subprocess.TimeoutExpired:
        code, state = 124, "timeout"
    except OSError:
        code, state = 127, "start_failed"
    result = {"state": state, "exit_code": code,
              "duration_ms": (time.monotonic_ns() - sync_started) // 1_000_000}
    if failure_reason:
        result["reason"] = failure_reason
    return result


def new_project_memory_sync(cwd, started):
    remaining = 590 - (time.monotonic_ns() - started) / 1_000_000_000
    if remaining < 1:
        return {"state": "deadline_exhausted", "exit_code": 124}
    sync_started = time.monotonic_ns()
    try:
        child = subprocess.run([sys.executable, str(GLOBAL_LAUNCHER), "sync"],
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.PIPE, cwd=cwd,
                               timeout=min(240, remaining), check=False)
        code = child.returncode
        state = "ready" if code == 0 else "failed"
    except subprocess.TimeoutExpired:
        code, state = 124, "timeout"
    except OSError:
        code, state = 127, "start_failed"
    return {"state": state, "exit_code": code,
            "duration_ms": (time.monotonic_ns() - sync_started) // 1_000_000}


def contextual_directive(output, selected):
    if not selected.get("memory"):
        if discovery_root(selected['project']) is not None:
            response = json.loads(output)
            specific = response.get('hookSpecificOutput', {})
            if specific.get('hookEventName') == 'UserPromptSubmit' and isinstance(specific.get('additionalContext'), str):
                specific['additionalContext'] += (
                    '\nThis directory is an authorized project-discovery parent. If this task creates '
                    'a direct child project, after its directory exists run the normal VRG selector '
                    'from that child directory so its own authorized project profile is selected. '
                    'Document substantial verified architecture, interface, testing, decisions and '
                    'operational state in that project. Record compact durable operational knowledge '
                    'in its authorized STATUS.md, DECISIONS.md or SOLUTIONS.md through the existing '
                    'managed note append command from the child directory. Do not claim documented '
                    'without a real note or no-new for a substantial new project. The parent turn '
                    'cannot borrow the child memory receipt; its Stop hook records that scope transfer.')
                return (json.dumps(response, ensure_ascii=False) + '\n').encode()
        return output
    response = json.loads(output)
    specific = response.get("hookSpecificOutput", {})
    if specific.get("hookEventName") != "UserPromptSubmit":
        return output
    context = specific.get("additionalContext")
    if not isinstance(context, str):
        return output
    if source_ready():
        context += (
            "\nVRG project context: " + selected['project'] +
            ". Consult vrg_sources for relevant technical context before repeating "
            "investigation; verify source revisions and provenance. Save only new "
            "verified durable knowledge in the authorized three-file memory; no-new "
            "remains valid. Never save prompts or transcripts.")
    specific["additionalContext"] = context + (
        "\nWhen a task creates or substantially implements a project, maintain "
        "substantive project documentation in the project's authorized writable "
        "paths, in the existing documentation structure when possible. Cover "
        "architecture and design decisions, setup and operation, public interfaces, "
        "testing and validation, known limitations and relevant outstanding work. "
        "For a new or substantially implemented project, organize these topics "
        "into multiple linked documents rather than leaving them all in one README: "
        "provide dedicated documents for architecture, setup and operations, public "
        "interfaces, testing and validation, and decisions and known limitations, "
        "in addition to the entry-point README. Adapt these to the actual project "
        "and reuse suitable existing documents. Base claims "
        "on the implementation and verified results, mark unknowns, and do not "
        "invent decisions or tests. Preserve the user's scope and permissions; "
        "do not create documentation for trivial or unrelated work. Project "
        "documentation is distinct from compact operational memory. A newly built "
        "project or a significant implementation can contain new durable knowledge "
        "even if its README or source code exists: evaluate verified decisions, "
        "limitations, solutions and validation for authorized memory. Do not "
        "declare no-new merely because project documentation exists. A new "
        "functional module with tests and an interface or recovery contract is "
        "durable project knowledge when its behavior is verified; record its "
        "compact API, invariants, failure handling and validation in the "
        "authorized operational memory. Reserve no-new:no-durable-result for "
        "turns without a durable technical outcome, and no-new:trivial for "
        "genuinely minor edits; a new multi-file component with verified tests "
        "does not fit either reason. Check the current source revisions and "
        "existing notes before writing, and do not duplicate them. Do not "
        "claim a memory update if it was unavailable or could not be written. "
        "If the user explicitly prohibits writes outside specified paths for "
        "this task, honor that narrower scope: do not write operational memory "
        "outside it, do not claim documented without a real note, and do not "
        "misclassify a durable result as no-new. Report the scope conflict; "
        "the bounded Stop continuation may request resolution.")
    if selected.get('managed_note_writes'):
        specific["additionalContext"] += (
            "\nFor authorized operational-memory updates, use the existing CLI "
            "`" + sys.executable + " " + str(GLOBAL_LAUNCHER) + " note append "
            "STATUS.md|DECISIONS.md|SOLUTIONS.md` with only the new compact text on "
            "stdin. This route uses the shared server's per-note CAS, durable backup "
            "and bounded conflict retry. Do not directly edit those three files "
            "while managed writes are enabled. For replacing existing text, use "
            "`note apply` with an exact expected revision and private proposed file; "
            "on REVISION_CONFLICT re-read and merge before retrying. If the "
            "managed route is unavailable, report the blocker instead of "
            "silently bypassing it. Honor any narrower user write restriction.")
    return (json.dumps(response, ensure_ascii=False) + "\n").encode()


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in EVENTS:
        return 64
    event = sys.argv[1]
    raw = sys.stdin.buffer.read(MAX_ENVELOPE_BYTES + 1)
    if len(raw) > MAX_ENVELOPE_BYTES:
        sys.stderr.write("VRG hook input rejected: exceeds 8388608-byte envelope limit\n")
        return 65
    try:
        payload = json.loads(raw)
        if not isinstance(payload, dict) or payload.get("hook_event_name") != event:
            sys.stderr.write("VRG hook input rejected: unexpected event schema\n")
            return 65
        session = payload.get("session_id")
        if not isinstance(session, str) or not session or len(session) > 256:
            sys.stderr.write("VRG hook input rejected: invalid session identifier\n")
            return 65
        turn = payload.get("turn_id")
        if turn is not None and (not isinstance(turn, str) or len(turn) > 256):
            sys.stderr.write("VRG hook input rejected: invalid turn identifier\n")
            return 65
        cwd = payload.get("cwd")
        if cwd is not None and (not isinstance(cwd, str) or not Path(cwd).is_absolute()):
            sys.stderr.write("VRG hook input rejected: cwd must be an absolute path\n")
            return 65
    except (UnicodeDecodeError, ValueError):
        sys.stderr.write("VRG hook input rejected: invalid UTF-8 or JSON\n")
        return 65
    # Preserve the native VRG input bound and closure identities. Never forward
    # prompt, last_assistant_message, transcript_path or unknown content fields.
    if "source" in payload and payload["source"] is not None and not isinstance(payload["source"], str):
        sys.stderr.write("VRG hook input rejected: invalid source type\n")
        return 65
    if "stop_hook_active" in payload and not isinstance(payload["stop_hook_active"], bool):
        sys.stderr.write("VRG hook input rejected: invalid continuation state\n")
        return 65
    fields = ("hook_event_name", "session_id", "turn_id", "cwd", "source", "stop_hook_active")
    raw = json.dumps({key: payload[key] for key in fields if key in payload},
                     ensure_ascii=True, separators=(",", ":")).encode()
    if len(raw) > MAX_EVENT_BYTES:
        sys.stderr.write("VRG hook input rejected: exceeds 65536-byte metadata limit\n")
        return 65
    del payload
    env = os.environ.copy()
    env["VRG_CLOSURE_ENABLED"] = "1"
    start = time.monotonic_ns()
    try:
        selected = select_hook(event, cwd or os.getcwd(), session, turn)
    except (OSError, ValueError, KeyError, TypeError):
        sys.stderr.write("VRG refused an invalid or changed project profile\n")
        return 78
    if event == 'UserPromptSubmit':
        try:
            save_discovery_snapshot(session, cwd or os.getcwd())
        except (OSError, ValueError):
            sys.stderr.write('VRG project discovery snapshot failed\n')
            return 74
    if event == 'Stop':
        try:
            children = new_project_children(session, cwd or os.getcwd())
        except (OSError, ValueError, KeyError):
            children = []
        if children:
            results = []
            memory_results = []
            for child_path in children:
                try:
                    maybe_enroll(str(child_path))
                    child_profile = resolve(str(child_path))
                    memory_results.append(new_project_memory_sync(str(child_path), start))
                    results.append(source_sync('Stop', str(child_path), time.monotonic_ns(), child_profile))
                except (OSError, ValueError, KeyError, TypeError):
                    memory_results.append({'state': 'registration_failed', 'exit_code': 78})
                    results.append({'state': 'registration_failed', 'exit_code': 78})
            entry = {'event': event, 'session_id_sha256': hashlib.sha256(session.encode()).hexdigest(),
                     'turn_id_sha256': hashlib.sha256(turn.encode()).hexdigest() if turn else None,
                     'project_sha256': hashlib.sha256(selected['project'].encode()).hexdigest(),
                     'result': 'new_project_scope_transferred', 'exit_code': 0,
                     'source_sync': results, 'memory_sync': memory_results,
                     'new_projects': len(children),
                     'duration_ms': 0}
            record(entry)
            snapshot_path(session).unlink(missing_ok=True)
            sys.stderr.write('VRG enrolled a new authorized project; parent turn has no transferable memory receipt.\n')
            return 0
        if not selected.get('memory'):
            record({'event': event,
                    'session_id_sha256': hashlib.sha256(session.encode()).hexdigest(),
                    'turn_id_sha256': hashlib.sha256(turn.encode()).hexdigest() if turn else None,
                    'project_sha256': hashlib.sha256(selected['project'].encode()).hexdigest(),
                    'result': 'unconfigured_scope_no_memory_receipt', 'exit_code': 0,
                    'source_sync': {'state': 'unconfigured_scope'}, 'duration_ms': 0})
            sys.stderr.write('VRG has no authorized memory root for this session; no documentation result was declared.\n')
            return 0
    executable, profile = selected['executable'], selected['profile']
    try:
        child = subprocess.run([executable, "--config", profile, EVENTS[event]], input=raw,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               env=env, cwd=cwd, timeout=590, check=False)
        code = child.returncode
        output, errors = child.stdout, child.stderr
        result = category(output, code)
    except subprocess.TimeoutExpired:
        code, output, errors, result = 124, b"", b"VRG hook timed out\n", "timeout"
    except OSError:
        code, output, errors, result = 127, b"", b"VRG hook could not start\n", "start_failed"
    try:
        source_result = source_sync(event, cwd or os.getcwd(), start, selected)
        if event == "UserPromptSubmit" and code == 0:
            output = contextual_directive(output, selected)
    except (OSError, ValueError, KeyError, TypeError):
        source_result = {"state": "invalid_runtime", "exit_code": 78}
    source_code = source_result.get("exit_code", 0)
    if source_code:
        errors += b"VRG source synchronization failed; source freshness is not certified.\n"
        if code == 0:
            code = source_code if source_code > 0 else 128 - source_code
    entry = {"source_sync": source_result, "config_path": profile, "executable": executable,
             "config_sha256": hashlib.sha256(Path(profile).read_bytes()).hexdigest(),
             "executable_sha256": hashlib.sha256(Path(executable).read_bytes()).hexdigest(),
             "project_sha256": hashlib.sha256(selected['project'].encode()).hexdigest(),
             "legacy_turn": selected['legacy'],
             "cwd_sha256": hashlib.sha256(str(Path(cwd or os.getcwd()).resolve()).encode()).hexdigest(),
             "event": event, "session_id_sha256": hashlib.sha256(session.encode()).hexdigest(),
             "turn_id_sha256": hashlib.sha256(turn.encode()).hexdigest() if turn else None,
             "exit_code": code, "result": result,
             "duration_ms": (time.monotonic_ns() - start) // 1000000}
    try:
        record(entry)
    except (OSError, ValueError):
        sys.stderr.write("VRG hook evidence write failed\n")
        return 74
    sys.stdout.buffer.write(output)
    sys.stderr.buffer.write(errors)
    return code if code >= 0 else 128 - code


if __name__ == "__main__":
    raise SystemExit(main())
