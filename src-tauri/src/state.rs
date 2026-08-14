//! Shared application state and event channel wiring.
//!
//! `AppState` is created once per window and stored as managed state.
//! `settings` holds user-facing configuration; `catalog` the embedded voice
//! catalog; `library` the installed-model library with its persistent
//! registry; `EventChannel` provides a thread-safe one-shot event send/receive
//! used to exercise the plumbing that Tauri's emitter will drive later.

use std::sync::{mpsc, Mutex};

use crate::catalog::Catalog;
use crate::library::Library;
use crate::paths;

/// User-facing settings. Grows with later slices.
#[derive(Debug, Default)]
pub struct Settings {
    pub last_voice: Option<String>,
    pub autoplay: bool,
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
}

impl Default for AppState {
    fn default() -> Self {
        let storage = paths::models_dir();
        Self {
            settings: Settings::default(),
            events: EventChannel::default(),
            catalog: Catalog::load(),
            library: Library::load(storage.path, storage.is_fallback),
        }
    }
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
    fn event_channel_send_receive_roundtrip() {
        let state = AppState::default();
        let payload = "download-progress:42".to_string();
        state.events.send(payload.clone()).expect("send");
        assert_eq!(state.events.recv().expect("recv"), payload);
    }
}