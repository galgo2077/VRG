fn install_runtime(
    destination: &std::path::Path,
    url: &str,
    size: u64,
    hash: &str,
    consent: bool,
    offline: bool,
) -> crate::error::VrgResult<bool> {
    super::install_runtime(destination, url, size, hash, consent, offline, &|_| Ok(()))
}
use sha2::{Digest, Sha256};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn fixture() -> std::path::PathBuf {
    let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).unwrap();
    }
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
            &script,
            &|_| Ok(())
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
            &script,
            &|_| Ok(())
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
            &script,
            &|_| Ok(())
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

#[cfg(unix)]
#[test]
fn rejected_abi_never_promotes_a_verified_download() {
    use std::os::unix::fs::PermissionsExt;
    let p = fixture();
    let script = p.join("fake-curl");
    let dest = p.join("runtime");
    fs::write(&script, b"#!/bin/sh\nprintf example\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let digest = hex::encode(Sha256::digest(b"example"));
    let result = super::install_runtime_with_curl(
        &dest,
        "https://example.org/runtime",
        7,
        &digest,
        true,
        false,
        &script,
        &|_| {
            Err(crate::error::VrgError::SemanticUnavailable(
                "ABI rejected".into(),
            ))
        },
    );
    assert!(result.is_err());
    assert!(!dest.exists());
    fs::remove_dir_all(p).unwrap();
}

#[cfg(unix)]
#[test]
fn recovery_keeps_both_own_link_and_changed_destination() {
    let p = fixture();
    let stage = p.join("stage");
    let dest = p.join("runtime");
    fs::write(&stage, b"ours").unwrap();
    let original = fs::metadata(&stage).unwrap();
    fs::hard_link(&stage, &dest).unwrap();
    let result = super::rollback_promoted(&dest, &original, "synthetic fsync failure");
    assert!(matches!(result, crate::error::VrgError::Recovery(_)));
    assert!(result.to_string().contains("verify size, SHA-256 and ABI"));
    assert_eq!(fs::read(&dest).unwrap(), b"ours");
    fs::remove_file(&dest).unwrap();
    fs::write(&dest, b"external").unwrap();
    let result = super::rollback_promoted(&dest, &original, "synthetic identity failure");
    assert!(result.to_string().contains("changed externally"));
    assert_eq!(fs::read(&dest).unwrap(), b"external");
    fs::remove_dir_all(p).unwrap();
}
