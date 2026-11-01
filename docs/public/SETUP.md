# VRG — Install, configure and verify

> **Version 0.5.0 guide — Linux x86_64 and Codex.** Distribution checks and public artifact verification are recorded in the [release record](https://github.com/galgo2077/VRG/releases/tag/v0.5.0). The version includes `setup`, `doctor` and opt-in Adaptive Memory.

The [historical v0.4.0-f4 tag](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4) remains intact; its certifications are not repeated or transferred to this version. macOS and Windows are outside this scope.

[Website setup](../setup.html) · [Español](SETUP.es.md) · [Adaptive Memory](ADAPTIVE_MEMORY.md) · [Operator guide](../PUBLIC_INSTALL.md)

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

## 2. Download or build 0.5.0

**Linux x86_64 binary 0.5.0:** verify `SHA256SUMS.txt` before extracting. Archive SHA-256: `55f011dffd88147c3988ad9e2cb11a58faa7da73d0108649262f6ecf3dcc8534`. Measured symbol requirements: GLIBC 2.39, GLIBCXX 3.4.31 and CXXABI 1.3.11 (validated host glibc 2.41).

```sh
VRG_DOWNLOAD_DIR="$(mktemp -d /tmp/vrg-download.XXXXXXXX)" || exit 1
( cd "$VRG_DOWNLOAD_DIR" &&
  curl -fLO https://github.com/galgo2077/VRG/releases/download/v0.5.0/vrg-v0.5.0-linux-x86_64.tar.gz &&
  curl -fLO https://github.com/galgo2077/VRG/releases/download/v0.5.0/SHA256SUMS.txt &&
  sha256sum -c SHA256SUMS.txt &&
  tar -xzf vrg-v0.5.0-linux-x86_64.tar.gz &&
  ./vrg-v0.5.0-linux-x86_64/vrg --help )
```



**Source build from the fixed 0.5.0 tag, after preparing dependencies:**

```sh
git clone --branch v0.5.0 --depth 1 https://github.com/galgo2077/VRG.git
cd VRG
ORT_LIB_LOCATION=/absolute/path/verified-onnxruntime ORT_OFFLINE=1 \
  CARGO_NET_OFFLINE=true cargo build --release --locked --offline
test -x target/release/vrg
./target/release/vrg --help
```

Building requires Rust 1.92+; the candidate uses Rust 1.98. Prepare the exact `Cargo.lock` dependencies and ONNX Runtime 1.31.0 native library separately. `--offline` cannot install missing dependencies. The [Linux workflow](https://github.com/galgo2077/VRG/blob/v0.5.0/.github/workflows/adaptive-linux-release.yml) records reproducible native preparation, with checksummed distribution and matching MIT/third-party notices.

With the verified native library in its own directory, build without network:

```sh
ORT_LIB_LOCATION=/absolute/path/verified-onnxruntime ORT_OFFLINE=1 \
  CARGO_NET_OFFLINE=true cargo build --release --locked --offline
```

**Crates.io alternative, once 0.5.0 is published:**

```sh
cargo install vrg --version 0.5.0 --locked
vrg --version
```

`cargo install` compiles locally and can download crates and the native runtime. It does not download the official executable or guarantee an offline installation. The prebuilt Linux archive avoids that build preparation. E5 model files are separate and required only for semantic retrieval.

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

**Current assistant compatibility:** Codex only. Other AI assistants have not been officially supported or validated, even if they use MCP. VRG's local CLI can index and search independently of Codex.

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

The source includes a portable Unix example at `examples/codex/hooks.json.example` and `scripts/vrg_hook_observer.py`. These are manual examples. Setup can create packaged project hook resources, but their actual definitions must still be reviewed and trusted in Codex. Review the scripts and source-specific environment variables `VRG_HOOK_EXECUTABLE` and `VRG_HOOK_EVIDENCE_DIR`; keep any evidence directory private and outside indexed roots. On Codex, open `/hooks`, review the exact `SessionStart`, `UserPromptSubmit` and `Stop` definitions, trust only if approved, then verify real event execution in a fresh session. The supplied portable observer has not inherited the certification of the original local observer.

VRG's MCP is read-only. Codex may update **explicitly approved** project documentation through ordinary file tools, while VRG indexes the change. Do not store transcripts or automatically expand source permissions.

## 6. Verification checklist

| Check | Expected evidence |
|---|---|
| Source | Repo/tag resolves to `v0.5.0` |
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

Historical F4 checks remain tied to their original revision and environment. They are not a routine installation step or a new Adaptive Memory certification.

## 7. Troubleshooting

| Symptom | First check |
|---|---|
| `cargo: command not found` | Install supported Rust toolchain with rustup |
| Native dependency build fails | Inspect the crate error; install required compiler/system library |
| No `target/release/vrg` | Verify build errors and package `[[bin]]` target |
| `--offline` missing crates | Prepare the exact locked Cargo cache and verified native library explicitly |
| `vrg status` fails | Inspect canonical config path, source root and derived-data permissions |
| `sync_pending` or `stale` | Wait for legitimate reconciliation; don't present old hits as current |
| MCP missing | Confirm `/mcp`, effective config and that Codex starts the right executable |
| Hook `modified`/`untrusted` | Review current definition in `/hooks`; don't modify trust files |
| Semantic model missing | Use text-only mode or install the verified model artifacts |

## 8. Distribution and operation 0.5.0

- [v0.5.0 release and source](https://github.com/galgo2077/VRG/releases/tag/v0.5.0).
- [Linux x86_64 archive](https://github.com/galgo2077/VRG/releases/download/v0.5.0/vrg-v0.5.0-linux-x86_64.tar.gz) and [SHA256SUMS](https://github.com/galgo2077/VRG/releases/download/v0.5.0/SHA256SUMS.txt).
- [vrg 0.5.0 crate](https://crates.io/crates/vrg/0.5.0). See the release record for independent download verification.

### Setup and doctor

Run `vrg setup` in a terminal for guided choices. For an authorized text-only project profile, review the preview first:

```sh
vrg --config /absolute/path/profile.toml setup \
  --project /absolute/path/project --root /absolute/path/approved-notes \
  --data-dir /absolute/path/derived --text-only --codex --allow-remote-context \
  --memory-dir /absolute/path/project/vrg-memory --non-interactive --yes --offline --dry-run
vrg --config /absolute/path/profile.toml doctor --json
```

Remove `--dry-run` to apply only the authorized plan. `--yes` does not grant hook trust. `--allow-remote-context` authorizes sending retrieved fragments to the configured Codex model; use `--cli-only` if that transfer is not authorized. `--root` is repeatable. `--model-dir` selects an existing model; `--download-model` explicitly requests download and cannot newly download in `--offline` mode.

When hooks are created, review their actual commands in `/hooks` and check real events in a new session. Doctor is passive by default; `--check-runtime` uses an isolated MCP client and does not establish a real Codex session. `doctor --fix --dry-run` previews recognized VRG-owned repairs; `--fix` applies them without broadening permissions. `setup --recover /absolute/path/transaction --dry-run` previews recovery; later conflicts require review and never justify deleting notes or databases.

For a starter profile without real notes, use `setup --project /absolute/path/project --synthetic --text-only --cli-only --non-interactive --yes --offline`; only generated `sample.md` is admitted. Adaptive Memory requires explicit `--adaptive-memory`; see [HOT/WARM/COLD, proposals, budgets and migration](ADAPTIVE_MEMORY.md). MCP stays read-only. Preserve the stable server until the selected profile and rollback are validated.
