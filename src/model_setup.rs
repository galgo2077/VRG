//! Pinned local model acquisition. No network is attempted in offline mode.
use crate::error::{VrgError, VrgResult};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Component, Path},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

/// Upstream author, MIT licensed. Revision and hashes are fixed; no 'latest'.
pub const MODEL_REVISION: &str = "614241f622f53c4eeff9890bdc4f31cfecc418b3";
pub const MODEL_SOURCE: &str = "https://huggingface.co/intfloat/multilingual-e5-small";
pub struct Artifact {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}
pub const ARTIFACTS: &[Artifact] = &[
    Artifact {
        name: "model.onnx",
        size: 470268510,
        sha256: "ca456c06b3a9505ddfd9131408916dd79290368331e7d76bb621f1cba6bc8665",
    },
    Artifact {
        name: "tokenizer.json",
        size: 17082730,
        sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
    },
    Artifact {
        name: "config.json",
        size: 653,
        sha256: "bbb7c1333fc4b3e27fbc9cd5d2070aabcc1d4dfb99917c3633e772f97545a6b6",
    },
    Artifact {
        name: "special_tokens_map.json",
        size: 167,
        sha256: "d05497f1da52c5e09554c0cd874037a083e1dc1b9cfd48034d1c717f1afc07a7",
    },
    Artifact {
        name: "tokenizer_config.json",
        size: 443,
        sha256: "a1d6bc8734a6f635dc158508bef000f8e2e5a759c7d92f984b2c86e5ff53425b",
    },
];
fn err(s: impl Into<String>) -> VrgError {
    VrgError::SemanticUnavailable(s.into())
}
fn no_symlinks(p: &Path) -> VrgResult<()> {
    if !p.is_absolute() || p.components().any(|x| matches!(x, Component::ParentDir)) {
        return Err(err("model path must be absolute without traversal"));
    }
    let mut x = std::path::PathBuf::new();
    for c in p.components() {
        x.push(c.as_os_str());
        match fs::symlink_metadata(&x) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(err("model path contains a symbolic link"));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub fn verify_model(dir: &Path) -> VrgResult<()> {
    no_symlinks(dir)?;
    for artifact in ARTIFACTS {
        let path = dir.join(artifact.name);
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != artifact.size {
            return Err(err(format!("invalid model artifact {}", artifact.name)));
        }
        let mut f = File::open(path)?;
        let mut digest = Sha256::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            digest.update(&buf[..n]);
        }
        if hex::encode(digest.finalize()) != artifact.sha256 {
            return Err(err(format!("checksum mismatch for {}", artifact.name)));
        }
    }
    Ok(())
}
/// Download only with explicit consent. Existing valid models are reused; existing
/// invalid or partial directories are never overwritten or deleted.
/// Requires an existing private parent and a separately installed curl executable.
pub fn download_model(dir: &Path, consent: bool, offline: bool) -> VrgResult<bool> {
    no_symlinks(dir)?;
    if dir.exists() {
        verify_model(dir)?;
        return Ok(false);
    }
    if offline {
        return Err(err("offline mode prohibits model download"));
    }
    if !consent {
        return Err(err("model download requires explicit consent"));
    }
    let parent = dir
        .parent()
        .ok_or_else(|| err("model path has no parent"))?;
    if !parent.is_dir() {
        return Err(err("model parent must already exist"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(parent)?.permissions().mode() & 0o077 != 0 {
            return Err(err("model parent must be private"));
        }
    }
    #[cfg(unix)]
    {
        let space = rustix::fs::statvfs(parent)
            .map_err(|_| err("cannot determine available model storage"))?;
        let available = u128::from(space.f_bavail) * u128::from(space.f_frsize);
        if !has_space(available) {
            return Err(err("insufficient free space for verified model download"));
        }
    }
    #[cfg(not(unix))]
    return Err(err(
        "automatic model download is not supported on this platform",
    ));
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| err(e.to_string()))?
        .as_nanos();
    let stage = parent.join(format!(".vrg-model-stage-{}-{stamp}", std::process::id()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&stage)?;
    let result = (|| {
        for item in ARTIFACTS {
            let url = format!("{MODEL_SOURCE}/resolve/{MODEL_REVISION}/onnx/{}", item.name);
            let status = Command::new("curl")
                .args([
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--connect-timeout",
                    "15",
                    "--max-time",
                    "900",
                    "--max-filesize",
                ])
                .arg(item.size.to_string())
                .arg("--output")
                .arg(stage.join(item.name))
                .arg(url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|_| err("curl is required to download the model"))?;
            if !status.success() {
                return Err(err(format!("download failed: {}", item.name)));
            }
        }
        verify_model(&stage)?;
        // create_dir claims the destination without replacing another installation;
        // a crash during promotion can leave an incomplete directory, never a valid replacement.
        builder.create(dir)?;
        for item in ARTIFACTS {
            fs::rename(stage.join(item.name), dir.join(item.name))?;
        }
        verify_model(dir)?;
        Ok(true)
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

#[cfg(unix)]
fn has_space(available: u128) -> bool {
    let required = ARTIFACTS
        .iter()
        .map(|item| u128::from(item.size))
        .sum::<u128>()
        + 16 * 1024 * 1024;
    available >= required
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn rejects_insufficient_space() {
        assert!(!super::has_space(0));
        assert!(!super::has_space(16 * 1024 * 1024));
        assert!(super::has_space(u128::MAX));
    }
}
