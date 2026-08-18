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
pub mod mp3;
pub mod paths;
pub mod pdf;
pub mod queue;
pub mod registry;
pub mod state;
pub mod synth;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = state::AppState::default();
    // Startup location-indicator event (REQ-LIB-7): active storage path and
    // fallback state travel through the event channel for the frontend.
    let _ = app_state.library.emit_location_indicator(&app_state.events);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            state::emit_event,
            state::get_models_dir,
            state::set_models_dir,
            state::reset_models_dir,
            state::get_output_dir,
            state::set_output_dir,
            state::reset_output_dir,
            state::get_mp3_auto_convert,
            state::set_mp3_auto_convert,
            catalog::catalog_languages,
            catalog::catalog_voices,
            pdf::extract_pdf_text,
            pdf::read_text_file,
            queue::queue_add_documents,
            queue::queue_add_text,
            queue::queue_remove,
            queue::queue_set_output_name,
            queue::queue_start,
            queue::queue_stop,
            queue::queue_state,
            mp3::convert_wav_to_mp3,
            synth::cancel_synthesis,
            synth::estimate_duration,
            synth::list_installed_voices,
            synth::synthesize
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}