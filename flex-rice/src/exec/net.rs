//! `net` executor: high-performance network throughput monitor for Waybar.
//!
//! Ports `network-speed.sh` to native Rust without spawning subprocesses (`ip`,
//! `awk`, `cat`, `sleep`, `printf`) or blocking Waybar with a 1-second sleep.
//! Throughput is computed from kernel statistics (`/proc/net/route` and
//! `/sys/class/net/<iface>/statistics/` or `/proc/net/dev`) against a lightweight
//! timestamped stat cache in `/tmp`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;

/// Environment variable to override the stat file path (test seam).
pub const STAT_PATH_ENV: &str = "FLEX_NET_STAT_PATH";

/// Fallback base directory for stat storage.
const DEFAULT_STAT_FILE: &str = "flex-net-speed.stat";

/// 1 Kilobyte in bytes (1024).
const KB: f64 = 1024.0;

/// 1 Megabyte in bytes (1024 * 1024 = 1048576).
const MB: f64 = 1_048_576.0;

/// Resolve the stat cache file path.
fn stat_file_path() -> PathBuf {
    if let Ok(path) = std::env::var(STAT_PATH_ENV) {
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

/// Find the default network interface name from `/proc/net/route`.
///
/// Looks for the route with destination `00000000` (the default gateway).
#[must_use]
pub fn default_interface() -> Option<String> {
    default_interface_from_route(Path::new("/proc/net/route"))
}

/// Parse default interface name from a route table file.
#[must_use]
pub fn default_interface_from_route(route_file: &Path) -> Option<String> {
    let content = std::fs::read_to_string(route_file).ok()?;
    for line in content.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let iface = fields.next()?;
        let dest = fields.next()?;
        if dest == "00000000" {
            return Some(iface.to_string());
        }
    }
    None
}

/// Read `(rx_bytes, tx_bytes)` for a network interface.
#[must_use]
pub fn interface_bytes(iface: &str) -> Option<(u64, u64)> {
    let rx_path = format!("/sys/class/net/{iface}/statistics/rx_bytes");
    let tx_path = format!("/sys/class/net/{iface}/statistics/tx_bytes");
    if let (Ok(rx_str), Ok(tx_str)) = (
        std::fs::read_to_string(rx_path),
        std::fs::read_to_string(tx_path),
    ) {
        if let (Ok(rx), Ok(tx)) = (rx_str.trim().parse::<u64>(), tx_str.trim().parse::<u64>()) {
            return Some((rx, tx));
        }
    }
    // Fallback: parse /proc/net/dev
    parse_proc_net_dev(Path::new("/proc/net/dev"), iface)
}

/// Parse `(rx_bytes, tx_bytes)` from `/proc/net/dev` for `target_iface`.
#[must_use]
pub fn parse_proc_net_dev(dev_file: &Path, target_iface: &str) -> Option<(u64, u64)> {
    let content = std::fs::read_to_string(dev_file).ok()?;
    for line in content.lines().skip(2) {
        let Some((iface_part, stats_part)) = line.split_once(':') else {
            continue;
        };
        if iface_part.trim() == target_iface {
            let mut fields = stats_part.split_whitespace();
            let rx_bytes = fields.next()?.parse::<u64>().ok()?;
            // skip 7 fields: packets, errs, drop, fifo, frame, compressed, multicast
            for _ in 0..7 {
                fields.next()?;
            }
            let tx_bytes = fields.next()?.parse::<u64>().ok()?;
            return Some((rx_bytes, tx_bytes));
        }
    }
    None
}

/// Format bytes-per-second into human-readable rate matching `network-speed.sh`.
///
/// - `>= 1 MB/s`: `"{:.1} MB/s"` (e.g. `1.2 MB/s`)
/// - `>= 1 KB/s`: `"{:.0} KB/s"` (e.g. `24 KB/s`)
/// - `< 1 KB/s`: `"{bytes} B/s"` (e.g. `512 B/s`, `0 B/s`)
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn format_speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= MB {
        format!("{:.1} MB/s", bytes_per_sec / MB)
    } else if bytes_per_sec >= KB {
        let rounded = (bytes_per_sec / KB).round() as u64;
        format!("{rounded} KB/s")
    } else {
        let rounded = bytes_per_sec.round() as u64;
        format!("{rounded} B/s")
    }
}

/// Read the previous sample `(timestamp_millis, rx_bytes, tx_bytes)` from cache.
fn read_stat(path: &Path) -> Option<(u128, u64, u64)> {
    let data = std::fs::read_to_string(path).ok()?;
    let mut fields = data.split_whitespace();
    let ts = fields.next()?.parse::<u128>().ok()?;
    let rx = fields.next()?.parse::<u64>().ok()?;
    let tx = fields.next()?.parse::<u64>().ok()?;
    Some((ts, rx, tx))
}

/// Write the current sample `(timestamp_millis, rx_bytes, tx_bytes)` to cache.
fn write_stat(path: &Path, ts: u128, rx: u64, tx: u64) {
    let content = format!("{ts} {rx} {tx}\n");
    let _ = std::fs::write(path, content);
}

/// Sample throughput and format Waybar JSON.
///
/// Returns a JSON string suitable for Waybar custom module ingestion:
/// `{"text":" <down>   <up>","class":"up"|"idle"}`
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn sample_json() -> String {
    let Some(iface) = default_interface() else {
        return r#"{"text":"⬇ ?  ⬆ ?","class":"idle"}"#.to_string();
    };
    let Some((rx, tx)) = interface_bytes(&iface) else {
        return r#"{"text":"⬇ ?  ⬆ ?","class":"idle"}"#.to_string();
    };

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let stat_path = stat_file_path();
    let (down_str, up_str, class) = if let Some((prev_ms, prev_rx, prev_tx)) = read_stat(&stat_path)
    {
        let elapsed_ms = now_ms.saturating_sub(prev_ms);
        if (50..=60_000).contains(&elapsed_ms) {
            let delta_sec = elapsed_ms as f64 / 1000.0;
            let rx_diff = rx.saturating_sub(prev_rx);
            let tx_diff = tx.saturating_sub(prev_tx);
            let rx_rate = rx_diff as f64 / delta_sec;
            let tx_rate = tx_diff as f64 / delta_sec;
            let class = if rx_diff > 0 || tx_diff > 0 {
                "up"
            } else {
                "idle"
            };
            (format_speed(rx_rate), format_speed(tx_rate), class)
        } else {
            (format_speed(0.0), format_speed(0.0), "idle")
        }
    } else {
        (format_speed(0.0), format_speed(0.0), "idle")
    };

    write_stat(&stat_path, now_ms, rx, tx);
    format!(r#"{{"text":" {down_str}   {up_str}","class":"{class}"}}"#)
}

/// Stream Waybar JSON continuously at the specified interval.
///
/// # Errors
///
/// When writing to stdout fails.
pub fn stream(interval: Duration) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    loop {
        let json = sample_json();
        writeln!(stdout, "{json}")?;
        stdout.flush()?;
        std::thread::sleep(interval);
    }
}
