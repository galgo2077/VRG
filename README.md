# VRG — Vault RAG Graph

**0.7.0-rc.1 preview:** opt-in automatic project onboarding for Linux x86_64 and Codex. See [setup and limitations](docs/public/AUTO_PROJECTS_PREVIEW.md). It requires an explicitly authorized project parent and an existing local semantic model; it does not migrate an existing installation automatically.

![VRG — Vault RAG Graph](docs/assets/vrg-banner.svg)

**Maintained by [galgo2077](https://github.com/galgo2077)** · [Website](https://galgo2077.github.io/VRG/) · [Setup](https://galgo2077.github.io/VRG/setup.html) · [Troubleshooting](https://galgo2077.github.io/VRG/#troubleshooting) · [Support development](https://buymeacoffee.com/galgo2077?new=1)

VRG is a local-first Rust backend for retrieving information from explicitly authorized Markdown roots. It combines SQLite/FTS5 text search, optional local embeddings, and a graph of explicit note relations. The original notes remain authoritative; indexes, vectors, and graph data are rebuildable derived state.

The CLI and a local stdio Model Context Protocol (MCP) server expose attributed results with source revisions and freshness information. Codex integration can use project hooks to reconcile approved roots and check selective operational documentation at the end of a turn. MCP tools remain read-only: the assistant uses normal file tools for any separately authorized documentation edits.

## Status

The Phase 4 baseline and local integration were validated on **Void Linux**. The reported `PHASE_4_PASS`, `VRG_HOOKS_PASS`, `VRG_AUTODOC_PASS`, and `VRG_FULLY_OPERATIONAL` results apply to the recorded local configuration and tested cases; they do not establish uninterrupted operation or certify another installation. macOS and Windows have not been certified. No Phase 5 certification is claimed.

The semantic model is optional. If no usable model is configured, text search remains available and semantic retrieval is reported as unavailable or degraded. VRG does not implicitly download models. Source-note writing through MCP is disabled.

## VRG 0.6.0 — Project Nuclei

The read-only local viewer presents registered projects as separate, consistently colored constellations. Explore real folders and chunks with smooth zoom, pan, a minimap, search, filters and a collapsible source inspector. The layout is deterministic and independent of the viewport. LOD draws subsets of real points while retaining every authorized loaded chunk; indexed, authorized, excluded, resident and drawn quantities remain distinct.

```sh
vrg graph /absolute/path/to/registered-project
vrg view --all
# Existing indexed-source graph queries retain their original syntax:
vrg graph root-id note.md
```

The viewer serves embedded assets on loopback, works offline and does not index, infer embeddings, edit notes or change hook trust. The session URL contains a temporary access secret; keep it private. [Viewer operation and permissions](docs/public/PROJECT_NUCLEI.md).

## Install — Linux x86_64

Use the [0.6.0 GitHub release](https://github.com/galgo2077/VRG/releases/tag/v0.6.0) and verify its `SHA256SUMS.txt` before extraction. Follow the archive's installation and runtime requirements. Keep the previous executable and profiles for rollback.

```sh
vrg --version
vrg setup --help
vrg doctor --help
vrg view --help
```

This release is distributed through GitHub. **Version 0.6.0 is not published to crates.io as part of this release.** The [0.5.0 crate](https://crates.io/crates/vrg/0.5.0) remains a separate published version; installing it does not install Project Nuclei.

A prepared source checkout builds with `cargo build --release --locked --offline`. Rust 1.92+ is declared; release builds use the pinned toolchain and pre-existing locked/native caches described by the [release protocol](docs/RELEASE_PROTOCOL.md). Offline builds cannot obtain missing dependencies or native libraries. The [English setup guide](docs/public/SETUP.md) and [guía en español](docs/public/SETUP.es.md) retain their explicitly versioned 0.5.0 instructions.

Setup produces a reviewable plan; doctor is passive by default. Adaptive Memory remains an explicit project opt-in. Existing stores use [offline-copy migration and rollback](docs/public/ADAPTIVE_MEMORY.md#existing-stores-and-rollback), without replacing a running stable store. Opening the viewer requires an existing authorized profile and derived index; it never runs setup or migration on your behalf.

The independent [v0.4.0-f4 baseline](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4) and [0.5.0 Adaptive Memory release](docs/public/ADAPTIVE_RELEASE_0.5.0.md) remain unchanged. Their historical certifications are not transferred to another package or installation.

## Assistant compatibility and support

For now, **Codex is the only officially supported AI assistant integration**, through VRG's read-only MCP interface and reviewed hooks. The local CLI can be used without Codex. Other assistants and MCP clients are not officially supported or validated yet.

**Buy Me a Coffee:** [Support VRG and galgo2077](https://buymeacoffee.com/galgo2077?new=1). This is an external site for voluntary contributions toward maintenance, documentation and further development. VRG remains free and open source; no donation is needed to download or use it.

## Installation help

See the [common issues section](https://galgo2077.github.io/VRG/#troubleshooting) for Cargo dependencies, PATH, MCP configuration, hook trust, synchronization, model files and Linux compatibility. More detailed instructions are in [the setup guide](docs/public/SETUP.md) and [la guía en español](docs/public/SETUP.es.md).

## Privacy and operational boundaries

- Roots are opt-in and must be canonical absolute paths. Indexing does not edit source Markdown.
- Keep derived data, models, logs, and backups outside source roots and out of Git.
- Built-in and configured exclusions reject protected paths and private-marked notes; they are not a semantic detector for every secret or private fact.
- MCP search and read results may contain note text. A remote Codex model can receive returned excerpts; decide whether that transfer is permitted before connecting it.
- Hooks need explicit review and trust in the Codex installation. They are not activated by installing VRG, and a trusted hook's successful execution must be verified in a real session.
- The portable Unix observer requires explicit local paths and separate review before use. It is a distribution adaptation, not the exact observer used in the recorded G04 validation. Do not publish local Codex configuration or trust records.

The distributed source does not include a user’s vaults, local databases, embeddings, or credentials.

## Adaptive Memory (Linux x86_64 / Codex)

The opt-in Adaptive Memory interface, exact archival, budgets and offline-copy migration are described in [English](docs/public/ADAPTIVE_MEMORY.md) and [Español](docs/public/ADAPTIVE_MEMORY.es.md). Summaries remain attributed derived previews; original source notes are preserved. Release validation results and measured limits are published separately.
