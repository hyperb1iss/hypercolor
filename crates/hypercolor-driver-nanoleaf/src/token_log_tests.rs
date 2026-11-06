//! The auth token travels in the URL path, so nothing that describes a
//! failed call may carry that URL. Each error is rendered every way a log
//! line could print it.

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::{NanoleafStreamSession, fetch_device_info, fetch_panel_layout};

const TOKEN: &str = "s3cr3t-auth-token";
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const UNAUTHORIZED: &[u8] =
    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const MALFORMED: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnope";

/// Answer every request with `response`.
async fn serve(response: &'static [u8]) -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("local address").port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await;
            let _ = stream.write_all(response).await;
        }
    });
    port
}

async fn closed_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind loopback");
    listener.local_addr().expect("local address").port()
}

#[track_caller]
fn assert_token_absent(error: &anyhow::Error) {
    let mut rendered = format!("{error} | {error:#} | {error:?}");
    for cause in error.chain() {
        write!(rendered, " | {cause} | {cause:?}").expect("write to a String");
    }
    assert!(!rendered.contains(TOKEN), "token leaked: {rendered}");
    assert!(
        rendered.contains("127.0.0.1"),
        "the error should still name the device: {rendered}"
    );
}

async fn external_control(port: u16) -> anyhow::Error {
    let session =
        NanoleafStreamSession::connect_with_udp_port(LOOPBACK, port, 9, TOKEN, Vec::new()).await;
    session.err().expect("the external-control request fails")
}

async fn assert_every_call_hides_the_token(port: u16) {
    let error = fetch_device_info(LOOPBACK, port, TOKEN)
        .await
        .expect_err("device info fails");
    assert_token_absent(&error);
    let error = fetch_panel_layout(LOOPBACK, port, TOKEN)
        .await
        .expect_err("panel layout fails");
    assert_token_absent(&error);
    assert_token_absent(&external_control(port).await);
}

#[tokio::test]
async fn a_refused_connection_never_prints_the_token() {
    assert_every_call_hides_the_token(closed_port().await).await;
}

#[tokio::test]
async fn an_error_status_never_prints_the_token() {
    assert_every_call_hides_the_token(serve(UNAUTHORIZED).await).await;
}

#[tokio::test]
async fn a_malformed_reply_never_prints_the_token() {
    let port = serve(MALFORMED).await;
    let error = fetch_device_info(LOOPBACK, port, TOKEN)
        .await
        .expect_err("device info does not decode");
    assert_token_absent(&error);
    let error = fetch_panel_layout(LOOPBACK, port, TOKEN)
        .await
        .expect_err("panel layout does not decode");
    assert_token_absent(&error);
}
