//! Text import commands for the "Load file" frontend action: PDF text
//! extraction and plain-text reads. The native dialog returns a real path, so
//! both commands operate on filesystem paths rather than WebView-uploaded bytes.

use std::path::PathBuf;

/// Frontend command: extract text from a PDF file at `path`.
#[tauri::command]
pub fn extract_pdf_text(path: String) -> Result<String, String> {
    let path = PathBuf::from(&path);
    if !path.is_file() {
        return Err(format!("File not found: {}", path.display()));
    }
    let text = pdf_extract::extract_text(&path)
        .map_err(|e| format!("Could not extract PDF text: {e}"))?;
    Ok(text)
}

/// Frontend command: read a plain-text file (e.g. .txt) at `path`.
#[tauri::command]
pub fn read_text_file(path: String) -> Result<String, String> {
    let path = PathBuf::from(&path);
    std::fs::read_to_string(&path).map_err(|e| format!("Could not read file: {e}"))
}