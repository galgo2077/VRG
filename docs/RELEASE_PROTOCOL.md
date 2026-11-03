# VRG GitHub release protocol

This protocol applies to a new Linux x86_64 GitHub release. It does not authorize publishing to another registry, rewriting existing tags/history, modifying source notes, changing global Codex configuration or migrating a live store. Historical Phase 4 and Adaptive Memory evidence keeps its original scope.

## 1. Freeze the public source revision

Work in an isolated release checkout and preserve the developer's local changes. Reuse the public repository's current main branch. Bring over only reviewed product source, tests and public documentation; do not copy private configuration or operational evidence.

Choose the version, update the package and lockfile version consistently, and record the exact commit. Record the parent/main revision and existing tag targets before publication. Do not move an old release tag or use a force push to resolve a concurrent change.

The release must be portable: project names, registries and source roots are installation data. Machine-specific paths, private project identities, notes, transcripts, screenshots of private data, databases, vectors, models, caches, backups, credentials and hook trust records must not enter the public tree or artifacts.

## 2. Build the exact source offline

Use the pinned toolchain, locked dependency graph and already prepared native-runtime cache. If required cache material is missing, handle its explicitly checksummed preparation separately; do not silently let a release build resolve new dependencies or download a native runtime.

```sh
CARGO_NET_OFFLINE=true ORT_OFFLINE=1 \
  cargo build --release --locked --offline
```

Supply the existing native library location when the native crate requires it. Set `RUSTFLAGS` path remapping for the checkout, account/cache paths and other build-specific prefixes. Record the exact build command, source commit, toolchain, target, environment requirements and binary hash. Check that the lockfile did not change and that the resulting binary reports the intended version.

## 3. Validate the changed release surface

Run only checks required by the changed components and distribution. Preserve valid prior evidence; do not re-run unrelated phase, platform or migration suites. New portability, CLI or packaging changes require their own affected checks against the release candidate.

Verify the read-only viewer/API, authorized profile resolution, identity/provenance, source-policy rejection paths, native CLI compatibility and packaged executable outside the build checkout. Use real authorized local data for the final viewer integration evidence; keep its source contents, session URL and screenshots private. Isolated fixtures may verify security and distribution contracts but must not be presented as a real-data functional certification.

Record commands, results, candidate hashes and limitations. A failing case is corrected and re-run with only its affected dependencies. Do not describe a build, local package or earlier candidate as independently published and verified.

## 4. Assemble and inspect the public artifacts

Package the exact validated executable, licenses, required native/dependency notices, public installation instructions and build provenance. Include a per-file manifest and SHA-256 checksums. Runtime assets are embedded; packaging them separately does not excuse a mismatch with the executable.

Build a deterministic archive where practical: fixed ordering and normalized metadata, no links or path traversal, no personal account ownership. List and inspect archive entries before upload. Verify the package after extraction outside the checkout.

Apply a publication gate to the staged source tree, binary strings, documentation, manifest and archive contents. Check already tracked files as well as additions. Exclude local configuration, source notes, secrets, databases/WAL, vectors, model files, raw logs, private screenshots, backups, cache contents and personal build paths. Confirm required license notices are present. Preserve full private receipts locally; publish only sanitized release facts.

## 5. Create the GitHub release

Use the configured native Git/GitHub tooling. Push the reviewed commit without forcing a changed remote head. Create a new tag for that exact commit and a draft GitHub release with notes describing actual changes, validation scope, runtime requirements, known limits and rollback.

Upload the source/distribution artifacts and checksum file to the draft. Check the server-side tag target, asset names, lengths and available digests. Publish only after the concrete draft assets are complete and the local gate has passed. A draft or successful upload alone is not completion.

Version 0.6.0 is a GitHub distribution. Do not publish a crate or tell users that `cargo install --version 0.6.0` is available unless that separate publication was authorized and independently verified.

## 6. Verify independent public availability

Download the published archive and checksums through unauthenticated HTTPS into a separate directory. Compare the downloaded bytes with the approved local hashes, extract safely and run the narrow distribution smoke with that downloaded binary. Confirm the public tag resolves to the reviewed source commit and the release is not still a draft.

Only then record the public URL, exact tag/commit, artifact hashes and completed checks. If any public-verification step fails, retain the truthful intermediate state and repair only the failed step.

## 7. Retain recovery

Keep the previous release, executable and private receipts. Install outside authorized source roots and preserve the configured stable MCP/hooks unless replacement is separately requested. Stop the viewer before replacing or removing its executable.

Rollback restores the previous executable or PATH selection and retains all profiles, notes and derived stores. Do not delete an index, copy an SQLite main file over a live WAL store or rewrite an existing GitHub tag as a rollback mechanism.
