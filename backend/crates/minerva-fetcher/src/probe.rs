//! Slide source selection. Play has no slide-role metadata, so a lecture
//! with several sources is resolved from pixels: frames at 20%, 50% and 80%
//! of each source are OCR'd and the source with the most, and largest,
//! confidently read text wins. Audio flags and source names are never used.
//!
//! ffprobe/ffmpeg read through a loopback proxy that caps each source at
//! 24 MiB of upstream bytes (MP4 index and tail reads included) and caches
//! aligned blocks, so the index the four processes each reread is fetched
//! once. Bearer URLs never reach subprocess arguments, logs or stored
//! errors; every failure is one of the fixed reasons below.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::serve::ListenerExt;
use chrono::Utc;
use futures::{StreamExt, TryStreamExt};
use minerva_app_core::visual_extraction::{self as ve, Candidate};
use minerva_db::queries::visual_extraction as queue;
use sqlx::PgPool;
use tokio::process::Command;
use tokio::sync::{OnceCell, Semaphore};

const PROBE_BYTES: u64 = 24 * 1024 * 1024;
const BLOCK_BYTES: u64 = 256 * 1024;
/// Loopback socket buffers on both ends of the proxy. ffmpeg reads open
/// ranges and hangs up once it has its frame; every byte buffered by then
/// was fetched from Play for nothing. Default buffers grow to megabytes, and
/// with Play a LAN hop away they fill before ffmpeg hangs up, exhausting
/// the budget. Small buffers keep each connection about a block ahead.
const SOCKET_BUFFER_BYTES: usize = 64 * 1024;
const SOCKET_BUFFER_ARG: &str = "65536";
const FRAME_FRACTIONS: [f64; 3] = [0.2, 0.5, 0.8];
/// Sources of one lecture probed at once; Play lectures have up to three.
const SOURCE_PROBES: usize = 3;
/// Below this no source is taken to show slides, so none is guessed.
const MIN_SLIDE_SCORE: f64 = 20.0;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(45);
/// ffprobe/ffmpeg/tesseract processes at once across all probes. Measured
/// peaks: ffmpeg grabbing a 720p frame ~70 MiB (1080p ~115 MiB), tesseract
/// ~45 MiB, so four keep the pod well inside its limit on a tight node.
static TOOL_SLOTS: Semaphore = Semaphore::const_new(4);
const LECTURE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// A probe claim older than this belonged to a probe that died.
const PROBE_STALE_SECS: i64 = 15 * 60;

type Reason = &'static str;

/// The slide source exists but can never be staged; retrying cannot help.
const TOO_LARGE: Reason = "slide source larger than the staging cap";
const BUDGET_EXHAUSTED: Reason = "probe byte budget exhausted";

/// Probe the earliest-expiring undecided lecture and queue its slide
/// source for download. False when there is nothing to probe.
pub async fn probe_tick(db: &PgPool) -> Result<bool, sqlx::Error> {
    let Some(claim) = queue::claim_probe(db, PROBE_STALE_SECS).await? else {
        return Ok(false);
    };
    let started = std::time::Instant::now();
    let picked = match serde_json::from_value::<Vec<Candidate>>(claim.candidates.clone()) {
        Ok(candidates) => pick(candidates, claim.deadline).await,
        Err(_) => Err("invalid candidates"),
    };
    let outcome = match picked {
        Ok(chosen) => {
            if queue::choose_download(db, &claim, &chosen.download_url, chosen.size_bytes).await? {
                "chosen"
            } else {
                "stale"
            }
        }
        Err(reason) => {
            tracing::warn!(job_id = %claim.id, reason, "slide source probe failed");
            let error = format!("slide source probe failed ({reason})");
            if queue::fail_probe(db, &claim, &error, reason != TOO_LARGE).await? {
                "failed"
            } else {
                "stale"
            }
        }
    };
    metrics::counter!("visual_extraction_probes_total", "outcome" => outcome).increment(1);
    metrics::histogram!("visual_extraction_probe_duration_seconds", "outcome" => outcome)
        .record(started.elapsed().as_secs_f64());
    Ok(true)
}

async fn pick(
    candidates: Vec<Candidate>,
    deadline: chrono::DateTime<Utc>,
) -> Result<Candidate, Reason> {
    for candidate in &candidates {
        ve::validate_download_url(&candidate.probe_url, deadline)?;
        ve::validate_download_url(&candidate.download_url, deadline)?;
    }
    if candidates.len() < 2 {
        return candidates.into_iter().next().ok_or("no candidates");
    }
    let budget = (deadline - Utc::now())
        .to_std()
        .unwrap_or_default()
        .min(LECTURE_TIMEOUT);
    // Every source must be scored: a failed slide probe must not make the
    // camera the winner by default.
    let urls: Vec<String> = candidates.iter().map(|c| c.probe_url.clone()).collect();
    let scores: Vec<f64> = tokio::time::timeout(
        budget,
        futures::stream::iter(urls)
            .map(|url| async move { score_source(&url).await })
            .buffered(SOURCE_PROBES)
            .try_collect(),
    )
    .await
    .map_err(|_| "probe timed out")??;
    let best = choose(&scores)?;
    let chosen = candidates
        .into_iter()
        .nth(best)
        .expect("one score per candidate");
    if chosen.size_bytes > Some(ve::FETCH_ESTIMATE_BYTES) {
        return Err(TOO_LARGE);
    }
    Ok(chosen)
}

/// Index of the source to stage.
fn choose(scores: &[f64]) -> Result<usize, Reason> {
    let (best, score) = scores
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .ok_or("no candidates")?;
    if scores.len() > 1 && score < MIN_SLIDE_SCORE {
        return Err("no readable slide source found after frame probes");
    }
    Ok(best)
}

async fn score_source(url: &str) -> Result<f64, Reason> {
    let proxy = Proxy::start(url).await?;
    // A tool cut off by the byte budget only reports that it failed.
    let budget = |reason| {
        if proxy.blocks.exhausted() {
            BUDGET_EXHAUSTED
        } else {
            reason
        }
    };
    let info = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-recv_buffer_size",
            SOCKET_BUFFER_ARG,
            "-show_entries",
            "format=duration",
            "-of",
            "json",
            &proxy.url,
        ],
        None,
    )
    .await
    .map_err(budget)?;
    let duration = serde_json::from_slice::<serde_json::Value>(&info)
        .ok()
        .and_then(|info| info["format"]["duration"].as_str()?.parse::<f64>().ok())
        .filter(|d| *d > 0.0 && *d < 24.0 * 3600.0)
        .ok_or("invalid video duration")?;
    let mut scores: Vec<f64> = futures::future::try_join_all(
        FRAME_FRACTIONS.map(|fraction| frame_score(&proxy.url, duration * fraction)),
    )
    .await
    .map_err(budget)?;
    scores.sort_by(f64::total_cmp);
    Ok(scores[scores.len() / 2])
}

async fn frame_score(url: &str, seconds: f64) -> Result<f64, Reason> {
    let seconds = seconds.to_string();
    let frame = run(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-recv_buffer_size",
            SOCKET_BUFFER_ARG,
            "-ss",
            &seconds,
            "-i",
            url,
            "-frames:v",
            "1",
            "-vf",
            "scale=1280:-1",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "pipe:1",
        ],
        None,
    )
    .await?;
    let tsv = run(
        "tesseract",
        &["stdin", "stdout", "--psm", "11", "tsv"],
        Some(frame),
    )
    .await?;
    Ok(text_score(&String::from_utf8_lossy(&tsv)))
}

/// Characters of confidently read words, each weighted by its height as a
/// share of the frame's: a full-screen slide beats the same slide shrunk
/// inside a composite, which plain character counts would tie.
fn text_score(tsv: &str) -> f64 {
    let mut lines = tsv.lines();
    let Some(columns) = lines.next() else {
        return 0.0;
    };
    let column = |name| columns.split('\t').position(|c| c == name);
    let (Some(level), Some(height), Some(conf), Some(text)) = (
        column("level"),
        column("height"),
        column("conf"),
        column("text"),
    ) else {
        return 0.0;
    };
    let rows: Vec<Vec<&str>> = lines.map(|line| line.split('\t').collect()).collect();
    let number = |row: &Vec<&str>, i: usize| row.get(i).and_then(|v| v.parse::<f64>().ok());
    let image_height = rows
        .iter()
        .find(|row| row.get(level) == Some(&"1"))
        .and_then(|row| number(row, height))
        .unwrap_or(0.0);
    rows.iter()
        .filter_map(|row| {
            let chars = row.get(text)?.trim().chars().count();
            if chars < 3 || number(row, conf)? < 50.0 {
                return None;
            }
            let weight = if image_height > 0.0 {
                100.0 * number(row, height).unwrap_or(0.0) / image_height
            } else {
                1.0
            };
            Some(chars as f64 * weight)
        })
        .sum()
}

/// Run a probe tool with no stderr and no OpenMP threads: frames are OCR'd
/// side by side, so tesseract's own threads would only oversubscribe.
async fn run(program: &str, args: &[&str], input: Option<Vec<u8>>) -> Result<Vec<u8>, Reason> {
    const FAILED: Reason = "video frame probe failed";
    let _slot = TOOL_SLOTS.acquire().await.map_err(|_| FAILED)?;
    let mut child = Command::new(program)
        .args(args)
        .env("OMP_THREAD_LIMIT", "1")
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| FAILED)?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(&input).await;
        });
    }
    let output = tokio::time::timeout(PROCESS_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| FAILED)?
        .map_err(|_| FAILED)?;
    if !output.status.success() {
        return Err(FAILED);
    }
    Ok(output.stdout)
}

/// Upstream blocks of one source, fetched at most once each and only while
/// the byte budget lasts. Concurrent readers of a block in flight wait for
/// it; a failed block may be retried, charged again.
struct Blocks {
    client: reqwest::Client,
    url: String,
    state: Mutex<BlockState>,
}

struct BlockState {
    remaining: u64,
    cells: HashMap<u64, Arc<OnceCell<Bytes>>>,
    total: Option<u64>,
    stopped: bool,
}

impl Blocks {
    async fn get(&self, index: u64) -> Result<Bytes, Reason> {
        let cell = {
            let mut state = self.state.lock().unwrap();
            if state.stopped {
                return Err("probe closed");
            }
            state.cells.entry(index).or_default().clone()
        };
        cell.get_or_try_init(|| self.fetch(index)).await.cloned()
    }

    async fn fetch(&self, index: u64) -> Result<Bytes, Reason> {
        {
            let mut state = self.state.lock().unwrap();
            if state.remaining < BLOCK_BYTES {
                return Err(BUDGET_EXHAUSTED);
            }
            state.remaining -= BLOCK_BYTES;
        }
        let start = index * BLOCK_BYTES;
        let end = start + BLOCK_BYTES - 1;
        // Never format reqwest errors: their Display can contain the URL.
        let response = self
            .client
            .get(&self.url)
            .header(header::RANGE, format!("bytes={start}-{end}"))
            .header(header::ACCEPT_ENCODING, "identity")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| "Play request")?;
        let span = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range);
        let (first, last, size) = match (response.status(), span) {
            (StatusCode::PARTIAL_CONTENT, Some(span)) => span,
            _ => return Err("media host did not return a byte range"),
        };
        if first != start || last != end.min(size - 1) || size <= last {
            return Err("invalid media byte range");
        }
        let expected = (last - first + 1) as usize;
        let mut data = Vec::with_capacity(expected);
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "Play stream")?;
            if data.len() + chunk.len() > expected {
                return Err("oversized media byte range");
            }
            data.extend_from_slice(&chunk);
        }
        if data.len() != expected {
            return Err("truncated media byte range");
        }
        let mut state = self.state.lock().unwrap();
        if state.total.is_some_and(|total| total != size) {
            return Err("media changed during probe");
        }
        state.total = Some(size);
        Ok(Bytes::from(data))
    }

    fn exhausted(&self) -> bool {
        self.state.lock().unwrap().remaining < BLOCK_BYTES
    }

    fn total(&self) -> u64 {
        self.state.lock().unwrap().total.unwrap_or(0)
    }
}

fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (span, size) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (first, last) = span.split_once('-')?;
    Some((first.parse().ok()?, last.parse().ok()?, size.parse().ok()?))
}

fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    let end = if end.is_empty() {
        None
    } else {
        Some(end.parse().ok()?)
    };
    Some((start.parse().ok()?, end))
}

/// Loopback HTTP server the probe tools read the source through. Dropping
/// it stops the server and fails any read still in progress.
struct Proxy {
    url: String,
    blocks: Arc<Blocks>,
    server: tokio::task::JoinHandle<()>,
}

impl Proxy {
    async fn start(url: &str) -> Result<Self, Reason> {
        let blocks = Arc::new(Blocks {
            client: crate::download::media_client()?,
            url: url.to_string(),
            state: Mutex::new(BlockState {
                remaining: PROBE_BYTES,
                cells: HashMap::new(),
                total: None,
                stopped: false,
            }),
        });
        // The first block also tells the total size every response reports.
        blocks.get(0).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "probe proxy")?;
        let port = listener.local_addr().map_err(|_| "probe proxy")?.port();
        let listener = listener.tap_io(|stream| {
            let _ = socket2::SockRef::from(&*stream).set_send_buffer_size(SOCKET_BUFFER_BYTES);
        });
        let app = axum::Router::new()
            .route("/video", axum::routing::get(serve_range))
            .with_state(blocks.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            url: format!("http://127.0.0.1:{port}/video"),
            blocks,
            server,
        })
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.blocks.state.lock().unwrap().stopped = true;
        self.server.abort();
    }
}

async fn serve_range(State(blocks): State<Arc<Blocks>>, headers: HeaderMap) -> Response {
    let total = blocks.total();
    let range = headers
        .get(header::RANGE)
        .map_or(Some((0, None)), |v| v.to_str().ok().and_then(parse_range));
    let Some((start, end)) = range else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let end = end.unwrap_or(u64::MAX).min(total.saturating_sub(1));
    if total == 0 || start > end {
        return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    }
    // Check the first block before promising a successful response.
    if blocks.get(start / BLOCK_BYTES).await.is_err() {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    let body = futures::stream::unfold(start, move |position| {
        let blocks = blocks.clone();
        async move {
            if position > end {
                return None;
            }
            match blocks.get(position / BLOCK_BYTES).await {
                Ok(block) => {
                    let offset = (position % BLOCK_BYTES) as usize;
                    let take = (block.len() - offset).min((end - position + 1) as usize);
                    Some((
                        Ok(block.slice(offset..offset + take)),
                        position + take as u64,
                    ))
                }
                // A cut-off body makes ffmpeg fail safely.
                Err(reason) => Some((Err(std::io::Error::other(reason)), end + 1)),
            }
        }
    });
    (
        StatusCode::PARTIAL_CONTENT,
        [
            (header::CONTENT_LENGTH, (end - start + 1).to_string()),
            (
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}"),
            ),
            (header::CONTENT_TYPE, "video/mp4".to_string()),
            (header::ACCEPT_RANGES, "bytes".to_string()),
        ],
        Body::from_stream(body),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn larger_confident_text_wins() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t1280\t720\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t0\t0\t100\t72\t96\tSlides\n\
                   5\t1\t1\t1\t1\t2\t0\t0\t100\t72\t30\tnoise\n\
                   5\t1\t1\t1\t1\t3\t0\t0\t100\t72\t90\tab\n";
        // Only "Slides" counts: 6 characters at 10% of the frame height.
        assert!((text_score(tsv) - 60.0).abs() < 1e-9);
        let small = tsv.replace("\t72\t96\t", "\t36\t96\t");
        assert!(text_score(&small) < text_score(tsv));
        assert_eq!(text_score(""), 0.0);
    }

    #[test]
    fn blank_probes_do_not_guess_camera() {
        assert!(choose(&[0.0, 0.0]).is_err());
        assert!(choose(&[10.0, 2.0]).is_err());
        assert_eq!(choose(&[50.0, 200.0]), Ok(1));
        assert_eq!(choose(&[0.0]), Ok(0));
    }

    #[test]
    fn ranges_parse_strictly() {
        assert_eq!(parse_range("bytes=5-"), Some((5, None)));
        assert_eq!(parse_range("bytes=5-9"), Some((5, Some(9))));
        assert_eq!(parse_range("bytes=-9"), None);
        assert_eq!(parse_content_range("bytes 0-9/100"), Some((0, 9, 100)));
        assert_eq!(parse_content_range("bytes 0-9/*"), None);
    }

    /// A fake Play media host serving `size` bytes of ranges, counting
    /// requests and how many are in flight at once.
    async fn media_host(size: u64) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let requests = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let (r, p) = (requests.clone(), peak.clone());
        let app = axum::Router::new().route(
            "/media.mp4",
            axum::routing::get(move |headers: HeaderMap| {
                let (requests, peak, active) = (r.clone(), p.clone(), active.clone());
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    let range = headers[header::RANGE].to_str().unwrap();
                    let (start, end) = parse_range(range).unwrap();
                    let end = end.unwrap().min(size - 1);
                    (
                        StatusCode::PARTIAL_CONTENT,
                        [(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"))],
                        vec![b'x'; (end - start + 1) as usize],
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}/media.mp4"), requests, peak)
    }

    async fn read(url: &str, start: u64, end: u64) -> Result<usize, reqwest::Error> {
        let response = reqwest::Client::new()
            .get(url)
            .header(header::RANGE, format!("bytes={start}-{end}"))
            .send()
            .await?
            .error_for_status()?;
        Ok(response.bytes().await?.len())
    }

    #[tokio::test]
    async fn probe_cannot_read_past_byte_budget() {
        let (url, requests, _) = media_host(64 * 1024 * 1024).await;
        let proxy = Proxy::start(&url).await.unwrap();
        // The whole file is far over budget: the body is cut off.
        assert!(read(&proxy.url, 0, 64 * 1024 * 1024 - 1).await.is_err());
        assert!(requests.load(Ordering::SeqCst) as u64 <= PROBE_BYTES / BLOCK_BYTES);
    }

    #[tokio::test]
    async fn blocks_are_fetched_in_parallel_once_each() {
        let (url, requests, peak) = media_host(32 * 1024 * 1024).await;
        let proxy = Proxy::start(&url).await.unwrap();
        let reads = (0..12).map(|i| {
            let start = (1 + i % 4) * BLOCK_BYTES;
            read(&proxy.url, start, start + 1023)
        });
        for result in futures::future::join_all(reads).await {
            assert_eq!(result.unwrap(), 1024);
        }
        // Block 0 at start, then blocks 1-4 once each, concurrently.
        assert_eq!(requests.load(Ordering::SeqCst), 5);
        assert!(peak.load(Ordering::SeqCst) > 1);
    }

    #[tokio::test]
    async fn stalled_open_range_reads_stay_near_the_reader() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (url, _, _) = media_host(64 * 1024 * 1024).await;
        let proxy = Proxy::start(&url).await.unwrap();
        let address = proxy
            .url
            .trim_start_matches("http://")
            .trim_end_matches("/video");
        // What ffmpeg does: an open range, a small receive buffer, a few
        // bytes read, then nothing until it hangs up.
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket
            .set_recv_buffer_size(SOCKET_BUFFER_BYTES as u32)
            .unwrap();
        let mut stream = socket.connect(address.parse().unwrap()).await.unwrap();
        stream
            .write_all(b"GET /video HTTP/1.1\r\nHost: x\r\nRange: bytes=0-\r\n\r\n")
            .await
            .unwrap();
        let mut head = [0; 1024];
        stream.read_exact(&mut head).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let charged = PROBE_BYTES - proxy.blocks.state.lock().unwrap().remaining;
        assert!(charged <= 6 * BLOCK_BYTES, "charged {charged} bytes");
    }

    #[tokio::test]
    async fn closed_probe_stops_serving() {
        let (url, _, _) = media_host(4 * 1024 * 1024).await;
        let proxy = Proxy::start(&url).await.unwrap();
        let address = proxy.url.clone();
        drop(proxy);
        assert!(read(&address, 0, 1023).await.is_err());
    }
}
