//! Bounded, lossless derived-object encoding and exact-source archival.
//!
//! The archive is not an authorization service. Callers must revalidate the
//! configured root, exclusion policy and current source revision before both
//! archiving and restoring. Original sources are never written by this module.

use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Write},
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};

use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{VrgError, VrgResult};

pub const MAX_OBJECT_BYTES: usize = 16 * 1024 * 1024;
pub const FRAME_HEADER_BYTES: usize = 46;
const MAX_FRAME_BYTES: usize = MAX_OBJECT_BYTES + FRAME_HEADER_BYTES;
const MAX_MANIFEST_BYTES: usize = 2048;
const ARCHIVE_HEADER_BYTES: usize = 8;
const CODEC_RAW: u8 = 0;
const CODEC_ZLIB: u8 = 1;
const INSPECTION_RAW_LIMIT: &str = "archive inspection raw budget exceeded";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct EncodedObject {
    pub codec: String,
    pub data: Vec<u8>,
    pub raw_len: u64,
    pub sha256: String,
}

/// Encode only when beneficial. DEFLATE has a fixed 32 KiB decode window;
/// level 3 reuses the already locked local flate2 dependency.
pub fn encode(raw: &[u8]) -> VrgResult<EncodedObject> {
    if raw.len() > MAX_OBJECT_BYTES {
        return Err(invalid("object exceeds 16 MiB limit"));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(3));
    encoder.write_all(raw)?;
    let compressed = encoder.finish()?;
    let (codec, data) = if compressed.len() < raw.len() {
        ("zlib", compressed)
    } else {
        ("raw", raw.to_vec())
    };
    Ok(EncodedObject {
        codec: codec.into(),
        data,
        raw_len: raw.len() as u64,
        sha256: hex::encode(Sha256::digest(raw)),
    })
}

/// Decode a single stream, checking its exact declared size and SHA-256.
/// No allocation depends on an unchecked length. Input, output and decoder
/// window are bounded, including dishonest lengths and expansion attacks.
pub fn decode(codec: &str, data: &[u8], raw_len: u64, sha256: &str) -> VrgResult<Vec<u8>> {
    if raw_len > MAX_OBJECT_BYTES as u64 || data.len() > MAX_OBJECT_BYTES {
        return Err(invalid("encoded or expanded object exceeds limit"));
    }
    validate_hash(sha256)?;
    let expected_len = raw_len as usize;
    let raw = match codec {
        "raw" => {
            if data.len() != expected_len {
                return Err(invalid("raw object length mismatch"));
            }
            data.to_vec()
        }
        "zlib" => {
            let mut decoder = Decompress::new(true);
            let mut output = Vec::with_capacity(expected_len.min(64 * 1024));
            let mut block = [0u8; 64 * 1024];
            loop {
                let before_in = decoder.total_in();
                let before_out = decoder.total_out();
                let offset =
                    usize::try_from(before_in).map_err(|_| invalid("decoder input overflow"))?;
                if offset > data.len() {
                    return Err(invalid("decoder input overflow"));
                }
                // At most one extra byte is decoded to detect a false length.
                let remaining = expected_len.saturating_sub(output.len());
                let available = block.len().min(remaining.saturating_add(1));
                let status = decoder
                    .decompress(
                        &data[offset..],
                        &mut block[..available],
                        FlushDecompress::Finish,
                    )
                    .map_err(|_| invalid("corrupt compressed object"))?;
                let produced = (decoder.total_out() - before_out) as usize;
                if produced > remaining {
                    return Err(invalid("expanded object exceeds declared length"));
                }
                output.extend_from_slice(&block[..produced]);
                if status == Status::StreamEnd {
                    if decoder.total_in() != data.len() as u64 || output.len() != expected_len {
                        return Err(invalid(
                            "compressed stream length mismatch or trailing data",
                        ));
                    }
                    break output;
                }
                if decoder.total_in() == before_in && decoder.total_out() == before_out {
                    return Err(invalid("truncated or stalled compressed object"));
                }
            }
        }
        _ => return Err(invalid("unsupported object codec")),
    };
    if hex::encode(Sha256::digest(&raw)) != sha256 {
        return Err(invalid("object SHA-256 mismatch"));
    }
    Ok(raw)
}

/// Binary frame: VRGA, version u8, codec u8, raw length LE u64,
/// uncompressed SHA-256 (32 bytes), then the exact encoded payload.
pub fn encode_frame(raw: &[u8]) -> VrgResult<Vec<u8>> {
    let object = encode(raw)?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES + object.data.len());
    frame.extend_from_slice(b"VRGA");
    frame.push(1);
    frame.push(if object.codec == "zlib" {
        CODEC_ZLIB
    } else {
        CODEC_RAW
    });
    frame.extend_from_slice(&object.raw_len.to_le_bytes());
    frame.extend_from_slice(&Sha256::digest(raw));
    frame.extend_from_slice(&object.data);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8]) -> VrgResult<Vec<u8>> {
    if frame.len() < FRAME_HEADER_BYTES || frame.len() > MAX_FRAME_BYTES {
        return Err(invalid("object frame length is invalid"));
    }
    if &frame[..4] != b"VRGA" || frame[4] != 1 {
        return Err(invalid("unsupported object frame version"));
    }
    let codec = match frame[5] {
        CODEC_RAW => "raw",
        CODEC_ZLIB => "zlib",
        _ => return Err(invalid("unsupported frame codec")),
    };
    let raw_len = u64::from_le_bytes(frame[6..14].try_into().expect("fixed-size checked frame"));
    decode(codec, &frame[46..], raw_len, &hex::encode(&frame[14..46]))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveRecord {
    pub schema_version: u8,
    pub scope: String,
    pub source_id: String,
    pub revision: String,
    /// Canonical payload hash within its privacy scope, shared by manifests.
    pub object_id: String,
    /// Source-membership identity; its filename is `<manifest_id>.arc`.
    pub manifest_id: String,
    pub raw_len: u64,
    /// This manifest plus its referenced shared blob. Do not sum this field
    /// across manifests to report physical usage; count filesystem files once.
    pub stored_bytes: u64,
    pub codec: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArchiveInspection {
    pub state: String,
    pub scanned_objects: usize,
    pub valid_objects: usize,
    pub corrupt_objects: usize,
    pub total_raw_bytes: u64,
    pub diagnostic_codes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveGcPlan {
    pub fingerprint: String,
    pub protected_fingerprint: String,
    pub eligible_manifest_ids: Vec<String>,
    pub eligible_object_ids: Vec<String>,
    pub protected_objects: usize,
    pub corrupt_objects: usize,
    pub reclaimable_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArchiveGcReport {
    pub removed_objects: usize,
    pub removed_bytes: u64,
    pub corrupt_preserved: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlobRecord {
    schema_version: u8,
    scope: String,
    revision: String,
    object_id: String,
    raw_len: u64,
    codec: String,
}

#[derive(Default)]
struct Catalogue {
    manifests: std::collections::BTreeMap<String, (ArchiveRecord, u64)>,
    blobs: std::collections::BTreeMap<String, (BlobRecord, u64)>,
    corrupt: BTreeSet<String>,
    fingerprints: Vec<(String, String)>,
    raw_bytes: u64,
    scanned: usize,
    partial: bool,
    diagnostic_codes: BTreeSet<String>,
}

/// Descriptor-based exact-source store. Callers authorize source membership
/// before every operation; a payload hash never grants source membership.
#[derive(Debug)]
pub struct ArchiveStore {
    dir: File,
}

impl ArchiveStore {
    /// Mutating initialization, reserved for explicit archive/maintenance use.
    #[cfg(unix)]
    pub fn open(store: &Path) -> VrgResult<Self> {
        use rustix::fs::{Mode, OFlags, mkdirat, openat};
        let root = open_canonical_directory(store)?;
        match mkdirat(&root, "cold", Mode::from_bits_truncate(0o700)) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(io_error(error)),
        }
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let dir = File::from(openat(&root, "cold", flags, Mode::empty()).map_err(io_error)?);
        rustix::fs::fchmod(&dir, Mode::from_bits_truncate(0o700)).map_err(io_error)?;
        let lock = File::from(
            openat(
                &dir,
                ".reader.lock",
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(io_error)?,
        );
        if !lock.metadata()?.is_file() {
            return Err(invalid("cold lease must be a regular file"));
        }
        dir.sync_all()?;
        root.sync_all()?;
        Ok(Self { dir })
    }

    #[cfg(not(unix))]
    pub fn open(_store: &Path) -> VrgResult<Self> {
        Err(invalid(
            "exact-source archive is available only on Linux/Unix",
        ))
    }

    pub fn archive(
        &self,
        scope: &str,
        source_id: &str,
        revision: &str,
        raw: &[u8],
    ) -> VrgResult<ArchiveRecord> {
        self.archive_checked(scope, source_id, revision, raw, || Ok(()))
    }

    /// Stage both representations, verify exact bytes, then revalidate source
    /// ACL/revision immediately before publishing blob and membership manifest.
    #[cfg(unix)]
    pub fn archive_checked(
        &self,
        scope: &str,
        source_id: &str,
        revision: &str,
        raw: &[u8],
        validate: impl FnOnce() -> VrgResult<()>,
    ) -> VrgResult<ArchiveRecord> {
        let _lease = self.reader_lease()?;
        validate_identity(scope, source_id, revision)?;
        if raw.len() > MAX_OBJECT_BYTES || hex::encode(Sha256::digest(raw)) != revision {
            return Err(invalid("archive source revision or length mismatch"));
        }
        let object_id = payload_id(scope, revision);
        let manifest_id = archive_id(scope, source_id, revision);
        let frame = encode_frame(raw)?;
        let codec = if frame[5] == CODEC_ZLIB {
            "zlib"
        } else {
            "raw"
        };
        let blob = BlobRecord {
            schema_version: 1,
            scope: scope.into(),
            revision: revision.into(),
            object_id: object_id.clone(),
            raw_len: raw.len() as u64,
            codec: codec.into(),
        };
        let metadata =
            serde_json::to_vec(&blob).map_err(|_| invalid("blob metadata encoding failed"))?;
        let mut blob_bytes = Vec::with_capacity(8 + metadata.len() + frame.len());
        blob_bytes.extend_from_slice(b"VRGB");
        blob_bytes.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        blob_bytes.extend_from_slice(&metadata);
        blob_bytes.extend_from_slice(&frame);
        let mut record = ArchiveRecord {
            schema_version: 2,
            scope: scope.into(),
            source_id: source_id.into(),
            revision: revision.into(),
            object_id,
            manifest_id,
            raw_len: raw.len() as u64,
            stored_bytes: 0,
            codec: codec.into(),
        };
        let mut manifest =
            serde_json::to_vec(&record).map_err(|_| invalid("manifest encoding failed"))?;
        for _ in 0..4 {
            let length = (manifest.len() + blob_bytes.len()) as u64;
            if record.stored_bytes == length {
                break;
            }
            record.stored_bytes = length;
            manifest =
                serde_json::to_vec(&record).map_err(|_| invalid("manifest encoding failed"))?;
        }
        if manifest.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("archive manifest exceeds bound"));
        }
        let staged_blob = self.stage(&blob_bytes)?;
        let staged_manifest = self.stage(&manifest)?;
        let verified_blob = self.read_file(&staged_blob.name, MAX_BLOB_BYTES)?;
        let (checked_blob, restored) = decode_blob(&verified_blob, MAX_OBJECT_BYTES)?;
        let verified_manifest = self.read_file(&staged_manifest.name, MAX_MANIFEST_BYTES)?;
        let checked_record: ArchiveRecord = serde_json::from_slice(&verified_manifest)
            .map_err(|_| invalid("invalid staged manifest"))?;
        validate_record(&checked_record)?;
        if restored != raw || checked_record != record || checked_blob.object_id != record.object_id
        {
            return Err(invalid("archive staging verification mismatch"));
        }
        validate()?;
        self.publish(
            &staged_blob,
            &format!("{}.blob", record.object_id),
            &blob_bytes,
        )?;
        // The payload directory entry must be durable before a membership
        // manifest can become durable and refer to it after power loss.
        self.dir.sync_all()?;
        self.publish(
            &staged_manifest,
            &format!("{}.arc", record.manifest_id),
            &manifest,
        )?;
        self.dir.sync_all()?;
        drop(staged_manifest);
        drop(staged_blob);
        self.dir.sync_all()?;
        Ok(record)
    }

    #[cfg(not(unix))]
    pub fn archive_checked(
        &self,
        _scope: &str,
        _source_id: &str,
        _revision: &str,
        _raw: &[u8],
        _validate: impl FnOnce() -> VrgResult<()>,
    ) -> VrgResult<ArchiveRecord> {
        Err(invalid("archival unavailable on this platform"))
    }

    pub fn restore(&self, scope: &str, source_id: &str, revision: &str) -> VrgResult<Vec<u8>> {
        let _lease = self.reader_lease()?;
        validate_identity(scope, source_id, revision)?;
        let manifest_id = archive_id(scope, source_id, revision);
        let manifest = self.read_file(&format!("{manifest_id}.arc"), MAX_MANIFEST_BYTES)?;
        let record: ArchiveRecord = serde_json::from_slice(&manifest)
            .map_err(|_| invalid("invalid archive membership manifest"))?;
        validate_record(&record)?;
        // Verify membership before opening a shared payload. Hash guessing or
        // another source's manifest cannot authorize this source.
        if record.scope != scope
            || record.source_id != source_id
            || record.revision != revision
            || record.manifest_id != manifest_id
        {
            return Err(invalid("archive source membership mismatch"));
        }
        let bytes = self.read_file(&format!("{}.blob", record.object_id), MAX_BLOB_BYTES)?;
        let (blob, raw) = decode_blob(&bytes, MAX_OBJECT_BYTES)?;
        if !manifest_matches_blob(&record, &blob)
            || record.stored_bytes != (manifest.len() + bytes.len()) as u64
        {
            return Err(invalid("archive manifest/blob integrity mismatch"));
        }
        Ok(raw)
    }

    /// Passive inspection does not mkdir/chmod/create a lock or compact SQLite.
    #[cfg(unix)]
    pub fn inspect_existing(
        store: &Path,
        max_objects: usize,
        max_total_raw: usize,
    ) -> VrgResult<ArchiveInspection> {
        use rustix::fs::{Mode, OFlags, openat};
        if max_objects == 0 || max_objects > 4096 || max_total_raw > 32 * 1024 * 1024 {
            return Err(invalid("invalid archive inspection limits"));
        }
        let mut report = ArchiveInspection {
            state: "valid".into(),
            scanned_objects: 0,
            valid_objects: 0,
            corrupt_objects: 0,
            total_raw_bytes: 0,
            diagnostic_codes: Vec::new(),
        };
        let root = match open_canonical_directory(store) {
            Ok(root) => root,
            Err(VrgError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                report.state = "absent".into();
                return Ok(report);
            }
            Err(error) => return Err(error),
        };
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let dir = match openat(&root, "cold", flags, Mode::empty()) {
            Ok(dir) => File::from(dir),
            Err(rustix::io::Errno::NOENT) => {
                report.state = "not_initialized".into();
                return Ok(report);
            }
            Err(error) => return Err(io_error(error)),
        };
        let archive = Self { dir };
        let _lease = match archive.cold_lock(false) {
            Ok(file) => Some(file),
            Err(VrgError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(VrgError::Conflict(_)) => {
                report.state = "partial".into();
                report.diagnostic_codes.push("archive_busy".into());
                return Ok(report);
            }
            Err(error) => return Err(error),
        };
        let catalogue = archive.catalogue(max_objects, max_total_raw)?;
        report.scanned_objects = catalogue.scanned;
        report.valid_objects = catalogue.blobs.len()
            + catalogue
                .manifests
                .values()
                .filter(|(record, bytes)| {
                    catalogue
                        .blobs
                        .get(&record.object_id)
                        .is_some_and(|(blob, blob_bytes)| {
                            manifest_matches_blob(record, blob)
                                && record.stored_bytes == bytes + blob_bytes
                        })
                })
                .count();
        report.corrupt_objects = catalogue.corrupt.len();
        report.total_raw_bytes = catalogue.raw_bytes;
        report.diagnostic_codes = catalogue.diagnostic_codes.into_iter().collect();
        report.state = if report.corrupt_objects > 0 {
            "corrupt"
        } else if catalogue.partial {
            "partial"
        } else {
            "valid"
        }
        .into();
        Ok(report)
    }

    #[cfg(not(unix))]
    pub fn inspect_existing(
        _store: &Path,
        _max_objects: usize,
        _max_total_raw: usize,
    ) -> VrgResult<ArchiveInspection> {
        Err(invalid("archive inspection unavailable on this platform"))
    }

    pub fn reader_lease(&self) -> VrgResult<File> {
        self.cold_lock(false)
    }

    #[cfg(unix)]
    fn cold_lock(&self, exclusive: bool) -> VrgResult<File> {
        use rustix::fs::{Mode, OFlags, openat};
        let access = if exclusive {
            OFlags::RDWR
        } else {
            OFlags::RDONLY
        };
        let file = File::from(
            openat(
                &self.dir,
                ".reader.lock",
                access | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io_error)?,
        );
        if !file.metadata()?.is_file() {
            return Err(invalid("cold lease must be a regular file"));
        }
        match if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        } {
            Ok(()) => Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => Err(VrgError::Conflict(
                "cold archive reader/maintenance lease is busy".into(),
            )),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    #[cfg(not(unix))]
    fn cold_lock(&self, _exclusive: bool) -> VrgResult<File> {
        Err(invalid("cold locking unavailable"))
    }

    pub fn plan_gc(&self, protected: &BTreeSet<(String, String)>) -> VrgResult<ArchiveGcPlan> {
        let _lease = self.reader_lease()?;
        self.gc_plan_locked(protected)
    }

    #[cfg(unix)]
    pub fn apply_gc(
        &self,
        plan: &ArchiveGcPlan,
        protected: &BTreeSet<(String, String)>,
    ) -> VrgResult<ArchiveGcReport> {
        let _lease = self.cold_lock(true)?;
        let fresh = self.gc_plan_locked(protected)?;
        if &fresh != plan {
            return Err(VrgError::Conflict(
                "cold GC plan or protection set is stale".into(),
            ));
        }
        let mut report = ArchiveGcReport {
            removed_objects: 0,
            removed_bytes: 0,
            corrupt_preserved: fresh.corrupt_objects,
        };
        // Manifests first. A crash leaves an orphan blob, never a dangling live
        // source reference. Replanning safely collects the orphan later.
        for (ids, extension) in [
            (&fresh.eligible_manifest_ids, "arc"),
            (&fresh.eligible_object_ids, "blob"),
        ] {
            for id in ids {
                validate_hash(id)?;
                let name = format!("{id}.{extension}");
                let bytes = self.read_file(
                    &name,
                    if extension == "arc" {
                        MAX_MANIFEST_BYTES
                    } else {
                        MAX_BLOB_BYTES
                    },
                )?;
                rustix::fs::unlinkat(&self.dir, name.as_str(), rustix::fs::AtFlags::empty())
                    .map_err(io_error)?;
                report.removed_objects += 1;
                report.removed_bytes += bytes.len() as u64;
            }
            // Persist manifest retirement before removing any payload. If a
            // crash interrupts the second phase, only unreferenced blobs remain.
            self.dir.sync_all()?;
        }
        self.dir.sync_all()?;
        Ok(report)
    }

    #[cfg(not(unix))]
    pub fn apply_gc(
        &self,
        _plan: &ArchiveGcPlan,
        _protected: &BTreeSet<(String, String)>,
    ) -> VrgResult<ArchiveGcReport> {
        Err(invalid("cold GC unavailable"))
    }

    fn gc_plan_locked(&self, protected: &BTreeSet<(String, String)>) -> VrgResult<ArchiveGcPlan> {
        let mut catalogue = self.catalogue(4096, 32 * 1024 * 1024)?;
        if catalogue.partial {
            return Err(invalid(
                "capacity_pressure: cold GC bounded verification incomplete; preserve all objects",
            ));
        }
        let mut eligible_manifest_ids = Vec::new();
        let mut referenced = BTreeSet::new();
        let mut protected_objects = 0;
        let mut reclaimable_bytes = 0;
        for (id, (record, bytes)) in &catalogue.manifests {
            if protected.contains(&(record.source_id.clone(), record.revision.clone())) {
                protected_objects += 1;
                referenced.insert(record.object_id.clone());
            } else {
                eligible_manifest_ids.push(id.clone());
                reclaimable_bytes += bytes;
            }
        }
        let mut eligible_object_ids = Vec::new();
        // Any corrupt manifest could still reference a blob we cannot interpret.
        // Preserve every payload until explicit repair restores full reachability.
        if catalogue.corrupt.is_empty() {
            for (id, (_, bytes)) in &catalogue.blobs {
                if !referenced.contains(id) {
                    eligible_object_ids.push(id.clone());
                    reclaimable_bytes += bytes;
                }
            }
        }
        catalogue.fingerprints.sort();
        let fingerprint = hex::encode(Sha256::digest(
            serde_json::to_vec(&catalogue.fingerprints)
                .map_err(|_| invalid("catalogue encoding failed"))?,
        ));
        let protected_fingerprint = hex::encode(Sha256::digest(
            serde_json::to_vec(protected).map_err(|_| invalid("protection encoding failed"))?,
        ));
        Ok(ArchiveGcPlan {
            fingerprint,
            protected_fingerprint,
            eligible_manifest_ids,
            eligible_object_ids,
            protected_objects,
            corrupt_objects: catalogue.corrupt.len(),
            reclaimable_bytes,
        })
    }

    #[cfg(unix)]
    fn catalogue(&self, max_entries: usize, max_raw: usize) -> VrgResult<Catalogue> {
        let directory = cap_std::fs::Dir::from_std_file(self.dir.try_clone()?);
        let mut output = Catalogue::default();
        let mut physical_bytes = 0;
        for entry in directory.entries()? {
            let entry = entry?;
            let os_name = entry.file_name();
            if os_name == ".reader.lock" {
                continue;
            }
            if output.scanned >= max_entries {
                output.partial = true;
                output.diagnostic_codes.insert("object_limit".into());
                break;
            }
            output.scanned += 1;
            let Some(name) = os_name.to_str() else {
                output.partial = true;
                output.diagnostic_codes.insert("invalid_object_name".into());
                continue;
            };
            if name.starts_with(".staging-") {
                output.partial = true;
                output.diagnostic_codes.insert("staging_present".into());
                continue;
            }
            let parsed = name
                .rsplit_once('.')
                .filter(|(id, ext)| valid_hash(id) && matches!(*ext, "arc" | "blob"));
            let Some((id, extension)) = parsed else {
                output.corrupt.insert(name.into());
                output
                    .fingerprints
                    .push((name.into(), "invalid_name".into()));
                continue;
            };
            let bytes = match self.read_file(
                name,
                if extension == "arc" {
                    MAX_MANIFEST_BYTES
                } else {
                    MAX_BLOB_BYTES
                },
            ) {
                Ok(bytes) => bytes,
                Err(VrgError::Io(error))
                    if error.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    output.partial = true;
                    output
                        .diagnostic_codes
                        .insert("object_permission_denied".into());
                    output
                        .fingerprints
                        .push((name.into(), "permission_denied_preserved".into()));
                    continue;
                }
                Err(_) => {
                    output.corrupt.insert(name.into());
                    output
                        .fingerprints
                        .push((name.into(), "unreadable_preserved".into()));
                    continue;
                }
            };
            physical_bytes += bytes.len();
            if physical_bytes > 64 * 1024 * 1024 {
                output.partial = true;
                output.diagnostic_codes.insert("physical_byte_limit".into());
                break;
            }
            output
                .fingerprints
                .push((name.into(), hex::encode(Sha256::digest(&bytes))));
            if extension == "arc" {
                let record = serde_json::from_slice::<ArchiveRecord>(&bytes)
                    .map_err(|_| invalid("invalid archive manifest"))
                    .and_then(|record| {
                        validate_record(&record)?;
                        if record.manifest_id != id {
                            return Err(invalid("manifest filename mismatch"));
                        }
                        Ok(record)
                    });
                match record {
                    Ok(record) => {
                        output
                            .manifests
                            .insert(id.into(), (record, bytes.len() as u64));
                    }
                    Err(_) => {
                        output.corrupt.insert(name.into());
                    }
                }
            } else {
                match decode_blob(&bytes, max_raw.saturating_sub(output.raw_bytes as usize)) {
                    Ok((blob, raw)) if blob.object_id == id => {
                        output.raw_bytes += raw.len() as u64;
                        output.blobs.insert(id.into(), (blob, bytes.len() as u64));
                    }
                    Err(VrgError::Storage(message)) if message == INSPECTION_RAW_LIMIT => {
                        output.partial = true;
                        output.diagnostic_codes.insert("raw_byte_limit".into());
                        break;
                    }
                    _ => {
                        output.corrupt.insert(name.into());
                    }
                }
            }
        }
        if !output.partial {
            let invalid =
                output
                    .manifests
                    .iter()
                    .filter_map(|(id, (record, bytes))| {
                        let valid = output.blobs.get(&record.object_id).is_some_and(
                            |(blob, blob_bytes)| {
                                manifest_matches_blob(record, blob)
                                    && record.stored_bytes == bytes + blob_bytes
                            },
                        );
                        (!valid).then_some(id.clone())
                    })
                    .collect::<Vec<_>>();
            for id in invalid {
                output.manifests.remove(&id);
                output.corrupt.insert(format!("{id}.arc"));
            }
        }
        if !output.corrupt.is_empty() {
            output.diagnostic_codes.insert("corrupt_object".into());
        }
        Ok(output)
    }

    #[cfg(not(unix))]
    fn catalogue(&self, _max_entries: usize, _max_raw: usize) -> VrgResult<Catalogue> {
        Err(invalid("cold catalogue unavailable"))
    }

    #[cfg(unix)]
    fn read_file(&self, name: &str, maximum: usize) -> VrgResult<Vec<u8>> {
        use rustix::fs::{Mode, OFlags, openat};
        // Internal single-component names only, never source paths.
        if name.is_empty() || name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
            return Err(invalid("invalid cold filename"));
        }
        let file = File::from(
            openat(
                &self.dir,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io_error)?,
        );
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > maximum as u64 {
            return Err(invalid("archive file type or size invalid"));
        }
        let mut bytes = Vec::with_capacity((metadata.len() as usize).min(65536));
        file.take(maximum as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > maximum {
            return Err(invalid("archive file grew beyond size limit"));
        }
        Ok(bytes)
    }

    #[cfg(not(unix))]
    fn read_file(&self, _name: &str, _maximum: usize) -> VrgResult<Vec<u8>> {
        Err(invalid("cold read unavailable"))
    }

    #[cfg(unix)]
    fn stage(&self, bytes: &[u8]) -> VrgResult<TemporaryArchive<'_>> {
        use rustix::fs::{Mode, OFlags, openat};
        let name = format!(
            ".staging-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = File::from(
            openat(
                &self.dir,
                name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(io_error)?,
        );
        let temporary = TemporaryArchive {
            directory: &self.dir,
            name,
        };
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(temporary)
    }

    #[cfg(unix)]
    fn publish(&self, staged: &TemporaryArchive<'_>, name: &str, expected: &[u8]) -> VrgResult<()> {
        use rustix::fs::{AtFlags, linkat};
        match linkat(
            &self.dir,
            staged.name.as_str(),
            &self.dir,
            name,
            AtFlags::empty(),
        ) {
            Ok(()) => Ok(()),
            Err(rustix::io::Errno::EXIST) => {
                if self.read_file(name, expected.len())? != expected {
                    return Err(invalid("existing archive identity collision or corruption"));
                }
                Ok(())
            }
            Err(error) => Err(io_error(error)),
        }
    }
}

const MAX_BLOB_BYTES: usize = ARCHIVE_HEADER_BYTES + MAX_MANIFEST_BYTES + MAX_FRAME_BYTES;

fn decode_blob(bytes: &[u8], max_raw: usize) -> VrgResult<(BlobRecord, Vec<u8>)> {
    if bytes.len() < 8 || bytes.len() > MAX_BLOB_BYTES || &bytes[..4] != b"VRGB" {
        return Err(invalid("invalid COLD blob header"));
    }
    let length = u32::from_le_bytes(bytes[4..8].try_into().expect("checked blob header")) as usize;
    if length == 0 || length > MAX_MANIFEST_BYTES || 8 + length > bytes.len() {
        return Err(invalid("invalid COLD blob metadata length"));
    }
    let record: BlobRecord = serde_json::from_slice(&bytes[8..8 + length])
        .map_err(|_| invalid("invalid COLD blob metadata"))?;
    validate_scope(&record.scope)?;
    validate_hash(&record.revision)?;
    if record.schema_version != 1
        || record.object_id != payload_id(&record.scope, &record.revision)
        || record.raw_len > MAX_OBJECT_BYTES as u64
    {
        return Err(invalid("COLD blob identity mismatch"));
    }
    if record.raw_len > max_raw as u64 {
        return Err(invalid(INSPECTION_RAW_LIMIT));
    }
    let frame = &bytes[8 + length..];
    let raw = decode_frame(frame)?;
    let codec = if frame[5] == CODEC_ZLIB {
        "zlib"
    } else {
        "raw"
    };
    if raw.len() as u64 != record.raw_len
        || hex::encode(Sha256::digest(&raw)) != record.revision
        || codec != record.codec
    {
        return Err(invalid("COLD blob checksum or length mismatch"));
    }
    Ok((record, raw))
}

fn validate_record(record: &ArchiveRecord) -> VrgResult<()> {
    validate_identity(&record.scope, &record.source_id, &record.revision)?;
    if record.schema_version != 2
        || record.object_id != payload_id(&record.scope, &record.revision)
        || record.manifest_id != archive_id(&record.scope, &record.source_id, &record.revision)
        || record.raw_len > MAX_OBJECT_BYTES as u64
        || !matches!(record.codec.as_str(), "raw" | "zlib")
    {
        return Err(invalid("archive manifest identity mismatch"));
    }
    Ok(())
}

fn manifest_matches_blob(manifest: &ArchiveRecord, blob: &BlobRecord) -> bool {
    manifest.scope == blob.scope
        && manifest.revision == blob.revision
        && manifest.object_id == blob.object_id
        && manifest.raw_len == blob.raw_len
        && manifest.codec == blob.codec
}

fn archive_id(scope: &str, source_id: &str, revision: &str) -> String {
    identity_hash(b"vrg-exact-manifest-v2", &[scope, source_id, revision])
}
fn payload_id(scope: &str, revision: &str) -> String {
    identity_hash(b"vrg-exact-payload-v2", &[scope, revision])
}
fn identity_hash(prefix: &[u8], values: &[&str]) -> String {
    let mut hash = Sha256::new();
    hash.update(prefix);
    for value in values {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    hex::encode(hash.finalize())
}

fn validate_identity(scope: &str, source_id: &str, revision: &str) -> VrgResult<()> {
    validate_scope(scope)?;
    validate_hash(revision)?;
    if source_id.is_empty()
        || source_id.len() > 240
        || source_id.starts_with('/')
        || source_id.contains(['\\', '\0'])
        || source_id
            .split('/')
            .any(|part| matches!(part, ".." | "." | ""))
    {
        return Err(invalid("invalid archive source identity"));
    }
    Ok(())
}
fn validate_scope(scope: &str) -> VrgResult<()> {
    if scope.is_empty() || scope.len() > 128 || scope.contains(['/', '\\', '\0']) {
        return Err(invalid("invalid archive privacy scope"));
    }
    Ok(())
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate_hash(hash: &str) -> VrgResult<()> {
    if valid_hash(hash) {
        Ok(())
    } else {
        Err(invalid("expected lowercase SHA-256 identity"))
    }
}
fn invalid(message: &str) -> VrgError {
    VrgError::Storage(message.into())
}

#[cfg(unix)]
fn open_canonical_directory(store: &Path) -> VrgResult<File> {
    use rustix::fs::{Mode, OFlags, open, openat};
    if !store.is_absolute() || store.parent().is_none() {
        return Err(invalid(
            "archive store must be an absolute non-root directory",
        ));
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut dir = File::from(open("/", flags, Mode::empty()).map_err(io_error)?);
    for component in store.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                dir = File::from(openat(&dir, name, flags, Mode::empty()).map_err(io_error)?);
            }
            _ => {
                return Err(invalid(
                    "archive directory contains a noncanonical component",
                ));
            }
        }
    }
    Ok(dir)
}

#[cfg(unix)]
struct TemporaryArchive<'a> {
    directory: &'a File,
    name: String,
}
#[cfg(unix)]
impl Drop for TemporaryArchive<'_> {
    fn drop(&mut self) {
        let _ = rustix::fs::unlinkat(
            self.directory,
            self.name.as_str(),
            rustix::fs::AtFlags::empty(),
        );
    }
}
#[cfg(unix)]
fn io_error(error: rustix::io::Errno) -> VrgError {
    VrgError::Io(std::io::Error::from(error))
}
