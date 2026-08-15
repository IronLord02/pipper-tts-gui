//! Piper text-to-speech synthesis.
//!
//! Drives the bundled piper CLI (`piper-runtime/piper.exe`) to turn text into
//! WAV audio. The runtime directory resolves next to the running executable
//! first, falling back to `CARGO_MANIFEST_DIR` (dev / `cargo test`). The voice
//! model files are looked up in the models storage location (see `paths`).
//!
//! `estimate_duration` predicts audio and wall time before a run; `synthesize`
//! splits the input into sentences, spawns piper once per sentence into a
//! staging WAV, emits a `synthesis-progress` event after each one so the GUI
//! can show real (determinate) progress, merges the same-format WAV parts,
//! and parses the real duration from the merged header.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::paths;

/// Name of the piper CLI executable inside the runtime directory.
const PIPER_EXE: &str = "piper.exe";
/// Voice model base name; both `<name>.onnx` and `<name>.onnx.json` are used.
const VOICE_MODEL: &str = "es_ES-carlfm-x_low";
/// Hard cap on accepted input size (characters) to guard against runaway jobs.
const MAX_INPUT_CHARS: usize = 100_000;
/// Wall-clock budget for a single piper run.
const PROCESS_TIMEOUT: Duration = Duration::from_secs(300);
/// Empirical real-time factor: wall time is ~0.06x the audio duration
/// (verified run: ~55 chars -> 3.1 s audio, infer 0.19 s).
const RTF: f64 = 0.06;

/// Maximum recursion depth when scanning for a runtime/models directory tree.
const MAX_SCAN_DEPTH: usize = 8;

/// First candidate directory that contains `piper.exe`.
fn find_piper_in(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|dir| dir.join(PIPER_EXE).is_file())
}

/// Find `piper.exe` recursively under `dir` (bounded depth), returning the
/// directory that contains it. Used as a fallback so the app keeps working
/// when the runtime lives in a nested/renamed folder (e.g. a USB stick where
/// the layout differs from the build folder).
fn find_piper_recursive(dir: &Path) -> Option<PathBuf> {
    find_piper_recursive_at(dir, 0)
}

fn find_piper_recursive_at(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth > MAX_SCAN_DEPTH {
        return None;
    }
    if dir.join(PIPER_EXE).is_file() {
        return Some(dir.to_path_buf());
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_piper_recursive_at(&path, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Resolve the runtime directory from an exe dir and (optionally) the crate
/// manifest dir. The exe dir wins when both contain a piper binary. Used
/// directly by `piper_runtime_dir` and by the deterministic unit tests.
fn resolve_runtime_from(exe_dir: &Path, manifest_dir: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = vec![exe_dir.join("piper-runtime")];
    if let Some(manifest) = manifest_dir {
        candidates.push(manifest.join("piper-runtime"));
    }
    find_piper_in(candidates).or_else(|| find_piper_recursive(exe_dir))
}

/// Directory holding the bundled piper CLI, or `None` when `piper.exe` is not
/// present next to the running executable nor under the crate manifest dir.
pub fn piper_runtime_dir() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .ok()
        .map(PathBuf::from);
    match exe_dir {
        Some(dir) => resolve_runtime_from(&dir, manifest_dir.as_deref()),
        None => find_piper_in(
            manifest_dir.into_iter().map(|dir| dir.join("piper-runtime")),
        ),
    }
}

/// The voice model pair `(onnx, json)` for `voice` anywhere under `dir`
/// (recursively, one level at a time), or `None` when either file is missing.
/// Both files must sit in the same folder, mirroring how users organize
/// `models/EN`, `models/ES`, etc.
fn model_files_for(dir: &Path, voice: &str) -> Option<(PathBuf, PathBuf)> {
    find_model_pair(dir, voice, 0)
}

fn find_model_pair(dir: &Path, voice: &str, depth: usize) -> Option<(PathBuf, PathBuf)> {
    if depth > MAX_SCAN_DEPTH {
        return None;
    }
    let onnx = dir.join(format!("{voice}.onnx"));
    let json = dir.join(format!("{voice}.onnx.json"));
    if onnx.is_file() && json.is_file() {
        return Some((onnx, json));
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_model_pair(&path, voice, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

/// The voice model pair `(onnx, json)` for the default voice inside `dir`, or
/// `None` when either file is missing.
fn model_files_in(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    model_files_for(dir, VOICE_MODEL)
}

/// The voice model pair `(onnx, json)` in the models storage location, or
/// `None` when either file is missing.
pub fn model_files() -> Option<(PathBuf, PathBuf)> {
    model_files_in(&paths::models_dir().path)
}

/// A Piper voice discovered in the models directory.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InstalledVoice {
    /// Model file stem, e.g. `es_ES-carlfm-x_low`.
    pub id: String,
    /// Language code, e.g. `es` (from the config `espeak.voice` when present).
    pub language: String,
    /// Human-readable label, e.g. `Spanish (es_ES-carlfm-x_low)`.
    pub display_name: String,
    /// Size of the `.onnx` model file in bytes (from file metadata only).
    pub size_bytes: u64,
    /// Piper quality from the config JSON, e.g. `low`, `x_low`, `medium`,
    /// `high`; `"unknown"` when absent.
    pub quality: String,
}

/// Human-readable language name for a known locale code; falls back to the
/// raw code when the language is not in the map.
fn language_name(code: &str) -> String {
    let name = match code {
        "es" => "Spanish",
        "en" => "English",
        "de" => "German",
        "fr" => "French",
        "it" => "Italian",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "zh" => "Chinese",
        _ => return code.to_string(),
    };
    name.to_string()
}

/// Derive the language code for a voice: prefer the `espeak.voice` field from
/// the config JSON, fall back to the id prefix before the first `-`.
fn voice_language(json_path: &Path, stem: &str) -> String {
    if let Ok(json) = std::fs::read_to_string(json_path) {
        if let Ok(config) = serde_json::from_str::<serde_json::Value>(&json) {
            if let Some(voice) = config
                .get("espeak")
                .and_then(|espeak| espeak.get("voice"))
                .and_then(|voice| voice.as_str())
            {
                if !voice.is_empty() {
                    return voice.to_string();
                }
            }
        }
    }
    stem.split('-').next().unwrap_or(stem).to_string()
}

/// Piper quality from the config JSON (`quality` field, e.g. `low`, `x_low`,
/// `medium`, `high`); `"unknown"` when the field is absent or unreadable.
fn voice_quality(json_path: &Path) -> String {
    if let Ok(json) = std::fs::read_to_string(json_path) {
        if let Ok(config) = serde_json::from_str::<serde_json::Value>(&json) {
            if let Some(quality) = config.get("quality").and_then(|quality| quality.as_str()) {
                if !quality.is_empty() {
                    return quality.to_string();
                }
            }
        }
    }
    "unknown".to_string()
}

/// Sort installed voices: Spanish first, then English, then the rest
/// alphabetically by language name, then by id.
fn sort_voices(voices: &mut [InstalledVoice]) {
    let rank = |language: &str| match language {
        "es" => 0,
        "en" => 1,
        _ => 2,
    };
    voices.sort_by(|a, b| {
        rank(&a.language)
            .cmp(&rank(&b.language))
            .then_with(|| a.language.cmp(&b.language))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Scan `dir` (recursively, e.g. `models/EN`, `models/ES`) for installed Piper
/// voices: every `*.onnx` file that also has a sibling `<stem>.onnx.json`
/// config. Returns an empty list when `dir` is missing or holds no complete
/// model pairs.
fn discover_installed_voices(dir: &Path) -> Vec<InstalledVoice> {
    let mut voices = Vec::new();
    scan_voices(dir, 0, &mut voices);
    sort_voices(&mut voices);
    voices
}

fn scan_voices(dir: &Path, depth: usize, out: &mut Vec<InstalledVoice>) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_voices(&path, depth + 1, out);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("onnx") {
            continue;
        }
        let stem = match path.file_stem().and_then(|stem| stem.to_str()) {
            Some(stem) => stem.to_string(),
            None => continue,
        };
        let json = path.with_extension("onnx.json");
        if !json.is_file() {
            continue;
        }
        let language = voice_language(&json, &stem);
        let display_name = format!("{} ({stem})", language_name(&language));
        let size_bytes = path.metadata().map(|meta| meta.len()).unwrap_or(0);
        let quality = voice_quality(&json);
        out.push(InstalledVoice {
            id: stem,
            language,
            display_name,
            size_bytes,
            quality,
        });
    }
}

/// Frontend command: list the Piper voices installed in the active models
/// directory (user-chosen override when set, bundled default otherwise).
/// Returns an empty list when the directory is missing or has no complete
/// model pairs.
#[tauri::command]
pub fn list_installed_voices(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let models_dir = crate::state::models_dir(&state);
    discover_installed_voices(&models_dir)
        .into_iter()
        .map(|voice| {
            serde_json::to_value(voice)
                .map_err(|error| format!("failed to serialize installed voice: {error}"))
        })
        .collect()
}

/// Empirical audio-duration estimate: ~0.06 s of audio per input character.
pub fn estimate_audio_secs(chars: usize) -> f64 {
    chars as f64 * 0.06
}

/// Parse a WAV duration in seconds from an in-memory RIFF/WAVE stream.
///
/// Reads the `fmt ` byte rate and the `data` chunk size. Returns `0.0` when
/// the header is missing, is not PCM, or lacks either chunk.
pub fn parse_wav_duration_secs(data: &[u8]) -> f64 {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return 0.0;
    }
    let mut byte_rate: Option<u32> = None;
    let mut data_size: Option<u64> = None;
    let mut pos = 12usize;
    while pos + 8 <= data.len() {
        let chunk_size =
            u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
                as usize;
        match &data[pos..pos + 4] {
            b"fmt " => {
                // fmt layout: format(2) channels(2) rate(4) byte_rate(4) ...
                if pos + 20 > data.len() {
                    return 0.0;
                }
                let audio_format = u16::from_le_bytes([data[pos + 8], data[pos + 9]]);
                if audio_format != 1 {
                    return 0.0;
                }
                byte_rate = Some(u32::from_le_bytes([
                    data[pos + 16],
                    data[pos + 17],
                    data[pos + 18],
                    data[pos + 19],
                ]));
            }
            b"data" => {
                data_size = Some(chunk_size as u64);
            }
            _ => {}
        }
        pos += 8 + chunk_size + (chunk_size % 2);
    }
    match (byte_rate, data_size) {
        (Some(rate), Some(size)) if rate > 0 => size as f64 / rate as f64,
        _ => 0.0,
    }
}

/// Split `text` into sentence-sized chunks for per-sentence synthesis.
///
/// A chunk breaks after sentence-ending punctuation (`.`, `!`, `?`, `…`) when
/// it is followed by whitespace, and at every newline (so pasted paragraph
/// breaks become chunk boundaries too). The punctuation stays attached to the
/// preceding chunk and surrounding whitespace is trimmed. Empty or
/// whitespace-only input yields an empty vec. Decimal numbers are not split
/// because a digit after the dot resets the pending terminator.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut pending_terminator = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        current.push(ch);

        // Hard break: paragraphs / pasted line breaks. Consecutive CR/LF are
        // swallowed so CRLF files do not produce empty chunks.
        if ch == '\n' || ch == '\r' {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current.clear();
            pending_terminator = false;
            while matches!(chars.peek(), Some('\r') | Some('\n')) {
                chars.next();
            }
            continue;
        }

        if matches!(ch, '.' | '!' | '?' | '…') {
            pending_terminator = true;
            continue;
        }

        // A terminator only cuts when whitespace follows: "3.14" and "etc."
        // followed by a letter stay in one chunk.
        if pending_terminator && ch.is_whitespace() {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current.clear();
            pending_terminator = false;
            while chars.peek().is_some_and(|next| next.is_whitespace()) {
                chars.next();
            }
            continue;
        }

        pending_terminator = false;
    }

    let tail = current.trim().to_string();
    if !tail.is_empty() {
        sentences.push(tail);
    }
    sentences
}

/// Locate the `data` chunk payload inside a WAV file: `(payload_offset,
/// payload_len)` in bytes, or `None` when the file is not a RIFF/WAVE stream
/// with a `data` chunk. Walks chunk headers without reading the payload.
fn wav_data_layout(path: &Path) -> Result<Option<(u64, u64)>, String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    let file_len = file
        .metadata()
        .map_err(|error| format!("stat {}: {error}", path.display()))?
        .len();

    let mut head = [0u8; 12];
    file.read_exact(&mut head)
        .map_err(|error| format!("read {} header: {error}", path.display()))?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Ok(None);
    }

    let mut pos: u64 = 12;
    while pos + 8 <= file_len {
        file.seek(SeekFrom::Start(pos))
            .map_err(|error| format!("seek {}: {error}", path.display()))?;
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk)
            .map_err(|error| format!("read {} chunk: {error}", path.display()))?;
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        if &chunk[0..4] == b"data" {
            return Ok(Some((pos + 8, size)));
        }
        pos += 8 + size + (size % 2);
    }
    Ok(None)
}

/// Merge same-format WAV files (one per synthesized sentence) into a single
/// WAV at `out`. Every sentence rendered by the same voice model is PCM with
/// an identical `fmt ` layout, so only the `data` payloads need concatenating.
/// The first part's header is kept (RIFF size patched) and each following
/// part's payload is appended. Returns the merged file size in bytes.
fn merge_wav_parts(parts: &[PathBuf], out: &Path) -> Result<u64, String> {
    use std::io::{BufWriter, Read, Seek, SeekFrom, Write};

    if parts.is_empty() {
        return Err("no WAV parts to merge".to_string());
    }

    let mut layouts = Vec::with_capacity(parts.len());
    let mut total_data: u64 = 0;
    for part in parts {
        let layout = wav_data_layout(part)?.ok_or_else(|| {
            format!("{} is not a WAV file with a data chunk", part.display())
        })?;
        total_data += layout.1;
        layouts.push(layout);
    }

    if parts.len() == 1 {
        std::fs::copy(&parts[0], out)
            .map_err(|error| format!("copy {}: {error}", parts[0].display()))?;
        return std::fs::metadata(out)
            .map(|meta| meta.len())
            .map_err(|error| format!("stat {}: {error}", out.display()));
    }

    // Keep the first part's full header (everything before its data payload)
    // and patch the RIFF size so it covers all appended payloads.
    let header_len = layouts[0].0 as usize;
    let mut header = vec![0u8; header_len];
    {
        let mut first = std::fs::File::open(&parts[0])
            .map_err(|error| format!("open {}: {error}", parts[0].display()))?;
        first
            .read_exact(&mut header)
            .map_err(|error| format!("read {} header: {error}", parts[0].display()))?;
    }
    let merged_size = header.len() as u64 + total_data;
    if header.len() >= 8 {
        header[4..8].copy_from_slice(&((merged_size - 8) as u32).to_le_bytes());
    }
    // The data chunk header is the last chunk before the payload: its size
    // field must cover every appended payload, not just the first part's.
    let data_size_field = layouts[0].0 as usize;
    if data_size_field >= 4 && data_size_field <= header.len() {
        header[data_size_field - 4..data_size_field]
            .copy_from_slice(&(total_data as u32).to_le_bytes());
    }

    let mut out_file = BufWriter::new(std::fs::File::create(out).map_err(|error| {
        format!("create {}: {error}", out.display())
    })?);
    out_file
        .write_all(&header)
        .map_err(|error| format!("write {} header: {error}", out.display()))?;

    for (part, layout) in parts.iter().zip(&layouts) {
        let mut src = std::fs::File::open(part)
            .map_err(|error| format!("open {}: {error}", part.display()))?;
        src.seek(SeekFrom::Start(layout.0))
            .map_err(|error| format!("seek {}: {error}", part.display()))?;
        let mut payload = src.take(layout.1);
        std::io::copy(&mut payload, &mut out_file)
            .map_err(|error| format!("merge {}: {error}", part.display()))?;
    }
    out_file
        .flush()
        .map_err(|error| format!("flush {}: {error}", out.display()))?;
    Ok(merged_size)
}

/// Estimate payload returned by `estimate_duration`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DurationEstimate {
    pub chars: usize,
    pub estimated_audio_secs: f64,
    pub estimated_process_secs: f64,
}

/// Frontend estimate endpoint: character count plus expected audio and wall
/// time, computed before any synthesis runs.
#[tauri::command]
pub fn estimate_duration(text: String) -> Result<serde_json::Value, String> {
    let chars = text.chars().count();
    let estimated_audio_secs = estimate_audio_secs(chars);
    serde_json::to_value(DurationEstimate {
        chars,
        estimated_audio_secs,
        estimated_process_secs: estimated_audio_secs * RTF,
    })
    .map_err(|error| format!("failed to serialize duration estimate: {error}"))
}

/// Synthesis payload returned by `synthesize`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SynthesisResult {
    pub wav_path: String,
    pub audio_secs: f64,
    pub estimated_audio_secs: f64,
    pub chars: usize,
}

/// Per-sentence progress emitted on the `synthesis-progress` event while
/// `synthesize` runs. `done` counts completed sentences (0 before the first
/// one), `percent` is `done / total * 100`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SynthesisProgress {
    pub done: usize,
    pub total: usize,
    pub percent: f64,
}

/// Default output directory: `<exe_dir>/output`, so user-generated WAVs land
/// next to the app instead of inside the models folder (which the user should
/// never have to touch). Falls back to the current directory when the exe dir
/// cannot be resolved.
fn default_output_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    exe_dir
        .unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        })
        .join("output")
}

/// Default timestamped output file under `out_dir`, avoiding collisions when
/// several runs land in the same second.
fn default_output_path(out_dir: &Path) -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let mut path = out_dir.join(format!("piper-tts-{secs}.wav"));
    let mut counter = 1u32;
    while path.exists() {
        path = out_dir.join(format!("piper-tts-{secs}-{counter}.wav"));
        counter += 1;
    }
    path
}

/// Trimmed, truncated copy of piper's stderr for user-facing errors.
fn stderr_excerpt(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .trim()
        .chars()
        .take(400)
        .collect()
}

/// Run piper once for a single sentence, writing a WAV to `out`. Feeds the
/// text on stdin, waits with a bounded timeout, races against the shared
/// cancellation token, and maps failures to user-facing messages.
async fn run_piper_sentence(
    runtime: &Path,
    onnx: &Path,
    json: &Path,
    text: &str,
    out: &Path,
    token: &CancellationToken,
) -> Result<(), String> {
    let mut command = tokio::process::Command::new(runtime.join(PIPER_EXE));
    command
        .arg("-m")
        .arg(onnx)
        .arg("-c")
        .arg(json)
        .arg("-f")
        .arg(out)
        .arg("--espeak_data")
        .arg(runtime.join("espeak-ng-data"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // CREATE_NO_WINDOW hides the piper console window so no terminal flashes
    // open on Windows while synthesis runs. Non-Windows targets are unchanged.
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start piper.exe: {error}"))?;

    // Feed the text and close stdin (EOF) so piper starts synthesizing.
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        if let Err(error) = stdin.write_all(text.as_bytes()).await {
            let _ = std::fs::remove_file(out);
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(format!("Failed to write text to piper: {error}"));
        }
        drop(stdin);
    }

    let child_stderr = child.stderr.take();

    let status = tokio::select! {
        result = tokio::time::timeout(PROCESS_TIMEOUT, child.wait()) => match result {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => {
                let _ = std::fs::remove_file(out);
                return Err(format!("Failed to wait for piper.exe: {error}"));
            }
            Err(_) => {
                let _ = std::fs::remove_file(out);
                return Err(
                    "Piper synthesis timed out after 300 seconds; the partial output was discarded."
                        .to_string(),
                );
            }
        },
        _ = token.cancelled() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = std::fs::remove_file(out);
            return Err("Synthesis cancelled.".to_string());
        }
    };

    // Drain piper's stderr so the pipe is fully closed and its content is
    // available for the failure excerpt below.
    let mut stderr = Vec::new();
    if let Some(mut stderr_pipe) = child_stderr {
        use tokio::io::AsyncReadExt;
        let _ = stderr_pipe.read_to_end(&mut stderr).await;
    }

    if !status.success() {
        let code = status
            .code()
            .map_or_else(String::new, |code| format!(" (exit code {code})"));
        let stderr_text = stderr_excerpt(&stderr);
        let detail = if stderr_text.is_empty() {
            "No error output was captured.".to_string()
        } else {
            format!("stderr: {stderr_text}")
        };
        let _ = std::fs::remove_file(out);
        return Err(format!("Piper failed{code}. {detail}"));
    }

    Ok(())
}

/// Split `text` into sentences, run piper once per sentence into a staging
/// WAV, emit a `synthesis-progress` event after each one, merge the
/// same-format parts into `out` (or the app's default output directory when
/// `None`), and report the produced file with its real audio duration.
#[tauri::command]
pub async fn synthesize(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    text: String,
    out_path: Option<String>,
    voice_id: Option<String>,
) -> Result<serde_json::Value, String> {
    use tauri::Emitter;

    if text.trim().is_empty() {
        return Err("No text to synthesize.".to_string());
    }
    let chars = text.chars().count();
    if chars > MAX_INPUT_CHARS {
        return Err(format!(
            "Text is too long ({chars} characters); the limit is {MAX_INPUT_CHARS}."
        ));
    }

    let sentences = split_sentences(&text);
    if sentences.is_empty() {
        return Err("No text to synthesize.".to_string());
    }

    let runtime = piper_runtime_dir().ok_or_else(|| {
        "Piper runtime not found. Expected piper.exe under <app>/piper-runtime.".to_string()
    })?;
    let models_dir = crate::state::models_dir(&state);
    let voice = voice_id.as_deref().unwrap_or(VOICE_MODEL);
    let (onnx, json) = model_files_for(&models_dir, voice).ok_or_else(|| {
        format!(
            "Voice '{voice}' is not installed. Place {voice}.onnx and {voice}.onnx.json in the models directory."
        )
    })?;

    let out = match out_path {
        Some(requested) => {
            let path = PathBuf::from(requested);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("Cannot create output directory: {error}"))?;
            }
            path
        }
        None => {
            let out_dir = default_output_dir();
            std::fs::create_dir_all(&out_dir)
                .map_err(|error| format!("Cannot create output directory: {error}"))?;
            default_output_path(&out_dir)
        }
    };

    // Stage one WAV per sentence in a temp dir; TempDir removes everything on
    // drop, including every error path below.
    let staging = tempfile::tempdir()
        .map_err(|error| format!("Cannot create staging directory: {error}"))?;

    // Store a token for this run so `cancel_synthesis` can interrupt it.
    let token = CancellationToken::new();
    *state.synthesis_cancel.lock().unwrap() = Some(token.clone());

    let total = sentences.len();
    let emit_progress = |done: usize| -> Result<(), String> {
        let percent = if total == 0 {
            0.0
        } else {
            done as f64 / total as f64 * 100.0
        };
        app.emit(
            "synthesis-progress",
            SynthesisProgress {
                done,
                total,
                percent,
            },
        )
        .map_err(|error| format!("failed to emit synthesis progress: {error}"))
    };

    // Tell the frontend the sentence count before any audio is generated.
    emit_progress(0)?;

    let mut parts: Vec<PathBuf> = Vec::with_capacity(total);
    for (index, sentence) in sentences.iter().enumerate() {
        if token.is_cancelled() {
            *state.synthesis_cancel.lock().unwrap() = None;
            return Err("Synthesis cancelled.".to_string());
        }

        let part = staging.path().join(format!("{index:04}.wav"));
        run_piper_sentence(&runtime, &onnx, &json, sentence, &part, &token).await?;
        parts.push(part);
        emit_progress(index + 1)?;
    }

    // Clear the stored token once the child has exited. Safe because a new
    // synthesis only starts after the frontend re-enables the button (busy
    // flag), and `cancel_synthesis` only acts on the token it takes from here.
    *state.synthesis_cancel.lock().unwrap() = None;

    merge_wav_parts(&parts, &out)?;

    let wav = std::fs::read(&out)
        .map_err(|error| format!("Piper exited successfully but the WAV could not be read: {error}"))?;
    let audio_secs = parse_wav_duration_secs(&wav);

    serde_json::to_value(SynthesisResult {
        wav_path: out.to_string_lossy().into_owned(),
        audio_secs,
        estimated_audio_secs: estimate_audio_secs(chars),
        chars,
    })
    .map_err(|error| format!("failed to serialize synthesis result: {error}"))
}

/// Frontend command: cancel the in-flight synthesis (if any).
#[tauri::command]
pub fn cancel_synthesis(state: tauri::State<'_, crate::state::AppState>) -> Result<(), String> {
    let token = state
        .synthesis_cancel
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?
        .take();
    if let Some(token) = token {
        token.cancel();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_audio_secs_scales_linearly_with_chars() {
        assert_eq!(estimate_audio_secs(0), 0.0);
        assert_eq!(estimate_audio_secs(100), 6.0);
    }

    /// Build a minimal 16-bit PCM WAV header for a `data_bytes` payload.
    fn build_wav_header(
        sample_rate: u32,
        channels: u16,
        bits_per_sample: u16,
        data_bytes: u32,
    ) -> Vec<u8> {
        let byte_rate = sample_rate * channels as u32 * u32::from(bits_per_sample / 8);
        let block_align = channels * (bits_per_sample / 8);
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        wav
    }

    #[test]
    fn wav_duration_parses_16bit_mono_16000() {
        let header = build_wav_header(16000, 1, 16, 8000);
        assert!((parse_wav_duration_secs(&header) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn wav_duration_returns_zero_for_garbage() {
        assert_eq!(parse_wav_duration_secs(&[]), 0.0);
        assert_eq!(parse_wav_duration_secs(b"not a wav at all"), 0.0);
        assert_eq!(parse_wav_duration_secs(b"RIFF\x00\x00\x00\x00WAVE"), 0.0);
    }

    #[test]
    fn wav_duration_ignores_non_pcm_formats() {
        let mut header = build_wav_header(16000, 1, 16, 8000);
        header[20..22].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(parse_wav_duration_secs(&header), 0.0);
    }

    #[test]
    fn runtime_resolution_prefers_exe_dir_when_present() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let exe_dir = tmp.path().join("bin");
        let exe_runtime = exe_dir.join("piper-runtime");
        std::fs::create_dir_all(&exe_runtime).expect("create exe runtime");
        std::fs::write(exe_runtime.join(PIPER_EXE), b"").expect("write fake piper");

        let manifest_runtime = tmp.path().join("src-tauri").join("piper-runtime");
        std::fs::create_dir_all(&manifest_runtime).expect("create manifest runtime");
        std::fs::write(manifest_runtime.join(PIPER_EXE), b"").expect("write fake piper");

        let resolved =
            resolve_runtime_from(&exe_dir, Some(tmp.path().join("src-tauri").as_path()));
        assert_eq!(resolved, Some(exe_runtime));
    }

    #[test]
    fn runtime_resolution_falls_back_to_manifest_dir() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let exe_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&exe_dir).expect("create exe dir");
        let runtime = tmp.path().join("src-tauri").join("piper-runtime");
        std::fs::create_dir_all(&runtime).expect("create runtime");
        std::fs::write(runtime.join(PIPER_EXE), b"").expect("write fake piper");

        let resolved =
            resolve_runtime_from(&exe_dir, Some(tmp.path().join("src-tauri").as_path()));
        assert_eq!(resolved, Some(runtime));
    }

    #[test]
    fn runtime_resolution_returns_none_when_missing() {
        let tmp = tempfile::tempdir().expect("temp dir");
        assert_eq!(resolve_runtime_from(tmp.path(), Some(tmp.path())), None);
    }

    #[test]
    fn runtime_resolution_finds_piper_recursively() {
        let tmp = tempfile::tempdir().expect("temp dir");
        // Nested/renamed layout, e.g. a USB stick: <root>/piper/es-ng/piper.exe
        let nested = tmp.path().join("piper").join("es-ng");
        std::fs::create_dir_all(&nested).expect("create nested dirs");
        std::fs::write(nested.join(PIPER_EXE), b"").expect("write fake piper");

        let resolved = resolve_runtime_from(tmp.path(), None);
        assert_eq!(resolved, Some(nested));
    }

    #[test]
    fn runtime_resolution_prefers_flat_piper_runtime_over_nested() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let flat = tmp.path().join("piper-runtime");
        std::fs::create_dir_all(&flat).expect("create flat runtime");
        std::fs::write(flat.join(PIPER_EXE), b"").expect("write fake piper");

        let nested = tmp.path().join("other").join("deeper");
        std::fs::create_dir_all(&nested).expect("create nested dirs");
        std::fs::write(nested.join(PIPER_EXE), b"").expect("write fake piper");

        let resolved = resolve_runtime_from(tmp.path(), None);
        assert_eq!(resolved, Some(flat), "flat piper-runtime wins over nested");
    }

    #[test]
    fn model_files_resolves_only_when_both_present() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let models = tmp.path().join("models");
        std::fs::create_dir_all(&models).expect("create models dir");

        assert_eq!(model_files_in(&models), None);

        std::fs::write(models.join(format!("{VOICE_MODEL}.onnx")), b"onnx").expect("write onnx");
        assert_eq!(model_files_in(&models), None, "json still missing");

        std::fs::write(models.join(format!("{VOICE_MODEL}.onnx.json")), b"{}").expect("write json");
        let (onnx, json) = model_files_in(&models).expect("both present");
        assert_eq!(onnx, models.join(format!("{VOICE_MODEL}.onnx")));
        assert_eq!(json, models.join(format!("{VOICE_MODEL}.onnx.json")));
    }

    #[test]
    fn model_files_for_resolves_custom_voice_and_missing() {
        let tmp = tempfile::tempdir().expect("temp dir");
        std::fs::write(tmp.path().join("en_US-lessac-medium.onnx"), b"onnx").expect("write onnx");
        std::fs::write(
            tmp.path().join("en_US-lessac-medium.onnx.json"),
            b"{}",
        )
        .expect("write json");

        let (onnx, json) =
            model_files_for(tmp.path(), "en_US-lessac-medium").expect("custom voice resolved");
        assert_eq!(onnx, tmp.path().join("en_US-lessac-medium.onnx"));
        assert_eq!(json, tmp.path().join("en_US-lessac-medium.onnx.json"));

        assert_eq!(model_files_for(tmp.path(), "missing-voice"), None);
    }

    #[test]
    fn discovers_installed_voices_with_language_and_display_name() {
        let tmp = tempfile::tempdir().expect("temp dir");
        std::fs::write(tmp.path().join("es_ES-carlfm-x_low.onnx"), b"onnx").expect("write onnx");
        std::fs::write(
            tmp.path().join("es_ES-carlfm-x_low.onnx.json"),
            br#"{"espeak":{"voice":"es"}}"#,
        )
        .expect("write json");
        std::fs::write(tmp.path().join("en_US-lessac-medium.onnx"), b"onnx").expect("write onnx");
        std::fs::write(
            tmp.path().join("en_US-lessac-medium.onnx.json"),
            br#"{"espeak":{"voice":"en"}}"#,
        )
        .expect("write json");

        let voices = discover_installed_voices(tmp.path());
        assert_eq!(voices.len(), 2);

        let spanish = &voices[0];
        assert_eq!(spanish.id, "es_ES-carlfm-x_low");
        assert_eq!(spanish.language, "es");
        assert_eq!(spanish.display_name, "Spanish (es_ES-carlfm-x_low)");

        let english = &voices[1];
        assert_eq!(english.id, "en_US-lessac-medium");
        assert_eq!(english.language, "en");
        assert_eq!(english.display_name, "English (en_US-lessac-medium)");
    }

    #[test]
    fn discovers_ignores_onnx_without_json_sibling() {
        let tmp = tempfile::tempdir().expect("temp dir");
        std::fs::write(tmp.path().join("es_ES-carlfm-x_low.onnx"), b"onnx").expect("write onnx");

        let voices = discover_installed_voices(tmp.path());
        assert!(voices.is_empty());
    }

    #[test]
    fn discovers_returns_empty_for_missing_dir() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let missing = tmp.path().join("does-not-exist");
        assert!(discover_installed_voices(&missing).is_empty());
    }

    #[test]
    fn language_falls_back_to_id_prefix_when_json_unparseable() {
        let tmp = tempfile::tempdir().expect("temp dir");
        std::fs::write(tmp.path().join("de_DE-thorsten.onnx"), b"onnx").expect("write onnx");
        std::fs::write(tmp.path().join("de_DE-thorsten.onnx.json"), b"not json")
            .expect("write json");

        let voices = discover_installed_voices(tmp.path());
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].language, "de_DE");
        assert_eq!(voices[0].display_name, "de_DE (de_DE-thorsten)");
    }

    #[test]
    fn language_prefers_espeak_voice_over_id_prefix() {
        let tmp = tempfile::tempdir().expect("temp dir");
        std::fs::write(tmp.path().join("es_ES-pablo.onnx"), b"onnx").expect("write onnx");
        std::fs::write(
            tmp.path().join("es_ES-pablo.onnx.json"),
            br#"{"espeak":{"voice":"es"}}"#,
        )
        .expect("write json");

        let voices = discover_installed_voices(tmp.path());
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].language, "es");
    }

    #[test]
    fn discovers_parses_size_and_quality() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let onnx = tmp.path().join("en_US-danny-low.onnx");
        std::fs::write(&onnx, b"0123456789").expect("write onnx");
        std::fs::write(
            tmp.path().join("en_US-danny-low.onnx.json"),
            br#"{"quality":"low","espeak":{"voice":"en"}}"#,
        )
        .expect("write json");

        let voices = discover_installed_voices(tmp.path());
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].size_bytes, 10);
        assert_eq!(voices[0].quality, "low");
        assert_eq!(voices[0].display_name, "English (en_US-danny-low)");
    }

    #[test]
    fn quality_defaults_to_unknown_when_absent() {
        let tmp = tempfile::tempdir().expect("temp dir");
        std::fs::write(tmp.path().join("es_ES-carlfm-x_low.onnx"), b"onnx").expect("write onnx");
        std::fs::write(
            tmp.path().join("es_ES-carlfm-x_low.onnx.json"),
            br#"{"espeak":{"voice":"es"}}"#,
        )
        .expect("write json");

        let voices = discover_installed_voices(tmp.path());
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].quality, "unknown");
    }

    #[test]
    fn discovers_voices_in_subdirectories() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let es = tmp.path().join("ES");
        let en = tmp.path().join("EN");
        std::fs::create_dir_all(&es).expect("create ES dir");
        std::fs::create_dir_all(&en).expect("create EN dir");

        std::fs::write(es.join("es_ES-carlfm-x_low.onnx"), b"onnx").expect("write onnx");
        std::fs::write(
            es.join("es_ES-carlfm-x_low.onnx.json"),
            br#"{"espeak":{"voice":"es"}}"#,
        )
        .expect("write json");
        std::fs::write(en.join("en_US-danny-low.onnx"), b"onnx").expect("write onnx");
        std::fs::write(
            en.join("en_US-danny-low.onnx.json"),
            br#"{"espeak":{"voice":"en"}}"#,
        )
        .expect("write json");

        let voices = discover_installed_voices(tmp.path());
        assert_eq!(voices.len(), 2);
        assert_eq!(voices[0].id, "es_ES-carlfm-x_low");
        assert_eq!(voices[0].language, "es");
        assert_eq!(voices[1].id, "en_US-danny-low");
        assert_eq!(voices[1].language, "en");
    }

    #[test]
    fn model_files_for_finds_voice_in_subdirectory() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let en = tmp.path().join("EN");
        std::fs::create_dir_all(&en).expect("create EN dir");
        std::fs::write(en.join("en_US-danny-low.onnx"), b"onnx").expect("write onnx");
        std::fs::write(en.join("en_US-danny-low.onnx.json"), b"{}").expect("write json");

        let (onnx, json) =
            model_files_for(tmp.path(), "en_US-danny-low").expect("voice resolved in subdir");
        assert_eq!(onnx, en.join("en_US-danny-low.onnx"));
        assert_eq!(json, en.join("en_US-danny-low.onnx.json"));
    }

    #[test]
    fn scan_ignores_unrelated_directories_without_onnx() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let output = tmp.path().join("output");
        std::fs::create_dir_all(&output).expect("create output dir");
        std::fs::write(output.join("piper-tts-1.wav"), b"RIFF").expect("write wav");

        let voices = discover_installed_voices(tmp.path());
        assert!(voices.is_empty());
    }

    #[test]
    fn default_output_path_lands_under_given_dir() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let out_dir = tmp.path().join("output");
        let path = default_output_path(&out_dir);
        assert_eq!(path.parent(), Some(out_dir.as_path()));
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("piper-tts-"));
        assert!(path.to_string_lossy().ends_with(".wav"));
    }

    #[test]
    fn split_sentences_splits_on_punctuation_and_keeps_it() {
        let sentences = split_sentences("Hola mundo. ¿Cómo estás? ¡Genial!");
        assert_eq!(sentences, vec!["Hola mundo.", "¿Cómo estás?", "¡Genial!"]);
    }

    #[test]
    fn split_sentences_splits_on_newlines() {
        let sentences = split_sentences("Primera línea\nSegunda línea.\r\nTercera.");
        assert_eq!(sentences, vec!["Primera línea", "Segunda línea.", "Tercera."]);
    }

    #[test]
    fn split_sentences_does_not_split_decimal_numbers_or_abbreviations_in_text() {
        let sentences = split_sentences("El valor es 3.14 y termina aquí. Fin.");
        assert_eq!(sentences, vec!["El valor es 3.14 y termina aquí.", "Fin."]);
    }

    #[test]
    fn split_sentences_handles_ellipsis_and_trimming() {
        let sentences = split_sentences("  Uno... Dos!!  Tres?  ");
        assert_eq!(sentences, vec!["Uno...", "Dos!!", "Tres?"]);
    }

    #[test]
    fn split_sentences_empty_or_whitespace_only_yields_empty() {
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   \n\t  ").is_empty());
    }

    #[test]
    fn split_sentences_no_terminators_yields_single_chunk() {
        let sentences = split_sentences("una sola oración sin puntuación");
        assert_eq!(sentences, vec!["una sola oración sin puntuación"]);
    }

    #[test]
    fn merge_wav_parts_concatenates_pcm_payloads() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let part_a = tmp.path().join("a.wav");
        let part_b = tmp.path().join("b.wav");
        let merged = tmp.path().join("merged.wav");

        let mut data_a = build_wav_header(16000, 1, 16, 8000);
        data_a.extend_from_slice(&[0xAA; 8000]);
        std::fs::write(&part_a, &data_a).expect("write part a");

        let mut data_b = build_wav_header(16000, 1, 16, 16000);
        data_b.extend_from_slice(&[0xBB; 16000]);
        std::fs::write(&part_b, &data_b).expect("write part b");

        let size = merge_wav_parts(&[part_a, part_b], &merged).expect("merge");
        assert_eq!(size, data_a.len() as u64 + 16000);

        let bytes = std::fs::read(&merged).expect("read merged");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as u64,
            size - 8
        );
        // Duration of the merged stream is the sum of the parts.
        assert!((parse_wav_duration_secs(&bytes) - 0.75).abs() < 1e-9);
        // The data payload is exactly part A's payload followed by part B's.
        let data_pos = bytes
            .windows(4)
            .position(|window| window == b"data")
            .map(|pos| pos + 8)
            .expect("data chunk");
        let payload = &bytes[data_pos..];
        assert_eq!(payload.len(), 24000);
        assert!(payload[..8000].iter().all(|byte| *byte == 0xAA));
        assert!(payload[8000..].iter().all(|byte| *byte == 0xBB));
    }

    #[test]
    fn merge_wav_parts_single_part_copies_file() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let part = tmp.path().join("single.wav");
        let merged = tmp.path().join("merged.wav");
        let mut wav = build_wav_header(16000, 1, 16, 4000);
        wav.extend_from_slice(&[0x11; 4000]);
        std::fs::write(&part, &wav).expect("write part");

        let size = merge_wav_parts(&[part], &merged).expect("merge");
        assert_eq!(size, wav.len() as u64);
        let bytes = std::fs::read(&merged).expect("read merged");
        assert_eq!(bytes, wav);
    }

    #[test]
    fn merge_wav_parts_rejects_non_wav_input() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let garbage = tmp.path().join("garbage.bin");
        let merged = tmp.path().join("merged.wav");
        std::fs::write(&garbage, b"this is not a wav").expect("write garbage");
        let error = merge_wav_parts(&[garbage], &merged).expect_err("must fail");
        assert!(error.contains("not a WAV"), "unexpected error: {error}");
    }
}