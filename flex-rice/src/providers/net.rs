//! `net` provider: bandwidth consumption monitor and network interface status.
//!
//! Implements Tab 1 (`Bandwidth`) with top network-consuming processes ("Top Talkers")
//! and Tab 2 (`Interfaces`) with network interface metadata and default route indicators.

use flex_core::{Menu, Row, RowId, Tab, Target};

use crate::exec::net::{
    format_bytes, format_speed, scan_interfaces, scan_top_talkers, InterfaceInfo, ProcessBandwidth,
};
use crate::providers;

/// Provider name for the `ACTION:` line.
pub const PROVIDER: &str = "net";

/// Tab 1 title: Top bandwidth consumers.
pub const TAB_BANDWIDTH: &str = "Bandwidth";

/// Tab 2 title: Network interfaces.
pub const TAB_INTERFACES: &str = "Interfaces";

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
    if procs.is_empty() {
        return vec![providers::empty_row("— no active network processes —")];
    }

    procs
        .iter()
        .map(|p| {
            let id = RowId::new(format!("proc:{}", p.pid));
            let label = format!("{} {}", p.comm, p.pid);
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

/// Build the complete interactive `flex-net` menu.
#[must_use]
pub fn net_menu() -> Menu {
    let tabs = vec![bandwidth_tab(), interfaces_tab()];
    providers::menu(PROVIDER, tabs)
}

/// Helper to locate row index in visible rows.
fn visible_position(menu: &Menu, predicate: impl Fn(&Row) -> bool) -> Option<usize> {
    let tab = menu.app.active_tab()?;
    menu.app
        .visible_rows()
        .iter()
        .position(|&index| tab.rows.get(index).is_some_and(&predicate))
}

/// Per-tick refresh for `flex-net`.
pub fn refresh(menu: &mut Menu) {
    let previous = menu.app.focused_row().map(|row| row.id.clone());

    if let Some(tab) = menu.app.tabs.get_mut(0) {
        if tab.name == TAB_BANDWIDTH {
            let procs = scan_top_talkers();
            tab.rows = build_bandwidth_rows(&procs);
        }
    }

    if let Some(tab) = menu.app.tabs.get_mut(1) {
        if tab.name == TAB_INTERFACES {
            let ifaces = scan_interfaces();
            tab.rows = build_interface_rows(&ifaces);
        }
    }

    if let Some(id) = previous {
        if let Some(position) = visible_position(menu, |row| row.id == id) {
            if let Some(state) = menu.app.active_tab_mut().map(|tab| &mut tab.state) {
                state.focus = position;
            }
        }
    }
    menu.app.clamp_focus();
}
