//! Shared application state and event channel wiring.
//!
//! `AppState` is created once per window and stored as managed state.
//! `settings` holds user-facing configuration; `catalog` the embedded voice
//! catalog; `library` the installed-model library with its persistent
//! registry; `EventChannel` provides a thread-safe one-shot event send/receive
//! used to exercise the plumbing that Tauri's emitter will drive later.

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::catalog::Catalog;
use crate::library::Library;
use crate::paths;

/// Name of the persisted settings file (next to the executable, appdata
/// fallback). Mirrors the exe-dir resolution pattern of `synth::piper_runtime_dir`.
const SETTINGS_FILE: &str = "piper-tts-settings.json";

/// User-facing settings. Grows with later slices.
#[derive(Debug, Default)]
pub struct Settings {
    pub last_voice: Option<String>,
    pub autoplay: bool,
}

/// How model downloads reach the network (REQ-LIB-3 runtime setting).
///
/// - `system`: let reqwest honor the environment / OS proxy configuration
///   (`HTTP_PROXY`, `HTTPS_PROXY`, etc.). Good for networks where the proxy
///   is already configured and reachable.
/// - `none`: connect directly, bypassing any proxy. This is the default and
///   matches the verified direct-download behavior.
/// - `manual`: use a specific proxy host and port (e.g. a corporate proxy
///   the app cannot discover on its own).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    None,
    System,
    Manual { host: String, port: u16 },
}

/// A thin wrapper over an mpsc channel used to verify event wiring.
#[derive(Debug)]
pub struct EventChannel {
    tx: Mutex<mpsc::Sender<String>>,
    rx: Mutex<mpsc::Receiver<String>>,
}

impl Default for EventChannel {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx: Mutex::new(tx),
            rx: Mutex::new(rx),
        }
    }
}

impl EventChannel {
    /// Send an event name through the channel. Non-blocking.
    pub fn send(&self, event: impl Into<String>) -> Result<(), String> {
        self.tx
            .lock()
            .map_err(|_| "event channel send lock poisoned".to_string())?
            .send(event.into())
            .map_err(|_| "event channel receiver dropped".to_string())
    }

    /// Receive one event name, blocking until one is available.
    pub fn recv(&self) -> Result<String, String> {
        self.rx
            .lock()
            .map_err(|_| "event channel recv lock poisoned".to_string())?
            .recv()
            .map_err(|_| "event channel sender dropped".to_string())
    }
}

/// Lifecycle of a queued item (see the `queue` module).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    Pending,
    Working,
    Done,
    Error,
}

/// One queued item (a PDF file or pasted text).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct QueueItem {
    /// Session-unique identifier (monotonic counter).
    pub id: String,
    /// Sidebar label: PDF file name without the `.pdf` extension, or
    /// "Texto pegado N" for pasted text.
    pub title: String,
    /// Output WAV base name without the `.wav` extension, editable by the
    /// user. Defaults to the PDF file stem (PDF items) or the title
    /// (text items). Always populated at creation.
    pub output_name: String,
    /// Absolute path of the source PDF; `None` for text items.
    pub pdf_path: Option<String>,
    /// Pasted text payload; `None` for PDF items whose text is extracted at
    /// run time.
    pub text: Option<String>,
    /// Current lifecycle state.
    pub status: QueueStatus,
    /// Failure message; `None` when the item never failed.
    pub error: Option<String>,
    /// Output WAV written next to the PDF (or in the default output dir for
    /// text items). Filled on success, then the item is removed from the list.
    pub wav_path: Option<String>,
    /// Output MP3 produced by the automatic post-synthesis conversion (same
    /// folder as the WAV, same base name). `None` when the user did not enable
    /// the MP3 auto-convert checkbox, or when the conversion failed.
    pub mp3_path: Option<String>,
    /// Real audio duration in seconds (filled on success).
    pub audio_secs: Option<f64>,
}

/// Session-only queue of PDF files and pasted text. Never persisted to disk.
#[derive(Debug, Default)]
pub struct QueueState {
    pub items: Vec<QueueItem>,
    /// `true` while the sequential loop in `queue_start` is running.
    pub running: bool,
    /// Token cancelled by `queue_stop`; the in-flight item aborts.
    pub cancel: Option<CancellationToken>,
    /// Monotonic id source for newly added items.
    pub next_id: u64,
    /// Monotonic title counter for pasted-text items ("Texto pegado N").
    pub next_text_id: u64,
    /// Item count when the current run started (for the `n/total` summary).
    pub run_total: usize,
    /// Items finished (done or failed) in the current run.
    pub run_completed: usize,
}

/// Shared managed state for the application.
#[derive(Debug)]
pub struct AppState {
    pub settings: Settings,
    pub events: EventChannel,
    pub catalog: Catalog,
    pub library: Library,
    /// Proxy selection for model downloads. Wrapped in a mutex because the
    /// frontend can change it at runtime while downloads read it.
    pub proxy: Mutex<ProxyMode>,
    /// User-chosen models folder (e.g. where the user keeps his voice models).
    /// `None` means the bundled default from `paths::models_dir()` is active.
    /// Persisted across sessions via the settings file.
    pub models_dir_override: Mutex<Option<PathBuf>>,
    /// User-chosen global output folder for queue synthesis. `None` keeps the
    /// per-item defaults (next to the PDF, or `<exe>/output` for text items).
    /// Persisted across sessions via the settings file.
    pub output_dir_override: Mutex<Option<PathBuf>>,
    /// When `true`, every finished synthesis (queue items and the direct
    /// convert panel) is automatically converted to MP3 (128 kbps) right after
    /// the WAV is written. Session-only, default off; the frontend exposes a
    /// checkbox.
    pub mp3_auto_convert: Mutex<bool>,
    /// Token for the currently running synthesis; cancelled by `cancel_synthesis`.
    pub synthesis_cancel: std::sync::Mutex<Option<tokio_util::sync::CancellationToken>>,
    /// Session-only queue of PDF files and pasted text (see the `queue` module).
    pub queue: Mutex<QueueState>,
    /// Serializes all synthesis work (convert panel + queue) so two
    /// piper runs never overlap. Set by `synth::synthesize` and `queue_start`.
    pub synthesis_busy: Mutex<bool>,
}

impl Default for AppState {
    fn default() -> Self {
        let storage = paths::models_dir();
        Self {
            settings: Settings::default(),
            events: EventChannel::default(),
            catalog: Catalog::load(),
            library: Library::load(storage.path, storage.is_fallback),
            proxy: Mutex::new(ProxyMode::default()),
            models_dir_override: Mutex::new(load_models_dir_override()),
            output_dir_override: Mutex::new(load_output_dir_override()),
            mp3_auto_convert: Mutex::new(false),
            synthesis_cancel: Mutex::new(None),
            queue: Mutex::new(QueueState::default()),
            synthesis_busy: Mutex::new(false),
        }
    }
}

/// Active models directory: the user-chosen override when set, otherwise the
/// bundled default from `paths`.
pub fn models_dir(state: &AppState) -> PathBuf {
    state
        .models_dir_override
        .lock()
        .ok()
        .and_then(|override_dir| override_dir.clone())
        .unwrap_or_else(|| paths::models_dir().path)
}

/// The user-chosen global output folder, if any. `None` means the per-item
/// defaults are active (PDFs next to the PDF, text in `<exe>/output`).
pub fn output_dir_override(state: &AppState) -> Option<PathBuf> {
    state
        .output_dir_override
        .lock()
        .ok()
        .and_then(|override_dir| override_dir.clone())
}

/// Active output directory: the user-chosen global folder when set, otherwise
/// the app's default output directory.
pub fn active_output_dir(state: &AppState) -> PathBuf {
    output_dir_override(state).unwrap_or_else(crate::synth::default_output_dir)
}

/// Read the current proxy mode (frontend settings panel).
#[tauri::command]
pub fn get_proxy(state: tauri::State<'_, AppState>) -> Result<ProxyMode, String> {
    state
        .proxy
        .lock()
        .map(|proxy| proxy.clone())
        .map_err(|_| "proxy lock poisoned".to_string())
}

/// Update the proxy mode (frontend settings panel).
#[tauri::command]
pub fn set_proxy(state: tauri::State<'_, AppState>, mode: ProxyMode) -> Result<(), String> {
    let mut proxy = state
        .proxy
        .lock()
        .map_err(|_| "proxy lock poisoned".to_string())?;
    *proxy = mode;
    Ok(())
}

/// Command exposed to the frontend that forwards a generic event through the
/// channel. Exercises the wiring end to end (frontend -> command -> channel).
#[tauri::command]
pub fn emit_event(state: tauri::State<'_, AppState>, event: String) -> Result<(), String> {
    state.events.send(event)
}

/// Path of the persisted settings file: `<exe_dir>/piper-tts-settings.json`,
/// falling back to `<data_dir>/piper-tts-gui/piper-tts-settings.json` when the
/// exe dir is not writable or cannot be resolved (the `paths` probe detects it
/// the same way as the models directory).
pub fn settings_file_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    if let Some(dir) = exe_dir.as_ref() {
        if paths::probe_writable(dir) {
            return dir.join(SETTINGS_FILE);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("piper-tts-gui")
        .join(SETTINGS_FILE)
}

/// Persisted user settings. Keys are additive and optional: missing or `null`
/// values mean "use the default", so older settings files keep working.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedSettings {
    #[serde(default)]
    models_dir: Option<String>,
    #[serde(default)]
    output_dir: Option<String>,
}

/// Read the persisted settings from a file. Missing or corrupt files (or a
/// `null`/absent value) produce the default settings.
fn read_settings(file: &Path) -> PersistedSettings {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Write the persisted settings to a file, creating the parent directory.
fn write_settings(file: &Path, settings: &PersistedSettings) -> Result<(), String> {
    let dir = file
        .parent()
        .ok_or_else(|| format!("settings path {} has no parent", file.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("create {}: {error}", dir.display()))?;
    let json = serde_json::to_string(settings)
        .map_err(|error| format!("serialize settings: {error}"))?;
    std::fs::write(file, json)
        .map_err(|error| format!("write {}: {error}", file.display()))
}

/// Read one persisted directory setting (`models_dir` or `output_dir`).
fn load_dir_setting_from(file: &Path, key: &str) -> Option<PathBuf> {
    let settings = read_settings(file);
    let value = match key {
        "models_dir" => settings.models_dir,
        "output_dir" => settings.output_dir,
        _ => return None,
    };
    value.filter(|path| !path.is_empty()).map(PathBuf::from)
}

/// Write one persisted directory setting (`models_dir` or `output_dir`).
/// `None` persists a `null` value (the default is restored on next load).
fn save_dir_setting_to(file: &Path, key: &str, path: Option<&Path>) -> Result<(), String> {
    let mut settings = read_settings(file);
    let value = path.map(|p| p.to_string_lossy().into_owned());
    match key {
        "models_dir" => settings.models_dir = value,
        "output_dir" => settings.output_dir = value,
        _ => return Err(format!("unknown settings key '{key}'")),
    }
    write_settings(file, &settings)
}

/// Load the persisted models-dir override, if any.
pub fn load_models_dir_override() -> Option<PathBuf> {
    load_dir_setting_from(&settings_file_path(), "models_dir")
}

/// Persist the models-dir override (`None` clears it).
pub fn save_models_dir_override(path: Option<&Path>) -> Result<(), String> {
    save_dir_setting_to(&settings_file_path(), "models_dir", path)
}

/// Load the persisted global output-folder override, if any.
pub fn load_output_dir_override() -> Option<PathBuf> {
    load_dir_setting_from(&settings_file_path(), "output_dir")
}

/// Persist the global output-folder override (`None` clears it).
pub fn save_output_dir_override(path: Option<&Path>) -> Result<(), String> {
    save_dir_setting_to(&settings_file_path(), "output_dir", path)
}

/// Set the in-memory override and persist it to the given settings file.
/// Tests pass a temp file so nothing real is written.
fn apply_models_dir_override(
    state: &AppState,
    path: Option<PathBuf>,
    settings_file: &Path,
) -> Result<(), String> {
    save_dir_setting_to(settings_file, "models_dir", path.as_deref())?;
    let mut guard = state
        .models_dir_override
        .lock()
        .map_err(|_| "models dir override lock poisoned".to_string())?;
    *guard = path;
    Ok(())
}

/// Set the in-memory global output-folder override and persist it.
fn apply_output_dir_override(
    state: &AppState,
    path: Option<PathBuf>,
    settings_file: &Path,
) -> Result<(), String> {
    save_dir_setting_to(settings_file, "output_dir", path.as_deref())?;
    let mut guard = state
        .output_dir_override
        .lock()
        .map_err(|_| "output dir override lock poisoned".to_string())?;
    *guard = path;
    Ok(())
}

/// Active models directory path (frontend label + scan target).
#[tauri::command]
pub fn get_models_dir(state: tauri::State<'_, AppState>) -> Result<String, String> {
    Ok(models_dir(&state).to_string_lossy().into_owned())
}

/// Point the app at a user-chosen models folder and remember it.
#[tauri::command]
pub fn set_models_dir(state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("Models folder path is empty.".to_string());
    }
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("'{path}' is not an existing directory."));
    }
    apply_models_dir_override(&state, Some(dir), &settings_file_path())
}

/// Forget the user-chosen folder and restore the bundled default.
#[tauri::command]
pub fn reset_models_dir(state: tauri::State<'_, AppState>) -> Result<(), String> {
    apply_models_dir_override(&state, None, &settings_file_path())
}

/// Active output directory path (frontend label + where queue files land).
#[tauri::command]
pub fn get_output_dir(state: tauri::State<'_, AppState>) -> Result<String, String> {
    Ok(active_output_dir(&state).to_string_lossy().into_owned())
}

/// Point every queue output at a user-chosen folder and remember it.
#[tauri::command]
pub fn set_output_dir(state: tauri::State<'_, AppState>, path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("Output folder path is empty.".to_string());
    }
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("'{path}' is not an existing directory."));
    }
    apply_output_dir_override(&state, Some(dir), &settings_file_path())
}

/// Forget the user-chosen folder and restore per-item defaults.
#[tauri::command]
pub fn reset_output_dir(state: tauri::State<'_, AppState>) -> Result<(), String> {
    apply_output_dir_override(&state, None, &settings_file_path())
}

/// Whether finished files are auto-converted to MP3 (frontend checkbox).
#[tauri::command]
pub fn get_mp3_auto_convert(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    state
        .mp3_auto_convert
        .lock()
        .map(|enabled| *enabled)
        .map_err(|_| "mp3 auto-convert lock poisoned".to_string())
}

/// Toggle the MP3 auto-conversion flag (frontend checkbox).
#[tauri::command]
pub fn set_mp3_auto_convert(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let mut flag = state
        .mp3_auto_convert
        .lock()
        .map_err(|_| "mp3 auto-convert lock poisoned".to_string())?;
    *flag = enabled;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_constructs_with_defaults() {
        let state = AppState::default();
        assert_eq!(state.settings.last_voice, None);
        assert!(!state.settings.autoplay);
        // No models recorded anywhere yet -> nothing installed.
        assert!(state.library.installed_ids().is_empty());
    }

    #[test]
    fn proxy_defaults_to_none_and_can_be_updated() {
        let state = AppState::default();
        assert_eq!(*state.proxy.lock().expect("lock"), ProxyMode::None);

        *state.proxy.lock().expect("lock") = ProxyMode::Manual {
            host: "172.16.21.3".to_string(),
            port: 3128,
        };
        assert_eq!(
            *state.proxy.lock().expect("lock"),
            ProxyMode::Manual {
                host: "172.16.21.3".to_string(),
                port: 3128,
            }
        );
    }

    #[test]
    fn proxy_mode_serde_roundtrip() {
        let manual = ProxyMode::Manual {
            host: "10.0.0.1".to_string(),
            port: 8080,
        };
        let json = serde_json::to_string(&manual).expect("serialize");
        assert_eq!(json, r#"{"mode":"manual","host":"10.0.0.1","port":8080}"#);
        let back: ProxyMode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, manual);

        assert_eq!(
            serde_json::to_string(&ProxyMode::None).expect("serialize"),
            r#"{"mode":"none"}"#
        );
        assert_eq!(
            serde_json::to_string(&ProxyMode::System).expect("serialize"),
            r#"{"mode":"system"}"#
        );
    }

    #[test]
    fn event_channel_send_receive_roundtrip() {
        let state = AppState::default();
        let payload = "download-progress:42".to_string();
        state.events.send(payload.clone()).expect("send");
        assert_eq!(state.events.recv().expect("recv"), payload);
    }

    #[test]
    fn default_has_no_models_dir_override() {
        let state = AppState::default();
        // Deterministic: force the override to None so the test does not depend
        // on whether a settings file exists next to the test binary.
        *state.models_dir_override.lock().expect("lock") = None;
        assert!(state.models_dir_override.lock().expect("lock").is_none());
        assert_eq!(models_dir(&state), paths::models_dir().path);
    }

    #[test]
    fn default_has_no_output_dir_override() {
        let state = AppState::default();
        // Deterministic: force the override to None so the test does not depend
        // on whether a settings file exists next to the test binary.
        *state.output_dir_override.lock().expect("lock") = None;
        assert!(state.output_dir_override.lock().expect("lock").is_none());
        assert_eq!(
            active_output_dir(&state),
            crate::synth::default_output_dir()
        );
    }

    #[test]
    fn set_and_reset_override_roundtrip() {
        let state = AppState::default();
        // Deterministic start: force no override (see default_has_no_models_dir_override).
        *state.models_dir_override.lock().expect("lock") = None;
        let tmp = tempfile::tempdir().expect("temp dir");
        let models = tmp.path().join("my-models");
        std::fs::create_dir_all(&models).expect("create models dir");
        let settings_file = tmp.path().join(SETTINGS_FILE);

        assert_eq!(models_dir(&state), paths::models_dir().path);

        apply_models_dir_override(&state, Some(models.clone()), &settings_file).expect("set");
        assert_eq!(models_dir(&state), models);
        assert_eq!(
            load_dir_setting_from(&settings_file, "models_dir"),
            Some(models)
        );

        apply_models_dir_override(&state, None, &settings_file).expect("reset");
        assert_eq!(models_dir(&state), paths::models_dir().path);
        assert_eq!(load_dir_setting_from(&settings_file, "models_dir"), None);
    }

    #[test]
    fn output_dir_override_persists_and_clears() {
        let state = AppState::default();
        *state.output_dir_override.lock().expect("lock") = None;
        let tmp = tempfile::tempdir().expect("temp dir");
        let out = tmp.path().join("my-audiobooks");
        std::fs::create_dir_all(&out).expect("create out dir");
        let settings_file = tmp.path().join(SETTINGS_FILE);

        assert_eq!(output_dir_override(&state), None);

        apply_output_dir_override(&state, Some(out.clone()), &settings_file).expect("set");
        assert_eq!(output_dir_override(&state), Some(out.clone()));
        assert_eq!(active_output_dir(&state), out);
        assert_eq!(
            load_dir_setting_from(&settings_file, "output_dir"),
            Some(out)
        );

        apply_output_dir_override(&state, None, &settings_file).expect("reset");
        assert_eq!(output_dir_override(&state), None);
        assert_eq!(
            active_output_dir(&state),
            crate::synth::default_output_dir()
        );
        assert_eq!(load_dir_setting_from(&settings_file, "output_dir"), None);
    }

    #[test]
    fn persistence_loads_and_saves_roundtrip() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let settings_file = tmp.path().join(SETTINGS_FILE);
        let models = tmp.path().join("models-dir");

        save_dir_setting_to(&settings_file, "models_dir", Some(&models)).expect("save");
        assert_eq!(
            load_dir_setting_from(&settings_file, "models_dir"),
            Some(models)
        );

        // Clearing writes `{"models_dir": null}` and reads back as `None`.
        save_dir_setting_to(&settings_file, "models_dir", None).expect("clear");
        assert!(settings_file.is_file());
        assert_eq!(load_dir_setting_from(&settings_file, "models_dir"), None);
    }

    #[test]
    fn persistence_ignores_corrupt_or_missing_file() {
        let tmp = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            load_dir_setting_from(&tmp.path().join("missing.json"), "output_dir"),
            None
        );

        let settings_file = tmp.path().join(SETTINGS_FILE);
        std::fs::write(&settings_file, b"this is not json {").expect("write corrupt");
        assert_eq!(
            load_dir_setting_from(&settings_file, "models_dir"),
            None
        );
    }

    #[test]
    fn settings_keys_persist_independently() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let settings_file = tmp.path().join(SETTINGS_FILE);
        let models = tmp.path().join("models-dir");
        let out = tmp.path().join("out-dir");

        // Saving one key must not wipe the other.
        save_dir_setting_to(&settings_file, "models_dir", Some(&models)).expect("save models");
        save_dir_setting_to(&settings_file, "output_dir", Some(&out)).expect("save output");
        assert_eq!(
            load_dir_setting_from(&settings_file, "models_dir"),
            Some(models)
        );
        assert_eq!(
            load_dir_setting_from(&settings_file, "output_dir"),
            Some(out.clone())
        );

        // Clearing one key keeps the other intact.
        save_dir_setting_to(&settings_file, "models_dir", None).expect("clear models");
        assert_eq!(load_dir_setting_from(&settings_file, "models_dir"), None);
        assert_eq!(load_dir_setting_from(&settings_file, "output_dir"), Some(out));
    }

    #[test]
    fn unknown_settings_key_is_rejected() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let settings_file = tmp.path().join(SETTINGS_FILE);
        assert!(save_dir_setting_to(&settings_file, "bogus", None).is_err());
    }
}