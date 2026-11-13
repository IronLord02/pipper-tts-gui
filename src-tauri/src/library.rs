//! Startup installed-model detection and storage resolution (REQ-LIB-1,
//! REQ-LIB-7).
//!
//! On startup the library loads the persisted registry (T-06) and exposes
//! which catalog voices are already installed so they are never re-downloaded
//! (REQ-LIB-1). The active storage location comes from `paths.rs` (exe-dir
//! primary, `%APPDATA%` fallback) and is surfaced through a location-indicator
//! event so the UI can show the active path and fallback state (REQ-LIB-7,
//! design D5 / F4).

use std::path::PathBuf;

use crate::registry::Registry;
use crate::state::EventChannel;

/// The installed-model library: registry plus resolved storage location.
#[derive(Debug)]
pub struct Library {
    registry: Registry,
    models_dir: PathBuf,
    is_fallback: bool,
}

impl Library {
    /// Load the library for a storage location. Reads the registry from
    /// `<models_dir>/library.json` (missing or corrupt -> empty registry).
    pub fn load(models_dir: PathBuf, is_fallback: bool) -> Self {
        let registry = Registry::load(&models_dir.join("library.json"));
        Self {
            registry,
            models_dir,
            is_fallback,
        }
    }

    /// The registry backing this library.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Active models directory (primary or fallback).
    pub fn models_dir(&self) -> &PathBuf {
        &self.models_dir
    }

    /// Whether the `%APPDATA%` fallback location is active (REQ-LIB-7).
    pub fn is_fallback(&self) -> bool {
        self.is_fallback
    }

    /// Path of the registry file on disk.
    pub fn registry_path(&self) -> PathBuf {
        self.models_dir.join("library.json")
    }

    /// Ids of all installed models, sorted.
    pub fn installed_ids(&self) -> Vec<String> {
        self.registry.ids()
    }

    /// Whether a model is already installed (REQ-LIB-1: installed models are
    /// shown as installed and never re-downloaded).
    pub fn is_installed(&self, id: &str) -> bool {
        self.registry.contains(id)
    }

    /// Location-indicator event payload: active storage path and whether the
    /// fallback is in use (REQ-LIB-7).
    pub fn location_indicator(&self) -> String {
        format!(
            "location:{}:{}",
            if self.is_fallback { "fallback" } else { "primary" },
            self.models_dir.display()
        )
    }

    /// Emit the location indicator through the app event channel (startup
    /// wiring; the Tauri event emitter lands with the events task).
    pub fn emit_location_indicator(&self, events: &EventChannel) -> Result<(), String> {
        events.send(self.location_indicator())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_model_is_detected_installed() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let models_dir = tmp.path().join("models");

        // A catalog voice recorded in the registry must be reported installed
        // (REQ-LIB-1: shown installed, not re-downloaded on restart).
        let catalog = crate::catalog::Catalog::load();
        let id = "en_US-lessac-medium";
        assert!(catalog.voice(id).is_some(), "fixture voice must exist in catalog");

        let mut registry = Registry::default();
        registry.insert(crate::registry::RegistryEntry {
            id: id.to_string(),
            path: models_dir.join("en_US-lessac-medium.onnx"),
            md5: "a3f1c9e2b4d5467f8a0c1d2e3f4b5a67".to_string(),
            version: "1.0".to_string(),
            speaker_count: 1,
        });
        registry.save(&models_dir.join("library.json")).expect("save");

        let library = Library::load(models_dir, false);
        assert!(library.is_installed(id));
        assert!(library.installed_ids().contains(&id.to_string()));
        assert!(!library.is_installed("en_GB-cori-high"));
    }

    #[test]
    fn empty_registry_reports_nothing_installed() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let library = Library::load(tmp.path().join("models"), false);
        assert!(library.installed_ids().is_empty());
    }

    #[test]
    fn fallback_location_indicator_is_emitted() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let fallback_dir = tmp.path().join("appdata").join("piper-tts-gui").join("models");
        let library = Library::load(fallback_dir.clone(), true);
        let events = EventChannel::default();
        library.emit_location_indicator(&events).expect("emit");

        let payload = events.recv().expect("recv");
        assert_eq!(
            payload,
            format!("location:fallback:{}", fallback_dir.display())
        );
        assert!(payload.starts_with("location:fallback:"));
        assert!(payload.contains(&fallback_dir.display().to_string()));
    }

    #[test]
    fn primary_location_indicator_marks_primary() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let library = Library::load(tmp.path().join("models"), false);
        let payload = library.location_indicator();
        assert!(payload.starts_with("location:primary:"));
        assert!(payload.contains(&tmp.path().join("models").display().to_string()));
    }
}