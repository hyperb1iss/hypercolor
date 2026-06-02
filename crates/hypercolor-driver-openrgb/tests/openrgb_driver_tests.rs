use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_driver_api::{
    DiscoveryConnectBehavior, DiscoveryRequest, DriverConfigView, DriverCredentialStore,
    DriverDiscoveryState, DriverHost, DriverModule, DriverRuntimeActions, DriverTrackedDevice,
    HealthStatus,
};
use hypercolor_driver_openrgb::{
    DESCRIPTOR, OpenRgbConfig, OpenRgbDriverModule, OpenRgbOwnership, OpenRgbOwnershipMode,
    OpenRgbTeardownPolicy,
};
use hypercolor_openrgb_sdk::{
    CLIENT_MAX_PROTOCOL_VERSION, ColorMode, ModeFlag, Packet, PacketDecoder, PacketHeader,
    PacketId, RgbColor,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::DeviceId;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn driver_discovers_connects_and_writes_through_sdk_bridge() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_driver_server(listener));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::DetectorPartitioned,
            allowed_detector_classes: vec!["hid".to_owned()],
            native_claimed_detector_classes: Vec::new(),
            allow_low_confidence: false,
        },
        detector_partition_confirmed: true,
        controller_fps: BTreeMap::from([("hid".to_owned(), 45)]),
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;

    let discovery = module
        .discovery()
        .expect("OpenRGB should expose discovery")
        .discover(
            &host,
            &DiscoveryRequest {
                timeout: Duration::from_secs(2),
                mdns_enabled: false,
            },
            view,
        )
        .await
        .expect("discovery should read fake OpenRGB controller");

    assert_eq!(discovery.devices.len(), 1);
    let discovered = &discovery.devices[0];
    assert_eq!(
        discovered.connect_behavior,
        DiscoveryConnectBehavior::AutoConnect
    );
    assert_eq!(discovered.metadata["endpoint"], endpoint.to_string());
    assert_eq!(discovered.metadata["detector_class"], "hid");
    assert_eq!(discovered.metadata["identity_confidence"], "high");
    assert_eq!(discovered.metadata["output_enabled"], "true");
    assert_eq!(discovered.info.name, "Acme Board");
    assert_eq!(discovered.info.capabilities.led_count, 2);
    assert_eq!(discovered.info.capabilities.max_fps, 45);
    assert!(discovered.info.capabilities.supports_direct);

    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    assert_eq!(devices.len(), 1);
    let device_id = devices[0].id;

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect selected controller");
    assert_eq!(backend.target_fps(&device_id), Some(45));
    tokio::time::sleep(Duration::from_millis(20)).await;
    backend
        .write_colors(&device_id, &[[10, 20, 30], [40, 50, 60]])
        .await
        .expect("backend should stream colors through SDK");

    let update = server.await.expect("server task should join");
    assert_eq!(update.header.device_index, 1);
    assert_eq!(update.header.packet_id, PacketId::UpdateLeds);
    assert_eq!(update.payload.len(), 14);
    assert_eq!(&update.payload[0..4], &14_u32.to_le_bytes());
    assert_eq!(&update.payload[4..6], &2_u16.to_le_bytes());
    assert_eq!(&update.payload[6..10], &[10, 20, 30, 0]);
    assert_eq!(&update.payload[10..14], &[40, 50, 60, 0]);
}

#[tokio::test]
async fn backend_reconnects_after_openrgb_socket_closes() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_reconnect_server(listener));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect selected controller");
    tokio::time::sleep(Duration::from_millis(20)).await;
    backend
        .write_colors(&device_id, &[[90, 80, 70], [60, 50, 40]])
        .await
        .expect("backend should reconnect and stream colors");

    let update = server.await.expect("server task should join");
    assert_eq!(update.header.device_index, 0);
    assert_eq!(update.header.packet_id, PacketId::UpdateLeds);
    assert_eq!(&update.payload[6..10], &[90, 80, 70, 0]);
    assert_eq!(&update.payload[10..14], &[60, 50, 40, 0]);
}

#[tokio::test]
async fn connect_re_resolves_controller_index_before_mode_setup() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_connect_remap_server(listener));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect remapped controller");
    backend
        .write_colors(&device_id, &[[9, 8, 7], [6, 5, 4]])
        .await
        .expect("backend should stream to remapped index");

    let update = server.await.expect("server task should join");
    assert_eq!(update.header.device_index, 1);
    assert_eq!(update.header.packet_id, PacketId::UpdateLeds);
    assert_eq!(&update.payload[6..10], &[9, 8, 7, 0]);
    assert_eq!(&update.payload[10..14], &[6, 5, 4, 0]);
}

#[tokio::test]
async fn connect_fails_when_re_resolved_controller_disappears() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_connect_missing_server(listener));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    let error = backend
        .connect(&device_id)
        .await
        .expect_err("backend should reject disappeared remap");
    assert!(error.to_string().contains("disappeared"));
    server.await.expect("server task should join");
}

#[tokio::test]
async fn backend_reports_openrgb_health_states() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_drain_server(listener));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let unknown_id = DeviceId::new();

    assert_eq!(
        backend
            .health_check(&unknown_id)
            .await
            .expect("unknown health should resolve"),
        HealthStatus::Unreachable
    );

    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;
    assert_eq!(
        backend
            .health_check(&device_id)
            .await
            .expect("discovered health should resolve"),
        HealthStatus::Degraded
    );

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect selected controller");
    assert_eq!(
        backend
            .health_check(&device_id)
            .await
            .expect("connected health should resolve"),
        HealthStatus::Healthy
    );

    backend
        .disconnect(&device_id)
        .await
        .expect("backend should disconnect selected controller");
    assert_eq!(
        backend
            .health_check(&device_id)
            .await
            .expect("disconnected discovered health should resolve"),
        HealthStatus::Degraded
    );
    server.abort();
}

#[tokio::test]
async fn connect_rejects_unapproved_active_mode_after_setup() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_setup_readback_server(
        listener,
        controller_payload_v5_with_restore_mode("Board", "SER123", "hidraw0"),
    ));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    let error = backend
        .connect(&device_id)
        .await
        .expect_err("backend should reject non-writable active readback");
    assert!(error.to_string().contains("active mode"));
    server.await.expect("server task should join");
}

#[tokio::test]
async fn connect_rejects_missing_active_mode_after_setup() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_setup_readback_server(
        listener,
        controller_payload_v5_with_invalid_active_mode("Board", "SER123", "hidraw0"),
    ));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    let error = backend
        .connect(&device_id)
        .await
        .expect_err("backend should reject missing active readback");
    assert!(error.to_string().contains("no active mode"));
    server.await.expect("server task should join");
}

#[tokio::test]
async fn frame_sink_collapses_burst_to_latest_openrgb_frame() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_latest_value_server(listener));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        teardown_policy: OpenRgbTeardownPolicy::LeaveLastFrame,
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect selected controller");
    let frame_sink = backend
        .frame_sink(&device_id)
        .expect("connected controller should expose frame sink");

    frame_sink
        .write_colors_shared(Arc::new(vec![[1, 2, 3], [4, 5, 6]]))
        .await
        .expect("sink should accept first queued frame");
    frame_sink
        .write_colors_shared(Arc::new(vec![[7, 8, 9], [10, 11, 12]]))
        .await
        .expect("sink should replace stale queued frame");
    frame_sink
        .write_colors_shared(Arc::new(vec![[13, 14, 15], [16, 17, 18]]))
        .await
        .expect("sink should accept latest queued frame");

    let update = server.await.expect("server task should join");
    assert_eq!(update.header.device_index, 0);
    assert_eq!(update.header.packet_id, PacketId::UpdateLeds);
    assert_eq!(&update.payload[6..10], &[13, 14, 15, 0]);
    assert_eq!(&update.payload[10..14], &[16, 17, 18, 0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_completes_while_frame_sink_writers_race() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_drain_server(listener));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        teardown_policy: OpenRgbTeardownPolicy::LeaveLastFrame,
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect selected controller");
    let frame_sink = backend
        .frame_sink(&device_id)
        .expect("connected controller should expose frame sink");

    let mut writers = Vec::new();
    for writer_index in 0_u8..4 {
        let frame_sink = Arc::clone(&frame_sink);
        writers.push(tokio::spawn(async move {
            for frame_index in 0_u8..64 {
                let color = writer_index.saturating_mul(64).saturating_add(frame_index);
                let _ = frame_sink
                    .write_colors_shared(Arc::new(vec![[color, 10, 20], [color, 30, 40]]))
                    .await;
                tokio::task::yield_now().await;
            }
        }));
    }
    tokio::task::yield_now().await;

    tokio::time::timeout(Duration::from_millis(500), backend.disconnect(&device_id))
        .await
        .expect("disconnect should not hang while sink writers race")
        .expect("disconnect should complete successfully");

    for writer in writers {
        writer.await.expect("writer task should join");
    }
    server.abort();
}

#[tokio::test]
async fn disconnect_restores_previous_openrgb_mode() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_teardown_on_disconnect_server(
        listener,
        controller_payload_v5_with_restore_mode("Board", "SER123", "hidraw0"),
        DisconnectExpectation::RestoreMode(1),
    ));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        teardown_policy: OpenRgbTeardownPolicy::RestorePreviousOrLeave,
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect selected controller");
    let frame_sink = backend
        .frame_sink(&device_id)
        .expect("connected controller should expose frame sink");
    backend
        .disconnect(&device_id)
        .await
        .expect("backend should restore previous mode on disconnect");
    let error = frame_sink
        .write_colors_shared(Arc::new(vec![[20, 30, 40]; 2]))
        .await
        .expect_err("stale frame sink should stop after disconnect");
    assert!(error.to_string().contains("disconnected"));

    let TeardownOutcome::Packet(restore) = server.await.expect("server task should join") else {
        panic!("restore teardown should write a packet");
    };
    assert_eq!(restore.header.device_index, 0);
    assert_eq!(restore.header.packet_id, PacketId::UpdateMode);
    assert_eq!(&restore.payload[4..8], &1_u32.to_le_bytes());
}

#[tokio::test]
async fn disconnect_fallback_blacks_out_without_previous_mode() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_teardown_on_disconnect_server(
        listener,
        controller_payload_v5("Board", "SER123", "hidraw0"),
        DisconnectExpectation::Blackout,
    ));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        teardown_policy: OpenRgbTeardownPolicy::RestorePreviousOrBlackout,
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect selected controller");
    backend
        .disconnect(&device_id)
        .await
        .expect("backend should blackout on disconnect");

    let TeardownOutcome::Packet(blackout) = server.await.expect("server task should join") else {
        panic!("blackout teardown should write a packet");
    };
    assert_blackout_packet(&blackout);
}

#[tokio::test]
async fn disconnect_blackout_policy_writes_zero_frame() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_teardown_on_disconnect_server(
        listener,
        controller_payload_v5_with_restore_mode("Board", "SER123", "hidraw0"),
        DisconnectExpectation::Blackout,
    ));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        teardown_policy: OpenRgbTeardownPolicy::Blackout,
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect selected controller");
    backend
        .disconnect(&device_id)
        .await
        .expect("backend should blackout on disconnect");

    let TeardownOutcome::Packet(blackout) = server.await.expect("server task should join") else {
        panic!("blackout teardown should write a packet");
    };
    assert_blackout_packet(&blackout);
}

#[tokio::test]
async fn disconnect_leave_last_frame_sends_no_teardown_packet() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let endpoint = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_teardown_on_disconnect_server(
        listener,
        controller_payload_v5_with_restore_mode("Board", "SER123", "hidraw0"),
        DisconnectExpectation::LeaveLastFrame,
    ));
    let config = OpenRgbConfig {
        endpoints: vec![endpoint],
        ownership: OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        },
        teardown_policy: OpenRgbTeardownPolicy::LeaveLastFrame,
        ..OpenRgbConfig::default()
    };
    let entry = config_entry(&config);
    let view = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry: &entry,
    };
    let host = NullHost;
    let module = OpenRgbDriverModule;
    let mut backend = module
        .build_output_backend(&host, view)
        .expect("backend construction should succeed")
        .expect("OpenRGB should build an output backend");
    let devices = backend
        .discover()
        .await
        .expect("backend discovery should read fake OpenRGB controller");
    let device_id = devices[0].id;

    backend
        .connect(&device_id)
        .await
        .expect("backend should connect selected controller");
    backend
        .disconnect(&device_id)
        .await
        .expect("backend should leave last frame on disconnect");

    let TeardownOutcome::NoPacket = server.await.expect("server task should join") else {
        panic!("leave-last-frame teardown should not write a packet");
    };
}

fn assert_blackout_packet(packet: &Packet) {
    assert_eq!(packet.header.device_index, 0);
    assert_eq!(packet.header.packet_id, PacketId::UpdateLeds);
    assert_eq!(packet.payload.len(), 14);
    assert_eq!(&packet.payload[0..4], &14_u32.to_le_bytes());
    assert_eq!(&packet.payload[4..6], &2_u16.to_le_bytes());
    assert_eq!(&packet.payload[6..10], &[0, 0, 0, 0]);
    assert_eq!(&packet.payload[10..14], &[0, 0, 0, 0]);
}

async fn run_driver_server(listener: TcpListener) -> Packet {
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .expect("fake OpenRGB server should accept client");
        if let Some(update) = handle_driver_connection(stream).await {
            return update;
        }
    }
}

async fn run_connect_remap_server(listener: TcpListener) -> Packet {
    let mut connection_index = 0_u32;
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .expect("fake OpenRGB server should accept client");
        if let Some(update) = handle_connect_remap_connection(stream, connection_index).await {
            return update;
        }
        connection_index = connection_index.saturating_add(1);
    }
}

async fn run_connect_missing_server(listener: TcpListener) {
    let mut connection_index = 0_u32;
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .expect("fake OpenRGB server should accept client");
        if handle_connect_missing_connection(stream, connection_index).await {
            return;
        }
        connection_index = connection_index.saturating_add(1);
    }
}

async fn handle_connect_missing_connection(mut stream: TcpStream, connection_index: u32) -> bool {
    let mut decoder = PacketDecoder::new();
    while let Some(packet) = read_next_packet(&mut stream, &mut decoder).await {
        match packet.header.packet_id {
            PacketId::RequestProtocolVersion => {
                assert_eq!(
                    packet.payload,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
                );
                send_packet(
                    &mut stream,
                    PacketId::RequestProtocolVersion,
                    0,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::SetClientName => {
                assert_eq!(packet.payload, b"Hypercolor\0");
            }
            PacketId::RequestControllerCount => {
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerCount,
                    0,
                    1_u32.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::RequestControllerData => {
                assert_eq!(packet.header.device_index, 0);
                let payload = if connection_index == 0 {
                    controller_payload_v5("Board", "SER123", "hidraw0")
                } else {
                    controller_payload_v5("Other Board", "OTHER", "hidraw1")
                };
                send_packet(&mut stream, PacketId::RequestControllerData, 0, payload).await;
                if connection_index > 0 {
                    return true;
                }
            }
            PacketId::SetCustomMode | PacketId::UpdateMode | PacketId::UpdateLeds => {
                panic!("connect should fail before OpenRGB output setup")
            }
            other => panic!("unexpected OpenRGB client packet: {other:?}"),
        }
    }
    false
}

async fn handle_connect_remap_connection(
    mut stream: TcpStream,
    connection_index: u32,
) -> Option<Packet> {
    let mut decoder = PacketDecoder::new();
    while let Some(packet) = read_next_packet(&mut stream, &mut decoder).await {
        match packet.header.packet_id {
            PacketId::RequestProtocolVersion => {
                assert_eq!(
                    packet.payload,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
                );
                send_packet(
                    &mut stream,
                    PacketId::RequestProtocolVersion,
                    0,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::SetClientName => {
                assert_eq!(packet.payload, b"Hypercolor\0");
            }
            PacketId::RequestControllerCount => {
                let count = if connection_index == 0 { 1_u32 } else { 2_u32 };
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerCount,
                    0,
                    count.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::RequestControllerData => {
                assert!(
                    connection_index > 0 || packet.header.device_index == 0,
                    "discovery should only request the initial controller"
                );
                let payload = if connection_index > 0 && packet.header.device_index == 0 {
                    controller_payload_v5("Other Board", "OTHER", "hidraw1")
                } else {
                    controller_payload_v5("Board", "SER123", "hidraw0")
                };
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerData,
                    packet.header.device_index,
                    payload,
                )
                .await;
            }
            PacketId::SetCustomMode => {
                assert_eq!(connection_index, 1);
                assert_eq!(packet.header.device_index, 1);
            }
            PacketId::UpdateMode => {
                assert_eq!(connection_index, 1);
                assert_eq!(packet.header.device_index, 1);
                assert!(!packet.payload.is_empty());
            }
            PacketId::UpdateLeds => return Some(packet),
            other => panic!("unexpected OpenRGB client packet: {other:?}"),
        }
    }
    None
}

async fn handle_driver_connection(mut stream: TcpStream) -> Option<Packet> {
    let mut decoder = PacketDecoder::new();
    let mut reordered = false;
    let mut notified_reorder = false;
    while let Some(packet) = read_next_packet(&mut stream, &mut decoder).await {
        match packet.header.packet_id {
            PacketId::RequestProtocolVersion => {
                assert_eq!(
                    packet.payload,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
                );
                send_packet(
                    &mut stream,
                    PacketId::RequestProtocolVersion,
                    0,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::SetClientName => {
                assert_eq!(packet.payload, b"Hypercolor\0");
            }
            PacketId::RequestControllerCount => {
                let count = if reordered { 2_u32 } else { 1_u32 };
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerCount,
                    0,
                    count.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::RequestControllerData => {
                assert_eq!(
                    packet.payload,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
                );
                let payload = if reordered && packet.header.device_index == 0 {
                    controller_payload_v5("Other Board", "OTHER", "hidraw1")
                } else {
                    controller_payload_v5("Board", "SER123", "hidraw0")
                };
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerData,
                    packet.header.device_index,
                    payload,
                )
                .await;
            }
            PacketId::SetCustomMode => {
                assert!(packet.header.device_index <= 1);
            }
            PacketId::UpdateMode => {
                assert!(packet.header.device_index <= 1);
                assert!(!packet.payload.is_empty());
                if !notified_reorder {
                    send_packet(&mut stream, PacketId::DeviceListUpdated, 0, Vec::new()).await;
                    reordered = true;
                    notified_reorder = true;
                }
            }
            PacketId::UpdateLeds => return Some(packet),
            other => panic!("unexpected OpenRGB client packet: {other:?}"),
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisconnectExpectation {
    RestoreMode(u32),
    Blackout,
    LeaveLastFrame,
}

#[derive(Debug)]
enum TeardownOutcome {
    Packet(Packet),
    NoPacket,
}

async fn run_teardown_on_disconnect_server(
    listener: TcpListener,
    controller_payload: Vec<u8>,
    expectation: DisconnectExpectation,
) -> TeardownOutcome {
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .expect("fake OpenRGB server should accept client");
        if let Some(outcome) =
            handle_teardown_connection(stream, &controller_payload, expectation).await
        {
            return outcome;
        }
    }
}

async fn handle_teardown_connection(
    mut stream: TcpStream,
    controller_payload: &[u8],
    expectation: DisconnectExpectation,
) -> Option<TeardownOutcome> {
    let mut decoder = PacketDecoder::new();
    let mut output_mode_updates = 0_u32;
    let mut saw_output_mode_setup = false;
    while let Some(packet) = read_next_packet(&mut stream, &mut decoder).await {
        match packet.header.packet_id {
            PacketId::RequestProtocolVersion => {
                assert_eq!(
                    packet.payload,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
                );
                send_packet(
                    &mut stream,
                    PacketId::RequestProtocolVersion,
                    0,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::SetClientName => {
                assert_eq!(packet.payload, b"Hypercolor\0");
            }
            PacketId::RequestControllerCount => {
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerCount,
                    0,
                    1_u32.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::RequestControllerData => {
                assert_eq!(packet.header.device_index, 0);
                let payload = if saw_output_mode_setup {
                    controller_payload_v5("Board", "SER123", "hidraw0")
                } else {
                    controller_payload.to_vec()
                };
                send_packet(&mut stream, PacketId::RequestControllerData, 0, payload).await;
            }
            PacketId::SetCustomMode => {
                assert_eq!(packet.header.device_index, 0);
            }
            PacketId::UpdateMode => {
                assert_eq!(packet.header.device_index, 0);
                let mode_index = u32::from_le_bytes([
                    packet.payload[4],
                    packet.payload[5],
                    packet.payload[6],
                    packet.payload[7],
                ]);
                if output_mode_updates == 0 {
                    assert_eq!(mode_index, 0);
                    output_mode_updates += 1;
                    saw_output_mode_setup = true;
                } else {
                    let DisconnectExpectation::RestoreMode(expected_mode_index) = expectation
                    else {
                        panic!("unexpected restore mode teardown packet");
                    };
                    assert_eq!(mode_index, expected_mode_index);
                    return Some(TeardownOutcome::Packet(packet));
                }
            }
            PacketId::UpdateLeds => {
                assert_eq!(expectation, DisconnectExpectation::Blackout);
                assert_blackout_packet(&packet);
                return Some(TeardownOutcome::Packet(packet));
            }
            other => panic!("unexpected OpenRGB client packet: {other:?}"),
        }
    }
    if saw_output_mode_setup && expectation == DisconnectExpectation::LeaveLastFrame {
        Some(TeardownOutcome::NoPacket)
    } else {
        None
    }
}

async fn run_setup_readback_server(listener: TcpListener, setup_readback_payload: Vec<u8>) {
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .expect("fake OpenRGB server should accept client");
        if handle_setup_readback_connection(stream, &setup_readback_payload).await {
            return;
        }
    }
}

async fn handle_setup_readback_connection(
    mut stream: TcpStream,
    setup_readback_payload: &[u8],
) -> bool {
    let mut decoder = PacketDecoder::new();
    let mut saw_output_mode_setup = false;
    while let Some(packet) = read_next_packet(&mut stream, &mut decoder).await {
        match packet.header.packet_id {
            PacketId::RequestProtocolVersion => {
                assert_eq!(
                    packet.payload,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
                );
                send_packet(
                    &mut stream,
                    PacketId::RequestProtocolVersion,
                    0,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::SetClientName => {
                assert_eq!(packet.payload, b"Hypercolor\0");
            }
            PacketId::RequestControllerCount => {
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerCount,
                    0,
                    1_u32.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::RequestControllerData => {
                assert_eq!(packet.header.device_index, 0);
                let payload = if saw_output_mode_setup {
                    setup_readback_payload.to_vec()
                } else {
                    controller_payload_v5("Board", "SER123", "hidraw0")
                };
                send_packet(&mut stream, PacketId::RequestControllerData, 0, payload).await;
                if saw_output_mode_setup {
                    return true;
                }
            }
            PacketId::SetCustomMode => {
                assert_eq!(packet.header.device_index, 0);
            }
            PacketId::UpdateMode => {
                assert_eq!(packet.header.device_index, 0);
                assert!(!packet.payload.is_empty());
                saw_output_mode_setup = true;
            }
            other => panic!("unexpected OpenRGB client packet: {other:?}"),
        }
    }
    false
}

async fn run_reconnect_server(listener: TcpListener) -> Packet {
    let mut connection_index = 0_u32;
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .expect("fake OpenRGB server should accept client");
        let close_after_mode_setup = connection_index == 1;
        connection_index += 1;
        if let Some(update) = handle_reconnect_connection(stream, close_after_mode_setup).await {
            return update;
        }
    }
}

async fn run_latest_value_server(listener: TcpListener) -> Packet {
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .expect("fake OpenRGB server should accept client");
        if let Some(update) = handle_reconnect_connection(stream, false).await {
            return update;
        }
    }
}

async fn run_drain_server(listener: TcpListener) {
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .expect("fake OpenRGB server should accept client");
        handle_drain_connection(stream).await;
    }
}

async fn handle_drain_connection(mut stream: TcpStream) {
    let mut decoder = PacketDecoder::new();
    while let Some(packet) = read_next_packet(&mut stream, &mut decoder).await {
        match packet.header.packet_id {
            PacketId::RequestProtocolVersion => {
                assert_eq!(
                    packet.payload,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
                );
                send_packet(
                    &mut stream,
                    PacketId::RequestProtocolVersion,
                    0,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::SetClientName => {
                assert_eq!(packet.payload, b"Hypercolor\0");
            }
            PacketId::RequestControllerCount => {
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerCount,
                    0,
                    1_u32.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::RequestControllerData => {
                assert_eq!(packet.header.device_index, 0);
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerData,
                    0,
                    controller_payload_v5("Board", "SER123", "hidraw0"),
                )
                .await;
            }
            PacketId::SetCustomMode | PacketId::UpdateMode | PacketId::UpdateLeds => {
                assert_eq!(packet.header.device_index, 0);
            }
            other => panic!("unexpected OpenRGB client packet: {other:?}"),
        }
    }
}

async fn handle_reconnect_connection(
    mut stream: TcpStream,
    close_after_mode_setup: bool,
) -> Option<Packet> {
    let mut decoder = PacketDecoder::new();
    while let Some(packet) = read_next_packet(&mut stream, &mut decoder).await {
        match packet.header.packet_id {
            PacketId::RequestProtocolVersion => {
                assert_eq!(
                    packet.payload,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
                );
                send_packet(
                    &mut stream,
                    PacketId::RequestProtocolVersion,
                    0,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::SetClientName => {
                assert_eq!(packet.payload, b"Hypercolor\0");
            }
            PacketId::RequestControllerCount => {
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerCount,
                    0,
                    1_u32.to_le_bytes().to_vec(),
                )
                .await;
            }
            PacketId::RequestControllerData => {
                assert_eq!(packet.header.device_index, 0);
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerData,
                    0,
                    controller_payload_v5("Board", "SER123", "hidraw0"),
                )
                .await;
            }
            PacketId::SetCustomMode => {
                assert_eq!(packet.header.device_index, 0);
            }
            PacketId::UpdateMode => {
                assert_eq!(packet.header.device_index, 0);
                assert!(!packet.payload.is_empty());
                if close_after_mode_setup {
                    return None;
                }
            }
            PacketId::UpdateLeds => return Some(packet),
            other => panic!("unexpected OpenRGB client packet: {other:?}"),
        }
    }
    None
}

async fn read_next_packet(stream: &mut TcpStream, decoder: &mut PacketDecoder) -> Option<Packet> {
    loop {
        if let Some(packet) = decoder
            .next_packet()
            .expect("fake server should decode client packet")
        {
            return Some(packet);
        }

        let mut bytes = [0_u8; 1024];
        let read = stream
            .read(&mut bytes)
            .await
            .expect("fake server should read client packet");
        if read == 0 {
            return None;
        }
        decoder.push(&bytes[..read]);
    }
}

async fn send_packet(
    stream: &mut TcpStream,
    packet_id: PacketId,
    device_index: u32,
    payload: Vec<u8>,
) {
    let size = u32::try_from(payload.len()).expect("test packet should fit u32");
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
        .expect("fake OpenRGB server should write packet");
}

fn config_entry(config: &OpenRgbConfig) -> DriverConfigEntry {
    let settings = serde_json::to_value(config).expect("config should serialize");
    let settings = settings
        .as_object()
        .expect("config should serialize to object")
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    DriverConfigEntry::enabled(settings)
}

fn controller_payload_v5(name: &str, serial: &str, location: &str) -> Vec<u8> {
    controller_payload_v5_inner(name, serial, location, false)
}

fn controller_payload_v5_with_restore_mode(name: &str, serial: &str, location: &str) -> Vec<u8> {
    controller_payload_v5_inner(name, serial, location, true)
}

fn controller_payload_v5_with_invalid_active_mode(
    name: &str,
    serial: &str,
    location: &str,
) -> Vec<u8> {
    controller_payload_v5_with_active_mode(name, serial, location, false, 99)
}

fn controller_payload_v5_inner(
    name: &str,
    serial: &str,
    location: &str,
    include_restore_mode: bool,
) -> Vec<u8> {
    let active_mode = if include_restore_mode { 1 } else { 0 };
    controller_payload_v5_with_active_mode(
        name,
        serial,
        location,
        include_restore_mode,
        active_mode,
    )
}

fn controller_payload_v5_with_active_mode(
    name: &str,
    serial: &str,
    location: &str,
    include_restore_mode: bool,
    active_mode: i32,
) -> Vec<u8> {
    let mut body = Vec::new();
    push_u32(&mut body, 0);
    push_i32(&mut body, 5);
    push_str(&mut body, name);
    push_str(&mut body, "Acme");
    push_str(&mut body, "Keyboard controller");
    push_str(&mut body, "1.2.3");
    push_str(&mut body, serial);
    push_str(&mut body, location);
    push_u16(&mut body, if include_restore_mode { 2 } else { 1 });
    push_i32(&mut body, active_mode);
    push_mode(&mut body);
    if include_restore_mode {
        push_restore_mode(&mut body);
    }
    push_u16(&mut body, 1);
    push_zone(&mut body);
    push_u16(&mut body, 2);
    push_str(&mut body, "LED 0");
    push_u32(&mut body, 0);
    push_str(&mut body, "LED 1");
    push_u32(&mut body, 1);
    push_u16(&mut body, 2);
    body.extend_from_slice(&RgbColor::new(1, 2, 3).to_wire_bytes());
    body.extend_from_slice(&RgbColor::new(4, 5, 6).to_wire_bytes());
    push_u16(&mut body, 0);
    push_u32(&mut body, 0);
    let size = u32::try_from(body.len()).expect("fixture should fit u32");
    body[0..4].copy_from_slice(&size.to_le_bytes());
    body
}

fn push_mode(body: &mut Vec<u8>) {
    push_str(body, "Direct");
    push_i32(body, 0);
    push_u32(body, ModeFlag::PerLedColor.mask());
    push_u32(body, 0);
    push_u32(body, 100);
    push_u32(body, 0);
    push_u32(body, 100);
    push_u32(body, 0);
    push_u32(body, 0);
    push_u32(body, 0);
    push_u32(body, 100);
    push_u32(body, 0);
    push_u32(body, ColorMode::PerLed.raw());
    push_u16(body, 0);
}

fn push_restore_mode(body: &mut Vec<u8>) {
    push_str(body, "Static");
    push_i32(body, 1);
    push_u32(body, 0);
    push_u32(body, 0);
    push_u32(body, 100);
    push_u32(body, 0);
    push_u32(body, 100);
    push_u32(body, 0);
    push_u32(body, 0);
    push_u32(body, 0);
    push_u32(body, 50);
    push_u32(body, 0);
    push_u32(body, ColorMode::ModeSpecific.raw());
    push_u16(body, 1);
    body.extend_from_slice(&RgbColor::new(8, 9, 10).to_wire_bytes());
}

fn push_zone(body: &mut Vec<u8>) {
    push_str(body, "Main");
    push_i32(body, 1);
    push_u32(body, 2);
    push_u32(body, 2);
    push_u32(body, 2);
    push_u16(body, 0);
    push_u16(body, 1);
    push_str(body, "Half");
    push_i32(body, 1);
    push_u32(body, 0);
    push_u32(body, 2);
    push_u32(body, 0);
}

fn push_str(body: &mut Vec<u8>, value: &str) {
    let len = u16::try_from(value.len() + 1).expect("fixture string should fit u16");
    push_u16(body, len);
    body.extend_from_slice(value.as_bytes());
    body.push(0);
}

fn push_u16(body: &mut Vec<u8>, value: u16) {
    body.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(body: &mut Vec<u8>, value: u32) {
    body.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(body: &mut Vec<u8>, value: i32) {
    body.extend_from_slice(&value.to_le_bytes());
}

#[derive(Debug, Default)]
struct NullHost;

#[async_trait]
impl DriverCredentialStore for NullHost {
    async fn get_json(&self, driver_id: &str, key: &str) -> Result<Option<Value>> {
        let _ = (driver_id, key);
        Ok(None)
    }

    async fn set_json(&self, driver_id: &str, key: &str, value: Value) -> Result<()> {
        let _ = (driver_id, key, value);
        Ok(())
    }

    async fn remove(&self, driver_id: &str, key: &str) -> Result<()> {
        let _ = (driver_id, key);
        Ok(())
    }
}

#[async_trait]
impl DriverRuntimeActions for NullHost {
    async fn activate_device(&self, device_id: DeviceId, backend_id: &str) -> Result<bool> {
        let _ = (device_id, backend_id);
        Ok(false)
    }

    async fn disconnect_device(
        &self,
        device_id: DeviceId,
        backend_id: &str,
        will_retry: bool,
    ) -> Result<bool> {
        let _ = (device_id, backend_id, will_retry);
        Ok(false)
    }
}

#[async_trait]
impl DriverDiscoveryState for NullHost {
    async fn tracked_devices(&self, driver_id: &str) -> Vec<DriverTrackedDevice> {
        let _ = driver_id;
        Vec::new()
    }

    fn load_cached_json(&self, driver_id: &str, key: &str) -> Result<Option<Value>> {
        let _ = (driver_id, key);
        Ok(None)
    }
}

impl DriverHost for NullHost {
    fn credentials(&self) -> &dyn DriverCredentialStore {
        self
    }

    fn runtime(&self) -> &dyn DriverRuntimeActions {
        self
    }

    fn discovery_state(&self) -> &dyn DriverDiscoveryState {
        self
    }
}
