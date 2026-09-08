//! The `openrgb` diagnose check (Spec 81 §2.4) against a fake OpenRGB SDK
//! server.

#![cfg(feature = "builtin-drivers")]

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::openrgb_diagnostics::{
    OpenRgbCheckState, OpenRgbEndpointProbe, OpenRgbProbeConfig, OpenRgbProbeOutcome,
    OutputDisabledRoute, openrgb_check, probe_openrgb_endpoints,
};
use hypercolor_openrgb_sdk::{
    CLIENT_MAX_PROTOCOL_VERSION, Packet, PacketDecoder, PacketHeader, PacketId,
};
use hypercolor_types::config::DriverConfigEntry;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Minimal OpenRGB server: negotiate, accept the client name, answer the
/// controller count, then hang up.
async fn run_fake_server(listener: TcpListener, protocol_version: u32, controllers: u32) {
    let (mut stream, _) = listener
        .accept()
        .await
        .expect("fake OpenRGB server should accept the probe");
    let mut decoder = PacketDecoder::new();

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.packet_id, PacketId::RequestProtocolVersion);
    send_packet(
        &mut stream,
        PacketId::RequestProtocolVersion,
        0,
        protocol_version.to_le_bytes().to_vec(),
    )
    .await;

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.packet_id, PacketId::SetClientName);
    assert_eq!(packet.payload, b"Hypercolor Diagnose\0");

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.packet_id, PacketId::RequestControllerCount);
    send_packet(
        &mut stream,
        PacketId::RequestControllerCount,
        0,
        controllers.to_le_bytes().to_vec(),
    )
    .await;
}

async fn read_next_packet(stream: &mut TcpStream, decoder: &mut PacketDecoder) -> Packet {
    loop {
        if let Some(packet) = decoder
            .next_packet()
            .expect("fake server should decode the probe's packet")
        {
            return packet;
        }
        let mut bytes = [0_u8; 1024];
        let read = stream
            .read(&mut bytes)
            .await
            .expect("fake server should read from the probe");
        assert_ne!(read, 0, "probe closed the connection early");
        decoder.push(&bytes[..read]);
    }
}

async fn send_packet(
    stream: &mut TcpStream,
    packet_id: PacketId,
    device_index: u32,
    payload: Vec<u8>,
) {
    let size = u32::try_from(payload.len()).expect("payload fits u32");
    let packet = Packet {
        header: PacketHeader {
            device_index,
            packet_id,
            size,
        },
        payload,
    };
    stream
        .write_all(&packet.encode())
        .await
        .expect("fake server should write");
}

fn probe_config(endpoints: Vec<SocketAddr>) -> OpenRgbProbeConfig {
    OpenRgbProbeConfig {
        endpoints,
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_secs(2),
        write_timeout: Duration::from_secs(2),
    }
}

#[tokio::test]
async fn the_probe_negotiates_and_counts_controllers() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake server should bind");
    let addr = listener.local_addr().expect("local addr");
    let server = tokio::spawn(run_fake_server(listener, CLIENT_MAX_PROTOCOL_VERSION, 3));

    let probes = probe_openrgb_endpoints(&probe_config(vec![addr])).await;
    assert_eq!(
        probes,
        vec![OpenRgbEndpointProbe {
            endpoint: addr,
            outcome: OpenRgbProbeOutcome::Reachable {
                protocol_version: CLIENT_MAX_PROTOCOL_VERSION,
                controller_count: 3,
            },
        }]
    );
    server.await.expect("fake server should finish cleanly");

    let check = openrgb_check(&OpenRgbCheckState::Probed {
        probes,
        disabled_routes: vec![OutputDisabledRoute {
            name: "Nollie N32".to_owned(),
            reason: "native driver owns this device (nollie)".to_owned(),
        }],
    });
    assert_eq!(check.category, "drivers");
    assert_eq!(check.name, "openrgb");
    assert_eq!(check.status, "pass");
    assert!(
        check.detail.contains(&format!(
            "{addr}: reachable, protocol v{CLIENT_MAX_PROTOCOL_VERSION}, 3 controller(s)"
        )),
        "detail names the endpoint result: {}",
        check.detail
    );
    assert!(
        check.detail.contains(
            "output-disabled routes: 1 (Nollie N32: native driver owns this device (nollie))"
        ),
        "detail lists disabled routes with reasons: {}",
        check.detail
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_is_a_warning_when_the_driver_is_enabled() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind to reserve a closed port");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);

    let probes = probe_openrgb_endpoints(&probe_config(vec![addr])).await;
    assert!(matches!(
        probes[0].outcome,
        OpenRgbProbeOutcome::Unreachable { .. }
    ));
    let check = openrgb_check(&OpenRgbCheckState::Probed {
        probes,
        disabled_routes: Vec::new(),
    });
    assert_eq!(check.status, "warning");
    assert!(check.detail.contains(&format!("{addr}: unreachable")));
    assert!(check.detail.contains("output-disabled routes: 0"));
}

#[test]
fn disabled_and_unavailable_bridges_pass() {
    let disabled = openrgb_check(&OpenRgbCheckState::Disabled);
    assert_eq!(
        (disabled.status.as_str(), disabled.detail.as_str()),
        ("pass", "bridge disabled")
    );
    let unavailable = openrgb_check(&OpenRgbCheckState::Unavailable);
    assert_eq!(unavailable.status, "pass");
}

#[test]
fn probe_config_reads_the_driver_entry() {
    let defaults = OpenRgbProbeConfig::from_driver_entry(&DriverConfigEntry::default());
    assert_eq!(
        defaults.endpoints,
        vec![
            "127.0.0.1:6742"
                .parse::<SocketAddr>()
                .expect("default endpoint")
        ]
    );
    assert_eq!(defaults.connect_timeout, Duration::from_millis(750));

    let entry = DriverConfigEntry::enabled(BTreeMap::from([
        (
            "endpoints".to_owned(),
            serde_json::json!(["192.168.1.20:6742", "not an address", "127.0.0.1:7000"]),
        ),
        ("connect_timeout_ms".to_owned(), serde_json::json!(250)),
        ("read_timeout_ms".to_owned(), serde_json::json!(60_000)),
    ]));
    let config = OpenRgbProbeConfig::from_driver_entry(&entry);
    assert_eq!(
        config.endpoints,
        vec![
            "192.168.1.20:6742".parse::<SocketAddr>().expect("first"),
            "127.0.0.1:7000".parse::<SocketAddr>().expect("second"),
        ],
        "unparseable endpoints are skipped, the rest kept in order"
    );
    assert_eq!(config.connect_timeout, Duration::from_millis(250));
    assert_eq!(
        config.read_timeout,
        Duration::from_secs(10),
        "timeouts clamp to the driver's ceiling"
    );
    assert_eq!(config.write_timeout, Duration::from_millis(750));
}

fn isolated_state() -> (AppState, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    (state, tempdir)
}

#[tokio::test]
async fn the_default_check_set_reports_the_bridge_as_disabled() {
    let (state, _tmp) = isolated_state();

    let response = state.domains.diagnostics.collect_default().await;
    let check = response
        .checks
        .iter()
        .find(|check| check.name == "openrgb")
        .expect("openrgb joins the default safe checks");
    assert_eq!(check.status, "pass");
    assert_eq!(check.detail, "bridge disabled");
}
