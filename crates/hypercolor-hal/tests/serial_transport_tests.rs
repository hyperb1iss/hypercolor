use std::time::Duration;

use hypercolor_hal::transport::Transport;
use hypercolor_hal::transport::TransportError;
use hypercolor_hal::transport::serial::UsbSerialTransport;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn send_receive_reads_until_focus_terminator() {
    let (client, mut server) = tokio::io::duplex(256);
    let transport = UsbSerialTransport::from_stream("test-focus", client);

    let server_task = tokio::spawn(async move {
        let mut command = [0_u8; 16];
        let read = server
            .read(&mut command)
            .await
            .expect("server should read command");
        assert_eq!(&command[..read], b"help\n");

        server
            .write_all(b"hardware.firmware\r\nled.mode\r\n.\r\n")
            .await
            .expect("server should write response");
    });

    let response = transport
        .send_receive(b"help\n", Duration::from_millis(200))
        .await
        .expect("transport should return response");

    assert_eq!(
        String::from_utf8(response).expect("response should be utf-8"),
        "hardware.firmware\nled.mode"
    );
    server_task.await.expect("server task should finish");
}

#[tokio::test]
async fn receive_times_out_when_terminator_never_arrives() {
    let (client, _server) = tokio::io::duplex(64);
    let transport = UsbSerialTransport::from_stream("test-timeout", client);

    let error = transport
        .receive(Duration::from_millis(25))
        .await
        .expect_err("receive should time out");

    assert!(matches!(error, TransportError::Timeout { .. }));
}
