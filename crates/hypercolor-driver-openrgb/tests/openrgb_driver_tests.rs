use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_driver_api::{
    DiscoveryConnectBehavior, DiscoveryRequest, DriverConfigView, DriverCredentialStore,
    DriverDiscoveryState, DriverHost, DriverModule, DriverRuntimeActions, DriverTrackedDevice,
};
use hypercolor_driver_openrgb::{
    DESCRIPTOR, OpenRgbConfig, OpenRgbDriverModule, OpenRgbOwnership, OpenRgbOwnershipMode,
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
    backend
        .write_colors(&device_id, &[[10, 20, 30], [40, 50, 60]])
        .await
        .expect("backend should stream colors through SDK");

    let update = server.await.expect("server task should join");
    assert_eq!(update.header.device_index, 0);
    assert_eq!(update.header.packet_id, PacketId::UpdateLeds);
    assert_eq!(update.payload.len(), 14);
    assert_eq!(&update.payload[0..4], &14_u32.to_le_bytes());
    assert_eq!(&update.payload[4..6], &2_u16.to_le_bytes());
    assert_eq!(&update.payload[6..10], &[10, 20, 30, 0]);
    assert_eq!(&update.payload[10..14], &[40, 50, 60, 0]);
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

async fn handle_driver_connection(mut stream: TcpStream) -> Option<Packet> {
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
                assert_eq!(
                    packet.payload,
                    CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
                );
                send_packet(
                    &mut stream,
                    PacketId::RequestControllerData,
                    0,
                    controller_payload_v5(),
                )
                .await;
            }
            PacketId::SetCustomMode => {
                assert_eq!(packet.header.device_index, 0);
            }
            PacketId::UpdateMode => {
                assert_eq!(packet.header.device_index, 0);
                assert!(!packet.payload.is_empty());
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

fn controller_payload_v5() -> Vec<u8> {
    let mut body = Vec::new();
    push_u32(&mut body, 0);
    push_i32(&mut body, 5);
    push_str(&mut body, "Board");
    push_str(&mut body, "Acme");
    push_str(&mut body, "Keyboard controller");
    push_str(&mut body, "1.2.3");
    push_str(&mut body, "SER123");
    push_str(&mut body, "hidraw0");
    push_u16(&mut body, 1);
    push_i32(&mut body, 0);
    push_mode(&mut body);
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
