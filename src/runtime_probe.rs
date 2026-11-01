//! macOS Intel ONNX Runtime ABI probe. A library executes code when loaded;
//! inspection must run only in the short-lived child process, after hash verification.
use crate::error::{VrgError, VrgResult};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn unavailable(message: &str) -> VrgError {
    VrgError::SemanticUnavailable(message.into())
}

/// Run the hidden verification command in a fresh process with no inherited environment.
/// This does not sandbox a malicious library. Only probe an authenticated, pinned artifact.
pub(crate) fn probe_in_child(path: &Path) -> VrgResult<()> {
    if !path.is_absolute() {
        return Err(unavailable("runtime probe requires an absolute path"));
    }
    let executable = std::env::current_exe()
        .map_err(|_| unavailable("cannot locate runtime probe executable"))?;
    let mut child = Command::new(executable)
        .arg("__verify-onnx-runtime")
        .arg(path)
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| unavailable("cannot start isolated ONNX Runtime probe"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(unavailable("ONNX Runtime ABI probe failed"))
                };
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable("ONNX Runtime ABI probe timed out"));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable("cannot observe ONNX Runtime ABI probe"));
            }
        }
    }
}

/// Load the candidate and ask its C API for version 24 without initializing the
/// process-global ort environment. Only call this function in the probe child.
/// The caller must first verify the artifact's provenance and digest.
pub(crate) fn inspect_api(path: &Path) -> VrgResult<()> {
    if !path.is_absolute() {
        return Err(unavailable("runtime probe requires an absolute path"));
    }
    #[repr(C)]
    struct ApiBase {
        get_api: Option<unsafe extern "C" fn(u32) -> *const std::ffi::c_void>,
        get_version: Option<unsafe extern "C" fn() -> *const std::ffi::c_char>,
    }
    // SAFETY: This operation executes library initialization and foreign code.
    // It is confined to the expendable child, and only authenticated bytes may be passed.
    unsafe {
        let library = libloading::Library::new(path)
            .map_err(|_| unavailable("cannot load ONNX Runtime library"))?;
        let get_base: libloading::Symbol<unsafe extern "C" fn() -> *const ApiBase> = library
            .get(b"OrtGetApiBase\0")
            .map_err(|_| unavailable("ONNX Runtime does not export OrtGetApiBase"))?;
        let base = get_base();
        if base.is_null() {
            return Err(unavailable("ONNX Runtime API base is null"));
        }
        let get_api = (*base)
            .get_api
            .ok_or_else(|| unavailable("ONNX Runtime GetApi is missing"))?;
        if get_api(24).is_null() {
            return Err(unavailable("ONNX Runtime does not provide API 24"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_relative_or_missing_library() {
        assert!(inspect_api(Path::new("relative.dylib")).is_err());
        let missing =
            std::env::temp_dir().join(format!("vrg-nonexistent-{}.dylib", std::process::id()));
        assert!(inspect_api(&missing).is_err());
        assert!(probe_in_child(Path::new("relative.dylib")).is_err());
    }
}
