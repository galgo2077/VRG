//! AM-T03/04 and synthetic AM-T16 subcase, prepared in AM2, run only in AM4.
//! The project-note restore and retrieval reactivation are separate coordinator cases.
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};
use vrg::adaptive_codec::{self, ArchiveStore, FRAME_HEADER_BYTES, MAX_OBJECT_BYTES};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vrg AM codec {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn am_t03_realistic_compressible_and_incompressible_round_trip() {
    let technical = "The store keeps source revisions and permissions separate. Do not remove the rollback generation. A COLD representation must preserve exact bytes, including UTF-8: á漢字.\n".repeat(80);
    let encoded = adaptive_codec::encode(technical.as_bytes()).unwrap();
    assert_eq!(encoded.codec, "zlib");
    assert!(encoded.data.len() < technical.len());
    assert_eq!(
        adaptive_codec::decode(
            &encoded.codec,
            &encoded.data,
            encoded.raw_len,
            &encoded.sha256
        )
        .unwrap(),
        technical.as_bytes()
    );
    let tiny = b"x";
    assert_eq!(adaptive_codec::encode(tiny).unwrap().codec, "raw");
    let mut state = 0x7742_9462_1835_aab1u64;
    let random = (0..32768)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect::<Vec<_>>();
    assert_eq!(adaptive_codec::encode(&random).unwrap().codec, "raw");
    for input in [Vec::new(), tiny.to_vec(), random, technical.into_bytes()] {
        let frame = adaptive_codec::encode_frame(&input).unwrap();
        assert_eq!(adaptive_codec::decode_frame(&frame).unwrap(), input);
    }
}

#[test]
fn am_t03_truncation_hash_corruption_trailing_and_unknown_codec_rejected() {
    let original = b"Exact source bytes, no invented restoration.\r\n".repeat(100);
    let frame = adaptive_codec::encode_frame(&original).unwrap();
    for end in [
        0,
        FRAME_HEADER_BYTES - 1,
        FRAME_HEADER_BYTES,
        frame.len() - 1,
    ] {
        assert!(adaptive_codec::decode_frame(&frame[..end]).is_err());
    }
    let mut corrupt = frame.clone();
    corrupt[14] ^= 1;
    assert!(adaptive_codec::decode_frame(&corrupt).is_err());
    let mut corrupt = frame.clone();
    corrupt[5] = 17;
    assert!(adaptive_codec::decode_frame(&corrupt).is_err());
    let mut corrupt = frame.clone();
    corrupt[4] = 2;
    assert!(adaptive_codec::decode_frame(&corrupt).is_err());
    let mut corrupt = frame;
    corrupt.push(0);
    assert!(adaptive_codec::decode_frame(&corrupt).is_err());
}

#[test]
fn am_t04_false_lengths_and_expansion_are_bounded_before_allocation() {
    let input = vec![b'A'; 1024 * 1024];
    let mut frame = adaptive_codec::encode_frame(&input).unwrap();
    frame[6..14].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(adaptive_codec::decode_frame(&frame).is_err());
    frame[6..14].copy_from_slice(&8u64.to_le_bytes());
    assert!(adaptive_codec::decode_frame(&frame).is_err());
    let object = adaptive_codec::encode(&input).unwrap();
    assert!(adaptive_codec::decode(&object.codec, &object.data, 0, &object.sha256).is_err());
    assert!(adaptive_codec::encode(&vec![0; MAX_OBJECT_BYTES + 1]).is_err());
    assert!(adaptive_codec::decode("raw", b"x", 1, "not-a-hash").is_err());
}

#[test]
fn am_t16_synthetic_exact_copy_round_trip_and_no_source_writes() {
    let sandbox = Sandbox::new();
    let original = b"---\r\ntitle: Exact archived source\r\n---\r\n# Decision\r\nDo not remove rollback before verification. Version 1.2 uses 17 ms.\r\n";
    let source = sandbox.0.join("authorized note.md");
    fs::write(&source, original).unwrap();
    let revision = sha(original);
    let archive = ArchiveStore::open(&sandbox.0).unwrap();
    let first = archive
        .archive(
            "project-A",
            "memory:authorized note.md",
            &revision,
            original,
        )
        .unwrap();
    let repeated = archive
        .archive(
            "project-A",
            "memory:authorized note.md",
            &revision,
            original,
        )
        .unwrap();
    assert_eq!(first, repeated);
    let restored = archive
        .restore("project-A", "memory:authorized note.md", &revision)
        .unwrap();
    assert_eq!(sha(&restored), revision);
    assert_eq!(restored, original);
    assert_eq!(fs::read(source).unwrap(), original);
    assert_eq!(archive_files(&sandbox.0, "arc").len(), 1);
    assert_eq!(archive_files(&sandbox.0, "blob").len(), 1);
    assert!(
        archive
            .restore("project-B", "memory:authorized note.md", &revision)
            .is_err()
    );
    assert!(
        archive
            .restore("project-A", "memory:different.md", &revision)
            .is_err()
    );
    assert!(
        archive
            .archive(
                "project-A",
                "memory:authorized note.md",
                &"0".repeat(64),
                original
            )
            .is_err()
    );
}

#[test]
fn am_t04_archive_identity_traversal_and_corrupt_manifest_rejected() {
    let sandbox = Sandbox::new();
    let archive = ArchiveStore::open(&sandbox.0).unwrap();
    let original = b"A recoverable technical decision.";
    let revision = sha(original);
    for source in [
        "../escape.md",
        "/absolute.md",
        "r:dir/../escape.md",
        "r:dir/./escape.md",
        "r:dir\\escape.md",
    ] {
        assert!(
            archive
                .archive("project", source, &revision, original)
                .is_err()
        );
    }
    assert!(
        archive
            .archive("../scope", "r:note.md", &revision, original)
            .is_err()
    );
    let record = archive
        .archive("project", "r:note.md", &revision, original)
        .unwrap();
    let object = sandbox
        .0
        .join("cold")
        .join(format!("{}.arc", record.manifest_id));
    assert_eq!(
        record.stored_bytes,
        fs::metadata(&object).unwrap().len()
            + fs::metadata(
                sandbox
                    .0
                    .join("cold")
                    .join(format!("{}.blob", record.object_id))
            )
            .unwrap()
            .len()
    );
    let mut corrupted = fs::read(&object).unwrap();
    corrupted[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&object, corrupted).unwrap();
    assert!(archive.restore("project", "r:note.md", &revision).is_err());
}

#[test]
fn am_t04_archive_authorization_revalidation_precedes_publication() {
    let sandbox = Sandbox::new();
    let archive = ArchiveStore::open(&sandbox.0).unwrap();
    let raw = b"Do not publish after permission revocation.";
    let revision = sha(raw);
    let observed_staging = std::cell::Cell::new(false);
    let result = archive.archive_checked("project", "r:note.md", &revision, raw, || {
        let names = fs::read_dir(sandbox.0.join("cold"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        observed_staging.set(
            names
                .iter()
                .filter(|name| name.to_string_lossy().starts_with(".staging-"))
                .count()
                == 2,
        );
        Err(vrg::VrgError::AccessDenied(
            "source permission revoked during staging".into(),
        ))
    });
    assert!(result.is_err());
    assert!(observed_staging.get());
    assert!(archive_files(&sandbox.0, "arc").is_empty());
    assert!(archive_files(&sandbox.0, "blob").is_empty());
    assert!(archive.restore("project", "r:note.md", &revision).is_err());
}

#[cfg(unix)]
#[test]
fn am_t04_archive_rejects_symlink_store_cold_and_object() {
    use std::os::unix::fs::symlink;
    let sandbox = Sandbox::new();
    let other = Sandbox::new();
    let alias = sandbox.0.join("alias");
    symlink(&other.0, &alias).unwrap();
    assert!(ArchiveStore::open(&alias).is_err());
    symlink(&other.0, sandbox.0.join("cold")).unwrap();
    assert!(ArchiveStore::open(&sandbox.0).is_err());
    fs::remove_file(sandbox.0.join("cold")).unwrap();
    let archive = ArchiveStore::open(&sandbox.0).unwrap();
    let bytes = b"private authorized fixture";
    let revision = sha(bytes);
    let record = archive
        .archive("project", "r:note.md", &revision, bytes)
        .unwrap();
    let file = sandbox
        .0
        .join("cold")
        .join(format!("{}.arc", record.manifest_id));
    fs::remove_file(&file).unwrap();
    let outside = other.0.join("outside");
    fs::write(&outside, b"outside untouched").unwrap();
    symlink(&outside, &file).unwrap();
    assert!(archive.restore("project", "r:note.md", &revision).is_err());
    assert!(
        archive
            .archive("project", "r:note.md", &revision, bytes)
            .is_err()
    );
    assert_eq!(fs::read(outside).unwrap(), b"outside untouched");
    assert_eq!(archive_files(&sandbox.0, "arc").len(), 1);
    assert_eq!(archive_files(&sandbox.0, "blob").len(), 1);
}

fn archive_files(store: &std::path::Path, extension: &str) -> Vec<PathBuf> {
    fs::read_dir(store.join("cold"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|value| value == extension))
        .collect()
}

#[test]
fn am_t02_archive_shares_payload_in_scope_and_requires_source_membership() {
    let sandbox = Sandbox::new();
    let archive = ArchiveStore::open(&sandbox.0).unwrap();
    let raw = b"Shared exact bytes do not grant cross-source permissions.";
    let revision = sha(raw);
    let first = archive
        .archive("scope-A", "r:first.md", &revision, raw)
        .unwrap();
    let second = archive
        .archive("scope-A", "r:second.md", &revision, raw)
        .unwrap();
    assert_eq!(first.object_id, second.object_id);
    assert_ne!(first.manifest_id, second.manifest_id);
    assert_eq!(archive_files(&sandbox.0, "arc").len(), 2);
    assert_eq!(archive_files(&sandbox.0, "blob").len(), 1);
    assert_eq!(
        archive
            .restore("scope-A", "r:second.md", &revision)
            .unwrap(),
        raw
    );
    assert!(
        archive
            .restore("scope-A", "r:ungranted.md", &revision)
            .is_err()
    );
    fs::remove_file(
        sandbox
            .0
            .join("cold")
            .join(format!("{}.arc", first.manifest_id)),
    )
    .unwrap();
    assert!(archive.restore("scope-A", "r:first.md", &revision).is_err());
    assert_eq!(
        archive
            .restore("scope-A", "r:second.md", &revision)
            .unwrap(),
        raw
    );
    let other = archive
        .archive("scope-B", "r:second.md", &revision, raw)
        .unwrap();
    assert_ne!(other.object_id, second.object_id);
    assert_eq!(archive_files(&sandbox.0, "blob").len(), 2);
}

#[test]
fn am_t11_passive_archive_inspection_does_not_initialize_or_mutate() {
    let sandbox = Sandbox::new();
    let absent = sandbox.0.join("absent");
    assert_eq!(
        ArchiveStore::inspect_existing(&absent, 16, 32 * 1024 * 1024)
            .unwrap()
            .state,
        "absent"
    );
    assert!(!absent.exists());
    assert_eq!(
        ArchiveStore::inspect_existing(&sandbox.0, 16, 32 * 1024 * 1024)
            .unwrap()
            .state,
        "not_initialized"
    );
    assert!(!sandbox.0.join("cold").exists());
    let archive = ArchiveStore::open(&sandbox.0).unwrap();
    let raw = b"An exact authorized project fixture.";
    let revision = sha(raw);
    let record = archive
        .archive("project", "r:note.md", &revision, raw)
        .unwrap();
    let snapshot = || {
        fs::read_dir(sandbox.0.join("cold"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (
                    path.file_name().unwrap().to_owned(),
                    sha(&bytes),
                    fs::metadata(&path).unwrap().modified().unwrap(),
                )
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    let before = snapshot();
    assert_eq!(
        ArchiveStore::inspect_existing(&sandbox.0, 16, 32 * 1024 * 1024)
            .unwrap()
            .state,
        "valid"
    );
    assert_eq!(snapshot(), before);
    let blob = sandbox
        .0
        .join("cold")
        .join(format!("{}.blob", record.object_id));
    let mut bytes = fs::read(&blob).unwrap();
    let end = bytes.len() - 1;
    bytes[end] ^= 1;
    fs::write(&blob, bytes).unwrap();
    let before = snapshot();
    let inspection = ArchiveStore::inspect_existing(&sandbox.0, 16, 32 * 1024 * 1024).unwrap();
    assert_eq!(inspection.state, "corrupt");
    assert!(
        inspection
            .diagnostic_codes
            .iter()
            .any(|code| code == "corrupt_object")
    );
    assert_eq!(snapshot(), before);
    let limited = ArchiveStore::inspect_existing(&sandbox.0, 1, 32 * 1024 * 1024).unwrap();
    assert!(
        limited
            .diagnostic_codes
            .iter()
            .any(|code| code == "object_limit")
    );
}

#[test]
fn am_t09_cold_gc_protects_revisions_scopes_readers_and_rejects_stale_plans() {
    use std::collections::BTreeSet;
    let sandbox = Sandbox::new();
    let archive = ArchiveStore::open(&sandbox.0).unwrap();
    let current = b"Current source revision remains protected.";
    let old = b"Unreachable old historical revision.";
    let current_sha = sha(current);
    let old_sha = sha(old);
    archive
        .archive("scope-A", "r:note.md", &current_sha, current)
        .unwrap();
    archive
        .archive("scope-B", "r:note.md", &current_sha, current)
        .unwrap();
    archive
        .archive("scope-A", "r:note.md", &old_sha, old)
        .unwrap();
    let protected = BTreeSet::from([("r:note.md".to_owned(), current_sha.clone())]);
    let plan = archive.plan_gc(&protected).unwrap();
    assert_eq!(plan.protected_objects, 2);
    assert_eq!(plan.eligible_manifest_ids.len(), 1);
    assert_eq!(plan.eligible_object_ids.len(), 1);
    let lease = archive.reader_lease().unwrap();
    assert!(archive.apply_gc(&plan, &protected).is_err());
    drop(lease);
    let report = archive.apply_gc(&plan, &protected).unwrap();
    assert_eq!(report.removed_objects, 2);
    assert!(report.removed_bytes > 0);
    assert_eq!(
        archive
            .restore("scope-A", "r:note.md", &current_sha)
            .unwrap(),
        current
    );
    assert_eq!(
        archive
            .restore("scope-B", "r:note.md", &current_sha)
            .unwrap(),
        current
    );
    assert!(archive.restore("scope-A", "r:note.md", &old_sha).is_err());
    assert!(archive.apply_gc(&plan, &protected).is_err());
    let plan = archive.plan_gc(&protected).unwrap();
    archive
        .archive("scope-A", "r:new.md", &old_sha, old)
        .unwrap();
    assert!(archive.apply_gc(&plan, &protected).is_err());
    let plan = archive.plan_gc(&protected).unwrap();
    let changed = BTreeSet::new();
    assert!(archive.apply_gc(&plan, &changed).is_err());
}

#[test]
fn am_t09_cold_gc_preserves_corruption_and_shared_reachable_payloads() {
    use std::collections::BTreeSet;
    let sandbox = Sandbox::new();
    let archive = ArchiveStore::open(&sandbox.0).unwrap();
    let raw = b"Shared payload must survive one removed manifest.";
    let revision = sha(raw);
    let kept = archive
        .archive("scope", "r:kept.md", &revision, raw)
        .unwrap();
    archive
        .archive("scope", "r:orphan.md", &revision, raw)
        .unwrap();
    let protected = BTreeSet::from([("r:kept.md".into(), revision.clone())]);
    let plan = archive.plan_gc(&protected).unwrap();
    assert_eq!(plan.eligible_manifest_ids.len(), 1);
    assert!(plan.eligible_object_ids.is_empty());
    archive.apply_gc(&plan, &protected).unwrap();
    assert_eq!(
        archive.restore("scope", "r:kept.md", &revision).unwrap(),
        raw
    );
    let orphan = b"A crash can leave a valid blob without its manifest.";
    let orphan_sha = sha(orphan);
    let orphan_record = archive
        .archive("scope", "r:crashed.md", &orphan_sha, orphan)
        .unwrap();
    fs::remove_file(
        sandbox
            .0
            .join("cold")
            .join(format!("{}.arc", orphan_record.manifest_id)),
    )
    .unwrap();
    let plan = archive.plan_gc(&protected).unwrap();
    assert_eq!(plan.eligible_object_ids.len(), 1);
    archive.apply_gc(&plan, &protected).unwrap();
    let manifest = sandbox
        .0
        .join("cold")
        .join(format!("{}.arc", kept.manifest_id));
    fs::write(&manifest, b"corrupt").unwrap();
    let before = fs::read(&manifest).unwrap();
    let plan = archive.plan_gc(&BTreeSet::new()).unwrap();
    assert_eq!(plan.corrupt_objects, 1);
    assert!(plan.eligible_object_ids.is_empty());
    assert!(plan.eligible_manifest_ids.is_empty());
    assert_eq!(
        archive
            .apply_gc(&plan, &BTreeSet::new())
            .unwrap()
            .removed_objects,
        0
    );
    assert_eq!(fs::read(manifest).unwrap(), before);
}
