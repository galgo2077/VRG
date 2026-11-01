//! Acquisition of a caller-pinned ONNX Runtime dylib; no model or URL is implied.
//! The containing private directory must already exist.
use crate::error::{VrgError, VrgResult};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

fn invalid(s: impl Into<String>) -> VrgError {
    VrgError::SemanticUnavailable(s.into())
}
fn check_path(path: &Path) -> VrgResult<()> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid(
            "runtime destination must be absolute without traversal",
        ));
    }
    let mut part = PathBuf::new();
    for component in path.components() {
        part.push(component.as_os_str());
        match fs::symlink_metadata(&part) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(invalid("runtime path contains a symlink"));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn validate_url(url: &str) -> VrgResult<()> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| invalid("runtime URL must use HTTPS"))?;
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty()
        || authority.contains('@')
        || url.contains(['?', '#', '\\'])
        || url
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(invalid(
            "runtime URL must not contain credentials, query, fragment or whitespace",
        ));
    }
    Ok(())
}
fn validate_digest(hash: &str) -> VrgResult<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(invalid(
            "runtime SHA-256 must contain 64 lowercase hexadecimal digits",
        ));
    }
    Ok(())
}
fn open_nofollow(path: &Path) -> VrgResult<File> {
    check_path(path)?;
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        return Err(invalid(
            "runtime installation is unsupported on this platform",
        ));
    }
    let f = opts.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(invalid("runtime destination must be a regular file"));
    }
    Ok(f)
}
fn verify(path: &Path, size: u64, hash: &str) -> VrgResult<()> {
    let mut file = open_nofollow(path)?;
    let metadata = file.metadata()?;
    private_owned(&metadata, false)?;
    if metadata.len() != size {
        return Err(invalid("runtime size mismatch"));
    }
    let mut hasher = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    if hex::encode(hasher.finalize()) != hash {
        return Err(invalid("runtime checksum mismatch"));
    }
    Ok(())
}
#[cfg(unix)]
fn private_owned(meta: &fs::Metadata, directory: bool) -> VrgResult<()> {
    use std::os::unix::fs::MetadataExt;
    if meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
        || (directory && !meta.is_dir())
        || (!directory && (!meta.is_file() || meta.nlink() != 1))
    {
        return Err(invalid(
            "runtime directory or file is not private and owned by the effective user",
        ));
    }
    Ok(())
}
#[cfg(not(unix))]
fn private_owned(_: &fs::Metadata, _: bool) -> VrgResult<()> {
    Err(invalid("unsupported platform"))
}
#[cfg(unix)]
fn same_inode(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}
#[cfg(not(unix))]
fn same_inode(_: &fs::Metadata, _: &fs::Metadata) -> bool {
    false
}
// Never unlink here: a same-UID process can replace the pathname at any time.
fn rollback_promoted(destination: &Path, original: &fs::Metadata, reason: &str) -> VrgError {
    let state = match fs::symlink_metadata(destination) {
        Ok(meta) if meta.is_file() && same_inode(&meta, original) => {
            "destination retains the created link"
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "destination is absent",
        Ok(_) => "destination changed externally",
        Err(_) => "destination could not be inspected",
    };
    VrgError::Recovery(format!(
        "{reason}; {state}: {}; no destination was removed or activated; verify size, SHA-256 and ABI before reuse",
        destination.display()
    ))
}
struct Staging(PathBuf, fs::Metadata);
impl Drop for Staging {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.0)
            .ok()
            .is_some_and(|m| m.is_file() && same_inode(&m, &self.1))
        {
            let _ = fs::remove_file(&self.0);
        }
    }
}
/// Returns true when newly installed, false when an identical installation is reused.
/// Offline mode can reuse an already verified file but never starts a network process.
/// The caller is responsible for selecting a trustworthy immutable HTTPS URL and hashes.
pub fn install_runtime(
    destination: &Path,
    url: &str,
    size: u64,
    sha256: &str,
    consent: bool,
    offline: bool,
    validate_abi: &dyn Fn(&Path) -> VrgResult<()>,
) -> VrgResult<bool> {
    install_runtime_with_curl(
        destination,
        url,
        size,
        sha256,
        consent,
        offline,
        Path::new("curl"),
        validate_abi,
    )
}

fn install_runtime_with_curl(
    destination: &Path,
    url: &str,
    size: u64,
    sha256: &str,
    consent: bool,
    offline: bool,
    curl: &Path,
    validate_abi: &dyn Fn(&Path) -> VrgResult<()>,
) -> VrgResult<bool> {
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        return Err(invalid(
            "runtime installation is unsupported on this platform",
        ));
    }
    check_path(destination)?;
    validate_url(url)?;
    validate_digest(sha256)?;
    if size == 0 {
        return Err(invalid("runtime size must be positive"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("runtime destination has no parent"))?;
    check_path(parent)?;
    private_owned(&fs::metadata(parent)?, true)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            verify(destination, size, sha256).map_err(|_| {
                invalid("existing runtime differs or is unsafe; it was not replaced")
            })?;
            validate_abi(destination)?;
            verify(destination, size, sha256)?;
            return Ok(false);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    if offline {
        return Err(invalid("offline mode prohibits runtime download"));
    }
    if !consent {
        return Err(invalid("runtime download requires explicit consent"));
    }
    if !parent.is_dir() {
        return Err(invalid("private runtime directory must already exist"));
    }
    #[cfg(unix)]
    {
        let space = rustix::fs::statvfs(parent)
            .map_err(|_| invalid("cannot determine available runtime storage"))?;
        let available = u128::from(space.f_bavail) * u128::from(space.f_frsize);
        if available < u128::from(size) + 4 * 1024 * 1024 {
            return Err(invalid("insufficient free space for the pinned runtime"));
        }
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| invalid(e.to_string()))?
        .as_nanos();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stage_path = parent.join(format!(
        ".vrg-runtime-{}-{stamp}-{sequence}",
        std::process::id()
    ));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&stage_path)?;
    let stage = Staging(stage_path, file.metadata()?);
    let mut child = Command::new(curl)
        .args([
            "--disable",
            "--fail",
            "--location",
            "--silent",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
            "--max-time",
            "900",
            "--max-filesize",
        ])
        .arg(size.to_string())
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| invalid("curl is required to download the runtime"))?;
    let result = (|| -> VrgResult<()> {
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| invalid("download stream unavailable"))?;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut buf = [0u8; 65536];
        loop {
            let n = output.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total = total
                .checked_add(n as u64)
                .ok_or_else(|| invalid("runtime size overflow"))?;
            if total > size {
                return Err(invalid("runtime download exceeds pinned size"));
            }
            file.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
        }
        if !child.wait()?.success() {
            return Err(invalid("runtime download failed"));
        }
        if total != size || hex::encode(hasher.finalize()) != sha256 {
            return Err(invalid("runtime download failed pinned size or checksum"));
        }
        file.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result?;
    if !same_inode(&stage.1, &fs::symlink_metadata(&stage.0)?) {
        return Err(invalid("runtime staging file changed"));
    }
    validate_abi(&stage.0)?;
    if !same_inode(&stage.1, &fs::symlink_metadata(&stage.0)?) {
        return Err(invalid(
            "runtime staging file changed during ABI validation",
        ));
    }
    verify(&stage.0, size, sha256)?;
    check_path(destination)?;
    match fs::hard_link(&stage.0, destination) {
        Ok(()) => {
            match fs::symlink_metadata(destination) {
                Ok(promoted) if promoted.is_file() && same_inode(&stage.1, &promoted) => (),
                _ => {
                    return Err(rollback_promoted(
                        destination,
                        &stage.1,
                        "runtime promotion identity mismatch",
                    ));
                }
            }
            if let Err(e) = File::open(parent).and_then(|f| f.sync_all()) {
                return Err(rollback_promoted(
                    destination,
                    &stage.1,
                    &format!("runtime directory synchronization failed: {e}"),
                ));
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            verify(destination, size, sha256)
                .map_err(|_| invalid("runtime destination changed; it was not replaced"))?;
            validate_abi(destination)?;
            verify(destination, size, sha256)?;
            Ok(false)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
#[path = "../tests/support/intel_runtime.rs"]
mod tests;
