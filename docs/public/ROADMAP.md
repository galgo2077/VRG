# VRG roadmap and verification gates

**Published:** [Phase 4 source release v0.4.0-f4](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4), Rust implementation, Cargo.lock, tests, documented MCP and portable Unix observer. Local validation has been reported on Void Linux for its recorded cases, not independently reproduced on all installations.

## Current distribution
- **Build from source:** [English guide](SETUP.md) / [Guía en español](SETUP.es.md).
- A Linux x86_64 archive is attached to v0.4.0-f4; it was tested on Void Linux only.
- The official [vrg 0.1.1 crate](https://crates.io/crates/vrg/0.1.1) is published; version 0.1.1 was installed and smoke-tested on Void Linux x86_64.
- **Version labels:** current crate `0.1.1`, F4 source Cargo `0.1.0`, milestone tag `v0.4.0-f4`. A later semantic-versioned distribution decision is needed.

## Next validation gates
1. Reproduce the tagged source build and synthetic smoke test outside the validated Void Linux environment.
2. Validate the portable Unix hook observer separately from the original Void Linux local G04 evidence.
3. Verify Linux release artifacts on additional environments before broadening compatibility claims.
4. Maintain the GitHub Pages deployment and verify future changes.
5. Assess macOS and Windows support separately; do not imply certification without tests.
6. Reproduce crates.io installation on additional environments before broadening support claims.

Private vaults, derived SQLite, embeddings, model files, credentials and hook trust state never belong in the public source release.
