//! `net` executor: high-performance network throughput monitor & process bandwidth tracker.
//!
//! Provides kernel statistics parsing for Waybar (`custom/net-speed`), Top Bandwidth Consumers
//! ("Top Talkers"), and interface details. Throughput is computed from kernel statistics
//! (`/proc/net/route`, `/sys/class/net/<iface>/statistics/`, `/proc/net/dev`, and `/proc/<pid>/io`)
//! against a lightweight timestamped stat cache.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};

use crate::exec::proc as proc_exec;

/// Environment variable to override the stat file path (test seam).
pub const STAT_PATH_ENV: &str = "FLEX_NET_STAT_PATH";

/// Fallback base directory for stat storage.
const DEFAULT_STAT_FILE: &str = "flex-net-speed.stat";

/// 1 Kilobyte in bytes (1024).
pub const KB: f64 = 1024.0;

/// 1 Megabyte in bytes (1024 * 1024 = 1048576).
pub const MB: f64 = 1_048_576.0;

/// 1 Gigabyte in bytes (1024 * 1024 * 1024 = 1073741824).
pub const GB: f64 = 1_073_741_824.0;

/// Information about an active process consuming network bandwidth.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessBandwidth {
    /// Process ID.
    pub pid: u32,
    /// Process name / command.
    pub comm: String,
    /// Inbound rate in bytes per second.
    pub rx_rate: f64,
    /// Outbound rate in bytes per second.
    pub tx_rate: f64,
    /// Total rate in bytes per second (`rx_rate + tx_rate`).
    pub total_rate: f64,
    /// Fractional share of total active process bandwidth (`0.0 ..= 1.0`).
    pub share: f32,
}

/// Information about a network interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceInfo {
    /// Interface name (e.g. `wlp15s0`, `enp14s0`, `lo`).
    pub name: String,
    /// Operational state (`up`, `down`, `unknown`).
    pub operstate: String,
    /// Whether this is the default route interface.
    pub is_default: bool,
    /// Assigned IP address with CIDR mask (e.g. `192.168.1.108/24`).
    pub ip_cidr: Option<String>,
    /// Default gateway IP (e.g. `192.168.1.1`).
    pub gateway: Option<String>,
    /// Total received bytes in session.
    pub rx_bytes: u64,
    /// Total transmitted bytes in session.
    pub tx_bytes: u64,
}

/// Process I/O sample: `(timestamp_millis, rchar, wchar)`.
#[derive(Debug, Clone, Copy)]
struct ProcIoSample {
    ts_ms: u128,
    rchar: u64,
    wchar: u64,
}

/// Fallback base directory for process stat storage.
const DEFAULT_PROC_STAT_FILE: &str = "flex-net-proc.stat";

/// Static process I/O sample cache across ticks.
fn proc_io_cache() -> &'static Mutex<HashMap<u32, ProcIoSample>> {
    static CACHE: OnceLock<Mutex<HashMap<u32, ProcIoSample>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

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

/// Resolve the process stat cache file path.
fn proc_stat_file_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !runtime_dir.is_empty() {
            return PathBuf::from(runtime_dir).join(DEFAULT_PROC_STAT_FILE);
        }
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
    std::env::temp_dir().join(format!("{DEFAULT_PROC_STAT_FILE}-{user}"))
}

/// Read cached process samples from disk.
fn read_proc_stat_cache(path: &Path) -> HashMap<u32, ProcIoSample> {
    let mut map = HashMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in content.lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(ts_ms) = fields.next().and_then(|s| s.parse::<u128>().ok()) else {
            continue;
        };
        let Some(rchar) = fields.next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let Some(wchar) = fields.next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        map.insert(pid, ProcIoSample { ts_ms, rchar, wchar });
    }
    map
}

/// Write process samples to disk.
fn write_proc_stat_cache(path: &Path, samples: &HashMap<u32, ProcIoSample>) {
    let mut buf = String::with_capacity(samples.len() * 40);
    for (pid, s) in samples {
        let _ = writeln!(buf, "{} {} {} {}", pid, s.ts_ms, s.rchar, s.wchar);
    }
    let _ = std::fs::write(path, buf);
}

/// Find the default network interface name from `/proc/net/route`.
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

/// Convert a little-endian 8-hex-digit string into an IPv4 address (e.g. `0101A8C0` -> `192.168.1.1`).
#[must_use]
pub fn hex_to_ipv4(hex: &str) -> Option<String> {
    if hex.len() != 8 {
        return None;
    }
    let val = u32::from_str_radix(hex, 16).ok()?;
    let b1 = u8::try_from(val & 0xFF).ok()?;
    let b2 = u8::try_from((val >> 8) & 0xFF).ok()?;
    let b3 = u8::try_from((val >> 16) & 0xFF).ok()?;
    let b4 = u8::try_from((val >> 24) & 0xFF).ok()?;
    Some(format!("{b1}.{b2}.{b3}.{b4}"))
}

/// Find default gateway and subnet info for interfaces from `/proc/net/route`.
#[must_use]
pub fn route_info(route_file: &Path) -> HashMap<String, (Option<String>, Option<String>)> {
    let mut map = HashMap::new();
    let Ok(content) = std::fs::read_to_string(route_file) else {
        return map;
    };
    for line in content.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let Some(iface) = fields.next() else { continue };
        let Some(dest) = fields.next() else { continue };
        let Some(gw_hex) = fields.next() else {
            continue;
        };
        let _flags = fields.next();
        let _refcnt = fields.next();
        let _use = fields.next();
        let _metric = fields.next();
        let mask_hex = fields.next();

        let gw = if dest == "00000000" {
            hex_to_ipv4(gw_hex)
        } else {
            None
        };

        let mask = mask_hex.and_then(hex_to_ipv4);
        let entry = map.entry(iface.to_string()).or_insert((None, None));
        if gw.is_some() {
            entry.0 = gw;
        }
        if mask.is_some() && entry.1.is_none() {
            entry.1 = mask;
        }
    }
    map
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

/// Format raw byte totals into human-readable unit (e.g. `4.8 GB`, `120 MB`, `12 KB`).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn format_bytes(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Read process `comm` (name) from `/proc/<pid>/stat`.
#[must_use]
pub fn read_proc_comm(stat_file: &Path) -> Option<String> {
    let content = std::fs::read_to_string(stat_file).ok()?;
    let open = content.find('(')?;
    let close = content.rfind(')')?;
    if close > open {
        Some(content[open + 1..close].to_string())
    } else {
        None
    }
}

/// Read process `(rchar, wchar)` from `/proc/<pid>/io`.
#[must_use]
pub fn read_proc_io(io_file: &Path) -> Option<(u64, u64)> {
    let content = std::fs::read_to_string(io_file).ok()?;
    let mut rx = None;
    let mut tx = None;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("rchar:") {
            rx = rest.trim().parse::<u64>().ok();
        } else if let Some(rest) = line.strip_prefix("wchar:") {
            tx = rest.trim().parse::<u64>().ok();
        }
    }
    match (rx, tx) {
        (Some(r), Some(t)) => Some((r, t)),
        _ => None,
    }
}

/// Scan `/proc` to collect and calculate per-process network I/O rates.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn scan_top_talkers() -> Vec<ProcessBandwidth> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let mut guard = match proc_io_cache().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };

    let proc_stat_path = proc_stat_file_path();
    if guard.is_empty() {
        *guard = read_proc_stat_cache(&proc_stat_path);
    }

    let proc_dir = Path::new("/proc");
    let Ok(entries) = std::fs::read_dir(proc_dir) else {
        return Vec::new();
    };

    let mut fresh_cache = HashMap::new();
    let mut results = Vec::new();

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name_str) = file_name.to_str() else {
            continue;
        };
        let Ok(pid) = name_str.parse::<u32>() else {
            continue;
        };

        let io_file = entry.path().join("io");
        let stat_file = entry.path().join("stat");

        let Some((rchar, wchar)) = read_proc_io(&io_file) else {
            continue;
        };
        let comm = read_proc_comm(&stat_file).unwrap_or_else(|| format!("pid-{pid}"));

        if let Some(prev) = guard.get(&pid) {
            let elapsed_ms = now_ms.saturating_sub(prev.ts_ms);
            if (100..=60_000).contains(&elapsed_ms) {
                let delta_sec = elapsed_ms as f64 / 1000.0;
                let rx_diff = rchar.saturating_sub(prev.rchar);
                let tx_diff = wchar.saturating_sub(prev.wchar);
                let rx_rate = rx_diff as f64 / delta_sec;
                let tx_rate = tx_diff as f64 / delta_sec;
                let total_rate = rx_rate + tx_rate;

                results.push(ProcessBandwidth {
                    pid,
                    comm,
                    rx_rate,
                    tx_rate,
                    total_rate,
                    share: 0.0,
                });
            }
        }

        fresh_cache.insert(
            pid,
            ProcIoSample {
                ts_ms: now_ms,
                rchar,
                wchar,
            },
        );
    }

    write_proc_stat_cache(&proc_stat_path, &fresh_cache);
    *guard = fresh_cache;

    // Calculate bandwidth share
    let total_active_bandwidth: f64 = results.iter().map(|p| p.total_rate).sum();
    for p in &mut results {
        if total_active_bandwidth > 0.0 {
            #[allow(clippy::cast_possible_truncation)]
            let share = (p.total_rate / total_active_bandwidth).clamp(0.0, 1.0) as f32;
            p.share = share;
        } else {
            p.share = 0.0;
        }
    }

    // Sort descending by total rate, then PID ascending
    results.sort_by(|a, b| {
        b.total_rate
            .partial_cmp(&a.total_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.pid.cmp(&b.pid))
    });

    results
}

/// Parse local IP address from `/proc/net/fib_trie`.
#[must_use]
pub fn local_ip_from_fib_trie(trie_file: &Path) -> Option<String> {
    let content = std::fs::read_to_string(trie_file).ok()?;
    let mut current_cidr = None;
    let mut pending_ip = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("+--") {
            if let Some(subnet) = trimmed.strip_prefix("+-- ") {
                if let Some((cidr, _)) = subnet.split_once(' ') {
                    if !cidr.starts_with("0.0.0.0") && !cidr.starts_with("127.") {
                        current_cidr = Some(cidr.to_string());
                    }
                }
            }
        } else if trimmed.starts_with("|--") {
            if let Some(ip) = trimmed.strip_prefix("|-- ") {
                if !ip.starts_with("0.0.0.0") && !ip.starts_with("127.") && !ip.ends_with(".255") {
                    pending_ip = Some(ip.to_string());
                }
            }
        } else if trimmed.contains("host LOCAL") {
            if let (Some(ip), Some(ref cidr)) = (pending_ip.take(), &current_cidr) {
                let mask = cidr.split_once('/').map_or("24", |(_, m)| m);
                return Some(format!("{ip}/{mask}"));
            }
        }
    }
    None
}

/// Scan system network interfaces.
#[must_use]
pub fn scan_interfaces() -> Vec<InterfaceInfo> {
    let default_iface = default_interface();
    let routes = route_info(Path::new("/proc/net/route"));
    let local_ip = local_ip_from_fib_trie(Path::new("/proc/net/fib_trie"));

    let mut list = Vec::new();
    let net_dir = Path::new("/sys/class/net");
    let Ok(entries) = std::fs::read_dir(net_dir) else {
        return list;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let operstate = std::fs::read_to_string(entry.path().join("operstate"))
            .map_or_else(|_| "unknown".to_string(), |s| s.trim().to_string());

        let is_default = default_iface.as_deref() == Some(&name);
        let (rx_bytes, tx_bytes) = interface_bytes(&name).unwrap_or((0, 0));

        let (gw, _) = routes.get(&name).cloned().unwrap_or((None, None));
        let ip_cidr = if is_default {
            local_ip.clone()
        } else if name == "lo" {
            Some("127.0.0.1/8".to_string())
        } else {
            None
        };

        list.push(InterfaceInfo {
            name,
            operstate,
            is_default,
            ip_cidr,
            gateway: gw,
            rx_bytes,
            tx_bytes,
        });
    }

    list.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.cmp(&b.name))
    });

    list
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

/// Sample throughput and format Waybar JSON with multiline Top Bandwidth Consumers tooltip.
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
    let (down_str, up_str, rx_rate, tx_rate) =
        if let Some((prev_ms, prev_rx, prev_tx)) = read_stat(&stat_path) {
            let elapsed_ms = now_ms.saturating_sub(prev_ms);
            if (50..=60_000).contains(&elapsed_ms) {
                let delta_sec = elapsed_ms as f64 / 1000.0;
                let rx_diff = rx.saturating_sub(prev_rx);
                let tx_diff = tx.saturating_sub(prev_tx);
                let rx_rate = rx_diff as f64 / delta_sec;
                let tx_rate = tx_diff as f64 / delta_sec;
                (
                    format_speed(rx_rate),
                    format_speed(tx_rate),
                    rx_rate,
                    tx_rate,
                )
            } else {
                (format_speed(0.0), format_speed(0.0), 0.0, 0.0)
            }
        } else {
            (format_speed(0.0), format_speed(0.0), 0.0, 0.0)
        };

    write_stat(&stat_path, now_ms, rx, tx);

    let total_rate = rx_rate + tx_rate;
    let class = if total_rate >= MB {
        "heavy"
    } else if total_rate > 0.0 {
        "up"
    } else {
        "idle"
    };

    let top_talkers = scan_top_talkers();
    let local_ip = local_ip_from_fib_trie(Path::new("/proc/net/fib_trie"))
        .unwrap_or_else(|| "127.0.0.1/8".to_string());
    let routes = route_info(Path::new("/proc/net/route"));
    let gw = routes
        .get(&iface)
        .and_then(|(g, _)| g.clone())
        .unwrap_or_else(|| "unknown".to_string());

    let iface_kind = if iface.starts_with("wl") {
        "Wi-Fi"
    } else if iface.starts_with('e') {
        "Ethernet"
    } else if iface.starts_with("wg") {
        "WireGuard"
    } else {
        "Network"
    };

    let mut tooltip = format!(
        "Interface: {iface} ({iface_kind})\nLocal IP:  {local_ip}\nGateway:   {gw}\nSession:   ⬇ {}  |  ⬆ {}",
        format_bytes(rx),
        format_bytes(tx)
    );

    if !top_talkers.is_empty() {
        tooltip.push_str("\n\nTop Bandwidth Consumers:");
        for (i, p) in top_talkers.iter().take(3).enumerate() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let share_pct = (p.share * 100.0).round() as u32;
            let rate_str = format_speed(p.rx_rate);
            let idx = i + 1;
            let _ = write!(
                tooltip,
                "\n  {idx}. {:<10} (PID {:<5})   ⬇ {:<9} ({share_pct}%)",
                p.comm, p.pid, rate_str
            );
        }
    }

    // Escape tooltip for JSON string
    let escaped_tooltip = tooltip
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"");

    format!(
        r#"{{"text":" {down_str}   {up_str}","tooltip":"{escaped_tooltip}","class":"{class}"}}"#
    )
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

/// Execute a net action (`signal:<pid>:<sig>`, `copy:<text>`, `wifi`).
///
/// # Errors
///
/// When subprocess execution fails.
pub fn execute(action_id: &str) -> Result<()> {
    if action_id == "noop" {
        return Ok(());
    }
    if let Some(rest) = action_id.strip_prefix("signal:") {
        let mut parts = rest.split(':');
        let pid_str = parts.next().context("missing pid in signal action")?;
        let sig_str = parts.next().context("missing sig in signal action")?;
        let sig = match sig_str {
            "SIGTERM" | "TERM" => proc_exec::Signal::Term,
            "SIGKILL" | "KILL" => proc_exec::Signal::Kill,
            "SIGSTOP" | "STOP" => proc_exec::Signal::Stop,
            "SIGCONT" | "CONT" => proc_exec::Signal::Cont,
            _ => anyhow::bail!("unknown signal {sig_str}"),
        };
        proc_exec::signal(pid_str, sig, None)?;
    } else if let Some(copy_text) = action_id.strip_prefix("copy:") {
        let mut cmd = std::process::Command::new("wl-copy");
        cmd.arg(copy_text);
        let _ = crate::spawn::status(&mut cmd)?;
    } else if action_id == "wifi" {
        let mut cmd = std::process::Command::new("flex");
        cmd.args(["popup", "menu", "flex-wifi"]);
        let _ = crate::spawn::spawn(&mut cmd)?;
    }
    Ok(())
}
