//! Integration smoke tests over the crate's public surface.
//!
//! These tests double as the explicit test target the build script needs for
//! `cargo:rustc-link-arg-tests` (the Common-Controls v6 manifest embedding),
//! which cargo only accepts when the package declares a test target.

use app_lib::paths::resolve_models_dir;
use app_lib::state::AppState;

#[test]
fn resolve_models_dir_prefers_exe_dir_when_writable() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let storage = resolve_models_dir(tmp.path());
    assert!(!storage.is_fallback);
    assert_eq!(storage.path, tmp.path().join("models"));
}

#[test]
fn app_state_event_roundtrip() {
    let state = AppState::default();
    state.events.send("download-progress:42".to_string()).expect("send");
    assert_eq!(state.events.recv().expect("recv"), "download-progress:42");
}