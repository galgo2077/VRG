# Getting started

VRG 0.5.0 targets Linux x86_64 and Codex, with `setup`, passive `doctor` and opt-in Adaptive Memory. See the [release verification record](https://github.com/galgo2077/VRG/releases/tag/v0.5.0) for artifact checks and measured limits.

- [English installation and operation](SETUP.md)
- [Guía en español](SETUP.es.md)
- [Web guide](../setup.html)
- [Adaptive Memory: HOT/WARM/COLD](ADAPTIVE_MEMORY.md)
- [Detailed operator guide](../PUBLIC_INSTALL.md)
- [Privacy and data boundaries](SECURITY.md)

Download [the Linux archive](https://github.com/galgo2077/VRG/releases/download/v0.5.0/vrg-v0.5.0-linux-x86_64.tar.gz) and verify [SHA256SUMS.txt](https://github.com/galgo2077/VRG/releases/download/v0.5.0/SHA256SUMS.txt) before extraction. The source and crate use the same version, `0.5.0`.

Crates.io installation compiles from source and may download locked dependencies and the native ONNX runtime:

```sh
cargo install vrg --version 0.5.0 --locked
vrg --version
```

For a source build, use the `v0.5.0` tag and prepare exact Cargo/native dependencies explicitly as described in [SETUP](SETUP.md). Rust builds use `--locked --offline`; offline mode cannot supply missing caches.

Start with an isolated synthetic profile before authorizing real notes. Setup requires explicit paths and consent; hook trust remains a separate review in Codex. macOS and Windows are outside this Linux release scope. The historical [v0.4.0-f4](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4) tag and its original certification remain intact.
