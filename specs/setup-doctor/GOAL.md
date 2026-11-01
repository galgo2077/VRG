# Setup and doctor: release gate

This work is subsequent to Phase 4. Its certification does not alter the historical F4 evidence or tag.

- Contract and baseline (10%): identify the public checkout and review the installed Codex and official integration contracts.
- Setup and safe configuration (25%): explicit authorization, non-writing plans, stable installation resources, idempotent application, private backups and recoverable changes.
- Codex, memory and model (20%): shared profile, read-only MCP, user-controlled hook trust, selective memory, text and offline modes and a verified consented model path.
- Doctor and repair (15%): passive default, truthful structured results and bounded repairs based on the same plan.
- Final validation (20%): S01–S11 from the task, including a clean installation and independent session recovery.
- Documentation and publication (10%): consistent reviewed public docs, new version, verified registry, release artifacts and website.

Each category is credited only when its observable requirements are met. Publication requires the complete release gate, not merely compiling a candidate.

## Candidate evidence and remaining gates

The candidate has directed coverage for dry plans, text mode, preservation of unrelated integration, project/configuration locks, preimage conflicts, selective repair, an isolated direct MCP handshake, and recovery after an interrupted synthetic application. A pinned upstream model was downloaded, its artifact checksums verified, and ONNX/tokenizer loaded once on Void Linux x86_64; subsequent offline reuse also succeeded. These results do not certify other platforms or a real Codex session.

Still required: complete S01–S11 against the final candidate, including real reviewed hooks and independent-session autodocumentation, full repair/permission edge cases, package and registry installation of the final revision, and verified release/site. Locks coordinate VRG operations but cannot stop an external editor that ignores them; configurations with distinct identities and other shared destinations require further evaluation. No publication or status PASS is implied by this candidate record.

The release gate additionally requires independently executed Linux x86_64, macOS arm64 and macOS Intel artifacts from the same versioned commit, with public checksums, registry installation and verified website links. The candidate matrix is a validation mechanism; it does not grant hook trust, certify Codex integration, publish a release, or establish macOS support until each native job passes. Historical F4 artifacts remain immutable.

Native candidate validation found that `ort` has no prebuilt macOS Intel runtime. A target-specific dynamic configuration restored the Intel build and passed text-mode setup, direct MCP and failure handling in the Intel probe. This is not a semantic-runtime pass: an upstream-source ONNX Runtime build, license review and real inference check are still required. Linux and macOS arm64 passed the earlier text-mode distribution matrix on a different candidate revision; rerun the final matrix for the exact release commit.
