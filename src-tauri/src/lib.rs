//! Application wiring for the Piper TTS Reader.
//!
//! The change defines a core state layer (registry handle, settings, event
//! channel) that later slices build the catalog, library, and download
//! subsystems on top of. Only the pieces required by this slice are stubbed
//! here; the real registry is introduced in a later task.

pub mod catalog;
pub mod paths;
pub mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![state::emit_event])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}