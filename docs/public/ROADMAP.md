# VRG roadmap and verification gates

**Published:** [Phase 4 source release v0.4.0-f4](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4), Rust implementation, Cargo.lock, tests, documented MCP and portable Unix observer. Local validation has been reported on Void Linux for its recorded cases, not independently reproduced on all installations.

## Current distribution
- **Build from source:** [English guide](SETUP.md) / [Guía en español](SETUP.es.md).
- A Linux x86_64 archive is attached to v0.4.0-f4; it was tested on Void Linux only.
- **No official crates.io publication** for this project has been independently verified.
- **Version labels:** Cargo `0.1.0`, milestone tag `v0.4.0-f4`. A later semantic-versioned distribution decision is needed.

## Next validation gates
1. Reproduce the tagged source build and a synthetic smoke test on a clean Linux environment.
2. Validate the portable Unix hook observer separately from the original Void Linux local G04 evidence.
3. Produce reproducible, checksummed Linux release artifacts only after required environment checks pass.
4. Enable and verify GitHub Pages for the static site.
5. Assess macOS and Windows support separately; do not imply certification without tests.
6. Decide whether/how to publish to crates.io after verifying the unique package name, ownership and release version. Avoid impersonating a different crate.

Private vaults, derived SQLite, embeddings, model files, credentials and hook trust state never belong in the public source release.
