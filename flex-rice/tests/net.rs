use flex_rice::exec::net::{self, STAT_PATH_ENV};
use std::path::PathBuf;

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
