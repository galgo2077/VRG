"""Enroll the project selected by a Codex cwd inside an authorized discovery root.

Discovery roots authorize project selection, never a scan of sibling projects.
The native VRG projection policy owns source discovery and secret screening.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tomllib

MODEL_ID = "intfloat/multilingual-e5-small"
ALLOWED_NOTES = ["STATUS.md", "DECISIONS.md", "SOLUTIONS.md"]
EXTENSIONS = "md py pyi rs js jsx mjs cjs ts tsx css html htm c h cc cpp hpp cxx hxx cs java kt go rb php swift m r lua pl sh bash zsh fish ps1 sql cu cuh wgsl glsl proto dart nim zig nix".split()
EXCLUDED = ".git .obsidian .codex .venv venv __pycache__ .cache node_modules target dist build .next .pytest_cache .mypy_cache .ruff_cache coverage backups logs secrets credentials keyrings .ssh .aws .gnupg".split()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def encoded(data):
    return (json.dumps(data, ensure_ascii=False, indent=2) + "\n").encode()


def checked_directory(path):
    path = Path(path)
    info = path.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or path.resolve(strict=True) != path):
        raise ValueError("project discovery requires an owned canonical directory")
    return path


def candidate(cwd, registry):
    here = checked_directory(Path(cwd).resolve(strict=True))
    roots = registry.get("discovery_roots", [])
    if not isinstance(roots, list):
        raise ValueError("invalid project discovery policy")
    matches = []
    for value in roots:
        root = checked_directory(value)
        if here == root or not here.is_relative_to(root):
            continue
        relative = here.relative_to(root)
        project = checked_directory(root / relative.parts[0])
        if project.parent != root:
            raise ValueError("discovered project is not a direct child")
        matches.append(project)
    if len(set(matches)) > 1:
        raise ValueError("ambiguous project discovery root")
    return matches[0] if matches else None


def _directory(path):
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or info.st_mode & 0o077 or path.resolve(strict=True) != path):
        raise ValueError("unsafe private project data directory")


def _file(path, data):
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    except FileExistsError:
        info = path.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_mode & 0o077 or info.st_nlink != 1
                or path.read_bytes() != data):
            raise ValueError("existing project metadata differs from prepared enrollment")
        return
    with os.fdopen(fd, "wb") as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())


def _toml_string(value):
    return json.dumps(str(value))


def prepare(project, registry):
    """Create only private, deterministic files; publication is the registry write."""
    project = checked_directory(project)
    key = re.sub(r"[^A-Za-z0-9_.-]", "_", project.name)[:48]
    key += "-" + sha(str(project).encode())[:12]
    base = Path(registry["data_base"]) / ("auto-" + key)
    model_dir = checked_directory(registry["model_dir"])
    profiles = Path(registry["profiles_dir"])
    for directory in (base, base / "memory", base / "memory-store",
                      base / "source-store", base / "corpus", profiles):
        _directory(directory)
    for name, heading in (("STATUS.md", "Status"), ("DECISIONS.md", "Decisions"),
                          ("SOLUTIONS.md", "Solutions")):
        _file(base / "memory" / name, ("# " + heading + "\n").encode())
    index = ("# " + project.name + "\n\nIndexed project sources and generated module documentation.\n").encode()
    _file(base / "corpus" / "INDEX.md", index)
    root_id = "auto-source-" + sha(str(project).encode())[:12]
    policy = {
        "schema": 2, "data_dir": str(base / "source-store"),
        "corpus_root_id": root_id, "corpus_path": str(base / "corpus"),
        "original_roots": [str(project)], "note_roots": [], "policy_probes": [],
        "policy_version": "vrg-known-utf8-v2", "max_source_bytes": 1048576,
        "part_bytes": 16384, "sources": [], "notes": [],
        "hubs": [{"relative_path": "INDEX.md", "initial_sha256": sha(index),
                  "title": project.name, "aliases": [project.name],
                  "source_ids": [], "note_paths": []}],
        "passthrough_views": [],
        "discovery": {"enabled": True, "extensions": EXTENSIONS,
                      "excluded_components": EXCLUDED, "excluded_paths": [],
                      "filenames": ["Makefile", "GNUmakefile", "Dockerfile", "Containerfile"],
                      "extensionless_shebang": False, "module_docs": True,
                      "direct_sources": True, "automatic_memory": True},
    }
    policy_path = base / "source-policy.json"
    policy_bytes = encoded(policy)
    _file(policy_path, policy_bytes)
    memory_config = (f'data_dir={_toml_string(base / "memory-store")}\n'
                     '[adaptive_memory]\nenabled=true\n'
                     '[[roots]]\nid="auto-memory"\n'
                     f'path={_toml_string(base / "memory")}\n'
                     'allowed_files=["STATUS.md","DECISIONS.md","SOLUTIONS.md"]\n'
                     '[semantic_model]\n'
                     f'model_id={_toml_string(MODEL_ID)}\n'
                     f'model_dir={_toml_string(model_dir)}\n'
                     'threads=2\nmax_length=512\n').encode()
    source_config = (f'data_dir={_toml_string(base / "source-store")}\n'
                     'chunk_bytes=384\nchunk_overlap_bytes=48\n'
                     '[adaptive_memory]\nenabled=true\n'
                     '[semantic_model]\n'
                     f'model_id={_toml_string(MODEL_ID)}\n'
                     f'model_dir={_toml_string(model_dir)}\n'
                     'threads=2\nmax_length=512\n'
                     '[[roots]]\n'
                     f'id={_toml_string(root_id)}\n'
                     f'path={_toml_string(base / "corpus")}\n'
                     '[projection_invalidation]\n'
                     f'manifest_path={_toml_string(policy_path)}\n'
                     f'manifest_sha256={_toml_string(sha(policy_bytes))}\n').encode()
    memory_profile = profiles / ("auto-" + key + "-memory.toml")
    source_profile = profiles / ("auto-" + key + "-sources.toml")
    _file(memory_profile, memory_config)
    _file(source_profile, source_config)
    # Validate syntax before publishing this entry to independent readers.
    tomllib.loads(memory_config.decode())
    tomllib.loads(source_config.decode())
    return {
        "project": str(project), "label": project.name, "key": "auto-" + key,
        "profile": str(memory_profile), "profile_sha256": sha(memory_config),
        "memory": str(base / "memory"), "data_dir": str(base / "memory-store"),
        "allowed_files": ALLOWED_NOTES, "managed_note_writes": True,
        "auto_enrolled": True,
        "authorization": "explicit discovery root authorization",
        "source": {"project_id": "auto-" + key, "project_root": str(project),
                   "aliases": [], "config_path": str(source_profile),
                   "config_sha256": sha(source_config),
                   "data_dir": str(base / "source-store"),
                   "root_id": root_id, "corpus_path": str(base / "corpus"),
                   "policy_path": str(policy_path), "policy_sha256": sha(policy_bytes)},
    }
