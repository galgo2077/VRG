# Installation and operation

**Published F4 source channel:** [GitHub v0.4.0-f4](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4) contains source and Cargo.lock, but no compiled binaries. We could not verify a crates.io release for this project's identity. The Cargo package version is 0.1.0; the tag denotes the F4 milestone. For a safe copy-and-paste demo and all checks, use [the complete setup guide](public/SETUP.md) or [the Spanish guide](public/SETUP.es.md).

Quick source build (dependencies may need a first-time download):

```sh
git clone --branch v0.4.0-f4 --depth 1 https://github.com/galgo2077/VRG.git
cd VRG
cargo build --release --locked
./target/release/vrg --help
```


These instructions describe a local deployment. Void Linux is the validated environment; other operating systems are not certified. VRG's recorded Phase 4 and integration results apply to the original tested configuration and do not certify a new installation. The portable observer is a separate distribution adaptation and is not itself covered by the recorded G04 validation.

## Operating-system support

- **Linux:** the steps below describe a source build. Void Linux was used for the recorded local validation; other distributions have not been certified.
- **macOS:** source builds and the Unix observer are experimental and unverified. If the required Rust toolchain and native dependencies are available, the Unix commands below are a starting point, not an installation guarantee. Validate locally before relying on them.
- **Windows:** native installation and hook operation are not supported or certified by the current distribution. The observer uses Unix `fcntl`; do not attempt to enable it as a native Windows hook. No Windows installer or validated WSL workflow is supplied.

## Requirements and build

- Rust 1.92 or newer and Cargo, with the locked dependencies already available for an offline build.
- A local directory for rebuildable derived state, outside the repository and source roots.
- At least one explicitly authorized Markdown root if you intend to index notes.
- Optional local model files for semantic retrieval. VRG supports `intfloat/multilingual-e5-small` as a user-supplied model; it does not fetch it automatically. The model directory must contain `model.onnx`, `tokenizer.json`, `config.json`, `special_tokens_map.json`, and `tokenizer_config.json` as regular files.

From the checkout:

```sh
cargo build --offline --locked
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
