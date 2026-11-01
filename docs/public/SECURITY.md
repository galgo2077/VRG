# Security and privacy boundaries

VRG's public-facing design is **local first and least privilege**. The following are intended policy boundaries; the complete source release is required for an independent implementation audit.

## Source access

Approved source roots must be explicit. Canonical path checks, exclusions, frontmatter privacy markers, and symlink containment are important safeguards. No tool should treat a broad filesystem permission as permission to index every file.

## Data model

Original Markdown stays editable and authoritative. Derived SQLite, FTS indexes, vectors, and graph data belong outside source directories. Backups and private source manifests must not be committed to the public repository.

## MCP and remote models

A local MCP server can return a snippet to an assistant. **If that assistant uses a remote model, the returned snippet may enter a remote context.** Local embeddings alone do not guarantee local inference. Only approved material should be used in that flow.

## Hooks and documentation

Hooks are privileged automation and require review in Codex. Public sample configurations must not include machine-specific trust state. Selective project documentation must use explicitly permitted files, not raw conversations or indiscriminate logs.

## Public repository hygiene

Exclude private vaults, `.env`, credentials, tokens, SQLite/WAL files, embeddings, model weights, session logs, hook trust stores, and private project memory. Scan newly added content **and already tracked files** before publishing a code snapshot.

## Reporting a potential issue

Avoid including secrets or actual private notes in public GitHub issues. Until a dedicated security contact is published, do not post sensitive exploit details or credentials publicly.

This document is a boundary statement, **not** a claim of completed third-party security certification.
