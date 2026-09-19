//! `net` provider: bandwidth consumption monitor, network interface status, and speedtest benchmark.
//!
//! Implements Tab 1 (`Bandwidth`) with top network-consuming processes ("Top Talkers"),
//! Tab 2 (`Interfaces`) with network interface metadata and default route indicators,
//! and Tab 3 (`Speedtest`) with non-blocking network throughput & latency benchmarks.

use flex_core::{Menu, Row, RowId, Tab, Target};

use crate::exec::net::{
    format_bytes, format_speed, scan_interfaces, scan_top_talkers, InterfaceInfo, ProcessBandwidth,
};
use crate::exec::speedtest::{self, SpeedtestPhase, SpeedtestSnapshot};
use crate::providers;

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "net";

pub const TAB_BANDWIDTH: &str = "Bandwidth";

pub const TAB_INTERFACES: &str = "Interfaces";

pub const TAB_SPEEDTEST: &str = "Speedtest";

pub const SPEEDTEST_RUN_ID: &str = "speedtest:run";

/// Build the `Bandwidth` tab: lists active processes sorted by bandwidth consumption.
#[must_use]
pub fn bandwidth_tab() -> Tab {
    let procs = scan_top_talkers();
    let rows = build_bandwidth_rows(&procs);
    let mut tab = Tab::with_rows(TAB_BANDWIDTH, rows);
    tab.bare_rows = false;
    tab.filterable = true;
    tab.deletable = true;
    tab
}

/// Convert process bandwidth records into wiremix `Row` objects.
fn build_bandwidth_rows(procs: &[ProcessBandwidth]) -> Vec<Row> {
    use std::fmt::Write as _;

    if procs.is_empty() {
        return vec![providers::empty_row("— no active network processes —")];
    }
    procs
        .iter()
        .map(|p| {
            let id = RowId::new(format!("proc:{}", p.pid));
            let mut label = String::with_capacity(p.comm.len() + 1 + 10);
            label.push_str(&p.comm);
            label.push(' ');
            let _ = write!(label, "{}", p.pid);
            let meta = format!(
                "⬇ {}  ⬆ {}",
                format_speed(p.rx_rate),
                format_speed(p.tx_rate)
            );
            let mut row = Row::with_meta(id, label, meta);
            row.confirmable = true;
            row.volume = Some(p.share);
            row
        })
        .collect()
}

/// Build the `Interfaces` tab: lists network interfaces and addresses.
#[must_use]
pub fn interfaces_tab() -> Tab {
    let ifaces = scan_interfaces();
    let rows = build_interface_rows(&ifaces);
    let mut tab = Tab::with_rows(TAB_INTERFACES, rows);
    tab.bare_rows = false;
    tab.filterable = true;
    tab.deletable = false;
    tab
}

/// Convert interface records into wiremix `Row` objects.
fn build_interface_rows(ifaces: &[InterfaceInfo]) -> Vec<Row> {
    if ifaces.is_empty() {
        return vec![providers::empty_row("— no network interfaces —")];
    }

    ifaces
        .iter()
        .map(|iface| {
            let id = RowId::new(format!("iface:{}", iface.name));
            let label = format!("{} ({})", iface.name, iface.operstate);
            let ip_part = iface.ip_cidr.as_deref().unwrap_or("no carrier");
            let meta = format!(
                "{ip_part}  ·  ⬇ {}  ⬆ {}",
                format_bytes(iface.rx_bytes),
                format_bytes(iface.tx_bytes)
            );

            let mut row = Row::with_meta(id, label, meta);
            row.is_default = iface.is_default;

            let mut targets = Vec::new();
            if let Some(ref ip) = iface.ip_cidr {
                let clean_ip = ip.split('/').next().unwrap_or(ip);
                targets.push(Target::new(
                    RowId::new(format!("copy:{clean_ip}")),
                    format!("Copy IPv4 ({clean_ip})"),
                ));
            }
            if let Some(ref gw) = iface.gateway {
                targets.push(Target::new(
                    RowId::new(format!("copy:{gw}")),
                    format!("Copy Gateway ({gw})"),
                ));
            }
            if iface.name.starts_with("wl") {
                targets.push(Target::new(RowId::new("wifi"), "Open Wi-Fi Picker"));
            }

            row.targets = targets;
            row
        })
        .collect()
}

/// Build the `Speedtest` tab: interactive throughput & latency benchmark.
#[must_use]
pub fn speedtest_tab() -> Tab {
    let snapshot = speedtest::get_snapshot();
    let rows = build_speedtest_rows(&snapshot);
    let mut tab = Tab::with_rows(TAB_SPEEDTEST, rows);
    tab.bare_rows = false;
    tab.filterable = false;
    tab.deletable = false;
    tab
}

/// Convert speedtest state snapshot into wiremix `Row` objects.
#[must_use]
pub fn build_speedtest_rows(snapshot: &SpeedtestSnapshot) -> Vec<Row> {
    let run_meta = match snapshot.phase {
        SpeedtestPhase::Idle => "[Enter] Start",
        SpeedtestPhase::TestingPing => "Probing Ping...",
        SpeedtestPhase::TestingDownload => "Testing Download...",
        SpeedtestPhase::TestingUpload => "Testing Upload...",
        SpeedtestPhase::Complete => "[Enter] Retest",
        SpeedtestPhase::Failed => "[Enter] Retry",
    };

    let run_row = Row::with_meta(RowId::new(SPEEDTEST_RUN_ID), "Run Full Speedtest", run_meta);

    let ping_row = Row::with_meta(
        RowId::new("speedtest:ping"),
        "Latency / Ping",
        snapshot.ping_display(),
    );

    let download_row = Row::with_meta(
        RowId::new("speedtest:download"),
        "Download Speed",
        snapshot.download_display(),
    );

    let upload_row = Row::with_meta(
        RowId::new("speedtest:upload"),
        "Upload Speed",
        snapshot.upload_display(),
    );

    let server_row = Row::with_meta(
        RowId::new("speedtest:server"),
        "Target Server",
        snapshot.target_server.clone(),
    );

    vec![run_row, ping_row, download_row, upload_row, server_row]
}

/// Build the complete interactive `flex-net` menu.
#[must_use]
pub fn net_menu() -> Menu {
    let mut tabs = vec![bandwidth_tab(), interfaces_tab(), speedtest_tab()];
    // Pids and scan snapshots come and go: searchable, but nothing stable
    // worth learning.
    for tab in &mut tabs {
        tab.learnable = false;
    }
    providers::menu(PROVIDER, tabs)
}

/// Per-tick refresh for `flex-net`.
///
/// OPT-6: skips unless a `net` tab is active (throttling to 3 s lives in
/// [`providers::tick_hook`]). OPT-9: reuses row allocations via
/// [`providers::sync_rows_in_place`] with a single focus-restore pass.
pub fn refresh(menu: &mut Menu) {
    let active = menu.app.active_tab().is_some_and(|tab| {
        tab.name == TAB_BANDWIDTH || tab.name == TAB_INTERFACES || tab.name == TAB_SPEEDTEST
    });
    if !active {
        return;
    }
    let previous = menu.app.focused_row().map(|row| row.id.clone());

    if let Some(tab) = menu
        .app
        .tabs
        .iter_mut()
        .find(|tab| tab.name == TAB_BANDWIDTH)
    {
        let procs = scan_top_talkers();
        let fresh = build_bandwidth_rows(&procs);
        super::sync_rows_in_place(&mut tab.rows, fresh);
    }

    if let Some(tab) = menu
        .app
        .tabs
        .iter_mut()
        .find(|tab| tab.name == TAB_INTERFACES)
    {
        let ifaces = scan_interfaces();
        let fresh = build_interface_rows(&ifaces);
        super::sync_rows_in_place(&mut tab.rows, fresh);
    }

    if let Some(tab) = menu
        .app
        .tabs
        .iter_mut()
        .find(|tab| tab.name == TAB_SPEEDTEST)
    {
        let snapshot = speedtest::get_snapshot();
        let fresh = build_speedtest_rows(&snapshot);
        super::sync_rows_in_place(&mut tab.rows, fresh);
    }

    super::restore_focus(menu, previous);
}
