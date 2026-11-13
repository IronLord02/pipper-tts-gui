//! Synthesis queue: sequential, one-at-a-time synthesis of multiple items.
//!
//! Items are PDF files added through a multi-select dialog and pasted text
//! added from the convert panel. They are processed strictly one after
//! another: the active item is marked `Working`, a WAV is written next to the
//! PDF (same folder, `<output-name>.wav`), into the app's default output
//! directory for text items, or into the user-chosen global output folder for
//! every item when one is set (`state::output_dir_override`), and on success
//! the item is marked `Done` (it stays visible; the frontend hides it after a
//! short delay). A failing item is marked `Error` and the loop continues with
//! the next pending item — one failure never stops the whole queue. Stopping
//! cancels the in-flight item via the same `CancellationToken` its synthesis
//! races against.
//!
//! Concurrency: the whole run is serialized against the convert panel through
//! the shared `AppState::synthesis_busy` flag. `queue_start` refuses to start
//! while a convert-panel synthesis is running (and vice versa), so two piper
//! jobs never overlap.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::Emitter;
use tokio_util::sync::CancellationToken;

use crate::mp3;
use crate::pdf;
use crate::state::{AppState, QueueItem, QueueState, QueueStatus};
use crate::synth;

/// Progress callback shape shared with `synth::synthesize_to_path`: invoked as
/// `(done, total)` after every synthesized sentence.
type ProgressFn = dyn Fn(usize, usize) -> Result<(), String> + Send + Sync;

/// Characters that Windows refuses in a file name.
const WINDOWS_INVALID_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Full snapshot payload carried by the `queue-updated` event and returned by
/// `queue_state`. The frontend re-renders the sidebar from this single shape.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct QueueSnapshot {
    pub running: bool,
    /// `true` on the final update of a run (queue finished or was stopped).
    pub finished: bool,
    /// Items finished (done or failed) in the current run.
    pub completed: usize,
    /// Item count when the current run started.
    pub total: usize,
    pub items: Vec<QueueItem>,
}

fn snapshot_of(queue: &QueueState, finished: bool) -> QueueSnapshot {
    QueueSnapshot {
        running: queue.running,
        finished,
        completed: queue.run_completed,
        total: queue.run_total,
        items: queue.items.clone(),
    }
}

fn emit_queue_update(app: &tauri::AppHandle, snapshot: &QueueSnapshot) -> Result<(), String> {
    app.emit("queue-updated", snapshot)
        .map_err(|error| format!("failed to emit queue-updated: {error}"))
}

/// Derive the output path for a PDF item: same directory as the PDF, filename
/// `<output-name>.wav`.
fn output_for(pdf_path: &str, output_name: &str) -> PathBuf {
    let path = PathBuf::from(pdf_path);
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    parent.join(format!("{output_name}.wav"))
}

/// Resolve where a queue item's WAV is written: the user-chosen global output
/// folder when one is set (both item kinds land there), otherwise the per-item
/// default (next to the PDF for PDF items, `<exe>/output` for text items).
fn resolve_queue_output_path(state: &AppState, item: &QueueItem) -> Result<PathBuf, String> {
    if let Some(dir) = crate::state::output_dir_override(state) {
        return synth::output_path_in_dir(&dir, &item.output_name);
    }
    if let Some(pdf_path) = item.pdf_path.clone() {
        return Ok(output_for(&pdf_path, &item.output_name));
    }
    synth::output_path_in_dir(&synth::default_output_dir(), &item.output_name)
}

/// Trim and clean a user-chosen output name: strip a trailing `.wav`
/// (case-insensitive), then reject characters invalid on Windows file names.
/// Returns the cleaned base name (without extension).
fn clean_output_name(name: &str) -> Result<String, String> {
    let mut cleaned = name.trim().to_string();
    if cleaned.is_empty() {
        return Err("The output name cannot be empty.".to_string());
    }
    if cleaned.to_ascii_lowercase().ends_with(".wav") {
        cleaned.truncate(cleaned.len() - 4);
        cleaned = cleaned.trim().to_string();
    }
    if cleaned.is_empty() {
        return Err("The output name cannot be empty.".to_string());
    }
    if let Some(bad) = cleaned
        .chars()
        .find(|ch| WINDOWS_INVALID_CHARS.contains(ch))
    {
        return Err(format!("'{bad}' is not allowed in a file name."));
    }
    Ok(cleaned)
}

/// Validate and queue one or more PDF paths. All paths must be existing `.pdf`
/// files; the whole call fails atomically on the first invalid path.
#[tauri::command]
pub fn queue_add_documents(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<QueueItem>, String> {
    let mut queue = state
        .queue
        .lock()
        .map_err(|_| "queue lock poisoned".to_string())?;
    if queue.running {
        return Err("The queue is running. Stop it before adding files.".to_string());
    }

    let mut added = Vec::with_capacity(paths.len());
    for path in paths {
        let pdf_path = PathBuf::from(&path);
        let is_pdf = pdf_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false);
        if !is_pdf {
            return Err(format!("{} is not a PDF file.", path));
        }
        if !pdf_path.is_file() {
            return Err(format!("File not found: {}", pdf_path.display()));
        }

        queue.next_id += 1;
        let stem = pdf_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("file")
            .to_string();
        let item = QueueItem {
            id: format!("q{}", queue.next_id),
            title: stem.clone(),
            output_name: stem,
            pdf_path: Some(path.clone()),
            text: None,
            status: QueueStatus::Pending,
            error: None,
            wav_path: None,
            mp3_path: None,
            audio_secs: None,
        };
        added.push(item.clone());
        queue.items.push(item);
    }

    let snapshot = snapshot_of(&queue, false);
    drop(queue);
    emit_queue_update(&app, &snapshot)?;
    Ok(added)
}

/// Queue a pasted text payload as a text item. The text is not synthesized
/// now: it accumulates in the queue and is spoken when the run starts.
#[tauri::command]
pub fn queue_add_text(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<Vec<QueueItem>, String> {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return Err("No text to add. Type or paste some text first.".to_string());
    }

    let mut queue = state
        .queue
        .lock()
        .map_err(|_| "queue lock poisoned".to_string())?;
    if queue.running {
        return Err("The queue is running. Stop it before adding text.".to_string());
    }

    queue.next_id += 1;
    queue.next_text_id += 1;
    let title = format!("Texto pegado {}", queue.next_text_id);
    let item = QueueItem {
        id: format!("q{}", queue.next_id),
        title: title.clone(),
        output_name: title,
        pdf_path: None,
        text: Some(trimmed),
        status: QueueStatus::Pending,
        error: None,
        wav_path: None,
        mp3_path: None,
        audio_secs: None,
    };

    let added = vec![item.clone()];
    queue.items.push(item);

    let snapshot = snapshot_of(&queue, false);
    drop(queue);
    emit_queue_update(&app, &snapshot)?;
    Ok(added)
}

/// Remove an item. `Pending`, `Error`, and finished `Done` items can be
/// removed; a `Working` item is rejected.
#[tauri::command]
pub fn queue_remove(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let mut queue = state
        .queue
        .lock()
        .map_err(|_| "queue lock poisoned".to_string())?;
    if queue.running {
        return Err("The queue is running. Remove files only after stopping it.".to_string());
    }

    let pos = queue
        .items
        .iter()
        .position(|item| item.id == id)
        .ok_or_else(|| "Item not found.".to_string())?;
    match queue.items[pos].status {
        QueueStatus::Pending | QueueStatus::Error | QueueStatus::Done => {
            queue.items.remove(pos);
        }
        QueueStatus::Working => {
            return Err(
                "Only queued, failed, or finished files can be removed while it is processed."
                    .to_string(),
            );
        }
    }

    let snapshot = snapshot_of(&queue, false);
    drop(queue);
    emit_queue_update(&app, &snapshot)?;
    Ok(())
}

/// Rename the output WAV of a queued item. Only allowed while the queue is
/// not running (same guard as `queue_remove`). The name is cleaned (trailing
/// `.wav` stripped, invalid Windows characters rejected) and stored as the
/// base name used when the item is synthesized.
#[tauri::command]
pub fn queue_set_output_name(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    let mut queue = state
        .queue
        .lock()
        .map_err(|_| "queue lock poisoned".to_string())?;
    if queue.running {
        return Err("The queue is running. Change file names only after stopping it.".to_string());
    }

    let cleaned = clean_output_name(&name)?;
    let item = queue
        .items
        .iter_mut()
        .find(|item| item.id == id)
        .ok_or_else(|| "Item not found.".to_string())?;
    item.output_name = cleaned;

    let snapshot = snapshot_of(&queue, false);
    drop(queue);
    emit_queue_update(&app, &snapshot)?;
    Ok(())
}

/// Synthesize one item's text. Over-limit items (more than `MAX_INPUT_CHARS`
/// characters) are batched into sentence-aligned chunks and each chunk is
/// synthesized separately, then merged into a single WAV — nothing is ever
/// silently truncated. `out_path` is always resolved by the caller (next to
/// the PDF, or in the default output directory for text items). Returns the
/// real audio duration.
///
/// `on_progress` forwards per-sentence progress to the queue loop so it can
/// emit the `queue-progress` event; over-limit items report cumulative
/// sentence offsets across every chunk.
async fn synthesize_chapter(
    state: &AppState,
    text: &str,
    out_path: &Path,
    voice_id: Option<&str>,
    token: &CancellationToken,
    on_progress: Option<Arc<ProgressFn>>,
) -> Result<synth::SynthesisResult, String> {
    if token.is_cancelled() {
        return Err("Synthesis cancelled.".to_string());
    }

    if text.chars().count() <= synth::MAX_INPUT_CHARS {
        let boxed = on_progress.as_ref().map(|emit| {
            let emit = emit.clone();
            Box::new(move |done: usize, total: usize| emit(done, total)) as Box<ProgressFn>
        });
        return synth::synthesize_to_path(
            state,
            text,
            Some(&out_path.to_string_lossy()),
            voice_id,
            token,
            boxed,
        )
        .await;
    }

    // Over-limit item: group sentences into chunks that each stay under the
    // per-run cap, synthesize every chunk, and merge the parts. The total
    // sentence count is computed up front so progress stays cumulative.
    let sentences = synth::split_sentences(text);
    let total_sentences = sentences.len();
    let mut chunks: Vec<String> = Vec::new();
    let mut chunk_sentence_counts: Vec<usize> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    let mut current_count = 0usize;
    for sentence in sentences {
        let len = sentence.chars().count();
        if current_len > 0 && current_len + len > synth::MAX_INPUT_CHARS {
            chunks.push(std::mem::take(&mut current));
            chunk_sentence_counts.push(current_count);
            current_len = 0;
            current_count = 0;
        }
        current.push_str(&sentence);
        current_len += len;
        current_count += 1;
        if current_len >= synth::MAX_INPUT_CHARS {
            chunks.push(std::mem::take(&mut current));
            chunk_sentence_counts.push(current_count);
            current_len = 0;
            current_count = 0;
        }
    }
    if current_len > 0 {
        chunks.push(current);
        chunk_sentence_counts.push(current_count);
    }

    let staging = tempfile::tempdir()
        .map_err(|error| format!("Cannot create staging directory: {error}"))?;
    let mut parts = Vec::with_capacity(chunks.len());
    let mut offset = 0usize;
    for (index, chunk) in chunks.iter().enumerate() {
        if token.is_cancelled() {
            return Err("Synthesis cancelled.".to_string());
        }
        let chunk_offset = offset;
        offset += chunk_sentence_counts[index];
        let part = staging.path().join(format!("item-chunk-{index:03}.wav"));
        let boxed = on_progress.as_ref().map(|emit| {
            let emit = emit.clone();
            Box::new(move |done: usize, _total: usize| {
                emit(chunk_offset + done, total_sentences)
            }) as Box<ProgressFn>
        });
        synth::synthesize_to_path(state, chunk, Some(&part.to_string_lossy()), voice_id, token, boxed)
            .await?;
        parts.push(part);
    }

    synth::merge_wav_parts(&parts, out_path)?;

    let wav = std::fs::read(out_path).map_err(|error| {
        format!("Piper exited successfully but the WAV could not be read: {error}")
    })?;
    let audio_secs = synth::parse_wav_duration_secs(&wav);
    let chars = text.chars().count();
    Ok(synth::SynthesisResult {
        wav_path: out_path.to_string_lossy().into_owned(),
        audio_secs,
        estimated_audio_secs: synth::estimate_audio_secs(chars),
        chars,
    })
}

/// Start the sequential queue run. `voice_id` is captured ONCE and used for
/// every item in the run (it cannot be changed mid-run). Picks the next
/// `Pending` item, marks it `Working`, synthesizes a WAV (next to the PDF, or
/// in the default output dir for text items, both using the item's editable
/// output name), marks it `Done` (kept in the list; the frontend hides it
/// after a delay) or `Error` (kept for manual removal), then advances. On
/// `queue_stop` the current item is cancelled and reverts to `Pending` so it
/// can be resumed.
#[tauri::command]
pub async fn queue_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    voice_id: Option<String>,
) -> Result<(), String> {
    // Serialize against the convert panel: only one piper job at a time. The
    // guard is scoped so it is released before any await below.
    {
        let mut busy = state
            .synthesis_busy
            .lock()
            .map_err(|_| "state lock poisoned".to_string())?;
        if *busy {
            return Err(
                "Synthesis is already running (convert panel). Wait for it to finish first."
                    .to_string(),
            );
        }
        *busy = true;
    }

    let token = CancellationToken::new();
    {
        let mut queue = state
            .queue
            .lock()
            .map_err(|_| "queue lock poisoned".to_string())?;
        if queue.running {
            *state.synthesis_busy.lock().unwrap() = false;
            return Err("The queue is already running.".to_string());
        }
        if !queue
            .items
            .iter()
            .any(|item| item.status == QueueStatus::Pending)
        {
            *state.synthesis_busy.lock().unwrap() = false;
            return Err("The queue has no pending files.".to_string());
        }
        queue.running = true;
        queue.cancel = Some(token.clone());
        // Done items from earlier runs stay in the list so the user can export
        // them as MP3 or remove them; they never run again, so they do not
        // count toward this run's total.
        queue.run_total = queue
            .items
            .iter()
            .filter(|item| item.status != QueueStatus::Done)
            .count();
        queue.run_completed = 0;
    }

    loop {
        // Cancellation between items ends the run cleanly.
        if token.is_cancelled() {
            let mut queue = state.queue.lock().unwrap();
            queue.running = false;
            queue.cancel = None;
            let snapshot = snapshot_of(&queue, true);
            drop(queue);
            *state.synthesis_busy.lock().unwrap() = false;
            emit_queue_update(&app, &snapshot)?;
            return Ok(());
        }

        // Pick the next pending item and mark it working.
        let next: QueueItem = {
            let mut queue = state
                .queue
                .lock()
                .map_err(|_| "queue lock poisoned".to_string())?;
            let pos = match queue
                .items
                .iter()
                .position(|item| item.status == QueueStatus::Pending)
            {
                Some(pos) => pos,
                None => {
                    // Nothing left to do: the run is over.
                    queue.running = false;
                    queue.cancel = None;
                    let snapshot = snapshot_of(&queue, true);
                    drop(queue);
                    *state.synthesis_busy.lock().unwrap() = false;
                    emit_queue_update(&app, &snapshot)?;
                    return Ok(());
                }
            };
            queue.items[pos].status = QueueStatus::Working;
            queue.items[pos].error = None;
            let item = queue.items[pos].clone();
            let snapshot = snapshot_of(&queue, false);
            drop(queue);
            emit_queue_update(&app, &snapshot)?;
            item
        };

        // Extract + synthesize outside the lock so the frontend can observe
        // the Working state while piper runs. The output path is resolved
        // here for both item kinds so the emitter can report per-sentence
        // progress while it writes.
        let outcome = {
            let progress: Option<Arc<ProgressFn>> = {
                let app = app.clone();
                let item_id = next.id.clone();
                Some(Arc::new(
                    move |done: usize, total: usize| -> Result<(), String> {
                        let percent = if total == 0 {
                            0.0
                        } else {
                            done as f64 / total as f64 * 100.0
                        };
                        app.emit(
                            "queue-progress",
                            serde_json::json!({
                                "item_id": item_id,
                                "done": done,
                                "total": total,
                                "percent": percent,
                            }),
                        )
                        .map_err(|error| format!("failed to emit queue progress: {error}"))
                    },
                ))
            };

            if let Some(text) = next.text.clone() {
                if text.trim().is_empty() {
                    Err("The text item is empty.".to_string())
                } else {
                    let out_path = resolve_queue_output_path(&state, &next)?;
                    synthesize_chapter(
                        &state,
                        &text,
                        &out_path,
                        voice_id.as_deref(),
                        &token,
                        progress,
                    )
                    .await
                }
            } else {
                let pdf_path = next.pdf_path.clone().unwrap_or_default();
                let out_path = resolve_queue_output_path(&state, &next)?;
                let extracted = pdf::extract_pdf_text(pdf_path);
                match extracted {
                    Ok(text) if text.trim().is_empty() => {
                        Err("The PDF contains no extractable text.".to_string())
                    }
                    Ok(text) => synthesize_chapter(
                        &state,
                        &text,
                        &out_path,
                        voice_id.as_deref(),
                        &token,
                        progress,
                    )
                    .await,
                    Err(error) => Err(error),
                }
            }
        };

        // A cancellation aborts the whole run: the in-flight item reverts
        // to Pending (resumable) and the remaining items stay queued.
        if matches!(&outcome, Err(error) if error.contains("Synthesis cancelled")) {
            let mut queue = state
                .queue
                .lock()
                .map_err(|_| "queue lock poisoned".to_string())?;
            if let Some(item) = queue.items.iter_mut().find(|item| item.id == next.id) {
                item.status = QueueStatus::Pending;
                item.error = Some("Synthesis cancelled.".to_string());
            }
            queue.running = false;
            queue.cancel = None;
            let snapshot = snapshot_of(&queue, true);
            drop(queue);
            *state.synthesis_busy.lock().unwrap() = false;
            emit_queue_update(&app, &snapshot)?;
            return Ok(());
        }

        match outcome {
            Ok(result) => {
                // Optional automatic MP3 conversion: when the checkbox is on,
                // encode the WAV right after synthesis while the item stays
                // Working, so the frontend mirrors `mp3-progress` events on
                // the item's own bar. A failed conversion does not fail the
                // item: the WAV succeeded, and the user can retry the MP3
                // with the queue button.
                let mp3_outcome: Result<Option<String>, String> = {
                    let auto = *state
                        .mp3_auto_convert
                        .lock()
                        .map_err(|_| "mp3 auto-convert lock poisoned".to_string())?;
                    if !auto {
                        Ok(None)
                    } else {
                        let wav = PathBuf::from(&result.wav_path);
                        let mp3 = wav.with_extension("mp3");
                        let task = {
                            let app = app.clone();
                            let wav = wav.clone();
                            let mp3 = mp3.clone();
                            let item_id = next.id.clone();
                            tauri::async_runtime::spawn_blocking(move || {
                                mp3::convert_to_mp3(&wav, &mp3, &mut |percent: f64| {
                                    let _ = app.emit(
                                        "mp3-progress",
                                        mp3::Mp3Progress {
                                            token: item_id.clone(),
                                            percent,
                                        },
                                    );
                                })
                            })
                            .await
                            .map_err(|error| {
                                format!("MP3 conversion task failed: {error}")
                            })?
                        };
                        match task {
                            Ok(_) => Ok(Some(mp3.to_string_lossy().into_owned())),
                            Err(error) => Err(format!("MP3 conversion failed: {error}")),
                        }
                    }
                };

                let mut queue = state
                    .queue
                    .lock()
                    .map_err(|_| "queue lock poisoned".to_string())?;
                if let Some(item) = queue.items.iter_mut().find(|item| item.id == next.id) {
                    item.status = QueueStatus::Done;
                    item.wav_path = Some(result.wav_path.clone());
                    item.audio_secs = Some(result.audio_secs);
                    match &mp3_outcome {
                        Ok(mp3_path) => {
                            item.mp3_path = mp3_path.clone();
                            item.error = None;
                        }
                        Err(error) => {
                            item.mp3_path = None;
                            item.error = Some(error.clone());
                        }
                    }
                }
                let snapshot = snapshot_of(&queue, false);
                // Done items stay in the list so the user sees the finished
                // state; the frontend hides them after a short delay.
                queue.run_completed += 1;
                drop(queue);
                emit_queue_update(&app, &snapshot)?;
            }
            Err(error) => {
                let mut queue = state
                    .queue
                    .lock()
                    .map_err(|_| "queue lock poisoned".to_string())?;
                if let Some(item) = queue.items.iter_mut().find(|item| item.id == next.id) {
                    item.status = QueueStatus::Error;
                    item.error = Some(error.clone());
                }
                queue.run_completed += 1;
                let snapshot = snapshot_of(&queue, false);
                drop(queue);
                emit_queue_update(&app, &snapshot)?;
            }
        }
    }
}

/// Cancel the running queue: triggers the same `CancellationToken` the
/// in-flight item's synthesis races against. The loop itself emits the
/// final `queue-updated` update.
#[tauri::command]
pub fn queue_stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let token = state
        .queue
        .lock()
        .map_err(|_| "queue lock poisoned".to_string())?
        .cancel
        .clone();
    if let Some(token) = token {
        token.cancel();
    }
    Ok(())
}

/// Full snapshot of the current queue state.
#[tauri::command]
pub fn queue_state(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let queue = state
        .queue
        .lock()
        .map_err(|_| "queue lock poisoned".to_string())?;
    serde_json::to_value(snapshot_of(&queue, false))
        .map_err(|error| format!("failed to serialize queue: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_output_name_strips_trailing_wav_case_insensitively() {
        assert_eq!(clean_output_name("capitulo1.wav").unwrap(), "capitulo1");
        assert_eq!(clean_output_name("capitulo1.WAV").unwrap(), "capitulo1");
        assert_eq!(clean_output_name("  capitulo1.wav  ").unwrap(), "capitulo1");
    }

    #[test]
    fn clean_output_name_keeps_valid_names_and_trims_whitespace() {
        assert_eq!(clean_output_name("capitulo 1").unwrap(), "capitulo 1");
        assert_eq!(clean_output_name("  nombre con espacios  ").unwrap(), "nombre con espacios");
        assert_eq!(clean_output_name("wav").unwrap(), "wav");
        assert_eq!(clean_output_name("archivo.wav.wav").unwrap(), "archivo.wav");
    }

    #[test]
    fn clean_output_name_rejects_empty_input() {
        assert!(clean_output_name("").is_err());
        assert!(clean_output_name("   ").is_err());
        assert!(clean_output_name(".wav").is_err());
        assert!(clean_output_name(" .WAV ").is_err());
    }

    #[test]
    fn clean_output_name_rejects_windows_invalid_characters() {
        for ch in ['<', '>', ':', '"', '/', '\\', '|', '?', '*'] {
            let name = format!("capitulo{ch}1");
            let error = clean_output_name(&name).unwrap_err();
            assert!(error.contains(ch), "expected '{ch}' flagged, got: {error}");
        }
    }

    #[test]
    fn output_for_uses_the_editable_output_name() {
        let out = output_for("C:\\libros\\capitulo.pdf", "mis-notas");
        assert_eq!(out, PathBuf::from("C:\\libros\\mis-notas.wav"));
    }

    #[test]
    fn resolve_queue_output_path_uses_per_item_defaults_without_override() {
        let state = AppState::default();
        *state.output_dir_override.lock().expect("lock") = None;

        let pdf_item = QueueItem {
            id: "q1".to_string(),
            title: "capitulo".to_string(),
            output_name: "capitulo".to_string(),
            pdf_path: Some("C:\\libros\\capitulo.pdf".to_string()),
            text: None,
            status: QueueStatus::Pending,
            error: None,
            wav_path: None,
            mp3_path: None,
            audio_secs: None,
        };
        assert_eq!(
            resolve_queue_output_path(&state, &pdf_item).unwrap(),
            PathBuf::from("C:\\libros\\capitulo.wav")
        );
    }

    #[test]
    fn resolve_queue_output_path_uses_global_folder_for_both_item_kinds() {
        let state = AppState::default();
        let tmp = tempfile::tempdir().expect("temp dir");
        *state.output_dir_override.lock().expect("lock") = Some(tmp.path().to_path_buf());

        let pdf_item = QueueItem {
            id: "q1".to_string(),
            title: "capitulo".to_string(),
            output_name: "capitulo".to_string(),
            pdf_path: Some("C:\\libros\\capitulo.pdf".to_string()),
            text: None,
            status: QueueStatus::Pending,
            error: None,
            wav_path: None,
            mp3_path: None,
            audio_secs: None,
        };
        let text_item = QueueItem {
            id: "q2".to_string(),
            title: "apuntes".to_string(),
            output_name: "apuntes".to_string(),
            pdf_path: None,
            text: Some("hola".to_string()),
            status: QueueStatus::Pending,
            error: None,
            wav_path: None,
            mp3_path: None,
            audio_secs: None,
        };
        let expected_pdf = tmp.path().join("capitulo.wav");
        let expected_text = tmp.path().join("apuntes.wav");
        assert_eq!(
            resolve_queue_output_path(&state, &pdf_item).unwrap(),
            expected_pdf
        );
        assert_eq!(
            resolve_queue_output_path(&state, &text_item).unwrap(),
            expected_text
        );
    }
}