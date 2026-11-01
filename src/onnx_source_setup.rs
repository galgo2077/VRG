//! Consented, pinned ONNX Runtime source build for Intel macOS.
//! The caller owns the private cache directory and validates the resulting ABI.
use crate::error::{VrgError, VrgResult};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const SOURCE_VERSION: &str = "1.24.1";
pub const SOURCE_COMMIT: &str = "470ae16099a74fe05e31f2530489332c0525edb5";
const SOURCE_URL: &str = "https://github.com/microsoft/onnxruntime.git";
const PREFIX: &str = "1.24.1-x86_64-";
const LIBRARY: &str = "libonnxruntime.dylib";
const MAX_ENTRIES: usize = 128;

#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    pub path: PathBuf,
    pub sha256: String,
    pub reused: bool,
}

fn unavailable(message: impl Into<String>) -> VrgError {
    VrgError::SemanticUnavailable(message.into())
}
fn check_path(path: &Path) -> VrgResult<()> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(unavailable(
            "runtime cache path must be absolute and contain no traversal",
        ));
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        match fs::symlink_metadata(&prefix) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(unavailable("runtime cache path contains a symlink"));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
#[cfg(unix)]
fn private_owned(metadata: &fs::Metadata, directory: bool) -> VrgResult<()> {
    use std::os::unix::fs::MetadataExt;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(unavailable(
            "runtime cache entries must be private, owned and non-symlink",
        ));
    }
    Ok(())
}
#[cfg(not(unix))]
fn private_owned(_: &fs::Metadata, _: bool) -> VrgResult<()> {
    Err(unavailable(
        "runtime cache is not supported on this platform",
    ))
}
fn regular_private(path: &Path) -> VrgResult<File> {
    check_path(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let file = options.open(path)?;
    private_owned(&file.metadata()?, false)?;
    Ok(file)
}
fn file_hash(path: &Path) -> VrgResult<String> {
    let mut f = regular_private(path)?;
    let mut digest = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let n = f.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        digest.update(&bytes[..n]);
    }
    Ok(hex::encode(digest.finalize()))
}
fn is_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Read-only lookup: no command, library loading, network or filesystem mutation.
/// Integrity is established against the locally generated content-addressed cache name,
/// not against a vendor-signed digest. The caller must still verify ABI and inference.
pub fn find_cached(cache_root: &Path) -> VrgResult<Option<RuntimeInfo>> {
    check_path(cache_root)?;
    let root = match fs::symlink_metadata(cache_root) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    private_owned(&root, true)?;
    let mut matches = Vec::new();
    for (index, entry) in fs::read_dir(cache_root)?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(unavailable("runtime cache contains too many entries"));
        }
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(expected) = name.strip_prefix(PREFIX) else {
            continue;
        };
        if !is_digest(expected) {
            return Err(unavailable("runtime cache entry has invalid identity"));
        }
        let dir = entry.path();
        check_path(&dir)?;
        private_owned(&fs::symlink_metadata(&dir)?, true)?;
        let provenance = read_small(&dir.join("SOURCE"), 256)
            .map_err(|_| unavailable("runtime cache provenance is missing or unsafe"))?;
        if provenance != format!("onnxruntime {SOURCE_VERSION}\ncommit {SOURCE_COMMIT}\n") {
            return Err(unavailable(
                "runtime cache provenance does not match the pinned source",
            ));
        }
        for notice in ["LICENSE", "ThirdPartyNotices.txt"] {
            if regular_private(&dir.join(notice))?.metadata()?.len() == 0 {
                return Err(unavailable("runtime cache license notice is empty"));
            }
        }
        let path = dir.join(LIBRARY);
        let actual = file_hash(&path).map_err(|_| unavailable("runtime cache entry is incomplete or unsafe; preserve it and inspect before retrying"))?;
        if actual != expected {
            return Err(unavailable(
                "runtime cache checksum mismatch; preserve it and inspect before retrying",
            ));
        }
        matches.push(RuntimeInfo {
            path,
            sha256: actual,
            reused: true,
        });
    }
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => Err(unavailable("multiple runtime cache entries require review")),
    }
}

#[cfg(unix)]
fn private_dir(path: &Path) -> VrgResult<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)?;
    private_owned(&fs::symlink_metadata(path)?, true)
}
#[cfg(not(unix))]
fn private_dir(_: &Path) -> VrgResult<()> {
    Err(unavailable("unsupported platform"))
}

#[cfg(unix)]
fn lock(cache_root: &Path) -> VrgResult<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = cache_root.join(".build.lock");
    check_path(&path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    private_owned(&file.metadata()?, false)?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| unavailable("another ONNX Runtime setup is running"))?;
    Ok(file)
}
#[cfg(not(unix))]
fn lock(_: &Path) -> VrgResult<File> {
    Err(unavailable("unsupported platform"))
}

/// Run without inherited Git, Python or compiler configuration. A timeout kills the
/// process group so source builds do not leave children running after a failure.
fn run(
    program: &OsStr,
    args: &[&OsStr],
    cwd: &Path,
    output: Option<&Path>,
    timeout: Duration,
) -> VrgResult<()> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", cwd)
        .env("TMPDIR", cwd)
        .env("LANG", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", "https")
        .env("PYTHONNOUSERSITE", "1")
        .env("CXXFLAGS", "-DHAS_ARRAY_BOUNDS=")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if let Some(path) = output {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        cmd.stdout(Stdio::from(opts.open(path)?));
    } else {
        cmd.stdout(Stdio::null());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|_| unavailable(format!("required command {:?} is unavailable", program)))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(unavailable(format!(
                    "command {:?} failed ({status})",
                    program
                )))
            };
        }
        if started.elapsed() >= timeout {
            #[cfg(unix)]
            if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(unavailable(format!("command {:?} timed out", program)));
        }
        thread::sleep(Duration::from_millis(200));
    }
}
fn command(
    program: &str,
    args: &[&str],
    cwd: &Path,
    output: Option<&Path>,
    timeout_secs: u64,
) -> VrgResult<()> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    run(
        OsStr::new(program),
        &args,
        cwd,
        output,
        Duration::from_secs(timeout_secs),
    )
}
fn read_small(path: &Path, max: usize) -> VrgResult<String> {
    let mut file = regular_private(path)?;
    if file.metadata()?.len() > max as u64 {
        return Err(unavailable("source verification output exceeded its limit"));
    }
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    Ok(s)
}
fn prerequisites(stage: &Path) -> VrgResult<()> {
    command("git", &["--version"], stage, None, 15)
        .map_err(|_| unavailable("Git is required to retrieve the pinned ONNX source"))?;
    command(
        "python3",
        &["-c", "import sys; sys.exit(sys.version_info < (3, 10))"],
        stage,
        None,
        15,
    )
    .map_err(|_| unavailable("Python 3.10 or newer is required to build ONNX Runtime"))?;
    let cmake = stage.join("cmake-version");
    command("cmake", &["--version"], stage, Some(&cmake), 15)
        .map_err(|_| unavailable("CMake 3.28 or newer is required to build ONNX Runtime"))?;
    let version = read_small(&cmake, 8192)?;
    let token = version
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(2))
        .unwrap_or("");
    let mut parts = token.split('.').filter_map(|x| x.parse::<u32>().ok());
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if (major, minor) < (3, 28) {
        return Err(unavailable(
            "CMake 3.28 or newer is required to build ONNX Runtime",
        ));
    }
    command("xcrun", &["--find", "clang"], stage, None, 15)
        .map_err(|_| unavailable("Xcode or Apple Command Line Tools with clang are required"))?;
    command("xcrun", &["--find", "make"], stage, None, 15)
        .map_err(|_| unavailable("Apple Command Line Tools with make are required"))?;
    Ok(())
}

/// Reuse or build a locally content-addressed runtime. The caller must display and
/// obtain explicit source-build consent, and must perform real model inference
/// before enabling semantic search. Missing dependencies never install software.
pub fn prepare(
    cache_root: &Path,
    consent: bool,
    offline: bool,
    validate_abi: &dyn Fn(&Path) -> VrgResult<()>,
) -> VrgResult<RuntimeInfo> {
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "x86_64")) {
        return Err(unavailable(
            "ONNX source building is supported only on native macOS Intel",
        ));
    }
    check_path(cache_root)?;
    if let Some(found) = find_cached(cache_root)? {
        validate_abi(&found.path)?;
        if file_hash(&found.path)? != found.sha256 {
            return Err(unavailable("cached runtime changed during validation"));
        }
        return Ok(found);
    }
    if offline {
        return Err(unavailable(
            "offline mode cannot download ONNX Runtime source",
        ));
    }
    if !consent {
        return Err(unavailable(
            "building ONNX Runtime from source requires explicit consent",
        ));
    }
    private_owned(
        &fs::symlink_metadata(cache_root).map_err(|_| {
            unavailable("create the dedicated private runtime cache before building")
        })?,
        true,
    )?;
    let _lock = lock(cache_root)?;
    if let Some(found) = find_cached(cache_root)? {
        validate_abi(&found.path)?;
        if file_hash(&found.path)? != found.sha256 {
            return Err(unavailable("cached runtime changed during validation"));
        }
        return Ok(found);
    }
    #[cfg(unix)]
    {
        let stats = rustix::fs::statvfs(cache_root)
            .map_err(|_| unavailable("cannot check free space for ONNX Runtime build"))?;
        let available = u128::from(stats.f_bavail) * u128::from(stats.f_frsize);
        if available < 12 * 1024 * 1024 * 1024_u128 {
            return Err(unavailable(
                "ONNX Runtime source build requires at least 12 GiB of free space",
            ));
        }
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| unavailable(e.to_string()))?
        .as_nanos();
    let stage = cache_root.join(format!(".source-stage-{}-{stamp}", std::process::id()));
    private_dir(&stage)?;
    prerequisites(&stage)?;
    let source = stage.join("source");
    command(
        "git",
        &[
            "clone",
            "--depth",
            "1",
            "--branch",
            "v1.24.1",
            "--",
            SOURCE_URL,
            source
                .to_str()
                .ok_or_else(|| unavailable("source path is not UTF-8"))?,
        ],
        &stage,
        None,
        1200,
    )?;
    let rev = stage.join("source-revision");
    command(
        "git",
        &["-C", source.to_str().unwrap(), "rev-parse", "HEAD"],
        &stage,
        Some(&rev),
        30,
    )?;
    if read_small(&rev, 128)?.trim() != SOURCE_COMMIT {
        return Err(unavailable(
            "official ONNX Runtime source revision did not match the pinned commit",
        ));
    }
    command(
        "git",
        &[
            "-C",
            source.to_str().unwrap(),
            "submodule",
            "update",
            "--init",
            "--recursive",
            "--depth",
            "1",
        ],
        &stage,
        None,
        1200,
    )?;
    let subs = stage.join("submodules");
    command(
        "git",
        &[
            "-C",
            source.to_str().unwrap(),
            "submodule",
            "status",
            "--recursive",
        ],
        &stage,
        Some(&subs),
        120,
    )?;
    let submodules = read_small(&subs, 1024 * 1024)?;
    if submodules.lines().any(|line| !line.starts_with(' ')) {
        return Err(unavailable(
            "ONNX Runtime submodule identity verification failed",
        ));
    }
    let build = stage.join("build");
    let build_script = source.join("build.sh");
    let args = [
        "--config",
        "Release",
        "--build_shared_lib",
        "--parallel",
        "4",
        "--skip_tests",
        "--skip_submodule_sync",
        "--osx_arch",
        "x86_64",
        "--build_dir",
        build
            .to_str()
            .ok_or_else(|| unavailable("build path is not UTF-8"))?,
        "--cmake_extra_defines",
        "onnxruntime_BUILD_UNIT_TESTS=OFF",
    ];
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    run(
        build_script.as_os_str(),
        &args,
        &source,
        None,
        Duration::from_secs(7200),
    )?;
    let built = build.join("Release/libonnxruntime.1.24.1.dylib");
    check_path(&built)?;
    let metadata = fs::symlink_metadata(&built)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(unavailable(
            "source build did not produce a regular runtime library",
        ));
    }
    let publish = stage.join("publish");
    private_dir(&publish)?;
    let candidate = publish.join(LIBRARY);
    let mut dest = OpenOptions::new();
    dest.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        dest.mode(0o600);
    }
    let mut dest = dest.open(&candidate)?;
    std::io::copy(&mut File::open(&built)?, &mut dest)?;
    dest.flush()?;
    dest.sync_all()?;
    let digest = file_hash(&candidate)?;
    validate_abi(&candidate)?;
    if file_hash(&candidate)? != digest {
        return Err(unavailable("runtime changed during ABI validation"));
    }
    let final_dir = cache_root.join(format!("{PREFIX}{digest}"));
    // Licenses and library are fully staged before atomic, non-replacing promotion.
    for name in ["LICENSE", "ThirdPartyNotices.txt"] {
        let from = source.join(name);
        check_path(&from)?;
        let mut output = OpenOptions::new();
        output.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            output.mode(0o600);
        }
        let mut output = output.open(publish.join(name))?;
        std::io::copy(&mut File::open(from)?, &mut output)?;
        output.sync_all()?;
    }
    let mut provenance = OpenOptions::new();
    provenance.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        provenance.mode(0o600);
    }
    let mut provenance = provenance.open(publish.join("SOURCE"))?;
    write!(
        provenance,
        "onnxruntime {SOURCE_VERSION}\ncommit {SOURCE_COMMIT}\n"
    )?;
    provenance.sync_all()?;
    File::open(&publish)?.sync_all()?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        &publish,
        rustix::fs::CWD,
        &final_dir,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|_| {
        unavailable("runtime cache destination exists or cannot be promoted; it was not replaced")
    })?;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return Err(unavailable(
        "atomic runtime promotion is unavailable on this platform",
    ));
    let final_path = final_dir.join(LIBRARY);
    if file_hash(&final_path)? != digest {
        return Err(unavailable("runtime changed during cache promotion"));
    }
    File::open(&final_dir)?.sync_all()?;
    File::open(cache_root)?.sync_all()?;
    // The directory was created exclusively for this build, inside the private root.
    let _ = fs::remove_dir_all(&stage);
    Ok(RuntimeInfo {
        path: final_path,
        sha256: digest,
        reused: false,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "vrg-onnx-source-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            private_dir(&root).unwrap();
            Self(root)
        }
        fn entry(&self, content: &[u8], digest: &str) -> PathBuf {
            let dir = self.0.join(format!("{PREFIX}{digest}"));
            private_dir(&dir).unwrap();
            for (name, content) in [
                ("LICENSE", "synthetic license".to_owned()),
                ("ThirdPartyNotices.txt", "synthetic notices".to_owned()),
                (
                    "SOURCE",
                    format!("onnxruntime {SOURCE_VERSION}\ncommit {SOURCE_COMMIT}\n"),
                ),
            ] {
                fs::write(dir.join(name), content).unwrap();
                fs::set_permissions(dir.join(name), fs::Permissions::from_mode(0o600)).unwrap();
            }
            let path = dir.join(LIBRARY);
            let mut opts = OpenOptions::new();
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = opts
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            f.write_all(content).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_cache_is_passive() {
        let f = Fixture::new();
        let missing = f.0.join("missing");
        assert!(find_cached(&missing).unwrap().is_none());
        assert!(!missing.exists());
    }
    #[test]
    fn verifies_content_addressed_cache_without_loading_code() {
        let f = Fixture::new();
        let bytes = b"synthetic runtime, not executable";
        let digest = hex::encode(Sha256::digest(bytes));
        let path = f.entry(bytes, &digest);
        let found = find_cached(&f.0).unwrap().unwrap();
        assert_eq!(found.path, path);
        assert_eq!(found.sha256, digest);
        assert!(found.reused);
    }
    #[test]
    fn rejects_corruption_and_world_readable_cache() {
        let f = Fixture::new();
        let digest = hex::encode(Sha256::digest(b"expected"));
        let path = f.entry(b"different", &digest);
        assert!(find_cached(&f.0).is_err());
        fs::write(&path, b"expected").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(find_cached(&f.0).is_err());
    }
    #[test]
    fn rejects_ambiguous_runtime_versions() {
        let f = Fixture::new();
        for bytes in [b"first".as_slice(), b"second".as_slice()] {
            f.entry(bytes, &hex::encode(Sha256::digest(bytes)));
        }
        assert!(find_cached(&f.0).is_err());
    }
}
