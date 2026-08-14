//! Application wiring for the Piper TTS Reader.
//!
//! The change defines the core state layer (settings, event channel, embedded
//! catalog) and the library subsystem (persistent registry, startup
//! installed-model detection, streaming downloads with progress/md5/cancel).
//! Later slices build text import, synthesis, estimation, the frontend views,
//! and the piper sidecar on top of these.

pub mod catalog;
pub mod download;
pub mod library;
pub mod paths;
pub mod registry;
pub mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = state::AppState::default();
    // Startup location-indicator event (REQ-LIB-7): active storage path and
    // fallback state travel through the event channel for the frontend.
    let _ = app_state.library.emit_location_indicator(&app_state.events);
    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            state::emit_event,
            state::get_proxy,
            state::set_proxy,
            catalog::catalog_languages,
            catalog::catalog_voices
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}