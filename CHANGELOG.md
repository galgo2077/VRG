# Changelog

## 0.6.0 — Project Nuclei

- Native read-only graph viewer with separate canonical project constellations and stable project colors.
- Deterministic project/folder layout, progressive transfer and GPU LOD using real chunk identities and authorized memberships.
- Smooth navigation, minimap, project/folder hover, source inspector, search, filters and explicit coverage counters.
- Shared worktree origins remain navigable without becoming duplicate projects; source registry schema 1/2 checks bind profiles, provenance and the active generation.
- Portable viewer configuration through XDG paths or an explicit source-registry override; no machine-specific project list is bundled.
- `vrg graph /absolute/project` opens the native viewer; `vrg view --all` opens the registered collection. `vrg graph ROOT_ID RELATIVE_PATH` remains an indexed-source relation query.
- Linux x86_64 GitHub distribution, offline release build, notices and checksums. No crates.io publication, model bundle, automatic migration or global installation replacement.

## 0.5.0 — Adaptive Memory

- Opt-in HOT/WARM/COLD proposals from Codex with strict source revision and access checks.
- Domain-separated immutable payloads and generation manifests, bounded lossless DEFLATE/raw encoding and exact-source archives.
- Explicit context budgets, attributed derived previews and exact expansion.
- Protected retention and maintenance, offline-copy schema 1→2 migration with rollback, passive doctor diagnostics.
- Linux x86_64 and Codex release scope; the global stable installation is not automatically replaced.

## 0.1.1

Published Cargo package with manual setup. F4 certification remains tied to its recorded historical revision and environment; its tag is unchanged.
