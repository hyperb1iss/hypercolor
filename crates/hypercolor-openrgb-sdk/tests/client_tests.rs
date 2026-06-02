use std::net::Ipv4Addr;
use std::time::Duration;

use hypercolor_openrgb_sdk::{
    CLIENT_MAX_PROTOCOL_VERSION, ColorMode, ModeFlag, OpenRgbClient, OpenRgbClientConfig,
    OpenRgbError, Packet, PacketDecoder, PacketHeader, PacketId, RgbColor, parse_controller_data,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn client_negotiates_fetches_controller_and_streams_leds() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake OpenRGB server should bind");
    let addr = listener
        .local_addr()
        .expect("fake OpenRGB server should expose local addr");
    let server = tokio::spawn(run_client_server(listener));
    let config = OpenRgbClientConfig {
        client_name: "Hypercolor Test".to_owned(),
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_secs(2),
        write_timeout: Duration::from_secs(2),
        ..OpenRgbClientConfig::default()
    };

    let mut client = OpenRgbClient::connect(addr, config)
        .await
        .expect("client should negotiate with fake server");
    assert_eq!(client.protocol_version(), CLIENT_MAX_PROTOCOL_VERSION);
    assert_eq!(
        client
            .controller_count()
            .await
            .expect("controller count should parse"),
        1
    );
    let pending = client
        .drain_pending_packets()
        .expect("async notifications should remain queued");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].header.packet_id, PacketId::DeviceListUpdated);

    let payload = client
        .controller_data_payload(0)
        .await
        .expect("raw controller data payload should be captured");
    assert_eq!(payload, controller_payload_v5());
    let controller =
        parse_controller_data(&payload, client.protocol_version()).expect("payload should parse");
    assert_eq!(controller.name, "Board");
    assert_eq!(controller.vendor, "Acme");
    assert_eq!(controller.leds.len(), 2);

    let controller = client
        .controller_data(0)
        .await
        .expect("controller data should parse");
    assert_eq!(controller.name, "Board");
    assert_eq!(controller.vendor, "Acme");
    assert_eq!(controller.leds.len(), 2);

    let malformed_payload = client
        .controller_data_payload(0)
        .await
        .expect("malformed raw controller data payload should still be captured");
    assert_eq!(malformed_payload, malformed_controller_payload_v5());
    assert!(
        matches!(
            parse_controller_data(&malformed_payload, client.protocol_version()),
            Err(OpenRgbError::DataSizeMismatch { actual, advertised })
                if actual == malformed_payload.len() && advertised > actual
        ),
        "malformed capture should fail on the advertised payload-size guard"
    );

    client
        .update_leds(0, &[RgbColor::new(10, 20, 30), RgbColor::new(40, 50, 60)])
        .await
        .expect("LED update should be written");

    let update = server.await.expect("server task should join");
    assert_eq!(update.header.device_index, 0);
    assert_eq!(update.header.packet_id, PacketId::UpdateLeds);
    assert_eq!(update.payload.len(), 14);
    assert_eq!(&update.payload[0..4], &14_u32.to_le_bytes());
    assert_eq!(&update.payload[4..6], &2_u16.to_le_bytes());
    assert_eq!(&update.payload[6..10], &[10, 20, 30, 0]);
    assert_eq!(&update.payload[10..14], &[40, 50, 60, 0]);
}

async fn run_client_server(listener: TcpListener) -> Packet {
    let (mut stream, _) = listener
        .accept()
        .await
        .expect("fake OpenRGB server should accept client");
    let mut decoder = PacketDecoder::new();

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.packet_id, PacketId::RequestProtocolVersion);
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

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.packet_id, PacketId::SetClientName);
    assert_eq!(packet.payload, b"Hypercolor Test\0");

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.packet_id, PacketId::RequestControllerCount);
    send_packet(&mut stream, PacketId::DeviceListUpdated, 0, Vec::new()).await;
    send_packet(
        &mut stream,
        PacketId::RequestControllerCount,
        0,
        1_u32.to_le_bytes().to_vec(),
    )
    .await;

    for _ in 0..2 {
        let packet = read_next_packet(&mut stream, &mut decoder).await;
        assert_eq!(packet.header.device_index, 0);
        assert_eq!(packet.header.packet_id, PacketId::RequestControllerData);
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

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.device_index, 0);
    assert_eq!(packet.header.packet_id, PacketId::RequestControllerData);
    assert_eq!(
        packet.payload,
        CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec()
    );
    send_packet(
        &mut stream,
        PacketId::RequestControllerData,
        0,
        malformed_controller_payload_v5(),
    )
    .await;

    read_next_packet(&mut stream, &mut decoder).await
}

async fn read_next_packet(stream: &mut TcpStream, decoder: &mut PacketDecoder) -> Packet {
    loop {
        if let Some(packet) = decoder
            .next_packet()
            .expect("fake server should decode client packet")
        {
            return packet;
        }

        let mut bytes = [0_u8; 1024];
        let read = stream
            .read(&mut bytes)
            .await
            .expect("fake server should read client packet");
        assert_ne!(read, 0, "client closed connection before test completed");
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

fn malformed_controller_payload_v5() -> Vec<u8> {
    controller_payload_v5()[..8].to_vec()
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
