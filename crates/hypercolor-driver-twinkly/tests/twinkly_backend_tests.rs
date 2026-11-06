//! Backend connect, streaming, brightness, and session upkeep against the
//! loopback fake (Spec 92 §8 and §10.5).

mod support;

use std::time::Duration;

use hypercolor_driver_api::DeviceWriteOutcome;
use hypercolor_driver_api::{DeviceBackend, DiscoveredDevice};
use hypercolor_driver_twinkly::config::TwinklyConfig;
use hypercolor_driver_twinkly::{
    ContentionPolicy, TwinklyBackend, TwinklyEndpoint, TwinklyScanOptions, TwinklyScanner,
    discovered_device,
};
use hypercolor_types::device::{DeviceError, DeviceTopologyHint};
use hypercolor_types::spatial::LedTopology;
use support::{FakeSpec, FakeTwinkly, TestResult};

async fn discover(endpoint: TwinklyEndpoint) -> TestResult<DiscoveredDevice> {
    let profiles = TwinklyScanner::new(TwinklyScanOptions::new(
        vec![endpoint],
        Vec::new(),
        Duration::from_millis(800),
    ))
    .scan_profiles()
    .await?;
    let profile = profiles.first().ok_or("fake device was not discovered")?;
    Ok(discovered_device(profile, 60))
}

fn backend() -> TwinklyBackend {
    TwinklyBackend::new(TwinklyConfig::default())
}

fn grid_layout(count: usize) -> serde_json::Value {
    let coordinates: Vec<serde_json::Value> = (0..count)
        .map(|index| {
            serde_json::json!({
                "x": (index % 2) as f64 * 2.0 - 1.0,
                "y": (index / 2) as f64,
                "z": 1.0,
            })
        })
        .collect();
    serde_json::json!({
        "source": "2d",
        "synthesized": false,
        "uuid": "00000000-0000-0000-0000-000000000000",
        "coordinates": coordinates,
        "code": 1000,
    })
}

#[tokio::test]
async fn connect_logs_in_reads_layout_and_enters_realtime() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(4).with_layout(grid_layout(4))).await?;
    let discovered = discover(fake.endpoint()).await?;
    let device_id = discovered.info.id;
    let backend = backend();
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    let paths: Vec<(String, String)> = fake
        .requests()
        .iter()
        .map(|request| (request.method.clone(), request.path.clone()))
        .collect();
    let tail: Vec<(&str, &str)> = paths
        .iter()
        .rev()
        .take(5)
        .rev()
        .map(|(method, path)| (method.as_str(), path.as_str()))
        .collect();
    assert_eq!(
        tail,
        [
            ("POST", "/xled/v1/login"),
            ("POST", "/xled/v1/verify"),
            ("GET", "/xled/v1/led/mode"),
            ("GET", "/xled/v1/led/layout/full"),
            ("POST", "/xled/v1/led/mode"),
        ]
    );
    assert_eq!(fake.mode(), "rt");

    let info = backend
        .connected_device_info(&device_id)
        .await?
        .ok_or("connected info")?;
    assert_eq!(info.total_led_count(), 4);
    assert_eq!(info.segments[0].topology, DeviceTopologyHint::Custom);
    let Some(LedTopology::Custom { positions }) = info.segments[0]
        .layout_hint
        .as_ref()
        .and_then(|hint| hint.topology.clone())
    else {
        return Err("expected custom positions".into());
    };
    let pairs: Vec<(f32, f32)> = positions.iter().map(|p| (p.x, p.y)).collect();
    assert_eq!(pairs, [(0.0, 1.0), (1.0, 1.0), (0.0, 0.0), (1.0, 0.0)]);

    let metadata = backend
        .connected_device_metadata(&device_id)
        .await?
        .ok_or("connected metadata")?;
    assert_eq!(metadata["layout_source"], "2d");

    let cadence = backend.output_cadence(&device_id).ok_or("cadence")?;
    assert_eq!(backend.target_fps(&device_id), Some(60));
    assert_eq!(cadence.max_frame_silence(), Some(Duration::from_secs(1)));
    Ok(())
}

#[tokio::test]
async fn frames_stream_as_version_three_fragments_with_the_token() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(400)).await?;
    let discovered = discover(fake.endpoint()).await?;
    let device_id = discovered.info.id;
    let backend = backend();
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;
    let token = fake.valid_token_bytes().ok_or("verified token")?;

    let colors = vec![[255_u8, 64, 0]; 400];
    backend.write_colors(&device_id, &colors).await?;
    let datagrams = fake.wait_for_datagrams(2).await;
    assert_eq!(datagrams.len(), 2, "1200 bytes take two fragments");
    assert_eq!(datagrams[0].len(), 12 + 900);
    assert_eq!(datagrams[1].len(), 12 + 300);
    for (index, datagram) in datagrams.iter().enumerate() {
        assert_eq!(datagram[0], 3);
        assert_eq!(&datagram[1..9], &token);
        assert_eq!(&datagram[9..12], &[0, 0, u8::try_from(index)?]);
        assert!(datagram[12..].chunks(3).all(|led| led == [255, 64, 0]));
    }

    let sink = backend.frame_sink(&device_id).ok_or("frame sink")?;
    sink.write_colors_shared(std::sync::Arc::new(vec![[1, 2, 3]; 400]))
        .await?;
    let datagrams = fake.wait_for_datagrams(4).await;
    assert_eq!(&datagrams[2][12..15], &[1, 2, 3]);
    Ok(())
}

#[tokio::test]
async fn rgbw_version_two_frames_put_white_first() -> TestResult {
    let spec = FakeSpec {
        firmware: "2.4.6".to_owned(),
        led_profile: "RGBW".to_owned(),
        ..FakeSpec::gen2_rgb(2)
    };
    let fake = FakeTwinkly::spawn(spec).await?;
    let discovered = discover(fake.endpoint()).await?;
    let device_id = discovered.info.id;
    let backend = backend();
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    backend
        .write_colors(&device_id, &[[10, 20, 30], [40, 50, 60]])
        .await?;
    let datagrams = fake.wait_for_datagrams(1).await;
    assert_eq!(datagrams.len(), 1);
    assert_eq!(datagrams[0][0], 2);
    assert_eq!(datagrams[0][9], 0);
    assert_eq!(&datagrams[0][10..], &[0, 10, 20, 30, 0, 40, 50, 60]);
    Ok(())
}

#[tokio::test]
async fn brightness_scales_every_led() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(2)).await?;
    let discovered = discover(fake.endpoint()).await?;
    let device_id = discovered.info.id;
    let backend = backend();
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    backend.set_brightness(&device_id, 128).await?;
    backend
        .write_colors(&device_id, &[[255, 100, 0], [0, 0, 255]])
        .await?;
    let datagrams = fake.wait_for_datagrams(1).await;
    assert_eq!(&datagrams[0][12..], &[128, 50, 0, 0, 0, 128]);
    Ok(())
}

#[tokio::test]
async fn upkeep_logs_in_again_after_another_client_takes_the_token() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(3)).await?;
    let discovered = discover(fake.endpoint()).await?;
    let device_id = discovered.info.id;
    let backend = backend().with_upkeep_interval(Duration::from_millis(50));
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;
    let first_token = fake.valid_token_bytes().ok_or("first token")?;

    fake.revoke_token();
    fake.set_mode("movie");
    let requests = fake
        .wait_for_requests(|requests| {
            requests
                .iter()
                .filter(|request| request.path == "/xled/v1/login")
                .count()
                >= 2
                && requests.last().is_some_and(|request| {
                    request.path == "/xled/v1/led/mode" && request.method == "POST"
                })
        })
        .await;
    assert!(
        requests
            .iter()
            .any(|request| request.method == "GET" && request.path == "/xled/v1/led/mode"),
        "upkeep polls the mode with its token"
    );
    assert_eq!(fake.mode(), "rt");
    let second_token = fake.valid_token_bytes().ok_or("second token")?;
    assert_ne!(first_token, second_token);

    backend.write_colors(&device_id, &[[9, 9, 9]; 3]).await?;
    let datagrams = fake.wait_for_datagrams(1).await;
    assert_eq!(&datagrams.last().ok_or("datagram")?[1..9], &second_token);
    Ok(())
}

#[tokio::test]
async fn upkeep_restores_realtime_mode() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(3)).await?;
    let discovered = discover(fake.endpoint()).await?;
    let device_id = discovered.info.id;
    let backend = backend().with_upkeep_interval(Duration::from_millis(50));
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    fake.set_mode("movie");
    for _ in 0..100 {
        if fake.mode() == "rt" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fake.mode(), "rt");
    let logins = fake
        .requests()
        .iter()
        .filter(|request| request.path == "/xled/v1/login")
        .count();
    assert_eq!(logins, 1, "a valid token is reused, not replaced");
    Ok(())
}

#[tokio::test]
async fn disconnect_stops_streaming_and_upkeep() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(3)).await?;
    let discovered = discover(fake.endpoint()).await?;
    let device_id = discovered.info.id;
    let backend = backend().with_upkeep_interval(Duration::from_millis(30));
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;
    let sink = backend.frame_sink(&device_id).ok_or("frame sink")?;

    backend.disconnect(&device_id).await?;
    assert!(sink.is_closed());
    assert!(matches!(
        backend.write_colors(&device_id, &[[0, 0, 0]; 3]).await,
        Err(DeviceError::Disconnected { .. })
    ));
    assert!(matches!(
        sink.write_colors_shared(std::sync::Arc::new(vec![[0, 0, 0]; 3]))
            .await,
        Err(DeviceError::Disconnected { .. })
    ));
    let settled = fake.requests().len();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(fake.requests().len(), settled, "no upkeep after disconnect");
    Ok(())
}

#[tokio::test]
async fn connect_refuses_a_different_device_at_the_address() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(3)).await?;
    let mut discovered = discover(fake.endpoint()).await?;
    let other = FakeTwinkly::spawn(FakeSpec {
        mac: "98:f4:ab:00:00:01".to_owned(),
        ..FakeSpec::gen2_rgb(3)
    })
    .await?;
    let other_endpoint = other.endpoint();
    discovered
        .metadata
        .insert("http_port".to_owned(), other_endpoint.http_port.to_string());
    let device_id = discovered.info.id;
    let backend = backend();
    backend.adopt_device(&discovered)?;
    let error = backend
        .connect(&device_id)
        .await
        .expect_err("a different MAC must not be adopted");
    assert!(matches!(error, DeviceError::ConnectionFailed { .. }));
    assert!(
        other
            .requests()
            .iter()
            .all(|request| request.path != "/xled/v1/login"),
        "no login on the wrong device"
    );
    Ok(())
}

#[tokio::test]
async fn missing_layout_falls_back_to_wiring_order() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(5)).await?;
    let discovered = discover(fake.endpoint()).await?;
    let device_id = discovered.info.id;
    let backend = backend();
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;
    let info = backend
        .connected_device_info(&device_id)
        .await?
        .ok_or("connected info")?;
    assert_eq!(info.segments[0].topology, DeviceTopologyHint::Strip);
    let metadata = backend
        .connected_device_metadata(&device_id)
        .await?
        .ok_or("connected metadata")?;
    assert_eq!(metadata["layout_source"], "unavailable");
    Ok(())
}

#[tokio::test]
async fn unadopted_devices_cannot_connect() {
    let discovered_id =
        hypercolor_driver_twinkly::scanner::fingerprint_for_mac("00:11:22:33:44:55")
            .stable_device_id();
    assert!(matches!(
        backend().connect(&discovered_id).await,
        Err(DeviceError::NotAdopted { .. })
    ));
}

fn logins(fake: &FakeTwinkly) -> usize {
    fake.requests()
        .iter()
        .filter(|request| request.path == "/xled/v1/login")
        .count()
}

async fn control_state(
    backend: &TwinklyBackend,
    id: &hypercolor_types::device::DeviceId,
) -> String {
    backend
        .connected_device_metadata(id)
        .await
        .ok()
        .flatten()
        .and_then(|metadata| metadata.get("control_state").cloned())
        .unwrap_or_default()
}

#[tokio::test]
async fn repeated_takeovers_pause_instead_of_fighting_then_resume() -> TestResult {
    let fake = std::sync::Arc::new(FakeTwinkly::spawn(FakeSpec::gen2_rgb(3)).await?);
    let discovered = discover(fake.endpoint()).await?;
    let device_id = discovered.info.id;
    let backend = backend()
        .with_upkeep_interval(Duration::from_millis(20))
        .with_contention_policy(ContentionPolicy {
            max_takeovers: 2,
            window: Duration::from_secs(10),
            initial_backoff: Duration::from_millis(400),
            max_backoff: Duration::from_secs(2),
        });
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;
    assert_eq!(control_state(&backend, &device_id).await, "streaming");

    // A rival client keeps logging in, which signs Hypercolor out.
    let rival_fake = std::sync::Arc::clone(&fake);
    let rival = tokio::spawn(async move {
        loop {
            rival_fake.revoke_token();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let mut yielded = false;
    for _ in 0..200 {
        if control_state(&backend, &device_id).await == "yielded_to_another_app" {
            yielded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    rival.abort();
    assert!(yielded, "the session steps back after repeated takeovers");
    let logins_while_paused = logins(&fake);
    assert_eq!(
        logins_while_paused, 2,
        "one re-login, then no more fighting"
    );
    assert_eq!(
        backend
            .write_colors_shared_outcome(&device_id, std::sync::Arc::new(vec![[1, 2, 3]; 3]))
            .await?,
        DeviceWriteOutcome::Declined
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        logins(&fake),
        logins_while_paused,
        "no logins during the pause"
    );

    // The rival went quiet; after the pause the session takes the lights back.
    let mut resumed = false;
    for _ in 0..200 {
        if control_state(&backend, &device_id).await == "streaming" {
            resumed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(resumed, "the session resumes after a quiet period");
    assert_eq!(logins(&fake), logins_while_paused + 1);
    assert_eq!(fake.mode(), "rt");
    assert_eq!(
        backend
            .write_colors_shared_outcome(&device_id, std::sync::Arc::new(vec![[1, 2, 3]; 3]))
            .await?,
        DeviceWriteOutcome::Sent
    );
    Ok(())
}

#[tokio::test]
async fn disconnect_restores_the_previous_mode() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(3)).await?;
    fake.set_mode("color");
    let discovered = discover(fake.endpoint()).await?;
    let backend = backend();
    backend.adopt_device(&discovered)?;
    backend.connect(&discovered.info.id).await?;
    assert_eq!(fake.mode(), "rt");
    backend.disconnect(&discovered.info.id).await?;
    assert_eq!(fake.mode(), "color");
    Ok(())
}

#[tokio::test]
async fn disconnect_leaves_a_device_that_was_already_realtime() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(3)).await?;
    fake.set_mode("rt");
    let discovered = discover(fake.endpoint()).await?;
    let backend = backend();
    backend.adopt_device(&discovered)?;
    backend.connect(&discovered.info.id).await?;
    let before = fake.requests().len();
    backend.disconnect(&discovered.info.id).await?;
    assert_eq!(fake.requests().len(), before, "nothing to restore");
    Ok(())
}
