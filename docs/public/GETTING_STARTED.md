# Getting started

The complete source is published in [VRG F4 v0.4.0-f4](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4), but **no compiled binaries are attached** and no official crates.io listing has been verified.

- [Full English setup and tests](SETUP.md)
- [Guía completa en español](SETUP.es.md)
- [Interactive web guide](../setup.html)
- [Detailed maintainer/operator manual](../PUBLIC_INSTALL.md)
- [Privacy and data boundaries](SECURITY.md)

Quick release source build (Rust 1.89+, Cargo, Git; build-time dependencies may require network):

```sh
git clone --branch v0.4.0-f4 --depth 1 https://github.com/galgo2077/VRG.git
cd VRG
cargo build --release --locked
./target/release/vrg --help
```

Run isolated synthetic-notes checks **before** allowing any real Markdown roots. Rust package version `0.1.0` and milestone tag `v0.4.0-f4` are separate identifiers. Void Linux is the validated local environment; other platforms are not certified.
