use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use hypercolor_driver_nanoleaf::{NanoleafStreamSession, encode_frame_into};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::timeout;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn encode_frame_into_serializes_panels_and_pads_missing_colors() -> TestResult {
    let mut packet = Vec::new();
    encode_frame_into(&mut packet, &[41, 42], &[[1, 2, 3]], 5)?;

    assert_eq!(
        packet,
        vec![
            0x00, 0x02, // panel count
            0x00, 0x29, 0x01, 0x02, 0x03, 0x00, 0x00, 0x05, // panel 41
            0x00, 0x2A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, // panel 42 padded black
        ]
    );

    Ok(())
}

#[tokio::test]
async fn stream_session_enables_external_control_and_sends_udp_frame() -> TestResult {
    let api_listener = TcpListener::bind("127.0.0.1:0").await?;
    let api_port = api_listener.local_addr()?.port();
    let api_task = tokio::spawn(async move {
        let (mut stream, _) = api_listener.accept().await.expect("accept API request");
        let request = read_http_request(&mut stream)
            .await
            .expect("read HTTP request");
        assert!(
            request.starts_with("PUT /api/v1/test-token/effects HTTP/1.1"),
            "unexpected request line: {request}"
        );
        assert!(
            request.contains("extControl"),
            "request body should enable external control: {request}"
        );
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .expect("write HTTP response");
    });

    let receiver = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let receiver_port = receiver.local_addr()?.port();

    let mut session = NanoleafStreamSession::connect_with_udp_port(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        api_port,
        receiver_port,
        "test-token",
        vec![41, 42],
    )
    .await?;

    session.send_frame(&[[10, 20, 30], [40, 50, 60]], 1).await?;

    let mut buf = [0_u8; 64];
    let (len, _) = timeout(Duration::from_millis(500), receiver.recv_from(&mut buf)).await??;
    assert_eq!(
        &buf[..len],
        &[
            0x00, 0x02, 0x00, 0x29, 10, 20, 30, 0, 0, 1, 0x00, 0x2A, 40, 50, 60, 0, 0, 1,
        ]
    );

    api_task.await?;
    Ok(())
}

async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = vec![0_u8; 4096];
    let mut total = 0_usize;

    loop {
        let read = stream.read(&mut buf[total..]).await?;
        if read == 0 {
            break;
        }
        total += read;

        if total >= 4 && buf[..total].windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if total == buf.len() {
            buf.resize(buf.len() * 2, 0);
        }
    }

    Ok(String::from_utf8_lossy(&buf[..total]).into_owned())
}
