//! Model storage path resolution (REQ-LIB-7).
//!
//! Primary storage is `<exe_dir>/models` (portable installs). A writability
//! probe with read-back decides whether that location is usable; otherwise the
//! app falls back to `%APPDATA%/piper-tts-gui/models` (via the `dirs` crate)
//! and reports the fallback through the `is_fallback` flag so the UI can show
//! a visible location indicator (design D5).
//!
//! Note (design F4): the asInvoker application manifest that disables UAC
//! virtualization is shipped with the packaging task; the probe is kept here
//! as the best-effort detection layer.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Sequence counter so concurrent probes never touch the same marker file.
static PROBE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Resolved model storage location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStorage {
    /// Absolute directory that holds (or will hold) downloaded models.
    pub path: PathBuf,
    /// `true` when the exe-dir primary was not usable and the `%APPDATA%`
    /// fallback was chosen instead.
    pub is_fallback: bool,
}

/// Probe whether `dir` accepts creating, reading back, and deleting a file.
///
/// The directory is created if missing; a marker file is then written, read
/// back and byte-compared, and finally removed. The read-back is deliberate
/// (F4): virtualization overlays can make a plain write report success while
/// the file is not actually readable at the same path. The marker name embeds
/// a sequence number so concurrent probes (parallel tests, concurrent Tauri
/// commands) never collide on the same file.
fn probe_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let seq = PROBE_SEQ.fetch_add(1, Ordering::Relaxed);
    let probe = dir.join(format!("piper-probe-{}-{seq}.tmp", std::process::id()));
    let payload: &[u8] = b"piper-tts-gui-probe";
    let written = std::fs::write(&probe, payload).is_ok();
    let read_back = written
        && std::fs::read(&probe)
            .map(|bytes| bytes.as_slice() == payload)
            .unwrap_or(false);
    let _ = std::fs::remove_file(&probe);
    read_back
}

/// Resolve the model storage directory, preferring `<exe_dir>/models`.
///
/// When the exe-dir primary fails the writability probe, the platform user
/// data dir (`%APPDATA%` on Windows) fallback is returned with `is_fallback`
/// set.
pub fn resolve_models_dir(exe_dir: &Path) -> ModelStorage {
    let primary = exe_dir.join("models");
    if probe_writable(&primary) {
        return ModelStorage {
            path: primary,
            is_fallback: false,
        };
    }

    let fallback = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("piper-tts-gui")
        .join("models");
    ModelStorage {
        path: fallback,
        is_fallback: true,
    }
}

/// Resolve the model storage directory for the running executable.
pub fn models_dir() -> ModelStorage {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_default();
    resolve_models_dir(&exe_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_temp_dir_uses_exe_dir() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let storage = resolve_models_dir(tmp.path());
        assert!(!storage.is_fallback);
        assert_eq!(storage.path, tmp.path().join("models"));
        // The probe must have created the directory as a side effect.
        assert!(storage.path.is_dir());
    }

    #[test]
    fn non_writable_primary_falls_back_to_appdata() {
        let tmp = tempfile::tempdir().expect("temp dir");
        // A plain file where the `models` directory would go makes the probe
        // fail deterministically on Windows and Unix alike.
        let blocker = tmp.path().join("models");
        std::fs::write(&blocker, b"not a directory").expect("blocker file");

        let storage = resolve_models_dir(tmp.path());
        assert!(storage.is_fallback);
        assert_eq!(
            storage.path,
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("piper-tts-gui")
                .join("models")
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_only_primary_falls_back_to_appdata() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("temp dir");
        let primary = tmp.path().join("models");
        std::fs::create_dir_all(&primary).expect("create primary");
        let mut perms = std::fs::metadata(&primary).expect("metadata").permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&primary, perms).expect("set read-only");

        let storage = resolve_models_dir(tmp.path());
        assert!(storage.is_fallback);
        assert_ne!(storage.path, primary);
    }
}