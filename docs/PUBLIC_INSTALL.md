# Installation and operation

**Version 0.5.0 — Linux x86_64 and Codex.** See the [release verification record](https://github.com/galgo2077/VRG/releases/tag/v0.5.0) for artifact checks and measured limits. Use the [English setup guide](public/SETUP.md), [Spanish guide](public/SETUP.es.md) and [Adaptive Memory guide](public/ADAPTIVE_MEMORY.md).

The [historical v0.4.0-f4 release](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4) remains unchanged. Its recorded certification belongs to its original revision and environment and is not a new certification of 0.5.0.

Once published, the Linux asset is `vrg-v0.5.0-linux-x86_64.tar.gz`, with `SHA256SUMS.txt`, under [v0.5.0](https://github.com/galgo2077/VRG/releases/tag/v0.5.0). Verify checksums before extracting. The crate installation compiles locally and can download crates/native dependencies:

```sh
cargo install vrg --version 0.5.0 --locked
vrg --version
```

## Operating-system support

This delivery covers Linux x86_64 and Codex. Measured glibc/libstdc++ requirements belong to the actual release build, not every Linux distribution. macOS, Windows and ONNX Intel work are outside this scope.

## Requirements and build

- Rust 1.92 or newer and Cargo, with exact locked dependencies and verified native ONNX Runtime 1.31.0 prepared separately before an offline build. See the pinned preparation in `.github/workflows/adaptive-linux-release.yml`.
- A local directory for rebuildable derived state, outside the repository and source roots.
- At least one explicitly authorized Markdown root if you intend to index notes.
- Optional local model files for semantic retrieval. VRG supports `intfloat/multilingual-e5-small` as a user-supplied model; it does not fetch it automatically; setup can request an explicit verified download with `--download-model`. The model directory must contain `model.onnx`, `tokenizer.json`, `config.json`, `special_tokens_map.json`, and `tokenizer_config.json` as regular files.

From the checkout:

```sh
ORT_LIB_LOCATION=/absolute/path/verified-onnxruntime ORT_OFFLINE=1 \
  CARGO_NET_OFFLINE=true cargo build --offline --locked
./target/debug/vrg init
./target/debug/vrg status
```

`init` creates a configuration with no source roots and does not overwrite an existing configuration. `--offline` requires the dependency cache to be prepared separately; Cargo cannot download missing packages in this mode.

## Configure an authorized root

Edit the VRG configuration created by `init` and replace every placeholder below with an **absolute path for your own installation**. Do not paste the example unchanged. Keep the data directory outside all roots, the checkout, and model directories. Never authorize an entire home directory merely for convenience.

```toml
data_dir = "/absolute/path/outside/roots/vrg-data"
capture_session_events = false

[[roots]]
id = "notes"
path = "/absolute/path/to/authorized/markdown"

# Optional, after installing the model files locally:
# [semantic_model]
# model_id = "intfloat/multilingual-e5-small"
# model_dir = "/absolute/path/outside/roots/local-model"
# threads = 2
# max_length = 512
```

Inspect the configured roots and exclusions before any scan. The local user is responsible for classifying content and authorizing its use; path exclusions and privacy frontmatter checks cannot reliably infer every sensitive note. Do not index source roots you have not approved.

```sh
./target/debug/vrg status
./target/debug/vrg index
./target/debug/vrg search "specific search terms" --root notes
```

`index` scans configured roots and publishes a derived generation. It does not edit the notes. Use it only after the root and privacy review. `status` and search expose synchronization state; do not present stale data as current. The optional `--allow-stale` switch explicitly changes that behavior.

## MCP and Codex

The stdio server is launched with `vrg mcp serve`. The four read-only retrieval tools are `vrg_status`, `vrg_search`, `vrg_read_source`, and `vrg_graph_neighbors`. Configure a Codex project MCP entry with your actual absolute executable and working directory; this illustrative template contains no credentials:

```toml
[mcp_servers.vrg]
command = "/absolute/path/to/vrg/target/debug/vrg"
args = ["mcp", "serve"]
cwd = "/absolute/path/to/vrg"
enabled = true
required = true
enabled_tools = ["vrg_status", "vrg_search", "vrg_read_source", "vrg_graph_neighbors"]
```

The server starts the configured watcher and performs startup reconciliation; a required server that cannot start may prevent the session from starting. New sessions may be required after rebuilding, because existing processes can retain an older executable. Verify the connected server's `vrg_status`, effective roots, generation, and freshness before relying on results.

MCP is read-only. Search and source reads can return note excerpts to the Codex model. Use remote inference only when the owner has explicitly permitted the relevant transfer; do not assume that local indexing implies local inference. Never include credentials, private notes, complete conversations, or transcripts in operational memory.

## Hooks and selective documentation

Codex project hooks can run VRG's `SessionStart`, `UserPromptSubmit`, and `Stop` integration. Hook definitions and scripts require explicit owner review and trust in Codex `/hooks`; trusting a project is a separate decision. Keep unrelated user-level hooks independent. Recheck trust after changing a definition and verify real event records and return codes in a new session. A trust status or a manually invoked script alone does not prove that Codex executed the hook.

The portable `scripts/vrg_hook_observer.py` is intended for Unix-like systems. Before configuring any hooks, set `VRG_HOOK_EXECUTABLE` to an existing canonical absolute path of this installation's executable regular `vrg` file and `VRG_HOOK_EVIDENCE_DIR` to an existing canonical absolute private evidence directory outside all source roots. The directory must belong to the running user and deny group and other access. Supply these values to the Codex process through your own local environment; they are installation-specific and must not be checked into Git. Review the observer and hook commands for your environment, and verify the paths and permissions before granting trust. This portable adaptation is not the exact observer used in the recorded G04 result.

On a Unix-like shell, the environment has this illustrative shape; replace both placeholders with the absolute paths for your installation before starting Codex:

```sh
export VRG_HOOK_EXECUTABLE='/absolute/path/to/vrg/target/debug/vrg'
export VRG_HOOK_EVIDENCE_DIR='/absolute/path/to/private/vrg-evidence'
```

Do not copy personal `.codex` configuration or trust records between machines. Use the hook example supplied with the distribution only after filling in and reviewing its own paths. Check the installed Codex documentation for the supported hook interface.

Selective documentation is a decision by the assistant: record only new, verified, durable project knowledge in files explicitly authorized by their owner, and avoid per-turn or duplicate notes. The observed hook workflow can request a continuation if a closure declaration is missing; neither instructions nor a successful synthetic test guarantees that every future turn will make the right semantic judgment. MCP remains read-only.

## Troubleshooting and recovery

- **Missing dependencies in offline build:** prepare the exact locked dependencies through an authorized process, then retry offline. Do not silently update the lockfile.
- **Semantic retrieval unavailable:** verify the configured model ID, absolute directory, regular-file requirements, and local model artifacts. Text retrieval may remain available in degraded mode.
- **MCP tools unavailable:** verify the project registration, executable, configuration, process identity, and startup result in a new session. A running old process may not reflect a rebuilt binary.
- **Hooks not running:** inspect `/hooks`, compare each event and command with the reviewed definition, confirm trust, and inspect bounded event metadata. Do not bypass trust or assume that a displayed entry executed.
- **Stale or failed synchronization:** inspect `vrg status`, source permissions and exclusions, and the configured data directory. Do not label a stale generation as current or change roots to work around an error.
- **Recovery:** derived data is rebuildable, but any rollback or restoration must be scoped to the authorized installation and its documented recovery policy. Never overwrite or clean original vaults or backups as a troubleshooting shortcut.

## Setup, doctor and Adaptive Memory in 0.5.0

`vrg setup` plans an explicitly authorized profile. Review `--dry-run` before applying it. It distinguishes text-only operation from a local embedding model and requires explicit consent for model downloads and remote Codex context. `vrg doctor` is passive by default; `--check-runtime` adds an isolated direct MCP probe, while `--fix --dry-run` previews only recognized VRG-owned repairs. Neither configuration nor doctor alone establishes hook trust or live Codex event execution.

Setup and recovery examples are in the [English guide](public/SETUP.md#setup-and-doctor) and [Spanish guide](public/SETUP.es.md#setup-y-doctor). The automatic project flow can create bounded hook resources for review; legacy observer examples above remain manual alternatives and do not replace the packaged supervisor.

Adaptive Memory defaults to disabled. Choose `--adaptive-memory` explicitly for the reviewed project profile. See [the memory guide](public/ADAPTIVE_MEMORY.md) for strict proposals, derived summaries, complete-response budgets, COLD integrity checks, safe retention and `memory migrate --offline-copy DEST`. Restore returns exact authorized archive data as JSON and never overwrites a source note. No archive or old summary bypasses current source permissions.
