use std::net::Ipv4Addr;
use std::time::Duration;

use hypercolor_openrgb_host::{DEFAULT_SERVER_PORT, PROBE_CLIENT_NAME, ServerProbe, probe_server};
use hypercolor_openrgb_sdk::{
    CLIENT_MAX_PROTOCOL_VERSION, Packet, PacketDecoder, PacketHeader, PacketId,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::test]
async fn probe_reports_version_and_controller_count_from_fake_server() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake server should bind");
    let addr = listener.local_addr().expect("local addr");
    let server = tokio::spawn(run_fake_server(listener, 3));

    let probe = probe_server(addr, TIMEOUT).await;
    assert_eq!(
        probe,
        ServerProbe {
            reachable: true,
            protocol_version: Some(CLIENT_MAX_PROTOCOL_VERSION),
            controller_count: Some(3),
            error: None,
        }
    );
    let client_name = server.await.expect("server task joins");
    assert_eq!(client_name, format!("{PROBE_CLIENT_NAME}\0").into_bytes());
}

#[tokio::test]
async fn probe_reports_unreachable_with_error_text() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind to reserve a port");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);

    let probe = probe_server(addr, TIMEOUT).await;
    assert!(!probe.reachable);
    assert_eq!(probe.protocol_version, None);
    assert_eq!(probe.controller_count, None);
    assert!(
        probe.error.is_some(),
        "an unreachable probe explains itself"
    );
}

#[tokio::test]
async fn probe_reports_reachable_but_incomplete_when_count_times_out() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fake server should bind");
    let addr = listener.local_addr().expect("local addr");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut decoder = PacketDecoder::new();
        let packet = read_next_packet(&mut stream, &mut decoder).await;
        assert_eq!(packet.header.packet_id, PacketId::RequestProtocolVersion);
        send_packet(
            &mut stream,
            PacketId::RequestProtocolVersion,
            CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec(),
        )
        .await;
        let _ = read_next_packet(&mut stream, &mut decoder).await;
        let _ = read_next_packet(&mut stream, &mut decoder).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    });

    let probe = probe_server(addr, Duration::from_millis(100)).await;
    assert!(probe.reachable);
    assert_eq!(probe.protocol_version, Some(CLIENT_MAX_PROTOCOL_VERSION));
    assert_eq!(probe.controller_count, None);
    assert!(
        probe
            .error
            .as_deref()
            .is_some_and(|error| error.contains("timed out")),
        "error should name the timeout: {:?}",
        probe.error
    );
    server.abort();
}

#[test]
fn default_port_matches_openrgb() {
    assert_eq!(DEFAULT_SERVER_PORT, 6742);
}

async fn run_fake_server(listener: TcpListener, controller_count: u32) -> Vec<u8> {
    let (mut stream, _) = listener.accept().await.expect("accept");
    let mut decoder = PacketDecoder::new();

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.packet_id, PacketId::RequestProtocolVersion);
    send_packet(
        &mut stream,
        PacketId::RequestProtocolVersion,
        CLIENT_MAX_PROTOCOL_VERSION.to_le_bytes().to_vec(),
    )
    .await;

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.packet_id, PacketId::SetClientName);
    let client_name = packet.payload;

    let packet = read_next_packet(&mut stream, &mut decoder).await;
    assert_eq!(packet.header.packet_id, PacketId::RequestControllerCount);
    send_packet(
        &mut stream,
        PacketId::RequestControllerCount,
        controller_count.to_le_bytes().to_vec(),
    )
    .await;
    client_name
}

async fn read_next_packet(stream: &mut TcpStream, decoder: &mut PacketDecoder) -> Packet {
    loop {
        if let Some(packet) = decoder.next_packet().expect("decode client packet") {
            return packet;
        }
        let mut bytes = [0_u8; 1024];
        let read = stream.read(&mut bytes).await.expect("read client packet");
        assert_ne!(read, 0, "client closed before the exchange finished");
        decoder.push(&bytes[..read]);
    }
}

async fn send_packet(stream: &mut TcpStream, packet_id: PacketId, payload: Vec<u8>) {
    let size = u32::try_from(payload.len()).expect("payload fits u32");
    let packet = Packet {
        header: PacketHeader {
            device_index: 0,
            packet_id,
            size,
        },
        payload,
    };
    stream
        .write_all(&packet.encode())
        .await
        .expect("write server packet");
}
