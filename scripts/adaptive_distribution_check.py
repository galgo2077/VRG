#!/usr/bin/env python3
"""AM-T17/18 isolated checks for a built or publicly downloaded Linux binary.

Creates only a new work directory. Existing models are read locally and never
downloaded. This runner does not build, install, publish or alter global profiles.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import time


SOURCE = ("# Release fixture\n\nAM_RELEASE_COLD_719. Do not delete original sources. "
          "Rollback requires a verified backup.\n").encode()


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def store_files(root):
    return {str(path.relative_to(root)): {"sha256": sha(path), "bytes": path.stat().st_size}
            for path in sorted(root.rglob("*")) if path.is_file() and not path.is_symlink()}


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def write_profile(path, memory, store, model=None):
    content = (f"data_dir = {json.dumps(str(store))}\n"
               "capture_session_events = false\n"
               "[[roots]]\nid = \"release\"\n"
               f"path = {json.dumps(str(memory))}\nexclusions = []\n"
               "[adaptive_memory]\nenabled = true\nretain_generations = 5\n"
               "max_response_bytes = 8192\nmax_context_units = 1536\n")
    if model:
        content += ("[semantic_model]\nmodel_id = \"intfloat/multilingual-e5-small\"\n"
                    f"model_dir = {json.dumps(str(model))}\nthreads = 2\nmax_length = 256\n")
    path.write_text(content)
    path.chmod(0o600)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--model-dir", type=Path)
    parser.add_argument("--schema1-fixture", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    model = args.model_dir.resolve(strict=True) if args.model_dir else None
    fixture = args.schema1_fixture.resolve(strict=True) if args.schema1_fixture else None
    if args.work.exists() or args.work.is_symlink():
        raise ValueError("work must be new; refusing to overwrite evidence")
    if model and args.work.resolve().is_relative_to(model):
        raise ValueError("work must be outside the read-only model directory")
    args.work.mkdir(parents=True, mode=0o700)
    work = args.work.resolve()
    environment = os.environ.copy()
    for variable in ["VRG_CONFIG", "VRG_DATA_DIR", "HF_HOME", "HUGGINGFACE_HUB_CACHE", "TRANSFORMERS_CACHE"]:
        environment.pop(variable, None)
    for variable, directory in [("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                                ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache"),
                                ("XDG_STATE_HOME", "state")]:
        location = work / directory
        location.mkdir()
        environment[variable] = str(location)
    environment["HF_HUB_OFFLINE"] = "1"
    environment["TRANSFORMERS_OFFLINE"] = "1"
    report = {"catalog": ["AM-T17", "AM-T18"], "status": "RUNNING", "binary_sha256": sha(binary),
              "binary_bytes": binary.stat().st_size, "isolated_home_xdg": True,
              "cwd_outside_checkout": not any(parent.joinpath(".git").exists() for parent in [work, *work.parents]),
              "feature_flags": {"text": True, "archive": True, "maintenance": True,
                                "semantic_requested": bool(model), "migration_requested": bool(fixture)}, "steps": []}

    def run(config, *arguments, as_json=True):
        command = [str(binary)] + (["--config", str(config)] if config else []) + list(arguments)
        started = time.monotonic()
        result = subprocess.run(command, cwd=work, env=environment, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=180, check=False)
        report["steps"].append({"command": list(arguments), "returncode": result.returncode,
                                "latency_seconds": time.monotonic() - started, "stdout_bytes": len(result.stdout)})
        if result.returncode:
            raise RuntimeError(f"{' '.join(arguments)} exited {result.returncode}: "
                               + result.stderr.decode(errors="replace")[:1200])
        return json.loads(result.stdout) if as_json else result.stdout.decode().strip()

    try:
        if not report["cwd_outside_checkout"]:
            raise ValueError("work must be outside every checkout")
        report["version"] = run(None, "--version", as_json=False)
        memory = work / "memory"
        memory.mkdir()
        note = memory / "release.md"
        note.write_bytes(SOURCE)
        source_sha = sha(note)
        store = work / "derived"
        config = work / "profile.toml"
        write_profile(config, memory, store)
        indexed = run(config, "index")
        plan = run(config, "memory", "plan")
        source_id = "release:release.md"
        refs = [source for source in plan["sources"] if source["source_id"] == source_id]
        if len(refs) != 1 or refs[0]["revision_sha256"] != source_sha:
            raise RuntimeError("memory plan did not attribute the release source revision")
        proposal = {"schema_version": 1, "proposal_id": "distribution-codex-tier-v1",
                    "project_id": plan["project_id"], "privacy_scope_id": plan["privacy_scope_id"],
                    "expected_generation": plan["expected_generation"],
                    "created_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                    "producer": {"kind": "codex", "identity": "Codex distribution fixture proposal"},
                    "operations": [{"operation_id": "cold-tier", "action": "set_tier", "memory_id": "release-fixture",
                                    "sources": [{"source_id": source_id, "revision_sha256": source_sha,
                                                 "span_start": 0, "span_end": len(SOURCE)}],
                                    "reason_code": "infrequent_but_valid", "target_tier": "COLD"}]}
        proposal_path = work / "proposal.json"
        write_json(proposal_path, proposal)
        applied = run(config, "memory", "apply", "--proposal", str(proposal_path))
        if not applied.get("applied"):
            raise RuntimeError("fresh COLD proposal was not applied")
        archived = run(config, "memory", "archive", "release", "release.md")
        restored = run(config, "memory", "restore", "release", "release.md", source_sha)
        if restored.get("raw_markdown", "").encode() != SOURCE:
            raise RuntimeError("archive did not restore exact source bytes")
        search = run(config, "search", "AM_RELEASE_COLD_719", "--text-only", "--limit", "1")
        hits = search.get("hits", [])
        if not hits or hits[0]["source"] != {"root_id": "release", "relative_path": "release.md"} or hits[0]["revision"] != source_sha:
            raise RuntimeError("COLD symbol retrieval lost source attribution")
        if not any(item.get("tier") == "COLD" for item in search.get("memory", [])):
            raise RuntimeError("retrieved source did not retain its COLD classification")
        stats = run(config, "memory", "stats")
        maintenance_plan = run(config, "memory", "maintain")
        maintenance_result = run(config, "memory", "maintain", "--apply")
        report["doctor"] = run(config, "doctor", "--json")
        if note.read_bytes() != SOURCE or sha(note) != source_sha:
            raise RuntimeError("distribution operations changed original source bytes")
        report["text_archive"] = {"generation": indexed["generation_id"], "source_sha256": source_sha,
                                  "restored_sha256": hashlib.sha256(restored["raw_markdown"].encode()).hexdigest(),
                                  "source_unchanged": True, "archive": archived,
                                  "cold_search": search, "stats": stats,
                                  "maintenance_plan": maintenance_plan, "maintenance_result": maintenance_result}
        if model:
            before = store_files(model)
            semantic_store = work / "semantic-derived"
            semantic_config = work / "semantic.toml"
            write_profile(semantic_config, memory, semantic_store, model)
            semantic_index = run(semantic_config, "index")
            if semantic_index.get("semantic_status", {}).get("state") != "ready":
                raise RuntimeError("configured local semantic model did not become ready")
            semantic_search = run(semantic_config, "search", "recover an original source after a rollback", "--limit", "1")
            if semantic_search.get("mode") != "hybrid" or not semantic_search.get("hits"):
                raise RuntimeError("distributed binary did not perform hybrid retrieval")
            database = semantic_store / "vrg.sqlite3"
            connection = sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)
            try:
                vectors = connection.execute("SELECT count(*) FROM vector_objects").fetchone()[0]
            finally:
                connection.close()
            if vectors < 1:
                raise RuntimeError("semantic fixture did not persist a document vector")
            if store_files(model) != before:
                raise RuntimeError("semantic use changed the supplied read-only model cache")
            report["semantic"] = {"state": "PASS", "model_identity": "intfloat/multilingual-e5-small",
                                  "existing_cache_used": True, "download_requested": False,
                                  "cache_files": before, "cache_total_bytes": sum(item["bytes"] for item in before.values()),
                                  "stored_document_vector_objects": vectors,
                                  "query_embedding_evidence": "successful hybrid retrieval requires a query embedding; raw queries are not stored",
                                  "search": semantic_search}
        if fixture:
            legacy = work / "legacy-derived"
            legacy.mkdir()
            database = legacy / "vrg.sqlite3"
            sql = fixture.read_text()
            connection = sqlite3.connect(database)
            try:
                connection.set_authorizer(lambda action, *_: sqlite3.SQLITE_DENY if action in [sqlite3.SQLITE_ATTACH, sqlite3.SQLITE_DETACH] else sqlite3.SQLITE_OK)
                connection.executescript(sql)
                if connection.execute("SELECT count(*) FROM generations").fetchone()[0] == 0:
                    body = "Do not publish until rollback is verified."
                    revision = hashlib.sha256(body.encode()).hexdigest()
                    connection.execute("INSERT INTO generations VALUES('legacy',NULL,1,1,1,1)")
                    connection.execute("INSERT INTO documents VALUES('legacy','release','legacy.md',?,'Legacy','{}',NULL)", (revision,))
                    connection.execute("INSERT INTO chunks VALUES('legacy','legacy-chunk','release','legacy.md',?,0,0,?,?)", (revision, len(body.encode()), body))
                    connection.execute("INSERT INTO chunk_fts(generation_id,chunk_id,root_id,relative_path,title,body) VALUES('legacy','legacy-chunk','release','legacy.md','Legacy',?)", (body,))
                    connection.execute("INSERT INTO active_generation VALUES(1,'legacy')")
                connection.commit()
            finally:
                connection.close()
            original_sha = sha(database)
            legacy_config = work / "legacy.toml"
            write_profile(legacy_config, memory, legacy)
            destination = work / "migrated-derived"
            migrated = run(legacy_config, "memory", "migrate", "--offline-copy", str(destination))
            if sha(database) != original_sha:
                raise RuntimeError("offline-copy migration changed original database bytes")
            copied = sqlite3.connect((destination / "vrg.sqlite3").as_uri() + "?mode=ro", uri=True)
            try:
                schema = copied.execute("SELECT version FROM schema_info").fetchone()[0]
                if schema != 2 or copied.execute("SELECT count(*) FROM chunk_manifests").fetchone()[0] < 1:
                    raise RuntimeError("migrated copy did not retain schema-1 content")
            finally:
                copied.close()
            report["migration"] = {"state": "PASS", "source_database_sha256": original_sha,
                                   "source_unchanged": True, "destination_schema": schema, "result": migrated,
                                   "fixture_sha256": sha(fixture)}
        report["derived_files"] = store_files(store)
        report["source_unchanged_at_end"] = note.read_bytes() == SOURCE
        if not report["source_unchanged_at_end"]:
            raise RuntimeError("original source changed during optional checks")
        report["status"] = "PASS"
    except Exception as error:
        report["status"] = "FAIL"
        report["error"] = f"{type(error).__name__}: {error}"
    write_json(work / "result.json", report)
    print(json.dumps({"status": report["status"], "result": str(work / "result.json")}, separators=(",", ":")))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
