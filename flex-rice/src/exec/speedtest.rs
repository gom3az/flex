//! `speedtest` executor: non-blocking network latency, jitter, and throughput benchmark engine.
//!
//! Provides TCP handshake latency/jitter probing, chunked streaming CDN download measurement,
//! and streaming upload measurement. State updates run via atomic/synchronized state
//! and persist to `$XDG_RUNTIME_DIR/flex-net-speedtest.stat`.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};

use crate::exec::net::{format_bytes, format_speed, MB};

/// Environment variable to override the speedtest stat file path (test seam).
pub const SPEEDTEST_STAT_PATH_ENV: &str = "FLEX_NET_SPEEDTEST_STAT_PATH";

/// Fallback base filename for speedtest stat storage.
const DEFAULT_STAT_FILE: &str = "flex-net-speedtest.stat";

/// Default target server description.
pub const DEFAULT_TARGET_SERVER: &str = "Cloudflare CDN (speed.cloudflare.com)";

/// Default ping target host.
pub const DEFAULT_PING_HOST: &str = "speed.cloudflare.com";

/// Default ping target port.
pub const DEFAULT_PING_PORT: u16 = 80;

/// Number of TCP handshake probes for latency & jitter.
pub const PING_PROBES: usize = 10;

/// Number of bytes to download during benchmark (50 MB).
pub const DOWNLOAD_TARGET_BYTES: u64 = 50_000_000;

/// Default download endpoint (50 MB payload).
pub const DEFAULT_DOWNLOAD_URL: &str = "https://speed.cloudflare.com/__down?bytes=50000000";

/// Default upload endpoint.
pub const DEFAULT_UPLOAD_URL: &str = "https://speed.cloudflare.com/__up";

/// Number of bytes to upload during benchmark (20 MB).
pub const UPLOAD_TARGET_BYTES: u64 = 20_000_000;

/// Reference speed used to normalize volume bars (100 MB/s).
pub const REFERENCE_MAX_SPEED_BPS: f64 = 100.0 * MB;

/// Benchmark execution phase state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpeedtestPhase {
    /// No benchmark currently running; ready to start.
    #[default]
    Idle,
    /// Measuring TCP round-trip latency & jitter.
    TestingPing,
    /// Measuring streaming download throughput.
    TestingDownload,
    /// Measuring streaming upload throughput.
    TestingUpload,
    /// Benchmark completed successfully.
    Complete,
    /// Benchmark encountered an error.
    Failed,
}

impl SpeedtestPhase {
    /// Human-readable description of current phase.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::TestingPing => "Probing Ping...",
            Self::TestingDownload => "Testing Download...",
            Self::TestingUpload => "Testing Upload...",
            Self::Complete => "Complete",
            Self::Failed => "Failed",
        }
    }

    /// Whether a benchmark is actively running.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(
            self,
            Self::TestingPing | Self::TestingDownload | Self::TestingUpload
        )
    }
}

/// Snapshot of benchmark results and live progress.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedtestSnapshot {
    /// Current execution state.
    pub phase: SpeedtestPhase,
    /// Measured TCP round-trip latency in milliseconds.
    pub ping_ms: Option<f64>,
    /// Measured latency jitter in milliseconds.
    pub jitter_ms: Option<f64>,
    /// Download throughput in bytes per second.
    pub download_bps: Option<f64>,
    /// Current downloaded bytes in active session.
    pub download_bytes: u64,
    /// Target total bytes to download.
    pub download_total_bytes: u64,
    /// Active download elapsed time in seconds.
    pub download_elapsed_secs: f64,
    /// Upload throughput in bytes per second.
    pub upload_bps: Option<f64>,
    /// Current uploaded bytes in active session.
    pub upload_bytes: u64,
    /// Target total bytes to upload.
    pub upload_total_bytes: u64,
    /// Active upload elapsed time in seconds.
    pub upload_elapsed_secs: f64,
    /// Target server name / description.
    pub target_server: String,
    /// Error message if benchmark failed.
    pub error: Option<String>,
    /// Timestamp of last completed test in milliseconds since UNIX epoch.
    pub timestamp_ms: u128,
}

impl Default for SpeedtestSnapshot {
    fn default() -> Self {
        Self {
            phase: SpeedtestPhase::Idle,
            ping_ms: None,
            jitter_ms: None,
            download_bps: None,
            download_bytes: 0,
            download_total_bytes: DOWNLOAD_TARGET_BYTES,
            download_elapsed_secs: 0.0,
            upload_bps: None,
            upload_bytes: 0,
            upload_total_bytes: UPLOAD_TARGET_BYTES,
            upload_elapsed_secs: 0.0,
            target_server: DEFAULT_TARGET_SERVER.to_string(),
            error: None,
            timestamp_ms: 0,
        }
    }
}

impl SpeedtestSnapshot {
    /// Compute normalized volume ratio `0.0 ..= 1.0` for download throughput.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn download_volume(&self) -> f32 {
        let rate = self.download_bps.unwrap_or(0.0);
        if rate <= 0.0 {
            0.0
        } else {
            (rate / REFERENCE_MAX_SPEED_BPS).clamp(0.0, 1.0) as f32
        }
    }

    /// Compute normalized volume ratio `0.0 ..= 1.0` for upload throughput.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn upload_volume(&self) -> f32 {
        let rate = self.upload_bps.unwrap_or(0.0);
        if rate <= 0.0 {
            0.0
        } else {
            (rate / REFERENCE_MAX_SPEED_BPS).clamp(0.0, 1.0) as f32
        }
    }

    /// Format ping / latency and jitter text.
    #[must_use]
    pub fn ping_display(&self) -> String {
        match (self.ping_ms, self.jitter_ms) {
            (Some(ping), Some(jitter)) => format!("{ping:.1} ms (jitter {jitter:.1} ms)"),
            (Some(ping), None) => format!("{ping:.1} ms"),
            (None, _) => {
                if self.phase == SpeedtestPhase::TestingPing {
                    "probing (5 probes)...".to_string()
                } else {
                    "—".to_string()
                }
            }
        }
    }

    /// Format download speed metadata text.
    #[must_use]
    pub fn download_display(&self) -> String {
        if self.phase == SpeedtestPhase::TestingDownload && self.download_bytes > 0 {
            let rate = self.download_bps.unwrap_or(0.0);
            #[allow(clippy::cast_precision_loss)]
            let pct = (self.download_bytes as f64 / DOWNLOAD_TARGET_BYTES as f64 * 100.0)
                .clamp(0.0, 100.0);
            format!(
                "{} ({} / {} • {pct:.0}%)",
                format_speed(rate),
                format_bytes(self.download_bytes),
                format_bytes(DOWNLOAD_TARGET_BYTES)
            )
        } else if let Some(rate) = self.download_bps {
            let mbps = (rate * 8.0) / 1_000_000.0;
            format!("{} ({mbps:.1} Mbps)", format_speed(rate))
        } else {
            "—".to_string()
        }
    }

    /// Format upload speed metadata text.
    #[must_use]
    pub fn upload_display(&self) -> String {
        if self.phase == SpeedtestPhase::TestingUpload && self.upload_bytes > 0 {
            let rate = self.upload_bps.unwrap_or(0.0);
            #[allow(clippy::cast_precision_loss)]
            let pct =
                (self.upload_bytes as f64 / UPLOAD_TARGET_BYTES as f64 * 100.0).clamp(0.0, 100.0);
            format!(
                "{} ({} / {} • {pct:.0}%)",
                format_speed(rate),
                format_bytes(self.upload_bytes),
                format_bytes(UPLOAD_TARGET_BYTES)
            )
        } else if let Some(rate) = self.upload_bps {
            let mbps = (rate * 8.0) / 1_000_000.0;
            format!("{} ({mbps:.1} Mbps)", format_speed(rate))
        } else {
            "—".to_string()
        }
    }
}

/// Global shared speedtest benchmark state.
fn shared_state() -> &'static Mutex<SpeedtestSnapshot> {
    static STATE: OnceLock<Mutex<SpeedtestSnapshot>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(SpeedtestSnapshot::default()))
}

/// Resolve the speedtest stat cache file path.
#[must_use]
pub fn stat_file_path() -> PathBuf {
    if let Ok(path) = std::env::var(SPEEDTEST_STAT_PATH_ENV) {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !runtime_dir.is_empty() {
            return PathBuf::from(runtime_dir).join(DEFAULT_STAT_FILE);
        }
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
    std::env::temp_dir().join(format!("{DEFAULT_STAT_FILE}-{user}"))
}

/// Parse cached speedtest stats from disk.
#[must_use]
pub fn read_cached_stat(path: &Path) -> Option<SpeedtestSnapshot> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut snapshot = SpeedtestSnapshot::default();
    let mut found_data = false;

    for line in content.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        match key {
            "timestamp_ms" => {
                if let Ok(ts) = val.parse::<u128>() {
                    snapshot.timestamp_ms = ts;
                }
            }
            "ping_ms" => {
                if let Ok(p) = val.parse::<f64>() {
                    snapshot.ping_ms = Some(p);
                    found_data = true;
                }
            }
            "jitter_ms" => {
                if let Ok(j) = val.parse::<f64>() {
                    snapshot.jitter_ms = Some(j);
                }
            }
            "download_bps" => {
                if let Ok(d) = val.parse::<f64>() {
                    snapshot.download_bps = Some(d);
                    found_data = true;
                }
            }
            "upload_bps" => {
                if let Ok(u) = val.parse::<f64>() {
                    snapshot.upload_bps = Some(u);
                    found_data = true;
                }
            }
            "target_server" if !val.is_empty() => {
                snapshot.target_server = val.to_string();
            }
            _ => {}
        }
    }

    if found_data {
        snapshot.phase = SpeedtestPhase::Complete;
        Some(snapshot)
    } else {
        None
    }
}

/// Write speedtest results to disk cache.
pub fn write_cached_stat(path: &Path, snapshot: &SpeedtestSnapshot) {
    let mut buf = String::with_capacity(256);
    let _ = writeln!(buf, "timestamp_ms: {}", snapshot.timestamp_ms);
    if let Some(ping) = snapshot.ping_ms {
        let _ = writeln!(buf, "ping_ms: {ping:.2}");
    }
    if let Some(jitter) = snapshot.jitter_ms {
        let _ = writeln!(buf, "jitter_ms: {jitter:.2}");
    }
    if let Some(down) = snapshot.download_bps {
        let _ = writeln!(buf, "download_bps: {down:.2}");
    }
    if let Some(up) = snapshot.upload_bps {
        let _ = writeln!(buf, "upload_bps: {up:.2}");
    }
    let _ = writeln!(buf, "target_server: {}", snapshot.target_server);
    let _ = std::fs::write(path, buf);
}

/// Get current speedtest snapshot, loading from cache if initialized as idle.
#[must_use]
pub fn get_snapshot() -> SpeedtestSnapshot {
    let mut guard = match shared_state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };

    if guard.phase == SpeedtestPhase::Idle && guard.timestamp_ms == 0 {
        if let Some(cached) = read_cached_stat(&stat_file_path()) {
            *guard = cached;
        }
    }

    guard.clone()
}

/// Mutate shared speedtest snapshot.
pub fn update_snapshot(f: impl FnOnce(&mut SpeedtestSnapshot)) {
    let mut guard = match shared_state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    f(&mut guard);
}

/// Reset shared state (used by test suites).
pub fn reset_state() {
    let mut guard = match shared_state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    *guard = SpeedtestSnapshot::default();
}

/// Measure TCP round-trip latency & jitter over multiple probes.
///
/// Returns `(average_latency_ms, jitter_ms)`.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn measure_tcp_ping(
    host: &str,
    port: u16,
    probes: usize,
    timeout: Duration,
) -> (Option<f64>, Option<f64>) {
    let target = format!("{host}:{port}");
    let Ok(addrs) = target.to_socket_addrs() else {
        return (None, None);
    };
    let addrs_vec: Vec<_> = addrs.collect();
    if addrs_vec.is_empty() {
        return (None, None);
    }

    let mut samples = Vec::with_capacity(probes);

    for _ in 0..probes {
        let start = Instant::now();
        let connected = addrs_vec
            .iter()
            .any(|addr| TcpStream::connect_timeout(addr, timeout).is_ok());
        if connected {
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            samples.push(elapsed_ms);
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    if samples.is_empty() {
        return (None, None);
    }

    let count = samples.len() as f64;
    let avg_latency = samples.iter().sum::<f64>() / count;

    let jitter = if samples.len() >= 2 {
        let mut diff_sum = 0.0;
        for i in 1..samples.len() {
            diff_sum += (samples[i] - samples[i - 1]).abs();
        }
        diff_sum / (samples.len() - 1) as f64
    } else {
        0.0
    };

    (Some(avg_latency), Some(jitter))
}

/// Stream chunked download from endpoint, calling progress hook on each chunk.
///
/// # Errors
///
/// When spawning or reading from curl fails.
#[allow(clippy::cast_precision_loss)]
pub fn measure_download<F>(url: &str, on_progress: F) -> Result<f64>
where
    F: Fn(u64, f64, f64),
{
    let mut cmd = Command::new("curl");
    cmd.args(["-s", "-N", "--max-time", "30", url])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().context("failed to spawn curl for download")?;
    let mut stdout = child.stdout.take().context("missing child stdout")?;

    let mut buffer = vec![0u8; 65536].into_boxed_slice();
    let mut total_bytes = 0u64;
    let start = Instant::now();

    loop {
        let bytes_read = stdout.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        total_bytes += bytes_read as u64;
        let elapsed = start.elapsed().as_secs_f64();
        let speed = total_bytes as f64 / elapsed.max(0.001);
        on_progress(total_bytes, elapsed, speed);
    }

    let _ = child.wait();
    let total_elapsed = start.elapsed().as_secs_f64();
    let final_speed = total_bytes as f64 / total_elapsed.max(0.001);

    Ok(final_speed)
}

/// Stream chunked payload upload to endpoint, calling progress hook on each chunk.
///
/// # Errors
///
/// When spawning or writing to curl fails.
#[allow(clippy::cast_precision_loss)]
pub fn measure_upload<F>(url: &str, target_bytes: u64, on_progress: F) -> Result<f64>
where
    F: Fn(u64, f64, f64),
{
    let mut cmd = Command::new("curl");
    cmd.args([
        "-s",
        "-o",
        "/dev/null",
        "-X",
        "POST",
        "--data-binary",
        "@-",
        "--max-time",
        "30",
        url,
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::null());

    let mut child = cmd.spawn().context("failed to spawn curl for upload")?;
    let mut stdin = child.stdin.take().context("missing child stdin")?;

    let chunk = vec![0u8; 65536].into_boxed_slice();
    let mut total_bytes = 0u64;
    let start = Instant::now();

    while total_bytes < target_bytes {
        #[allow(clippy::cast_possible_truncation)]
        let remaining = (target_bytes - total_bytes) as usize;
        let to_write = remaining.min(chunk.len());
        if stdin.write_all(&chunk[..to_write]).is_err() {
            break;
        }
        total_bytes += to_write as u64;
        let elapsed = start.elapsed().as_secs_f64();
        let speed = total_bytes as f64 / elapsed.max(0.001);
        on_progress(total_bytes, elapsed, speed);
    }

    drop(stdin);
    let _ = child.wait();

    let total_elapsed = start.elapsed().as_secs_f64();
    let final_speed = total_bytes as f64 / total_elapsed.max(0.001);

    Ok(final_speed)
}

/// Run the complete speedtest benchmark sequence synchronously.
#[must_use]
pub fn run_benchmark() -> SpeedtestSnapshot {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    update_snapshot(|s| {
        s.phase = SpeedtestPhase::TestingPing;
        s.ping_ms = None;
        s.jitter_ms = None;
        s.download_bps = None;
        s.download_bytes = 0;
        s.upload_bps = None;
        s.upload_bytes = 0;
        s.error = None;
        s.timestamp_ms = now_ms;
    });

    let (ping_res, jitter_res) = measure_tcp_ping(
        DEFAULT_PING_HOST,
        DEFAULT_PING_PORT,
        PING_PROBES,
        Duration::from_secs(2),
    );

    update_snapshot(|s| {
        s.ping_ms = ping_res;
        s.jitter_ms = jitter_res;
        s.phase = SpeedtestPhase::TestingDownload;
    });

    let download_res = measure_download(DEFAULT_DOWNLOAD_URL, |bytes, elapsed, speed| {
        update_snapshot(|s| {
            s.download_bytes = bytes;
            s.download_elapsed_secs = elapsed;
            s.download_bps = Some(speed);
        });
    });

    let final_download = match download_res {
        Ok(speed) => Some(speed),
        Err(e) => {
            update_snapshot(|s| {
                s.error = Some(format!("download error: {e}"));
            });
            None
        }
    };

    update_snapshot(|s| {
        s.download_bps = final_download;
        s.phase = SpeedtestPhase::TestingUpload;
    });

    let upload_res = measure_upload(
        DEFAULT_UPLOAD_URL,
        UPLOAD_TARGET_BYTES,
        |bytes, elapsed, speed| {
            update_snapshot(|s| {
                s.upload_bytes = bytes;
                s.upload_elapsed_secs = elapsed;
                s.upload_bps = Some(speed);
            });
        },
    );

    let final_upload = match upload_res {
        Ok(speed) => Some(speed),
        Err(e) => {
            update_snapshot(|s| {
                s.error = Some(format!("upload error: {e}"));
            });
            None
        }
    };

    update_snapshot(|s| {
        s.upload_bps = final_upload;
        s.phase = SpeedtestPhase::Complete;
    });

    let final_snapshot = get_snapshot();
    write_cached_stat(&stat_file_path(), &final_snapshot);

    final_snapshot
}

/// Trigger benchmark execution on a background thread if not already active.
pub fn trigger_background() {
    let current_phase = get_snapshot().phase;
    if current_phase.is_running() {
        return;
    }

    let _ = crate::spawn::BgTask::spawn(run_benchmark);
}

/// Run terminal CLI benchmark (`flex-net -B` / `flex-net --speedtest`) and print summary.
///
/// # Errors
///
/// When writing to stdout fails.
pub fn run_cli_benchmark() -> Result<()> {
    println!("Flex Network Speedtest Benchmark");
    println!("Target Server: {DEFAULT_TARGET_SERVER}\n");

    print!("  Latency / Ping:    Probing (5 probes)...");
    let _ = std::io::stdout().flush();

    let (ping_opt, jitter_opt) = measure_tcp_ping(
        DEFAULT_PING_HOST,
        DEFAULT_PING_PORT,
        PING_PROBES,
        Duration::from_secs(2),
    );

    match (ping_opt, jitter_opt) {
        (Some(ping), Some(jitter)) => {
            println!("\r  Latency / Ping:    {ping:.1} ms (jitter {jitter:.1} ms)");
        }
        (Some(ping), None) => {
            println!("\r  Latency / Ping:    {ping:.1} ms");
        }
        (None, _) => {
            println!("\r  Latency / Ping:    Unavailable");
        }
    }

    print!("  Download Speed:    Downloading...");
    let _ = std::io::stdout().flush();

    let down_speed = measure_download(DEFAULT_DOWNLOAD_URL, |bytes, _, speed| {
        print!(
            "\r  Download Speed:    {} ({})",
            format_speed(speed),
            format_bytes(bytes)
        );
        let _ = std::io::stdout().flush();
    });

    match &down_speed {
        Ok(speed) => {
            let mbps = (speed * 8.0) / 1_000_000.0;
            println!(
                "\r  Download Speed:    {} ({mbps:.1} Mbps)    ",
                format_speed(*speed)
            );
        }
        Err(e) => {
            println!("\r  Download Speed:    Failed ({e})           ");
        }
    }

    print!("  Upload Speed:      Uploading...");
    let _ = std::io::stdout().flush();

    let up_speed = measure_upload(
        DEFAULT_UPLOAD_URL,
        UPLOAD_TARGET_BYTES,
        |bytes, _, speed| {
            print!(
                "\r  Upload Speed:      {} ({})",
                format_speed(speed),
                format_bytes(bytes)
            );
            let _ = std::io::stdout().flush();
        },
    );

    match &up_speed {
        Ok(speed) => {
            let mbps = (speed * 8.0) / 1_000_000.0;
            println!(
                "\r  Upload Speed:      {} ({mbps:.1} Mbps)    ",
                format_speed(*speed)
            );
        }
        Err(e) => {
            println!("\r  Upload Speed:      Failed ({e})           ");
        }
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let snapshot = SpeedtestSnapshot {
        phase: SpeedtestPhase::Complete,
        ping_ms: ping_opt,
        jitter_ms: jitter_opt,
        download_bps: down_speed.ok(),
        download_bytes: DOWNLOAD_TARGET_BYTES,
        download_total_bytes: DOWNLOAD_TARGET_BYTES,
        download_elapsed_secs: 0.0,
        upload_bps: up_speed.ok(),
        upload_bytes: UPLOAD_TARGET_BYTES,
        upload_total_bytes: UPLOAD_TARGET_BYTES,
        upload_elapsed_secs: 0.0,
        target_server: DEFAULT_TARGET_SERVER.to_string(),
        error: None,
        timestamp_ms: now_ms,
    };

    write_cached_stat(&stat_file_path(), &snapshot);
    println!("\nResults cached to {}", stat_file_path().display());

    Ok(())
}
