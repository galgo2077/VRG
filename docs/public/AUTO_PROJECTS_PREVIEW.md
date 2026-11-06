# Automatic project onboarding preview (0.7.0-rc.1)

This Linux x86_64 and Codex preview opts in one existing, owned project parent. It does not scan its existing children. A new direct child is enrolled when Codex works from that child's directory, or when a Codex turn started in the parent creates it and Stop observes its new directory and README. Each enrolled project gets a private three-note memory root, a separate source corpus, semantic vectors from a user-supplied local model, and dynamic bindings in one shared VRG process.

This preview does not migrate an existing VRG installation. Keep the previous release and its configuration. Test with a separate home/account first. The setup command refuses to replace an existing auto-project registry or manifest. It prepares configuration fragments but does not edit Codex settings, trust hooks, start a service, or authorize any root other than the one supplied.

## Prepare

Use the unpacked Linux distribution, which includes the binary and installer scripts. Confirm that the local `intfloat/multilingual-e5-small` directory contains `model.onnx`, `tokenizer.json`, `config.json`, `special_tokens_map.json`, and `tokenizer_config.json`. The model is not bundled or downloaded by this release. Select an owned canonical parent whose new direct children are intended projects. This grants discovery only; each child's source policy is created separately and names that exact child.

```sh
python3 scripts/vrg_auto_install.py \
  --discovery-root /absolute/path/to/authorized/projects \
  --model-dir /absolute/path/to/local/multilingual-e5-small \
  --binary /absolute/path/to/unpacked/vrg/bin/vrg \
  --port 59005
```

The installer creates private state under `~/.config/vrg/auto-projects.json`, `~/.local/state/vrg-auto`, `~/.local/share/vrg-auto`, and `~/.local/lib/vrg-auto`. It prints the exact server command and paths to two Codex configuration fragments. Review the generated registry, manifest and fragments before using them. Select an unused loopback port; do not copy this example port blindly.

Run the printed `vrg mcp shared MANIFEST` command under your existing user process supervisor. The service must remain running for Codex MCP and managed note CAS. It binds only `127.0.0.1`, rejects browser origins and uses per-binding bearer tokens. Keep the manifest private. Add the reviewed MCP fragment to Codex `config.toml`; merge the three hook entries into `hooks.json` without removing other hooks. Use Codex's normal hook trust review. Start a new Codex session after configuration.

## Daily trial

Create a new project from the authorized parent with an ordinary product prompt. To resolve its profile inside that project, use the installed `vrg_global.py resolve`; for code retrieval, use `vrg_sources.py search QUERY`. The standard parent hook may direct the agent to select its own project profile and document verified durable work. Managed notes use `vrg_global.py note append STATUS.md` (or DECISIONS.md/SOLUTIONS.md) and the native shared-server CAS endpoint. At Stop, the parent hook records a scope-transfer receipt and synchronizes the new project's memory and sources. It does not forge a `documented` or `no-new` closure for the child.

Check the separate source and memory profiles, vector counts, semantic status, graph relations and a conceptual retrieval query from another Codex session. Compare with `--text-only` and verify any claimed semantic-only hit has a persisted vector and original revision. A README or mechanical index alone does not establish curated operational documentation.

## Limits and recovery

- Discovery is limited to new direct children of the one explicitly authorized parent. Existing siblings, arbitrary paths and symlinks are not enrolled by presence alone.
- The local model must already be installed. A text fallback does not establish semantic success.
- Source extensions and excluded components are defined by the generated per-project policy. Review it for the data you intend to index; generic exclusions cannot classify every private file.
- The parent Stop record proves transfer and synchronization, not a native child turn receipt. A child session can produce its own native closure.
- Automatically enrolled projects are available through their generated VRG profiles and CLI. The read-only browser viewer still uses its separate explicit project registry; this preview does not register new projects there.
- This preview is opt-in and Linux x86_64 only. It does not replace the existing stable MCP executable, viewer, indices or notes. Stop the preview service and restore the prior Codex configuration to roll back. Retain the private registry and stores for review; do not overwrite a live SQLite/WAL store or delete original project files.

An isolated maintainer experiment verified onboarding, CAS, incremental indexing, local 384-dimensional E5 vectors, semantic-only retrieval, graph links and context recovery. It is evidence for that installation, not a portability certification of this release. The release candidate must pass its own isolated installation and downloaded-artifact checks before publication.
