//! Local WAV -> MP3 conversion.
//!
//! The LAME encoder is compiled from source into the binary via
//! `mp3lame-encoder` / `mp3lame-sys`, so conversion never shells out to an
//! external ffmpeg/lame binary and never depends on the operating system. The
//! same release build works on Windows (.exe), Linux (portable tarball), and macOS.
//!
//! The flow is: parse the WAV `fmt ` + `data` chunks, feed the 16-bit PCM
//! payload to LAME in bounded chunks (so multi-hour audiobooks never load into
//! memory), stream the MP3 frames to disk, and report real progress through a
//! `mp3-progress` event.

use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, InterleavedPcm, MonoPcm, Quality};
use serde::Serialize;
use tauri::Emitter;

/// MP3 constant bitrate (kbps). 128 kbps mono is transparent for
/// Piper-quality speech and keeps file sizes small.
const MP3_BITRATE_KBPS: Bitrate = Bitrate::Kbps128;
/// LAME encoding quality (0 = best .. 9 = worst). 3 balances speed and quality
/// for near-real-time conversion of long audiobooks.
const MP3_QUALITY: Quality = Quality::VeryNice;
/// PCM frames encoded per chunk (~0.37 s of mono audio at 22.05 kHz).
const CHUNK_FRAMES: usize = 8192;

/// Payload returned by `convert_wav_to_mp3`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Mp3Result {
    pub mp3_path: String,
    pub size_bytes: u64,
    /// Source audio duration in seconds (derived from the WAV payload).
    pub duration_secs: f64,
}

/// Progress emitted on the `mp3-progress` event while conversion runs. `token`
/// is the caller-chosen identifier ("direct" for the convert panel, the queue
/// item id for queue conversions) so the frontend routes the update to the
/// right progress bar.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Mp3Progress {
    pub token: String,
    pub percent: f64,
}

/// Parsed WAV layout needed for encoding: the `fmt ` parameters and the exact
/// `data` payload range (offset + length in bytes).
#[derive(Debug, Clone, Copy)]
struct WavInfo {
    audio_format: u16,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    data_offset: u64,
    data_len: u64,
}

/// Walk the RIFF/WAVE chunks of `path` and return the `fmt ` parameters plus
/// the `data` chunk payload range. Reads only chunk headers, never the audio
/// payload, so large files parse instantly.
fn parse_wav(path: &Path) -> Result<WavInfo, String> {
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
        return Err(format!("{} is not a WAV file.", path.display()));
    }

    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (format, channels, rate, bits)
    let mut data: Option<(u64, u64)> = None;

    let mut pos: u64 = 12;
    while pos + 8 <= file_len {
        file.seek(SeekFrom::Start(pos))
            .map_err(|error| format!("seek {}: {error}", path.display()))?;
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk)
            .map_err(|error| format!("read {} chunk: {error}", path.display()))?;
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        match &chunk[0..4] {
            b"fmt " => {
                if size < 16 || pos + 8 + 16 > file_len {
                    return Err(format!("{} has an invalid fmt chunk.", path.display()));
                }
                let mut payload = [0u8; 16];
                file.read_exact(&mut payload)
                    .map_err(|error| format!("read {} fmt: {error}", path.display()))?;
                fmt = Some((
                    u16::from_le_bytes([payload[0], payload[1]]),
                    u16::from_le_bytes([payload[2], payload[3]]),
                    u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]),
                    u16::from_le_bytes([payload[14], payload[15]]),
                ));
            }
            b"data" => {
                data = Some((pos + 8, size));
            }
            _ => {}
        }
        pos += 8 + size + (size % 2);
    }

    let (audio_format, channels, sample_rate, bits_per_sample) = fmt
        .ok_or_else(|| format!("{} has no fmt chunk.", path.display()))?;
    let (data_offset, data_len) =
        data.ok_or_else(|| format!("{} has no data chunk.", path.display()))?;

    Ok(WavInfo {
        audio_format,
        sample_rate,
        channels,
        bits_per_sample,
        data_offset,
        data_len,
    })
}

/// Encode the PCM payload of `wav_path` into an MP3 at `mp3_path` using the
/// embedded LAME encoder. `on_progress` is invoked with a 0..100 percentage
/// after every chunk whose rounded value changes.
///
/// The payload is streamed in bounded chunks (never loaded whole), so a
/// multi-hour WAV converts with constant memory use. Used by the frontend
/// command below and by the queue's automatic post-synthesis conversion.
pub(crate) fn convert_to_mp3(
    wav_path: &Path,
    mp3_path: &Path,
    on_progress: &mut dyn FnMut(f64),
) -> Result<Mp3Result, String> {
    let info = parse_wav(wav_path)?;
    if info.audio_format != 1 {
        return Err(format!(
            "{} is not PCM audio; only PCM WAV files can be converted.",
            wav_path.display()
        ));
    }
    if info.bits_per_sample != 16 {
        return Err(format!(
            "{} is {}-bit audio; only 16-bit PCM WAV files can be converted.",
            wav_path.display(),
            info.bits_per_sample
        ));
    }
    if info.channels != 1 && info.channels != 2 {
        return Err(format!(
            "{} has {} channels; only mono and stereo audio can be converted.",
            wav_path.display(),
            info.channels
        ));
    }
    if info.sample_rate == 0 {
        return Err(format!("{} has an invalid sample rate.", wav_path.display()));
    }

    // Trim any trailing bytes that do not form a whole PCM frame so the
    // chunk loop below never reads a partial sample pair.
    let frame_bytes = u64::from(info.channels) * 2;
    let data_len = info.data_len - (info.data_len % frame_bytes);
    if data_len == 0 {
        return Err(format!("{} has no audio data.", wav_path.display()));
    }

    let mut encoder = Builder::new()
        .ok_or_else(|| "failed to allocate the MP3 encoder".to_string())?
        .with_num_channels(info.channels as u8)
        .map_err(|error| format!("set MP3 channels: {error}"))?
        .with_sample_rate(info.sample_rate)
        .map_err(|error| format!("set MP3 sample rate: {error}"))?
        .with_brate(MP3_BITRATE_KBPS)
        .map_err(|error| format!("set MP3 bitrate: {error}"))?
        .with_quality(MP3_QUALITY)
        .map_err(|error| format!("set MP3 quality: {error}"))?
        .build()
        .map_err(|error| format!("initialize the MP3 encoder: {error}"))?;

    let channels = usize::from(info.channels);
    let samples_per_chunk = CHUNK_FRAMES * channels;
    let mut raw = vec![0u8; samples_per_chunk * 2];
    let mut pcm = vec![0i16; samples_per_chunk];
    let mut out_buf = Vec::new();
    out_buf.reserve(mp3lame_encoder::max_required_buffer_size(samples_per_chunk));

    if let Some(parent) = mp3_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create output directory: {error}"))?;
    }
    let mut out_file = BufWriter::new(std::fs::File::create(mp3_path).map_err(|error| {
        format!("create {}: {error}", mp3_path.display())
    })?);

    let mut src = std::fs::File::open(wav_path)
        .map_err(|error| format!("open {}: {error}", wav_path.display()))?;
    src.seek(SeekFrom::Start(info.data_offset))
        .map_err(|error| format!("seek {}: {error}", wav_path.display()))?;

    let mut remaining = data_len;
    let mut last_percent = -1i32;
    while remaining > 0 {
        let want = usize::min(remaining as usize, raw.len());
        src.read_exact(&mut raw[..want])
            .map_err(|error| format!("read {}: {error}", wav_path.display()))?;

        // Little-endian 16-bit PCM bytes -> i16 samples.
        let sample_count = want / 2;
        for (index, sample) in pcm[..sample_count].iter_mut().enumerate() {
            *sample = i16::from_le_bytes([raw[2 * index], raw[2 * index + 1]]);
        }

        out_buf.clear();
        let encoded = if channels == 1 {
            encoder.encode_to_vec(MonoPcm(&pcm[..sample_count]), &mut out_buf)
        } else {
            encoder.encode_to_vec(InterleavedPcm(&pcm[..sample_count]), &mut out_buf)
        }
        .map_err(|error| format!("encode MP3: {error}"))?;
        out_file
            .write_all(&out_buf[..encoded])
            .map_err(|error| format!("write {}: {error}", mp3_path.display()))?;

        remaining -= want as u64;
        let percent = ((1.0 - remaining as f64 / data_len as f64) * 100.0) as i32;
        if percent != last_percent {
            last_percent = percent;
            on_progress(percent as f64);
        }
    }

    out_buf.clear();
    let tail = encoder
        .flush_to_vec::<FlushNoGap>(&mut out_buf)
        .map_err(|error| format!("finalize MP3: {error}"))?;
    if tail > 0 {
        out_file
            .write_all(&out_buf[..tail])
            .map_err(|error| format!("write {}: {error}", mp3_path.display()))?;
    }
    out_file
        .flush()
        .map_err(|error| format!("flush {}: {error}", mp3_path.display()))?;
    on_progress(100.0);

    let size_bytes = std::fs::metadata(mp3_path)
        .map(|meta| meta.len())
        .map_err(|error| format!("stat {}: {error}", mp3_path.display()))?;
    let byte_rate = f64::from(info.sample_rate) * f64::from(info.channels) * 2.0;
    let duration_secs = if byte_rate > 0.0 {
        data_len as f64 / byte_rate
    } else {
        0.0
    };

    Ok(Mp3Result {
        mp3_path: mp3_path.to_string_lossy().into_owned(),
        size_bytes,
        duration_secs,
    })
}

/// Frontend command: convert a WAV file to MP3 using the embedded LAME
/// encoder. `token` identifies the caller so the emitted `mp3-progress`
/// events can be routed to the right progress bar. Runs the blocking encode
/// on the async runtime's blocking pool so the UI never freezes.
#[tauri::command]
pub async fn convert_wav_to_mp3(
    app: tauri::AppHandle,
    wav_path: String,
    mp3_path: String,
    token: Option<String>,
) -> Result<serde_json::Value, String> {
    let wav = PathBuf::from(&wav_path);
    let mp3 = PathBuf::from(&mp3_path);
    if !wav.is_file() {
        return Err(format!("WAV file not found: {}", wav.display()));
    }

    let token = token.unwrap_or_else(|| "mp3".to_string());
    let app = app.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        convert_to_mp3(&wav, &mp3, &mut |percent: f64| {
            let _ = app.emit(
                "mp3-progress",
                Mp3Progress {
                    token: token.clone(),
                    percent,
                },
            );
        })
    })
    .await
    .map_err(|error| format!("MP3 conversion task failed: {error}"))?;

    let result = result?;
    serde_json::to_value(result)
        .map_err(|error| format!("failed to serialize MP3 result: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal 16-bit PCM WAV for `data_bytes` bytes of payload,
    /// followed by the payload itself.
    fn build_wav(sample_rate: u32, channels: u16, bits_per_sample: u16, data: &[u8]) -> Vec<u8> {
        let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample / 8);
        let block_align = channels * (bits_per_sample / 8);
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
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
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(data);
        wav
    }

    #[test]
    fn parse_wav_reads_fmt_and_data_layout() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("voice.wav");
        let mut data = build_wav(22050, 1, 16, &vec![0u8; 44100]);
        data.extend_from_slice(b"junk-after-data");
        std::fs::write(&path, &data).expect("write wav");

        let info = parse_wav(&path).expect("parse");
        assert_eq!(info.audio_format, 1);
        assert_eq!(info.sample_rate, 22050);
        assert_eq!(info.channels, 1);
        assert_eq!(info.bits_per_sample, 16);
        assert_eq!(info.data_len, 44100);
        assert!(info.data_offset > 40);
    }

    #[test]
    fn parse_wav_rejects_non_wav_input() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("garbage.bin");
        std::fs::write(&path, b"this is not a wav").expect("write garbage");
        let error = parse_wav(&path).expect_err("must fail");
        assert!(error.contains("not a WAV"), "unexpected error: {error}");
    }

    #[test]
    fn parse_wav_rejects_missing_data_chunk() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let path = tmp.path().join("no-data.wav");
        // A WAV with a valid fmt chunk but no data chunk.
        let mut wav = b"RIFF\x28\x00\x00\x00WAVEfmt ".to_vec();
        wav.extend_from_slice(&16u32.to_le_bytes());
        // fmt payload: PCM(1), mono, 22050 Hz, byte rate, block align, 16-bit.
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&22050u32.to_le_bytes());
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        std::fs::write(&path, &wav).expect("write");
        let error = parse_wav(&path).expect_err("must fail");
        assert!(error.contains("no data chunk"), "unexpected error: {error}");
    }

    #[test]
    fn convert_wav_encodes_pcm_to_mp3() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let wav_path = tmp.path().join("in.wav");
        let mp3_path = tmp.path().join("out.mp3");

        // One second of a 440 Hz sine at 22.05 kHz mono, 16-bit PCM.
        let mut samples = Vec::with_capacity(22050 * 2);
        for i in 0..22050 {
            let value = ((i as f64 * 440.0 * 2.0 * std::f64::consts::PI / 22050.0).sin()
                * 10000.0) as i16;
            samples.extend_from_slice(&value.to_le_bytes());
        }
        std::fs::write(&wav_path, build_wav(22050, 1, 16, &samples)).expect("write wav");

        let mut progress = Vec::new();
        let result = convert_to_mp3(&wav_path, &mp3_path, &mut |percent| {
            progress.push(percent);
        })
        .expect("convert");

        assert_eq!(result.mp3_path, mp3_path.to_string_lossy().into_owned());
        assert!(result.size_bytes > 100, "mp3 too small: {}", result.size_bytes);
        assert!((result.duration_secs - 1.0).abs() < 0.01);

        let bytes = std::fs::read(&mp3_path).expect("read mp3");
        assert_eq!(bytes[0], 0xFF, "MP3 must start with an MPEG frame sync");
        assert!(!progress.is_empty(), "progress must be reported");
        assert_eq!(*progress.last().unwrap(), 100.0);
    }

    #[test]
    fn convert_handles_stereo_interleaved_pcm() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let wav_path = tmp.path().join("in-stereo.wav");
        let mp3_path = tmp.path().join("out-stereo.mp3");

        // Half a second of stereo silence (two channels interleaved).
        let samples = vec![0u8; 22050 / 2 * 2 * 2];
        std::fs::write(&wav_path, build_wav(22050, 2, 16, &samples)).expect("write wav");

        let result = convert_to_mp3(&wav_path, &mp3_path, &mut |_| {}).expect("convert");
        assert!(result.size_bytes > 100);
        assert!((result.duration_secs - 0.5).abs() < 0.01);
    }

    #[test]
    fn convert_rejects_8bit_wav() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let wav_path = tmp.path().join("in-8bit.wav");
        let mp3_path = tmp.path().join("out-8bit.mp3");
        std::fs::write(&wav_path, build_wav(22050, 1, 8, &vec![0u8; 44100]))
            .expect("write wav");

        let error = convert_to_mp3(&wav_path, &mp3_path, &mut |_| {}).expect_err("must fail");
        assert!(error.contains("16-bit"), "unexpected error: {error}");
    }

    #[test]
    fn convert_rejects_non_pcm_wav() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let wav_path = tmp.path().join("in-float.wav");
        let mp3_path = tmp.path().join("out-float.mp3");
        let mut wav = build_wav(22050, 1, 16, &vec![0u8; 44100]);
        wav[20..22].copy_from_slice(&3u16.to_le_bytes()); // IEEE float format
        std::fs::write(&wav_path, &wav).expect("write wav");

        let error = convert_to_mp3(&wav_path, &mp3_path, &mut |_| {}).expect_err("must fail");
        assert!(error.contains("PCM"), "unexpected error: {error}");
    }

    #[test]
    fn convert_rejects_missing_source() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let missing = tmp.path().join("missing.wav");
        let mp3_path = tmp.path().join("out.mp3");
        let error = convert_to_mp3(&missing, &mp3_path, &mut |_| {}).expect_err("must fail");
        assert!(error.contains("open"), "unexpected error: {error}");
    }
}
