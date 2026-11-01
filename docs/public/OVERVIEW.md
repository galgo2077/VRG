# VRG — Public overview

VRG (Vault RAG Graph) is a Rust-based local-first knowledge system for Markdown vaults. The goal is to help assistants recover **the right project context**, rather than rereading whole repositories or preserving entire conversations.

## Intended workflow

1. A maintainer approves a specific source-root allowlist.
2. VRG validates paths and exclusions before reading eligible Markdown.
3. Derived indexes store revisions, text search data, optional semantic vectors, and link relationships outside source vaults.
4. A read-only MCP adapter enables Codex to search, inspect attributed sources, and navigate relations.
5. Approved project documentation can be updated selectively by Codex's ordinary file tools, while VRG incrementally indexes changes.

## Assistant compatibility

Currently **Codex is the only officially supported assistant integration**. VRG's local CLI is usable independently. Other MCP clients have not been validated or announced as supported.

## Voluntary support

[Buy Me a Coffee — galgo2077](https://buymeacoffee.com/galgo2077?new=1) is an external donation page supporting VRG maintenance and development. The project remains free and open source; donating is optional.

## Boundaries

- No implicit access to all of `$HOME`.
- No automatic rewriting of original Obsidian documents during indexing.
- No assumption that a local embedding model means the *assistant model* is local.
- No storage of full chat transcripts as a default memory mechanism.
- No unverified relationships presented as facts.

## Publication state

This source distribution includes installation guidance. Reported validation applies to tested local cases and does not certify another installation.

See [Getting started](GETTING_STARTED.md), [Security](SECURITY.md), and [Roadmap](ROADMAP.md).
