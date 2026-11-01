use super::install_runtime;
use sha2::{Digest, Sha256};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn fixture() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "vrg-runtime-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    path
}
#[test]
fn offline_reuses_verified_but_never_creates_missing_or_replaces_different() {
    let p = fixture();
    let dest = p.join("libonnxruntime.dylib");
    let digest = hex::encode(Sha256::digest(b"example"));
    assert!(
        install_runtime(
            &dest,
            "https://example.org/pinned.dylib",
            7,
            &digest,
            true,
            true
        )
        .is_err()
    );
    assert!(!dest.exists());
    fs::write(&dest, b"example").unwrap();
    assert!(
        !install_runtime(
            &dest,
            "https://example.org/pinned.dylib",
            7,
            &digest,
            false,
            true
        )
        .unwrap()
    );
    fs::write(&dest, b"changed").unwrap();
    assert!(
        install_runtime(
            &dest,
            "https://example.org/pinned.dylib",
            7,
            &digest,
            true,
            false
        )
        .is_err()
    );
    assert_eq!(fs::read(&dest).unwrap(), b"changed");
    fs::remove_dir_all(p).unwrap();
}
#[test]
fn requires_consent_and_refuses_insecure_sources_and_traversal() {
    let p = fixture();
    let dest = p.join("runtime.dylib");
    let digest = hex::encode(Sha256::digest(b"example"));
    assert!(
        install_runtime(
            &dest,
            "https://example.org/pinned",
            7,
            &digest,
            false,
            false
        )
        .is_err()
    );
    assert!(install_runtime(&dest, "http://example.org/pinned", 7, &digest, true, false).is_err());
    assert!(
        install_runtime(
            &dest,
            "https://user:secret@example.org/pinned",
            7,
            &digest,
            true,
            false
        )
        .is_err()
    );
    assert!(
        install_runtime(
            &p.join("../escape"),
            "https://example.org/pinned",
            7,
            &digest,
            true,
            false
        )
        .is_err()
    );
    assert!(!dest.exists());
    fs::remove_dir_all(p).unwrap();
}
#[cfg(unix)]
#[test]
fn rejects_existing_symlink() {
    let p = fixture();
    let victim = p.join("victim");
    let dest = p.join("runtime");
    fs::write(&victim, b"example").unwrap();
    std::os::unix::fs::symlink(&victim, &dest).unwrap();
    let digest = hex::encode(Sha256::digest(b"example"));
    assert!(install_runtime(&dest, "https://example.org/pinned", 7, &digest, true, true).is_err());
    assert_eq!(fs::read(victim).unwrap(), b"example");
    fs::remove_dir_all(p).unwrap();
}

#[cfg(unix)]
#[test]
fn stages_verified_download_and_discards_corrupt_or_interrupted_data() {
    use std::os::unix::fs::PermissionsExt;
    let p = fixture();
    let script = p.join("fake-curl");
    let digest = hex::encode(Sha256::digest(b"example"));
    let dest = p.join("runtime");
    fs::write(&script, b"#!/bin/sh\nprintf example\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        super::install_runtime_with_curl(
            &dest,
            "https://example.org/runtime",
            7,
            &digest,
            true,
            false,
            &script
        )
        .unwrap()
    );
    assert_eq!(fs::read(&dest).unwrap(), b"example");
    fs::remove_file(&dest).unwrap();
    fs::write(&script, b"#!/bin/sh\nprintf corrupt\n").unwrap();
    assert!(
        super::install_runtime_with_curl(
            &dest,
            "https://example.org/runtime",
            7,
            &digest,
            true,
            false,
            &script
        )
        .is_err()
    );
    assert!(!dest.exists());
    fs::write(&script, b"#!/bin/sh\nprintf partial\nexit 23\n").unwrap();
    assert!(
        super::install_runtime_with_curl(
            &dest,
            "https://example.org/runtime",
            7,
            &digest,
            true,
            false,
            &script
        )
        .is_err()
    );
    assert!(!dest.exists());
    assert!(fs::read_dir(&p).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".vrg-runtime-")
    }));
    fs::remove_dir_all(p).unwrap();
}
