# VRG — Install, configure and verify

> **Release channel (1 November 2026).** [`v0.4.0-f4`](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4) is a **source release**, with Rust application code, tests and `Cargo.lock`, but **no prebuilt binary assets**. The crate has not been verified as published under this project's ownership on crates.io; don't assume `cargo install vrg` downloads this project. The tag is a **Phase 4 release label**; the Cargo package still reports `0.1.0`. macOS and Windows aren't certified, and the portable Unix hook observer is not the same implementation as the original local G04 verification.

[Website setup](../setup.html) · [Español](SETUP.es.md) · [Existing operator guide](../PUBLIC_INSTALL.md) · [Source code](https://github.com/galgo2077/VRG)

## 1. Prerequisites

**Linux** is the documented source-build path. The maintainer's validated environment was Void Linux; other Linux environments require their own verification.

Install:
- Git.
- Rust **1.92 or newer**, with Cargo. Use [rustup](https://rustup.rs) or the official [Rust installation guide](https://www.rust-lang.org/tools/install).
- A C/C++ build toolchain, `make` and `pkg-config` if demanded by the locked dependency build.

Check:

```sh
git --version
rustc --version
cargo --version
cc --version
pkg-config --version
```

If a command is missing, install it with your distribution's normal package manager. On Void Linux check precise package names with `xbps-query -Rs`. Do not assume an untested package install command works across all distributions.

## 2. Download and compile (available now)

**Recommended source build from the fixed F4 tag:**

```sh
git clone --branch v0.4.0-f4 --depth 1 https://github.com/galgo2077/VRG.git
cd VRG
cargo build --release --locked
test -x target/release/vrg
./target/release/vrg --help
```

The first build fetches dependencies from Cargo's configured registry if needed. `--locked` holds the lockfile version selection. An offline build is only possible after those packages are cached:

```sh
cargo build --release --locked --offline
```

**Alternative, unverified on a clean external machine:** Cargo supports installing directly from a tagged Git source:

```sh
cargo install --git https://github.com/galgo2077/VRG.git --tag v0.4.0-f4 --locked
vrg --help
```

That command builds from source; it does **not** download an official binary and is not a crates.io installation. If it fails in your environment, use the clone-and-build procedure and inspect the build error. The package version shown by `--version` can be `0.1.0` even though the tag is `v0.4.0-f4`.

## 3. Run a safe smoke test with synthetic notes

Do **not** grant VRG access to real notes just to test an installation. After compiling from step 2, run this from the repository checkout. This script creates a unique temporary directory and never overwrites an existing demo:

```sh
DEMO_BASE="$(mktemp -d /tmp/vrg-demo.XXXXXXXX)" || { echo "Cannot create demo directory" >&2; exit 1; }
mkdir "$DEMO_BASE/notes"
printf '# Demo\n\nThe blue zafiro compass is a synthetic test.\n' > "$DEMO_BASE/notes/demo.md"
DEMO_CFG="$DEMO_BASE/config.toml"
cat > "$DEMO_CFG" <<EOF
data_dir = "$DEMO_BASE/data"
capture_session_events = false

[[roots]]
id = "demo"
path = "$DEMO_BASE/notes"
allowed_files = ["demo.md"]
EOF
./target/release/vrg --config "$DEMO_CFG" status
./target/release/vrg --config "$DEMO_CFG" index
./target/release/vrg --config "$DEMO_CFG" search "zafiro compass" --root demo --text-only
```

Check that the search returns an attributed hit from `demo.md`, and that the source Markdown remains unchanged. All other Markdown files in the demo root are omitted by `allowed_files`. No local semantic model is required for text-only search.

To check incremental synchronization, **only edit your synthetic `demo.md`**, then run a new query or an explicit `index` in this isolated profile. Do not treat a manually triggered index as evidence of automatic watching; that requires a separate running watcher/session test.

## 4. Optional local semantic retrieval

VRG's local validation used `intfloat/multilingual-e5-small`. It is not bundled or downloaded automatically. A compatible local model directory should contain:

- `model.onnx`
- `tokenizer.json`
- `config.json`
- `special_tokens_map.json`
- `tokenizer_config.json`

Verify the exact model revision, hashes, license and tokenizer compatibility before enabling it in `semantic_model`. Text-only search works without a model. Installing VRG does not install or configure a local Codex inference model.

## 5. Connect MCP to Codex

Start with your **synthetic config** and a project-local `.codex/config.toml`, not private vaults. Replace the placeholders with actual absolute paths:

```toml
[mcp_servers.vrg]
command = "/absolute/path/to/VRG/target/release/vrg"
args = ["--config", "/absolute/path/to/demo.toml", "mcp", "serve"]
cwd = "/absolute/path/to/VRG"
enabled = true
required = true
enabled_tools = [
  "vrg_status",
  "vrg_search",
  "vrg_read_source",
  "vrg_graph_neighbors"
]
```

The executable's actual command is `vrg --config CONFIG mcp serve`. Start a **new** Codex session in the trusted project and open `/mcp`; verify the four tools, then call `vrg_status` and perform only synthetic search/read/graph checks. Verify the server's configured roots and data directory before requesting content. A remote Codex model can see returned snippets, even if VRG itself is local.

### Hooks and selective documentation

The source includes a portable Unix example at `examples/codex/hooks.json.example` and `scripts/vrg_hook_observer.py`. They are **not automatically installed** or trusted. Review the scripts and source-specific environment variables `VRG_HOOK_EXECUTABLE` and `VRG_HOOK_EVIDENCE_DIR`; keep any evidence directory private and outside indexed roots. On Codex, open `/hooks`, review the exact `SessionStart`, `UserPromptSubmit` and `Stop` definitions, trust only if approved, then verify real event execution in a fresh session. The supplied portable observer has not inherited the certification of the original local observer.

VRG's MCP is read-only. Codex may update **explicitly approved** project documentation through ordinary file tools, while VRG indexes the change. Do not store transcripts or automatically expand source permissions.

## 6. Verification checklist

| Check | Expected evidence |
|---|---|
| Source | Repo/tag resolves to `v0.4.0-f4` |
| Build | `target/release/vrg` exists; `--help` responds |
| Config | `status` reports the intended demo root |
| Index | Only `demo.md` is indexed |
| Search | Text-only hit attributed to the correct source/revision |
| Graph | Synthetic relationships only; unresolved links are not fabricated |
| Model | Explicit `ready` if verified model installed; otherwise documented degraded/text-only |
| MCP | Four read-only tools visible and callable from real Codex |
| Hooks | Reviewed/trusted and actual events observed, with exit codes |
| Privacy | Excluded/private notes cannot be retrieved or read |
| Freshness | New revision not served as an old revision marked `ready` |
| Persistence | An independent session retrieves synthetic documented knowledge |
| Recovery | Only derived/synthetic data used for destructive testing; original notes untouched |

### Maintainer-directed F4 checks

These tests are present in the F4 source tree. They require the correct fixture and dependency environment; do **not** run them against personal vaults or interpret success on one machine as cross-platform certification.

```sh
cargo test --locked --test codex_auto_sync f4_01_private_metadata_and_external_paths_are_excluded -- --exact
cargo test --locked --features f4-instrumentation --test codex_auto_sync f4_04_concurrent_publication_and_journal_recovery_are_deterministic -- --exact
cargo test --locked --test codex_auto_sync f4_05_mcp_exposes_read_only_tools_and_enforces_root_boundaries -- --exact
```

F4-03 requires a verified local model and environment variable `VRG_F4_MODEL_DIR`; refer to the maintainer's pinned model manifest before running it. Use `--offline` only when all dependencies are cached.

## 7. Troubleshooting

| Symptom | First check |
|---|---|
| `cargo: command not found` | Install supported Rust toolchain with rustup |
| Native dependency build fails | Inspect the crate error; install required compiler/system library |
| No `target/release/vrg` | Verify build errors and package `[[bin]]` target |
| `--offline` missing crates | Fetch locked dependencies once through an authorized online build |
| `vrg status` fails | Inspect canonical config path, source root and derived-data permissions |
| `sync_pending` or `stale` | Wait for legitimate reconciliation; don't present old hits as current |
| MCP missing | Confirm `/mcp`, effective config and that Codex starts the right executable |
| Hook `modified`/`untrusted` | Review current definition in `/hooks`; don't modify trust files |
| Semantic model missing | Use text-only mode or install the verified model artifacts |

## 8. Distribution status

- **Source:** [v0.4.0-f4 release](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4).
- **Prebuilt release binaries:** none on that release.
- **crates.io package:** no verified official publication for this project.
- **Cargo package version:** `0.1.0`. The tag `v0.4.0-f4` identifies the **Phase 4 source milestone**, not the Cargo semver release.
- **Certified OS:** local cases on Void Linux. Do not claim native Windows or macOS support until independently verified.

See [Installation and operation](../PUBLIC_INSTALL.md) for additional owner/operator details and recovery.
