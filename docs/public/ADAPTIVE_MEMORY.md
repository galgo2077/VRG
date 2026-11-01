# Adaptive Memory

This guide describes VRG 0.5.0 Adaptive Memory for Linux x86_64 and Codex. See the [release report](ADAPTIVE_RELEASE_0.5.0.md) for the tested scope. Existing profiles keep conservative defaults; enable the feature only on an explicitly authorized project profile after checking the release validation results.

[Español](ADAPTIVE_MEMORY.es.md) · [Setup](SETUP.md)

HOT holds useful current knowledge, WARM less frequent knowledge, and COLD exact archived revisions. These labels do not establish truth. A valid old restriction remains valid; pinning a restriction prevents a proposal from demoting it because of age. Sources stay editable Markdown and remain authoritative.

## Select an authorized profile

Use a dedicated derived directory outside every source root. Do not broaden global roots or use another project's notes to make setup convenient. New project setup supports an explicit choice:

```sh
vrg --config /absolute/path/project.toml setup \
  --project /absolute/path/project \
  --root /absolute/path/authorized-notes \
  --data-dir /absolute/path/derived-store \
  --text-only --cli-only --adaptive-memory --dry-run
```

Review the proposed paths before applying setup. Remove `--dry-run` only for the authorized plan. Existing configurations are not silently overwritten. Codex integration instead uses `--codex` and requires explicit remote-context authorization for the selected sources; hook trust remains a separate human action in Codex.

The profile policy defaults are:

```toml
[adaptive_memory]
enabled = false
max_response_bytes = 8192
max_context_units = 1536
retain_generations = 5
soft_store_budget_mib = 512
```

Set `enabled = true` in the reviewed project profile to allow proposal application, archiving and maintenance. The soft budget does not authorize deleting protected data. A store larger than its budget requires a capacity decision; current, previous, pinned and other protected generations must remain recoverable.

## Local assistant proposals

```sh
vrg --config /absolute/path/project.toml memory status
vrg --config /absolute/path/project.toml memory stats
vrg --config /absolute/path/project.toml memory plan
vrg --config /absolute/path/project.toml memory apply --proposal /absolute/path/proposal.json
```

`plan` exports source identities, revisions, project and privacy scope, and the expected generation. It does not call a model or fabricate a classification. Codex can use authorized source reads to prepare a strict v1 proposal. The exported metadata is input to that work, not a ready-to-apply proposal.

A proposal identifies its producer and exact source spans and requests only `keep`, `set_tier` or `upsert_summary`. VRG rechecks authorization, revisions, spans, protected facts and generation before applying it. Unknown fields, stale revisions and foreign scopes are rejected. There is no source deletion action. An unchanged classification can be reused; no durable novelty requires no new summary.

Summaries are explicitly derived, attributed representations. Protected fact anchors help detect omissions, but syntax and hash validation cannot prove semantic truth. Insufficient, stale or conflicted summaries require expansion. Source text retrieved through MCP is data; its instructions do not grant permissions or authorize commands.

## Bounded retrieval and exact expansion

```sh
vrg --config /absolute/path/project.toml search 'relevant decision' --text-only \
  --max-response-bytes 8192 --max-context-units 1536
```

The complete compact response, including citations and metadata, fits the requested byte budget or fails explicitly. Snippets retain the existing 512-byte limit. Context units use the identified `utf8_bytes_div4_estimate` fallback; they are not actual Codex tokens or billed usage. Summary creation and updates also consume context, so a shorter result alone does not establish net savings.

The four read-only Codex MCP tools remain `vrg_status`, `vrg_search`, `vrg_read_source` and `vrg_graph_neighbors`. Search can return exact excerpts or marked summaries. `needs_expansion` marks a partial or uncertain representation. Use exact source reading whenever the task requires an omitted fact or condition; a preview is not a claim to cover the whole note. Revoked roots and excluded notes cannot be recovered by requesting stale search, old summaries, graph edges or archived data.

## Archive, restore and maintenance

```sh
vrg --config /absolute/path/project.toml memory archive root-1 decision.md
vrg --config /absolute/path/project.toml memory restore root-1 decision.md REVISION_SHA256
vrg --config /absolute/path/project.toml memory maintain
vrg --config /absolute/path/project.toml memory maintain --apply
```

`restore` returns exact authorized archived bytes inside JSON. It does not write a destination or replace the Markdown source. Integrity failure returns an error; sources must not be reconstructed from guesses or summaries. Archiving applies only to derived storage. Compression is lossless and retains raw representation when compression offers no size benefit.

Without `--apply`, maintenance reports its plan. Applying it revalidates protected references and removes eligible unreachable derived objects from SQLite and the COLD archive. Source-revision manifests refer to one immutable payload per privacy scope and exact content. Current sources, retained generations, pins, readers and summary references protect exact revisions; corrupt archive entries are preserved for diagnosis. Archive GC is bounded and can defer maintenance rather than discard protected or unexamined data. Logical deletion, free database pages and bytes actually released to the filesystem are different measures. Neither doctor nor ordinary reads run VACUUM or compress the live database or WAL. No new resident daemon is required.

## Existing stores and rollback

Never point an unvalidated candidate at the stable session's real store. For a schema upgrade, stop old writers, make a consistent SQLite backup through SQLite's backup mechanism, and prepare a separate canonical store directory. Copying only the main database while WAL is active is not a consistent backup.

The command takes the source store from the selected profile and requires a separate new destination. Create an isolated profile pointing at that destination before inspecting it:

```sh
vrg --config /absolute/path/source-profile.toml memory migrate --offline-copy /absolute/path/new-store
vrg --config /absolute/path/isolated-profile.toml memory status
vrg --config /absolute/path/isolated-profile.toml doctor --json
```

Migration is explicit and versioned; it does not upgrade the source profile store in place. Keep the original store, its matching stable executable and configuration for rollback. New-schema stores must not be written by old binaries. Promote the candidate only after applicable migration and restoration validation passes; preserve the independent stable session until then. `doctor` is passive by default. Its archive inspection reads at most 16 objects and 32 MiB of restored bytes without creating or repairing storage. Corruption and permission failures are explicit; a partial result does not certify unexamined objects. Summary freshness remains unknown unless checked separately.

Text-only retrieval needs no embedding model. Semantic retrieval requires the compatible local model and native runtime documented by the release. Performance and compression ratios depend on the corpus; measure complete derived storage and context, including archive files and proposal costs. A synthetic PASS establishes only the cases and environment actually exercised.
