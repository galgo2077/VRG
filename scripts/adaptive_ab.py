#!/usr/bin/env python3
"""AM-T02/14 synthetic MCP A/B runner. Run only in the AM4 validation phase.

Inputs are a frozen corpus/manifest and a separately authored proposal template.
This runner never generates summaries or claims a script is a language model.
It measures the real product retrieval and counts all explicit expansions.
Only its newly created work directory is mutable; an existing directory is refused.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import os
from pathlib import Path
import re
import resource
import select
import shutil
import sqlite3
import subprocess
import time


TOKENIZER = r"VRG lexical-v1: regex \w+|[^\w\s] Unicode, evaluation units only"


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def sha_file(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_path(source_id, corpus):
    """Reverse the product's canonical percent/colon identifier escaping."""
    components = source_id.split(":")
    if len(components) != 2:
        raise ValueError("noncanonical proposal source identifier")
    decoded = [part.replace("%3A", ":").replace("%25", "%") for part in components]
    escaped = [part.replace("%", "%25").replace(":", "%3A") for part in decoded]
    if escaped != components or decoded[0] != "corpus":
        raise ValueError("proposal source identifier is outside the corpus scope")
    path = corpus / decoded[1]
    if path.resolve().parent != corpus.resolve():
        raise ValueError("proposal source path escapes the flat corpus")
    return path


def units(text):
    return len(re.findall(r"\w+|[^\w\s]", text, flags=re.UNICODE))


def measurement(value, native_bytes=None):
    encoded = compact(value)
    return {"canonical_response_bytes": len(encoded.encode()), "evaluation_units": units(encoded),
            "native_cli_bytes": native_bytes}


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def run_cli(binary, config, arguments, cwd):
    started = time.monotonic()
    process = subprocess.run([str(binary), "--config", str(config), *arguments], cwd=cwd,
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, timeout=60, check=False)
    elapsed = time.monotonic() - started
    if process.returncode:
        raise RuntimeError(f"{arguments[0]} exited {process.returncode}: "
                           + process.stderr.decode(errors="replace")[:1200])
    return json.loads(process.stdout), elapsed, len(process.stdout)


def mcp_call(binary, config, tool, arguments, cwd):
    """Use the actual Codex read-only MCP interface for both A/B variants.

    Initialization is protocol overhead, reported separately from context output.
    Application JSON and complete MCP result-envelope units are both reported.
    """
    process = subprocess.Popen([str(binary), "--config", str(config), "mcp", "serve"],
                               cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, bufsize=0)

    def send(value):
        process.stdin.write((compact(value) + "\n").encode())
        process.stdin.flush()

    pending = bytearray()

    def receive(expected):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            while b"\n" in pending:
                line, _, rest = pending.partition(b"\n")
                pending[:] = rest
                message = json.loads(line)
                if message.get("id") == expected:
                    if "error" in message:
                        raise RuntimeError("MCP request failed: " + compact(message["error"]))
                    return message["result"]
            readable, _, _ = select.select([process.stdout], [], [], max(0, deadline - time.monotonic()))
            if not readable:
                break
            chunk = os.read(process.stdout.fileno(), 65536)
            if not chunk:
                raise RuntimeError("MCP source server exited before response")
            pending.extend(chunk)
            if len(pending) > 1024 * 1024:
                raise RuntimeError("MCP response exceeds fixture safety limit")
        raise TimeoutError("MCP source response timeout")

    started = time.monotonic()
    try:
        send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "vrg-adaptive-ab", "version": "1"}}})
        initialized = receive(1)
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": tool, "arguments": arguments}})
        result = receive(2)
        if result.get("isError"):
            raise RuntimeError("MCP tool returned an error")
        contents = [json.loads(block["text"]) for block in result.get("content", []) if block.get("type") == "text"]
        if len(contents) != 1:
            raise RuntimeError("MCP tool result did not contain one JSON application response")
        return contents[0], result, time.monotonic() - started, measurement(initialized)
    finally:
        if process.stdin:
            process.stdin.close()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


def snapshot_directory(path):
    output = {}
    for file in sorted(path.rglob("*")):
        if file.is_symlink():
            raise ValueError("fixture cannot contain symlinks")
        if file.is_file():
            output[str(file.relative_to(path))] = sha_file(file)
    return output


def disk_usage(path):
    entries = {}
    for file in path.rglob("*"):
        if file.is_symlink():
            raise ValueError("derived fixture cannot contain symlinks")
        if file.is_file():
            entries[str(file.relative_to(path))] = file.stat().st_size
    return {"total_bytes": sum(entries.values()), "files": entries}


def database_counts(store, checkpoint=False):
    # Writes, including checkpoint, are confined to the runner-owned synthetic store.
    database = store / "vrg.sqlite3"
    with sqlite3.connect(database) as connection:
        if checkpoint:
            result = connection.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()
            if result[0]:
                raise RuntimeError("fixture WAL checkpoint is busy; no stabilized measurement")
        tables = {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        counts = {table: connection.execute(f'SELECT count(*) FROM "{table}"').fetchone()[0]
                  for table in ["generations", "documents", "chunks", "chunk_manifests", "text_objects", "vector_objects", "vectors"]
                  if table in tables}
        counts["schema_version"] = connection.execute("SELECT version FROM schema_info WHERE singleton=1").fetchone()[0]
        counts["active_generation"] = connection.execute("SELECT generation_id FROM active_generation WHERE singleton=1").fetchone()[0]
    return counts


def profile(path, root, store, candidate, manifest):
    text = (f"data_dir = {json.dumps(str(store))}\n"
            "capture_session_events = false\nchunk_bytes = 2400\nchunk_overlap_bytes = 240\n"
            "[[roots]]\nid = \"corpus\"\n"
            f"path = {json.dumps(str(root))}\nexclusions = []\n")
    if candidate:
        text += ("[adaptive_memory]\nenabled = true\nretain_generations = 12\n"
                 f"max_response_bytes = {manifest['response_bytes']}\n"
                 f"max_context_units = {manifest['estimated_units']}\n")
    path.write_text(text)
    path.chmod(0o600)


def prepare_proposal(template, plan, corpus):
    proposal = copy.deepcopy(template)
    for key in ["project_id", "privacy_scope_id", "expected_generation"]:
        proposal[key] = plan[key]
    sources = {entry["source_id"]: entry for entry in plan["sources"]}
    for operation in proposal["operations"]:
        for reference in operation["sources"]:
            source_id = reference["source_id"]
            if source_id not in sources:
                raise ValueError("proposal template references a source outside the fixture plan")
            path = source_path(source_id, corpus)
            raw = path.read_bytes()
            if hashlib.sha256(raw).hexdigest() != sources[source_id]["revision_sha256"]:
                raise ValueError("proposal plan source revision mismatch")
            reference.update(revision_sha256=sources[source_id]["revision_sha256"], span_start=0, span_end=len(raw))
    return proposal


def evaluate_query(binary, config, query, corpus, cwd):
    response, mcp_result, elapsed, handshake = mcp_call(binary, config, "vrg_search", {
        "query": query["query"], "semantic": False, "limit": 1,
        "root_ids": ["corpus"], "graph_hops": 0}, cwd)
    first = measurement(response)
    first["complete_mcp_result"] = measurement(mcp_result)
    first["protocol_handshake_separate"] = handshake
    expected = query["expected_source"]
    expected_revision = sha_file(corpus / expected)
    matches = [hit for hit in response.get("hits", []) if hit["source"] == {"root_id": "corpus", "relative_path": expected}
               and hit["revision"] == expected_revision]
    snippets = "\n".join(hit["snippet"] for hit in matches)
    missing = [fact for fact in query["required_facts"] if fact not in snippets]
    expansion_flag = bool(response.get("budget", {}).get("needs_expansion")) or any(
        memory.get("needs_expansion") for memory in response.get("memory", []))
    expansions = []
    if matches and missing:
        source = matches[0]["source"]
        document, output, seconds, handshake = mcp_call(binary, config, "vrg_read_source", {
            "root_id": source["root_id"], "relative_path": source["relative_path"],
            "max_bytes": 32768}, cwd)
        measured = measurement(document)
        measured["complete_mcp_result"] = measurement(output)
        exact = document.get("raw_markdown", "")
        valid = document.get("source") == matches[0]["source"] and document.get("revision") == expected_revision
        if valid:
            missing = [fact for fact in query["required_facts"] if fact not in snippets + "\n" + exact]
        expansions.append({**measured, "latency_seconds": seconds, "source_revision_valid": valid,
                           "reason": "mandatory_facts_missing",
                           "protocol_handshake_separate": handshake})
        elapsed += seconds
    return {"query": query["query"], "expected_source": expected, "source_revision_valid": bool(matches),
            "partial_preview_warning": expansion_flag,
            "partial_preview_count": sum(bool(memory.get("needs_expansion")) for memory in response.get("memory", [])),
            "mandatory_facts_pass": bool(matches) and not missing,
            "missing_facts": missing, "search": first, "expansions": expansions,
            "total_context_bytes": first["canonical_response_bytes"] + sum(x["canonical_response_bytes"] for x in expansions),
            "total_evaluation_units": first["evaluation_units"] + sum(x["evaluation_units"] for x in expansions),
            "total_mcp_result_bytes": first["complete_mcp_result"]["canonical_response_bytes"] + sum(x["complete_mcp_result"]["canonical_response_bytes"] for x in expansions),
            "total_mcp_result_evaluation_units": first["complete_mcp_result"]["evaluation_units"] + sum(x["complete_mcp_result"]["evaluation_units"] for x in expansions),
            "latency_seconds": elapsed}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["baseline", "candidate", "fixture", "manifest", "work", "proposal-template"]:
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    args.baseline = args.baseline.resolve(strict=True)
    args.candidate = args.candidate.resolve(strict=True)
    args.fixture = args.fixture.resolve(strict=True)
    args.manifest = args.manifest.resolve(strict=True)
    args.proposal_template = args.proposal_template.resolve(strict=True)
    if args.work.resolve().is_relative_to(args.fixture):
        raise ValueError("work directory must be outside the frozen fixture")
    if args.work.exists() or args.work.is_symlink():
        raise ValueError("work directory must be new; refusing to overwrite prior evidence")
    args.work.mkdir(parents=True, mode=0o700)
    args.work = args.work.resolve()
    report = {"catalog": ["AM-T02", "AM-T14"], "status": "RUNNING", "tokenizer": TOKENIZER,
              "retrieval_interface": "Codex read-only MCP vrg_search, semantic=false, limit=1, same corpus root, existing 512-byte snippet bounds",
              "expansion_policy": "quality-oracle evaluator expands both variants only when frozen task-mandatory facts lack attribution; partial-preview warnings are reported, not suppressed; this is not autonomous sufficiency judgment or general source completeness",
              "provider_billed_tokens": "unknown", "model_fixed_bytes": 0,
              "baseline_binary_sha256": sha_file(args.baseline), "candidate_binary_sha256": sha_file(args.candidate),
              "fixture_manifest_sha256": sha_file(args.manifest), "proposal_template_sha256": sha_file(args.proposal_template)}
    try:
        manifest = json.loads(args.manifest.read_text())
        if manifest["documents"] != 30 or manifest["revisions"] != 12 or len(manifest["queries"]) != 8:
            raise ValueError("expected the frozen 30-document/12-revision/eight-query fixture")
        before = snapshot_directory(args.fixture)
        if before != manifest["files"]:
            raise ValueError("fixture differs from its frozen file manifest")
        proposal_template = json.loads(args.proposal_template.read_text())
        corpus = args.work / "corpus"
        corpus.mkdir()
        variants = {}
        for name, binary in [("baseline", args.baseline), ("candidate", args.candidate)]:
            store = args.work / name / "derived"
            store.mkdir(parents=True)
            config = args.work / name / "config.toml"
            profile(config, corpus, store, name == "candidate", manifest)
            variants[name] = {"binary": binary, "store": store, "config": config, "generations": [],
                              "sampled_peak_derived_bytes": 0, "index_latency_seconds": []}
        for revision in range(12):
            source = args.fixture / f"r{revision:02}"
            if len(list(source.glob("*.md"))) != 30:
                raise ValueError("revision does not contain thirty Markdown documents")
            for file in source.iterdir():
                shutil.copy2(file, corpus / file.name)
            for variant in variants.values():
                indexed, elapsed, _ = run_cli(variant["binary"], variant["config"], ["index"], args.work)
                variant["generations"].append(indexed["generation_id"])
                variant["index_latency_seconds"].append(elapsed)
                variant["sampled_peak_derived_bytes"] = max(variant["sampled_peak_derived_bytes"], disk_usage(variant["store"])["total_bytes"])
        candidate = variants["candidate"]
        before_noop = database_counts(candidate["store"])
        for _ in range(100):
            result, _, _ = run_cli(candidate["binary"], candidate["config"], ["index"], args.work)
            if result["generation_id"] != before_noop["active_generation"]:
                raise RuntimeError("no-op index published a new generation")
        after_noop = database_counts(candidate["store"])
        report["AM-T02_noop"] = {"iterations": 100, "before": before_noop, "after": after_noop,
                                  "pass": before_noop == after_noop}
        plan, _, _ = run_cli(candidate["binary"], candidate["config"], ["memory", "plan"], args.work)
        proposal = prepare_proposal(proposal_template, plan, corpus)
        proposal_path = args.work / "proposal.json"
        write_json(proposal_path, proposal)
        applied, _, _ = run_cli(candidate["binary"], candidate["config"], ["memory", "apply", "--proposal", str(proposal_path)], args.work)
        report["proposal_apply"] = applied
        candidate["sampled_peak_derived_bytes"] = max(candidate["sampled_peak_derived_bytes"], disk_usage(candidate["store"])["total_bytes"])
        # Observable proposal input/output units; no assertion of billed model cost.
        source_ids = {ref["source_id"] for op in proposal["operations"] for ref in op["sources"]}
        proposal_input = compact(plan) + "\n" + "\n".join(source_path(sid, corpus).read_text() for sid in sorted(source_ids))
        report["cost"] = {"counted_preparation_material": {"bytes": len(proposal_input.encode()), "evaluation_units": units(proposal_input)},
                          "counted_proposal_output": measurement(proposal), "input_accounting_limit": "plan metadata and referenced raw sources counted; actual model input, prompting and validation overhead are unknown", "per_update_observable_cost": "not measured; proposals applied after final revision only",
                          "billed_model_cost": "unknown", "classification_generated_by_runner": False}
        results = {}
        for name, variant in variants.items():
            queries = [evaluate_query(variant["binary"], variant["config"], query, corpus, args.work) for query in manifest["queries"]]
            variant["sampled_peak_derived_bytes"] = max(variant["sampled_peak_derived_bytes"], disk_usage(variant["store"])["total_bytes"])
            counts = database_counts(variant["store"], checkpoint=True)
            if counts["generations"] != 12:
                raise RuntimeError(f"{name} does not retain the twelve equivalent revisions")
            results[name] = {"queries": queries, "context_bytes": sum(q["total_context_bytes"] for q in queries),
                             "evaluation_units": sum(q["total_evaluation_units"] for q in queries),
                             "complete_mcp_result_bytes": sum(q["total_mcp_result_bytes"] for q in queries),
                             "complete_mcp_result_evaluation_units": sum(q["total_mcp_result_evaluation_units"] for q in queries),
                             "mandatory_facts_pass": all(q["mandatory_facts_pass"] for q in queries),
                             "latency_limit_pass": all(q["latency_seconds"] <= manifest["latency_limit_seconds"] for q in queries),
                             "max_query_latency_seconds": max(q["latency_seconds"] for q in queries),
                             "stabilized_derived": disk_usage(variant["store"]), "database_counts": counts,
                             "sampled_peak_derived_bytes": variant["sampled_peak_derived_bytes"],
                             "peak_sampling_limit": "sampled between commands; intra-command transient peak unknown",
                             "backup_bytes": 0, "model_bytes": 0, "index_latency_seconds": variant["index_latency_seconds"]}
        report["variants"] = results
        old, new = results["baseline"], results["candidate"]
        saved_units = old["evaluation_units"] - new["evaluation_units"]
        report["comparison"] = {"saved_context_bytes": old["context_bytes"] - new["context_bytes"],
                                "saved_evaluation_units": saved_units,
                                "saved_complete_mcp_result_bytes": old["complete_mcp_result_bytes"] - new["complete_mcp_result_bytes"],
                                "saved_complete_mcp_result_evaluation_units": old["complete_mcp_result_evaluation_units"] - new["complete_mcp_result_evaluation_units"],
                                "saved_stabilized_derived_bytes": old["stabilized_derived"]["total_bytes"] - new["stabilized_derived"]["total_bytes"],
                                "break_even_queries_under_counted_material_only": math.ceil((units(proposal_input) + units(compact(proposal))) / (saved_units / 8)) if saved_units > 0 else None,
                                "net_billed_savings": "unknown; provider cost not observable"}
        report["source_fixture_unchanged"] = snapshot_directory(args.fixture) == before
        report["current_corpus_unchanged"] = snapshot_directory(corpus) == {k.removeprefix("r11/"): v for k, v in before.items() if k.startswith("r11/")}
        rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
        report["peak_child_rss_kib"] = rss
        report["rss_measurement"] = "Linux getrusage cumulative maximum over child commands, no model configured"
        report["rss_limit_pass"] = rss <= manifest["rss_limit_mib_excluding_model"] * 1024
        passed = (report["AM-T02_noop"]["pass"] and report["source_fixture_unchanged"]
                  and report["current_corpus_unchanged"] and report["rss_limit_pass"]
                  and old["mandatory_facts_pass"] and new["mandatory_facts_pass"]
                  and old["latency_limit_pass"] and new["latency_limit_pass"]
                  and report["comparison"]["saved_stabilized_derived_bytes"] > 0 and saved_units > 0)
        report["status"] = "PASS" if passed else "FAIL"
    except Exception as error:
        report["status"] = "ERROR"
        report["error"] = f"{type(error).__name__}: {error}"
    write_json(args.work / "result.json", report)
    print(compact({"status": report["status"], "result": str(args.work / "result.json")}))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
