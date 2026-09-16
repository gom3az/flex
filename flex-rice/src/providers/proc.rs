//! Process provider: a native, filterable process list for `flex proc`.
//!
//! Replaces the htop-based `kill-menu.sh`: rows come straight from `/proc`
//! (std-only, no crates), refresh once per engine tick with focus preserved
//! by pid, and the executor signals the selected pid.
//!
//! - Row `label` is `"<comm> <pid>"`, so the engine's label fuzzy-filter
//!   matches either the process name or the pid out of the box.
//! - Row `meta` is `"<cpu%> <mem%> <user>"`; `confirmable` arms the danger
//!   flow (Enter → SIGTERM, Delete → SIGKILL).
//! - CPU% is the delta of `utime+stime` over the delta of total system
//!   jiffies since the previous scan (per-core, so a busy multi-threaded
//!   process can read >100% like `top`); the first scan reports `0.0`.
//! - Kernel threads (empty `/proc/<pid>/cmdline`) are hidden unless
//!   `FLEX_PROC_KTHREADS` is set non-empty/non-`0`.
//!
//! Everything is best-effort and panic-free: a `/proc` entry that vanishes
//! mid-scan is skipped, and unparsable fields degrade to `0`/`?`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use flex_core::{Menu, Row, RowId, Tab};

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "proc";
/// Tab title.
pub const TAB_NAME: &str = "Processes";
/// Env: also show kernel threads (empty cmdline).
const KTHREADS_ENV: &str = "FLEX_PROC_KTHREADS";
/// `procfs` mount point.
const PROC_ROOT: &str = "/proc";

/// Previous jiffy counters for one pid (for the CPU delta).
#[derive(Debug, Clone, Copy)]
struct Sample {
    /// `utime + stime` at the previous scan.
    jiffies: u64,
    /// Total system jiffies at the previous scan.
    total: u64,
}

/// Per-pid CPU sampling state (survives ticks; guarded against poisoning).
fn samples() -> &'static Mutex<HashMap<u32, Sample>> {
    static STATE: OnceLock<Mutex<HashMap<u32, Sample>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// uid → username, read once from `/etc/passwd`.
fn users() -> &'static HashMap<u32, String> {
    static USERS: OnceLock<HashMap<u32, String>> = OnceLock::new();
    USERS.get_or_init(load_passwd)
}

/// Build the `Processes` tab: standard rows, `deletable` (Delete = SIGKILL).
#[must_use]
pub fn proc_tab() -> Tab {
    let mut tab = Tab::with_rows(TAB_NAME, scan());
    tab.deletable = true;
    tab
}

/// One `/proc` sweep into CPU-sorted rows.
#[must_use]
pub fn scan() -> Vec<Row> {
    let total = read_total_jiffies().unwrap_or(0);
    let mem_total = read_mem_total_kb().unwrap_or(0);
    let show_kthreads = kthreads_enabled();
    let users = users();
    let mut samples = match samples().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let mut fresh: HashMap<u32, Sample> = HashMap::with_capacity(samples.len());
    // (cpu%, pid, row) so the CPU sort does not re-parse the meta string.
    let mut collected: Vec<(f32, u32, Row)> = Vec::new();
    for pid in pids() {
        let Some((comm, utime, stime)) = read_stat(pid) else {
            continue;
        };
        let jiffies = utime.saturating_add(stime);
        let cpu = samples
            .get(&pid)
            .map_or(0.0, |prev| cpu_percent(prev, jiffies, total));
        fresh.insert(pid, Sample { jiffies, total });
        if !show_kthreads && !is_userspace(pid) {
            continue;
        }
        let (uid, rss_kb) = read_status(pid).unwrap_or((0, 0));
        let mem = mem_percent(rss_kb, mem_total);
        let user = users.get(&uid).cloned().unwrap_or_else(|| uid.to_string());
        collected.push((cpu, pid, proc_row(pid, &comm, cpu, mem, &user)));
    }
    *samples = fresh;

    collected.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    collected.into_iter().map(|(_, _, row)| row).collect()
}

/// Per-tick refresh: rebuild the rows and keep the cursor on the same pid
/// (matched by row id through the filter, like the Wi-Fi scan).
pub fn refresh(menu: &mut Menu) {
    let previous = menu.app.focused_row().map(|row| row.id.clone());
    let fresh = scan();
    let Some(tab) = menu.app.tabs.iter_mut().find(|tab| tab.name == TAB_NAME) else {
        return;
    };
    tab.rows = fresh;
    if let Some(id) = previous {
        if let Some(position) = visible_position(menu, |row| row.id == id) {
            if let Some(state) = menu.app.active_tab_mut().map(|tab| &mut tab.state) {
                state.focus = position;
            }
        }
    }
    menu.app.clamp_focus();
}

/// The process state character (`R`/`S`/`T`/`Z`/…) for `pid`, if readable.
#[must_use]
pub fn process_state(pid: u32) -> Option<char> {
    let data = std::fs::read_to_string(proc_path(pid, "stat")).ok()?;
    let rest = stat_after_comm(&data)?;
    rest.split_whitespace().next()?.chars().next()
}

/// Position of the first row matching `pred` in the **filtered** view (what
/// `TabState::focus` indexes), or `None`.
fn visible_position(menu: &Menu, pred: impl Fn(&Row) -> bool) -> Option<usize> {
    let rows = &menu.app.active_tab()?.rows;
    menu.app
        .visible_rows()
        .into_iter()
        .position(|index| rows.get(index).is_some_and(&pred))
}

/// One process row: label `<comm> <pid>`, meta `<cpu%> <mem%> <user>`,
/// `confirmable` (danger arm).
fn proc_row(pid: u32, comm: &str, cpu: f32, mem: f32, user: &str) -> Row {
    let label = format!("{comm} {pid}");
    let meta = format!("{cpu:5.1}% {mem:5.1}% {user}");
    let mut row = Row::with_meta(RowId::new(pid.to_string()), label, meta);
    row.confirmable = true;
    row
}

/// CPU% from the jiffy deltas; `0.0` when there is no baseline.
///
/// `#[allow]`: a display percentage does not need `u64`-exact precision.
#[allow(clippy::cast_precision_loss)]
fn cpu_percent(prev: &Sample, jiffies: u64, total: u64) -> f32 {
    let delta_proc = jiffies.saturating_sub(prev.jiffies);
    let delta_total = total.saturating_sub(prev.total);
    if delta_total == 0 {
        0.0
    } else {
        100.0 * delta_proc as f32 / delta_total as f32
    }
}

/// Resident-set percentage of total memory; `0.0` when total is unknown.
///
/// `#[allow]`: a display percentage does not need `u64`-exact precision.
#[allow(clippy::cast_precision_loss)]
fn mem_percent(rss_kb: u64, total_kb: u64) -> f32 {
    if total_kb == 0 {
        0.0
    } else {
        100.0 * rss_kb as f32 / total_kb as f32
    }
}

/// Numeric `/proc` entries (unreadable/unparseable names skipped).
fn pids() -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(PROC_ROOT) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if let Ok(name) = entry.file_name().into_string() {
            if let Ok(pid) = name.parse::<u32>() {
                out.push(pid);
            }
        }
    }
    out
}

/// `/proc/<pid>/<file>`.
fn proc_path(pid: u32, file: &str) -> PathBuf {
    PathBuf::from(PROC_ROOT).join(pid.to_string()).join(file)
}

/// `(comm, utime, stime)` from `/proc/<pid>/stat`, parsing `comm` between the
/// first `(` and the **last** `)` (comm may contain spaces/parens).
fn read_stat(pid: u32) -> Option<(String, u64, u64)> {
    let data = std::fs::read_to_string(proc_path(pid, "stat")).ok()?;
    parse_stat(&data)
}

/// [`read_stat`] over raw `stat` text.
fn parse_stat(data: &str) -> Option<(String, u64, u64)> {
    let open = data.find('(')?;
    let close = data.rfind(')')?;
    if close <= open {
        return None;
    }
    let comm = data.get(open + 1..close)?.to_string();
    let rest = stat_after_comm(data)?;
    let mut fields = rest.split_whitespace();
    // state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt
    for _ in 0..11 {
        fields.next()?;
    }
    let utime = fields.next()?.parse::<u64>().ok()?;
    let stime = fields.next()?.parse::<u64>().ok()?;
    Some((comm, utime, stime))
}

/// The `stat` text after `") "` (the comm terminator), for field splitting.
fn stat_after_comm(data: &str) -> Option<&str> {
    let close = data.rfind(')')?;
    data.get(close + 1..)
}

/// `(uid, rss_kb)` from `/proc/<pid>/status` (missing fields → `0`).
fn read_status(pid: u32) -> Option<(u32, u64)> {
    let data = std::fs::read_to_string(proc_path(pid, "status")).ok()?;
    Some(parse_status(&data))
}

/// [`read_status`] over raw `status` text.
fn parse_status(data: &str) -> (u32, u64) {
    let mut uid = 0_u32;
    let mut rss_kb = 0_u64;
    for line in data.lines() {
        if let Some(value) = line.strip_prefix("Uid:") {
            if let Some(first) = value.split_whitespace().next() {
                uid = first.parse().unwrap_or(0);
            }
        } else if let Some(value) = line.strip_prefix("VmRSS:") {
            if let Some(first) = value.split_whitespace().next() {
                rss_kb = first.parse().unwrap_or(0);
            }
        }
    }
    (uid, rss_kb)
}

/// Whether `pid` has a non-empty `cmdline` (a user-space process; kernel
/// threads have none).
fn is_userspace(pid: u32) -> bool {
    std::fs::read(proc_path(pid, "cmdline")).is_ok_and(|bytes| !bytes.is_empty())
}

/// Total system jiffies from `/proc/stat` (`cpu …` line, all fields summed).
fn read_total_jiffies() -> Option<u64> {
    let data = std::fs::read_to_string(PathBuf::from(PROC_ROOT).join("stat")).ok()?;
    let line = data.lines().next()?;
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    Some(fields.filter_map(|field| field.parse::<u64>().ok()).sum())
}

/// `MemTotal` in kB from `/proc/meminfo`.
fn read_mem_total_kb() -> Option<u64> {
    let data = std::fs::read_to_string(PathBuf::from(PROC_ROOT).join("meminfo")).ok()?;
    for line in data.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            return value.split_whitespace().next()?.parse::<u64>().ok();
        }
    }
    None
}

/// Whether kernel threads should be shown (`FLEX_PROC_KTHREADS` non-empty and
/// not `0`).
fn kthreads_enabled() -> bool {
    std::env::var(KTHREADS_ENV).is_ok_and(|value| !value.is_empty() && value != "0")
}

/// uid → username from `/etc/passwd` (malformed lines skipped).
fn load_passwd() -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let Ok(data) = std::fs::read_to_string("/etc/passwd") else {
        return map;
    };
    for line in data.lines() {
        let mut fields = line.split(':');
        let Some(name) = fields.next() else { continue };
        let _passwd = fields.next();
        let Some(uid) = fields.next().and_then(|uid| uid.parse::<u32>().ok()) else {
            continue;
        };
        map.insert(uid, name.to_string());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_parses_comm_with_parens_and_spaces() {
        // `(a ) b)` as comm, then the numeric tail.
        let line = "42 (weird ) name) S 1 42 42 0 -1 4194560 0 0 0 0 7 9 0 0 20 0 1 0 0 0 0";
        let (comm, utime, stime) = parse_stat(line).expect("stat parses");
        assert_eq!(comm, "weird ) name");
        assert_eq!(utime, 7);
        assert_eq!(stime, 9);
    }

    #[test]
    fn stat_rejects_malformed_lines() {
        assert!(parse_stat("").is_none());
        assert!(parse_stat("no parens").is_none());
        assert!(parse_stat("1 (x)").is_none(), "too few fields");
    }

    #[test]
    fn status_reads_uid_and_rss() {
        let data = "Name:\tfirefox\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t  2048 kB\n";
        assert_eq!(parse_status(data), (1000, 2048));
    }

    #[test]
    fn status_missing_fields_degrade_to_zero() {
        assert_eq!(parse_status("Name:\tfoo\n"), (0, 0));
    }

    #[test]
    fn cpu_percent_uses_the_jiffy_delta() {
        let prev = Sample {
            jiffies: 100,
            total: 1_000,
        };
        // 25 of 500 total jiffies -> 5%.
        assert!((cpu_percent(&prev, 125, 1_500) - 5.0).abs() < f32::EPSILON);
        assert!(
            cpu_percent(&prev, 100, 1_000).abs() < f32::EPSILON,
            "no total delta"
        );
    }

    #[test]
    fn scan_finds_at_least_this_process() {
        let pid = std::process::id().to_string();
        let rows = scan();
        assert!(
            rows.iter().any(|row| row.id.as_str() == pid),
            "the scanning process appears in /proc"
        );
    }
}
