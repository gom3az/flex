use std::path::PathBuf;

use flex_rice::exec::net::{self, STAT_PATH_ENV};
use flex_rice::providers::net as net_provider;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flex-net-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

#[test]
fn format_speed_units_and_rounding() {
    assert_eq!(net::format_speed(0.0), "0 B/s");
    assert_eq!(net::format_speed(512.0), "512 B/s");
    assert_eq!(net::format_speed(1023.9), "1024 B/s");
    assert_eq!(net::format_speed(1024.0), "1 KB/s");
    assert_eq!(net::format_speed(24_576.0), "24 KB/s");
    assert_eq!(net::format_speed(1_048_576.0), "1.0 MB/s");
    assert_eq!(net::format_speed(1_572_864.0), "1.5 MB/s");
    assert_eq!(net::format_speed(104_857_600.0), "100.0 MB/s");
}

#[test]
fn format_bytes_scales_units() {
    assert_eq!(net::format_bytes(500), "500 B");
    assert_eq!(net::format_bytes(1024), "1 KB");
    assert_eq!(net::format_bytes(1024 * 1024), "1.0 MB");
    assert_eq!(net::format_bytes(1_572_864), "1.5 MB");
    assert_eq!(net::format_bytes(1024 * 1024 * 1024 * 4), "4.0 GB");
}

#[test]
fn hex_to_ipv4_parses_little_endian() {
    assert_eq!(
        net::hex_to_ipv4("0101A8C0"),
        Some("192.168.1.1".to_string())
    );
    assert_eq!(net::hex_to_ipv4("00000000"), Some("0.0.0.0".to_string()));
    assert_eq!(net::hex_to_ipv4("0100007F"), Some("127.0.0.1".to_string()));
    assert_eq!(net::hex_to_ipv4("invalid"), None);
}

#[test]
fn default_interface_from_route_parses_default_gateway() {
    let dir = scratch("route-parse");
    let route_file = dir.join("route");
    let content =
        "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                   eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
                   wlp2s0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n";
    std::fs::write(&route_file, content).expect("write mock route");
    let iface = net::default_interface_from_route(&route_file);
    assert_eq!(iface.as_deref(), Some("wlp2s0"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_proc_net_dev_extracts_bytes() {
    let dir = scratch("dev-parse");
    let dev_file = dir.join("dev");
    let content = "Inter-|   Receive                                                |  Transmit\n\
                   face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
                     lo: 100000      10    0    0    0     0          0         0   100000      10    0    0    0     0       0          0\n\
                 wlp2s0: 50000000   1000    0    0    0     0          0         0  20000000    500    0    0    0     0       0          0\n";
    std::fs::write(&dev_file, content).expect("write mock dev");
    let stats = net::parse_proc_net_dev(&dev_file, "wlp2s0");
    assert_eq!(stats, Some((50_000_000, 20_000_000)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_proc_io_and_comm_parses_mock_files() {
    let dir = scratch("proc-io");
    let io_file = dir.join("io");
    let stat_file = dir.join("stat");

    std::fs::write(
        &io_file,
        "rchar: 123456\nwchar: 654321\nsyscr: 10\nsyscw: 5\n",
    )
    .expect("write io");
    std::fs::write(
        &stat_file,
        "1234 (firefox) S 1 1234 1234 0 -1 4194304 ...\n",
    )
    .expect("write stat");

    let io = net::read_proc_io(&io_file);
    assert_eq!(io, Some((123_456, 654_321)));

    let comm = net::read_proc_comm(&stat_file);
    assert_eq!(comm.as_deref(), Some("firefox"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sample_json_emits_valid_waybar_payload() {
    let dir = scratch("sample-json");
    let stat_file = dir.join("speed.stat");
    std::env::set_var(STAT_PATH_ENV, stat_file.display().to_string());

    let json1 = net::sample_json();
    assert!(
        json1.starts_with('{') && json1.ends_with('}'),
        "valid json brackets: {json1}"
    );
    assert!(json1.contains("\"text\":"), "has text property: {json1}");
    assert!(json1.contains("\"class\":"), "has class property: {json1}");

    // Second sample immediately follows: stat file now exists
    let json2 = net::sample_json();
    assert!(json2.starts_with('{') && json2.ends_with('}'));

    std::env::remove_var(STAT_PATH_ENV);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn net_menu_constructs_all_tabs() {
    let menu = net_provider::net_menu();
    assert_eq!(menu.provider, net_provider::PROVIDER);
    assert_eq!(menu.app.tabs.len(), 3);
    assert_eq!(menu.app.tabs[0].name, net_provider::TAB_BANDWIDTH);
    assert_eq!(menu.app.tabs[1].name, net_provider::TAB_INTERFACES);
    assert_eq!(menu.app.tabs[2].name, net_provider::TAB_SPEEDTEST);
}

#[test]
fn bandwidth_tab_rows_carry_volume_metrics() {
    let tab = net_provider::bandwidth_tab();
    assert_eq!(tab.name, net_provider::TAB_BANDWIDTH);
    assert!(tab.filterable);
    assert!(tab.deletable);
    for row in &tab.rows {
        if row.id.as_str() != "noop" {
            assert!(row.volume.is_some(), "row has volume metric: {row:?}");
            assert!(row.confirmable, "row is confirmable: {row:?}");
        }
    }
}

#[test]
fn interfaces_tab_rows_have_metadata_and_targets() {
    let tab = net_provider::interfaces_tab();
    assert_eq!(tab.name, net_provider::TAB_INTERFACES);
    assert!(tab.filterable);
    assert!(!tab.deletable);
    for row in &tab.rows {
        if row.id.as_str() != "noop" {
            assert!(row.meta.is_some(), "row has meta: {row:?}");
        }
    }
}

#[test]
fn execute_noop_is_ok() {
    assert!(net::execute("noop").is_ok());
}
