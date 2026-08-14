//! Streaming model download with progress, md5 verification, cancel, and
//! delete+retry (REQ-LIB-3, REQ-LIB-4, REQ-LIB-5, REQ-LIB-6).
//!
//! `download` performs a single attempt: the Content-Length header drives
//! `bytes_total`, per-chunk reads produce monotonic `DownloadProgress`
//! snapshots (bytes done, total, percent, speed, ETA), and a
//! `CancellationToken` stops the transfer mid-flight, deleting the partial
//! file. When the catalog supplies an md5 digest (REQ-LIB-4), the finished
//! file is verified against it; a mismatch reports `Md5Mismatch` and removes
//! the corrupt file so a retry starts clean (REQ-LIB-5).
//!
//! `download_with_retry` wraps `download` in a bounded retry loop: failed or
//! corrupt attempts delete the partial file and re-download; user
//! cancellation is never retried (REQ-LIB-6).
//!
//! Progress is delivered through an `on_progress` callback; the events task
//! forwards these snapshots to the Tauri event emitter and the frontend.

use std::fs;
use std::path::Path;
use std::time::Instant;

use reqwest::Client;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

/// Progress snapshot emitted after every received chunk (REQ-LIB-3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DownloadProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub percent: f64,
    pub speed_bps: f64,
    pub eta_s: f64,
}

/// Download failure modes (REQ-LIB-4, REQ-LIB-5, REQ-LIB-6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// Transport-level failure (connect, TLS, chunk read).
    Http(String),
    /// The server answered with a non-2xx status.
    HttpStatus(u16),
    /// Local file system failure.
    Io(String),
    /// Finished file did not match the catalog md5 (REQ-LIB-4); the corrupt
    /// file was already removed.
    Md5Mismatch { expected: String, actual: String },
    /// User cancelled the transfer (REQ-LIB-6); the partial file was removed.
    Cancelled,
}

impl DownloadError {
    /// Whether a retry can plausibly fix this failure (everything except user
    /// cancellation).
    pub fn is_retryable(&self) -> bool {
        !matches!(self, DownloadError::Cancelled)
    }
}

/// A fresh client per call keeps tests hermetic (no connection pooling across
/// independent test servers) at negligible cost for one-shot downloads.
///
/// `.no_proxy()` forces direct connections: the local test server must never
/// be routed through a system/environment proxy (a proxied environment would
/// answer 503 for `127.0.0.1`), and model downloads go straight to the
/// HuggingFace host.
fn client() -> Client {
    Client::builder()
        .no_proxy()
        .build()
        .expect("reqwest client build")
}

/// Single download attempt (REQ-LIB-3, REQ-LIB-4, REQ-LIB-6).
///
/// The destination file is created only after a successful handshake, and is
/// removed on cancellation or md5 mismatch so no partial/corrupt file is ever
/// left behind.
pub async fn download(
    url: &str,
    dest: &Path,
    expected_md5: Option<&str>,
    token: &CancellationToken,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<(), DownloadError> {
    if token.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }

    let mut response = client()
        .get(url)
        .send()
        .await
        .map_err(|err| DownloadError::Http(err.to_string()))?;

    if !response.status().is_success() {
        return Err(DownloadError::HttpStatus(response.status().as_u16()));
    }

    let bytes_total = response.content_length().unwrap_or(0);
    let started = Instant::now();
    let mut bytes_done: u64 = 0;
    let mut file = fs::File::create(dest)
        .map_err(|err| DownloadError::Io(format!("create {}: {err}", dest.display())))?;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| DownloadError::Http(err.to_string()))?
    {
        if token.is_cancelled() {
            drop(file);
            let _ = fs::remove_file(dest);
            return Err(DownloadError::Cancelled);
        }
        bytes_done += chunk.len() as u64;
        std::io::Write::write_all(&mut file, &chunk)
            .map_err(|err| DownloadError::Io(format!("write {}: {err}", dest.display())))?;
        on_progress(progress_snapshot(
            bytes_done,
            bytes_total,
            started.elapsed().as_secs_f64(),
        ));
    }

    drop(file);

    if token.is_cancelled() {
        let _ = fs::remove_file(dest);
        return Err(DownloadError::Cancelled);
    }

    if let Some(expected) = expected_md5 {
        let actual = file_md5(dest)?;
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            let _ = fs::remove_file(dest);
            return Err(DownloadError::Md5Mismatch {
                expected: expected.trim().to_string(),
                actual,
            });
        }
    }

    Ok(())
}

/// Bounded retry loop over `download` (REQ-LIB-5): failed or corrupt attempts
/// delete the partial file and re-download; cancellation propagates without
/// retrying (REQ-LIB-6).
pub async fn download_with_retry(
    url: &str,
    dest: &Path,
    expected_md5: Option<&str>,
    token: &CancellationToken,
    max_attempts: usize,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<(), DownloadError> {
    let mut attempts = 0usize;
    loop {
        if token.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        attempts += 1;
        match download(url, dest, expected_md5, token, &mut on_progress).await {
            Ok(()) => return Ok(()),
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            Err(err) => {
                let _ = fs::remove_file(dest);
                if attempts >= max_attempts {
                    return Err(err);
                }
            }
        }
    }
}

/// Compute the snapshot for one progress emission (REQ-LIB-3).
fn progress_snapshot(bytes_done: u64, bytes_total: u64, elapsed_s: f64) -> DownloadProgress {
    let percent = if bytes_total > 0 {
        (bytes_done as f64 / bytes_total as f64) * 100.0
    } else {
        0.0
    };
    let speed_bps = if elapsed_s > 0.0 {
        bytes_done as f64 / elapsed_s
    } else {
        0.0
    };
    let remaining = bytes_total.saturating_sub(bytes_done);
    let eta_s = if speed_bps > 0.0 {
        remaining as f64 / speed_bps
    } else {
        0.0
    };
    DownloadProgress {
        bytes_done,
        bytes_total,
        percent,
        speed_bps,
        eta_s,
    }
}

/// Hex-encoded md5 of a file on disk (REQ-LIB-4).
fn file_md5(path: &Path) -> Result<String, DownloadError> {
    use md5::{Digest, Md5};
    let bytes = fs::read(path)
        .map_err(|err| DownloadError::Io(format!("read {}: {err}", path.display())))?;
    let digest = Md5::digest(&bytes);
    Ok(digest.iter().map(|byte| format!("{:02x}", byte)).collect())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;

    use super::*;

    /// Hex-encoded md5 of a byte slice (test helper).
    fn md5_hex(bytes: &[u8]) -> String {
        use md5::{Digest, Md5};
        let digest = Md5::digest(bytes);
        digest.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Tiny HTTP/1.1 server for the tests: serves `body` with a Content-Length
    /// header. The first `fail_first` requests answer 500 (retry tests);
    /// `chunk_delay_ms` throttles writes so cancellation can be observed
    /// mid-transfer. Returns the bound address and a request counter.
    async fn spawn_test_server(
        body: Vec<u8>,
        fail_first: usize,
        chunk_delay_ms: u64,
    ) -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();

        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let body = body.clone();
                let counter = counter.clone();
                tokio::spawn(async move {
                    // Read the request head (everything up to CRLF CRLF).
                    let mut buf = [0u8; 2048];
                    let mut head = Vec::new();
                    loop {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => {
                                head.extend_from_slice(&buf[..n]);
                                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                                if head.len() > 16 * 1024 {
                                    return;
                                }
                            }
                        }
                    }

                    let request_no = counter.fetch_add(1, Ordering::SeqCst);
                    if request_no < fail_first {
                        let _ = sock
                            .write_all(
                                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .await;
                        let _ = sock.shutdown().await;
                        return;
                    }

                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    if sock.write_all(header.as_bytes()).await.is_err() {
                        return;
                    }
                    for chunk in body.chunks(4096) {
                        if sock.write_all(chunk).await.is_err() {
                            return; // client cancelled / dropped the connection
                        }
                        if chunk_delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(chunk_delay_ms)).await;
                        }
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });

        (addr, requests)
    }

    #[tokio::test]
    async fn streams_progress_monotonically_and_verifies_md5() {
        let body: Vec<u8> = b"piper-tts-gui test payload ".repeat(10_000); // ~240 KB
        let expected_md5 = md5_hex(&body);
        let (addr, _requests) = spawn_test_server(body.clone(), 0, 0).await;

        let tmp = tempfile::tempdir().expect("temp dir");
        let dest = tmp.path().join("en_US-lessac-medium.onnx");
        let token = CancellationToken::new();
        let mut progress = Vec::new();

        download(
            &format!("http://{addr}/en_US-lessac-medium.onnx"),
            &dest,
            Some(&expected_md5),
            &token,
            |p| progress.push(p),
        )
        .await
        .expect("download succeeds");

        assert!(progress.len() >= 2, "several progress events expected");
        let mut last_done = 0u64;
        for event in &progress {
            assert!(event.bytes_done >= last_done, "bytes done must be monotonic");
            last_done = event.bytes_done;
            assert_eq!(event.bytes_total, body.len() as u64);
            assert!((0.0..=100.0).contains(&event.percent));
            assert!(event.speed_bps >= 0.0);
            assert!(event.eta_s >= 0.0);
        }
        let last = progress.last().expect("last event");
        assert_eq!(last.bytes_done, body.len() as u64);
        assert!((last.percent - 100.0).abs() < f64::EPSILON);

        // File landed and matches the served bytes.
        let on_disk = std::fs::read(&dest).expect("file on disk");
        assert_eq!(on_disk, body);
    }

    #[tokio::test]
    async fn md5_mismatch_reports_corrupt_and_removes_file() {
        let body: Vec<u8> = b"payload".repeat(1000);
        let (addr, _requests) = spawn_test_server(body.clone(), 0, 0).await;
        let tmp = tempfile::tempdir().expect("temp dir");
        let dest = tmp.path().join("model.onnx");
        let token = CancellationToken::new();

        let err = download(
            &format!("http://{addr}/model.onnx"),
            &dest,
            Some("00000000000000000000000000000000"), // wrong md5
            &token,
            |_| {},
        )
        .await
        .expect_err("md5 mismatch must fail");

        assert!(matches!(err, DownloadError::Md5Mismatch { .. }));
        assert!(!dest.exists(), "corrupt file must be removed for a clean retry");
    }

    #[tokio::test]
    async fn retry_redownloads_after_failure() {
        let body: Vec<u8> = b"retry me ".repeat(2000);
        let expected_md5 = md5_hex(&body);
        let (addr, requests) = spawn_test_server(body.clone(), 1, 0).await; // first request -> 500
        let tmp = tempfile::tempdir().expect("temp dir");
        let dest = tmp.path().join("model.onnx");
        let token = CancellationToken::new();

        download_with_retry(
            &format!("http://{addr}/model.onnx"),
            &dest,
            Some(&expected_md5),
            &token,
            3,
            |_| {},
        )
        .await
        .expect("retry succeeds");

        assert_eq!(requests.load(Ordering::SeqCst), 2, "exactly one retry");
        assert_eq!(std::fs::read(&dest).expect("file on disk"), body);
    }

    #[tokio::test]
    async fn retry_exhaustion_returns_last_error() {
        let (addr, requests) = spawn_test_server(Vec::new(), 999, 0).await; // always 500
        let tmp = tempfile::tempdir().expect("temp dir");
        let dest = tmp.path().join("model.onnx");
        let token = CancellationToken::new();

        let err = download_with_retry(
            &format!("http://{addr}/model.onnx"),
            &dest,
            None,
            &token,
            2,
            |_| {},
        )
        .await
        .expect_err("all attempts fail");

        assert!(matches!(err, DownloadError::HttpStatus(500)));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancel_stops_mid_transfer_and_removes_partial() {
        let body: Vec<u8> = b"cancellable ".repeat(20_000); // ~240 KB
        let (addr, _requests) = spawn_test_server(body.clone(), 0, 5).await; // 5 ms per 4 KB chunk
        let tmp = tempfile::tempdir().expect("temp dir");
        let dest = tmp.path().join("model.onnx");
        let token = CancellationToken::new();
        let cancel_token = token.clone();

        let mut observed = Vec::new();
        let err = download(
            &format!("http://{addr}/model.onnx"),
            &dest,
            Some(&md5_hex(&body)),
            &cancel_token,
            |p| {
                observed.push(p.bytes_done);
                if p.bytes_done > 0 {
                    cancel_token.cancel();
                }
            },
        )
        .await
        .expect_err("cancel must stop the download");

        assert!(matches!(err, DownloadError::Cancelled));
        assert!(!dest.exists(), "partial file must be deleted on cancel");
        let last_done = observed.last().copied().unwrap_or(0);
        assert!(last_done > 0, "transfer must have started");
        assert!(last_done < body.len() as u64, "must stop mid-transfer");
    }

    #[tokio::test]
    async fn cancelled_token_aborts_before_request() {
        let token = CancellationToken::new();
        token.cancel();
        let tmp = tempfile::tempdir().expect("temp dir");
        let err = download(
            "http://127.0.0.1:9/unused.onnx",
            &tmp.path().join("model.onnx"),
            None,
            &token,
            |_| {},
        )
        .await
        .expect_err("pre-cancelled token must abort");
        assert!(matches!(err, DownloadError::Cancelled));
    }

    #[tokio::test]
    async fn progress_flows_through_event_channel_pattern() {
        // Same shape the Tauri event emitter (events task) will forward:
        // progress payloads travel through the app's event channel.
        let body: Vec<u8> = b"channel events ".repeat(2000);
        let (addr, _requests) = spawn_test_server(body.clone(), 0, 0).await;
        let tmp = tempfile::tempdir().expect("temp dir");
        let dest = tmp.path().join("model.onnx");
        let token = CancellationToken::new();
        let channel = crate::state::EventChannel::default();

        download(
            &format!("http://{addr}/model.onnx"),
            &dest,
            Some(&md5_hex(&body)),
            &token,
            |p| {
                let _ = channel.send(format!(
                    "progress:{}:{}:{}:{}:{}",
                    p.bytes_done, p.bytes_total, p.percent, p.speed_bps, p.eta_s
                ));
            },
        )
        .await
        .expect("download");

        let event = channel.recv().expect("first progress event");
        let fields: Vec<&str> = event.split(':').collect();
        assert_eq!(fields[0], "progress");
        assert_eq!(fields.len(), 6, "progress + 5 numeric fields");
        let done: u64 = fields[1].parse().expect("bytes done numeric");
        let total: u64 = fields[2].parse().expect("bytes total numeric");
        assert_eq!(total, body.len() as u64);
        assert!(done <= total);
    }
}