//! Shared application state and event channel wiring.
//!
//! `AppState` is created once per window and stored as managed state.
//! `settings` holds user-facing configuration; `catalog` the embedded voice
//! catalog; `library` the installed-model library with its persistent
//! registry; `EventChannel` provides a thread-safe one-shot event send/receive
//! used to exercise the plumbing that Tauri's emitter will drive later.

use std::sync::{mpsc, Mutex};

use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::library::Library;
use crate::paths;

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
        }
    }
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
}