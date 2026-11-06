//! Bounds on untrusted LAN input: reply sizes, LED counts, redirects, and
//! discovery fanout (Spec 92 §3.3 and §4.5).

mod support;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use hypercolor_driver_api::CoverageGap;
use hypercolor_driver_support::http::HttpError;
use hypercolor_driver_twinkly::api::{
    ApiError, AuthToken, Gestalt, MAX_REPLY_BYTES, TwinklyClient, decode_layout_reply, decode_reply,
};
use hypercolor_driver_twinkly::protocol::{
    LedProfile, MAX_LED_COUNT, ProtocolError, RtFrameEncoder, RtProtocol, validate_led_count,
};
use hypercolor_driver_twinkly::scanner::{
    MAX_CONCURRENT_PROBES, TwinklyDeviceProfile, collect_discovery_replies,
};
use hypercolor_driver_twinkly::{TwinklyEndpoint, TwinklyScanOptions, TwinklyScanner};
use support::{FakeSpec, FakeTwinkly, TestResult};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

fn endpoint(port: u16) -> TwinklyEndpoint {
    TwinklyEndpoint {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        http_port: port,
        rt_port: 7777,
    }
}

/// Serve one canned HTTP response to every connection.
async fn serve_forever(response: Vec<u8>) -> TestResult<(u16, Arc<AtomicUsize>)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);
    let response = Arc::new(response);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            let response = Arc::clone(&response);
            tokio::spawn(async move {
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request).await;
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    Ok((port, connections))
}

#[tokio::test]
async fn oversized_declared_bodies_are_refused() -> TestResult {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        MAX_REPLY_BYTES + 1
    )
    .into_bytes();
    let (port, _) = serve_forever(response).await?;
    let client = TwinklyClient::new(endpoint(port), Duration::from_secs(2))?;
    assert!(matches!(
        client.gestalt().await,
        Err(ApiError::TooLarge { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn oversized_streamed_bodies_are_refused_while_reading() -> TestResult {
    // No Content-Length: the body runs until the connection closes.
    let mut response =
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n".to_vec();
    response.extend(std::iter::repeat_n(b' ', MAX_REPLY_BYTES + 4096));
    let (port, _) = serve_forever(response).await?;
    let client = TwinklyClient::new(endpoint(port), Duration::from_secs(2))?;
    assert!(matches!(
        client.gestalt().await,
        Err(ApiError::TooLarge { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_slow_drip_fails_at_the_request_deadline() -> TestResult {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request).await;
                let head = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(head).await;
                // Always making progress, never finishing.
                while stream.write_all(b" ").await.is_ok() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            });
        }
    });
    let client = TwinklyClient::new(endpoint(port), Duration::from_millis(300))?;
    let started = Instant::now();
    let error = client.gestalt().await.expect_err("dripped reply");
    assert!(
        matches!(
            error,
            ApiError::Transport {
                source: HttpError::DeadlineExceeded,
                ..
            }
        ),
        "{error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the deadline is absolute: {:?}",
        started.elapsed()
    );
    Ok(())
}

#[tokio::test]
async fn redirects_are_not_followed_with_the_token() -> TestResult {
    let (target_port, target_connections) =
        serve_forever(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec()).await?;
    let redirect = format!(
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/xled/v1/led/mode\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes();
    let (port, _) = serve_forever(redirect).await?;
    let client = TwinklyClient::new(endpoint(port), Duration::from_secs(2))?;
    let token = AuthToken::decode("AQIDBAUGBwg=", None)?;
    assert!(matches!(
        client.mode(&token).await,
        Err(ApiError::Status { status: 302, .. })
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        target_connections.load(Ordering::SeqCst),
        0,
        "the token never reaches the redirect target"
    );
    Ok(())
}

#[test]
fn layouts_with_more_coordinates_than_any_device_are_refused() {
    let coordinates: Vec<serde_json::Value> = (0..=MAX_LED_COUNT)
        .map(|_| serde_json::json!({ "x": 0, "y": 0 }))
        .collect();
    let body = serde_json::json!({ "source": "2d", "coordinates": coordinates, "code": 1000 });
    let error = decode_layout_reply("layout", body.to_string().as_bytes())
        .expect_err("oversized layout is refused");
    assert!(matches!(error, ApiError::Malformed { .. }));

    let small =
        serde_json::json!({ "source": "2d", "coordinates": [{ "x": 0, "y": 1 }], "code": 1000 });
    assert!(decode_layout_reply("layout", small.to_string().as_bytes()).is_ok());
}

fn gestalt(led_count: u32, family: Option<&str>) -> Gestalt {
    let mut value = serde_json::json!({
        "mac": "98:f4:ab:1c:c1:90",
        "number_of_led": led_count,
        "led_profile": "RGB",
        "code": 1000,
    });
    if let Some(family) = family {
        value["fw_family"] = serde_json::json!(family);
    }
    decode_reply("gestalt", value.to_string().as_bytes()).expect("gestalt decodes")
}

#[test]
fn impossible_led_counts_never_become_profiles() {
    let profile = |count: u32, family: Option<&str>, firmware: &str| {
        TwinklyDeviceProfile::from_gestalt(endpoint(80), &gestalt(count, family), firmware)
    };
    assert!(
        profile(u32::MAX, Some("F"), "2.4.6").is_err(),
        "V2 with u32::MAX LEDs"
    );
    assert!(
        profile(u32::MAX, Some("G"), "2.8.3").is_err(),
        "V3 with u32::MAX LEDs"
    );
    assert!(profile(0, Some("G"), "2.8.3").is_err(), "no LEDs");
    assert!(profile(300, Some("D"), "2.3.8").is_err(), "V1 above 255");
    assert!(profile(250, Some("F"), "2.4.6").is_ok());
    assert!(profile(1200, Some("G"), "2.8.3").is_ok());
}

#[test]
fn encoders_refuse_counts_before_allocating() {
    assert_eq!(
        RtFrameEncoder::new(RtProtocol::V2, LedProfile::Rgb, 4_294_967_295, TOKEN).err(),
        Some(ProtocolError::LedCount {
            leds: 4_294_967_295
        })
    );
    assert!(validate_led_count(RtProtocol::V2, LedProfile::Rgbw, MAX_LED_COUNT).is_ok());
    assert!(validate_led_count(RtProtocol::V3, LedProfile::Rgb, MAX_LED_COUNT + 1).is_err());
    assert!(validate_led_count(RtProtocol::V1, LedProfile::Rgb, 0).is_err());
}

#[tokio::test]
async fn scans_skip_devices_reporting_impossible_counts() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec {
        firmware: "2.4.6".to_owned(),
        ..FakeSpec::gen2_rgb(u32::MAX)
    })
    .await?;
    let profiles = TwinklyScanner::new(TwinklyScanOptions::new(
        vec![fake.endpoint()],
        Vec::new(),
        Duration::from_millis(800),
    ))
    .scan()
    .await?
    .profiles;
    assert!(profiles.is_empty());
    Ok(())
}

#[tokio::test]
async fn discovery_replies_from_one_address_are_admitted_once() -> TestResult {
    let first = FakeTwinkly::spawn(FakeSpec::gen2_rgb(10)).await?;
    let second = FakeTwinkly::spawn(FakeSpec::gen2_rgb(10)).await?;
    let found = collect_discovery_replies(
        &[first.discovery_addr(), second.discovery_addr()],
        Duration::from_millis(300),
        8,
    )
    .await?
    .found;
    assert_eq!(found, [IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    Ok(())
}

/// Linux routes all of 127.0.0.0/8 to the loopback interface, so distinct
/// source addresses can be tested there without leaving the host.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn discovery_stops_admitting_at_its_limit() -> TestResult {
    let mut targets = Vec::new();
    let mut responders = Vec::new();
    for host in 2..=5_u8 {
        let socket = tokio::net::UdpSocket::bind((Ipv4Addr::new(127, 0, 0, host), 0)).await?;
        targets.push(socket.local_addr()?);
        responders.push(tokio::spawn(async move {
            let mut buffer = [0_u8; 64];
            while let Ok((_, source)) = socket.recv_from(&mut buffer).await {
                let _ = socket
                    .send_to(&[host, 0, 0, 127, b'O', b'K', b'x', 0], source)
                    .await;
            }
        }));
    }
    let replies = collect_discovery_replies(&targets, Duration::from_millis(400), 2).await?;
    for responder in responders {
        responder.abort();
    }
    assert_eq!(
        replies.found.len(),
        2,
        "admission stops at the limit: {replies:?}"
    );
    assert_eq!(
        replies.gaps,
        vec![CoverageGap::AdmissionFull { refused: 2 }],
        "the devices it refused keep the scan partial"
    );
    Ok(())
}

#[tokio::test]
async fn probes_run_with_bounded_concurrency_inside_the_scan_budget() -> TestResult {
    // Listeners that accept and never answer, counting open connections.
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut endpoints = Vec::new();
    for _ in 0..(MAX_CONCURRENT_PROBES * 3) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        endpoints.push(endpoint(listener.local_addr()?.port()));
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                let active = Arc::clone(&active);
                tokio::spawn(async move {
                    let mut buffer = [0_u8; 1024];
                    while matches!(stream.read(&mut buffer).await, Ok(read) if read > 0) {}
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
    }
    let started = Instant::now();
    let profiles = TwinklyScanner::new(TwinklyScanOptions::new(
        endpoints,
        Vec::new(),
        Duration::from_millis(600),
    ))
    .scan()
    .await?
    .profiles;
    assert!(profiles.is_empty());
    assert!(
        peak.load(Ordering::SeqCst) <= MAX_CONCURRENT_PROBES,
        "at most {MAX_CONCURRENT_PROBES} probes at once, saw {}",
        peak.load(Ordering::SeqCst)
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the scan stays inside its budget: {:?}",
        started.elapsed()
    );
    Ok(())
}
