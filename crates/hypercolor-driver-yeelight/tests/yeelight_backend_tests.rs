//! Search, connect, music mode, the normal-mode quota guard, and the frame
//! sink against the loopback fake (Spec 94 §3, §5, §6, and §8).

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_driver_api::{DeviceBackend, DeviceWriteOutcome, DiscoveredDevice};
use hypercolor_driver_yeelight::backend::{NORMAL_MODE_REPLAY, representative_color};
use hypercolor_driver_yeelight::config::YeelightConfig;
use hypercolor_driver_yeelight::scanner::{fingerprint_for_id, route_from_metadata, search};
use hypercolor_driver_yeelight::{
    YeelightBackend, YeelightScanOptions, YeelightScanner, discovered_device,
};
use hypercolor_types::device::{DeviceError, DeviceTopologyHint};
use support::{ControlPush, FakeSpec, FakeYeelight, TestResult};

async fn discover(fake: &FakeYeelight, config: &YeelightConfig) -> TestResult<DiscoveredDevice> {
    let profiles = YeelightScanner::new(YeelightScanOptions {
        unicast_targets: vec![fake.search_addr()],
        multicast: false,
        timeout: Duration::from_millis(1_000),
    })
    .scan()
    .await?
    .found;
    let profile = profiles.first().ok_or("fake device was not found")?;
    let max_fps = if config.music_mode && profile.capabilities().music {
        config.max_fps
    } else {
        1
    };
    Ok(discovered_device(profile, max_fps, config.music_mode))
}

async fn connected(
    fake: &FakeYeelight,
    config: YeelightConfig,
) -> TestResult<(YeelightBackend, DiscoveredDevice)> {
    let discovered = discover(fake, &config).await?;
    let backend = YeelightBackend::new(config);
    backend.adopt_device(&discovered)?;
    backend.connect(&discovered.info.id).await?;
    Ok((backend, discovered))
}

#[tokio::test]
async fn search_reads_identity_without_opening_a_connection() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        name: "Desk Lamp".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let discovered = discover(&fake, &YeelightConfig::default()).await?;
    assert!(!fake.searches().is_empty());
    assert_eq!(
        fake.control_connections(),
        0,
        "scans never use a control slot"
    );

    assert_eq!(
        discovered.fingerprint,
        fingerprint_for_id("0x000000000015243f")
    );
    assert_eq!(discovered.info.name, "Desk Lamp");
    assert_eq!(discovered.info.vendor, "Yeelight");
    assert_eq!(discovered.info.model.as_deref(), Some("color"));
    assert_eq!(discovered.info.total_led_count(), 1);
    assert_eq!(
        discovered.info.segments[0].topology,
        DeviceTopologyHint::Point
    );
    assert_eq!(discovered.info.capabilities.max_fps, 25);
    assert!(discovered.claim.is_some());
    assert_eq!(discovered.metadata["light_class"], "color");
    assert_eq!(discovered.metadata["music_mode"], "on");
    let route = route_from_metadata(&discovered.metadata)?;
    assert_eq!(route.control, fake.control_addr());
    assert_eq!(route.search, fake.search_addr());
    Ok(())
}

#[tokio::test]
async fn devices_without_a_drivable_light_are_skipped() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        support: "get_prop set_power toggle".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let profiles = YeelightScanner::new(YeelightScanOptions {
        unicast_targets: vec![fake.search_addr()],
        multicast: false,
        timeout: Duration::from_millis(1_000),
    })
    .scan()
    .await?
    .found;
    assert!(profiles.is_empty());
    Ok(())
}

#[tokio::test]
async fn music_mode_streams_scenes_without_a_quota() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec::color_bulb()).await?;
    let (backend, discovered) = connected(&fake, YeelightConfig::default()).await?;
    let id = discovered.info.id;

    let control = fake.wait_for_control(1).await;
    assert_eq!(control[0]["method"], "set_music");
    assert_eq!(control[0]["params"][0], 1);
    assert_eq!(control[0]["params"][1], "127.0.0.1");

    let metadata = backend
        .connected_device_metadata(&id)
        .await?
        .ok_or("metadata")?;
    assert_eq!(metadata["stream_mode"], "music");
    let cadence = backend.output_policy(&id).ok_or("cadence")?;
    assert_eq!(
        backend
            .output_policy(&id)
            .map(hypercolor_driver_api::OutputPolicy::display_fps),
        Some(25)
    );
    assert_eq!(cadence.min_interval(), Some(Duration::from_millis(40)));

    for color in [[128, 64, 0], [0, 0, 255], [0, 0, 0], [255, 0, 0]] {
        let outcome = backend
            .write_colors_shared_outcome(&id, Arc::new(vec![color]))
            .await?;
        assert_eq!(outcome, DeviceWriteOutcome::Sent);
    }
    let music = fake.wait_for_music(4).await;
    let calls: Vec<(String, serde_json::Value)> = music
        .iter()
        .map(|line| {
            (
                line["method"].as_str().unwrap_or_default().to_owned(),
                line["params"].clone(),
            )
        })
        .collect();
    assert_eq!(
        calls,
        [
            (
                "set_scene".to_owned(),
                serde_json::json!(["color", 0x00ff_8000, 50])
            ),
            (
                "set_scene".to_owned(),
                serde_json::json!(["color", 0x0000_00ff, 100])
            ),
            (
                "set_power".to_owned(),
                serde_json::json!(["off", "sudden", 30])
            ),
            (
                "set_scene".to_owned(),
                serde_json::json!(["color", 0x00ff_0000, 100])
            ),
        ]
    );
    assert_eq!(
        fake.control_lines().len(),
        1,
        "frames never touch the control connection"
    );

    let duplicate = backend
        .write_colors_shared_outcome(&id, Arc::new(vec![[255, 0, 0]]))
        .await?;
    assert_eq!(duplicate, DeviceWriteOutcome::SuppressedDuplicate);
    Ok(())
}

#[tokio::test]
async fn normal_mode_paces_commands_inside_the_quota() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec::color_bulb()).await?;
    let config = YeelightConfig {
        music_mode: false,
        ..YeelightConfig::default()
    };
    let (backend, discovered) = connected(&fake, config).await?;
    let id = discovered.info.id;
    assert_eq!(discovered.info.capabilities.max_fps, 1);
    let cadence = backend.output_policy(&id).ok_or("cadence")?;
    assert_eq!(cadence.min_interval(), Some(Duration::from_secs(1)));
    assert_eq!(cadence.replay_after(), Some(Duration::from_secs(3)));

    let first = backend
        .write_colors_shared_outcome(&id, Arc::new(vec![[0, 255, 0]]))
        .await?;
    let early = backend
        .write_colors_shared_outcome(&id, Arc::new(vec![[0, 0, 255]]))
        .await?;
    assert_eq!(first, DeviceWriteOutcome::Sent);
    assert_eq!(early, DeviceWriteOutcome::SuppressedCadence);
    let control = fake.wait_for_control(1).await;
    assert_eq!(control.len(), 1);
    assert_eq!(control[0]["method"], "set_scene");
    assert_eq!(
        control[0]["params"],
        serde_json::json!(["color", 0x0000_ff00, 100])
    );
    let metadata = backend
        .connected_device_metadata(&id)
        .await?
        .ok_or("metadata")?;
    assert_eq!(metadata["stream_mode"], "normal");
    assert_eq!(metadata["music_mode"], "off");
    Ok(())
}

#[tokio::test]
async fn quota_errors_pause_normal_mode_output() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        quota_rejections: usize::MAX,
        ..FakeSpec::color_bulb()
    })
    .await?;
    let config = YeelightConfig {
        music_mode: false,
        ..YeelightConfig::default()
    };
    let (backend, discovered) = connected(&fake, config).await?;
    let id = discovered.info.id;
    let first = backend
        .write_colors_shared_outcome(&id, Arc::new(vec![[0, 255, 0]]))
        .await?;
    assert_eq!(first, DeviceWriteOutcome::Sent);
    // Wait out the one-second command interval; the quota error has
    // arrived by then and blocks the next frame.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let blocked = backend
        .write_colors_shared_outcome(&id, Arc::new(vec![[0, 0, 255]]))
        .await?;
    assert_eq!(blocked, DeviceWriteOutcome::SuppressedCadence);
    assert_eq!(fake.control_lines().len(), 1);
    Ok(())
}

#[tokio::test]
async fn music_mode_falls_back_when_the_device_never_connects() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        music_never_connects: true,
        ..FakeSpec::color_bulb()
    })
    .await?;
    let (backend, discovered) = connected(&fake, YeelightConfig::default()).await?;
    let id = discovered.info.id;
    let metadata = backend
        .connected_device_metadata(&id)
        .await?
        .ok_or("metadata")?;
    assert_eq!(metadata["stream_mode"], "normal");
    assert_eq!(metadata["music_mode"], "failed");
    assert_eq!(
        backend
            .output_policy(&id)
            .and_then(|policy| policy.replay_after()),
        Some(NORMAL_MODE_REPLAY)
    );
    backend.write_colors(&id, &[[255, 0, 0]]).await?;
    let control = fake.wait_for_control(2).await;
    assert_eq!(control[1]["method"], "set_scene");
    Ok(())
}

#[tokio::test]
async fn white_lights_power_on_then_dim() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        model: "mono".to_owned(),
        support: "get_prop set_default set_power toggle set_bright start_cf stop_cf cron_add cron_get cron_del set_music".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let (backend, discovered) = connected(&fake, YeelightConfig::default()).await?;
    backend.set_brightness(&discovered.info.id, 128).await?;
    backend
        .write_colors(&discovered.info.id, &[[255, 255, 255]])
        .await?;
    let music = fake.wait_for_music(2).await;
    assert_eq!(music[0]["method"], "set_power");
    assert_eq!(music[0]["params"], serde_json::json!(["on", "sudden", 30]));
    assert_eq!(music[1]["method"], "set_bright");
    assert_eq!(music[1]["params"], serde_json::json!([50, "sudden", 30]));
    Ok(())
}

#[tokio::test]
async fn disconnect_stops_music_mode_and_closes_the_sink() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec::color_bulb()).await?;
    let (backend, discovered) = connected(&fake, YeelightConfig::default()).await?;
    let id = discovered.info.id;
    let sink = backend.frame_sink(&id).ok_or("frame sink")?;
    backend.disconnect(&id).await?;
    assert!(sink.is_closed());
    // The retained sink does not keep the connections open.
    for _ in 0..200 {
        if fake.closed_connections() == (1, 1) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fake.closed_connections(), (1, 1));
    assert!(matches!(
        sink.write_colors_shared(Arc::new(vec![[1, 2, 3]])).await,
        Err(DeviceError::Disconnected { .. })
    ));
    let control = fake.wait_for_control(2).await;
    assert_eq!(control[1]["method"], "set_music");
    assert_eq!(control[1]["params"], serde_json::json!([0]));
    Ok(())
}

#[tokio::test]
async fn connect_refuses_a_different_device_at_the_address() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec::color_bulb()).await?;
    let mut discovered = discover(&fake, &YeelightConfig::default()).await?;
    let other = FakeYeelight::spawn(FakeSpec {
        id: "0x00000000000000aa".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    discovered.metadata.insert(
        "search_port".to_owned(),
        other.search_addr().port().to_string(),
    );
    let backend = YeelightBackend::new(YeelightConfig::default());
    backend.adopt_device(&discovered)?;
    assert!(matches!(
        backend.connect(&discovered.info.id).await,
        Err(DeviceError::ConnectionFailed { .. })
    ));
    assert_eq!(other.control_connections(), 0);
    Ok(())
}

#[test]
fn multi_led_frames_average_to_one_color() {
    assert_eq!(representative_color(&[]), [0, 0, 0]);
    assert_eq!(
        representative_color(&[[200, 0, 0], [0, 0, 100]]),
        [100, 0, 50]
    );
}

#[tokio::test]
async fn a_quota_rejected_frame_is_resent_after_the_pause() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        quota_rejections: 1,
        ..FakeSpec::color_bulb()
    })
    .await?;
    let config = YeelightConfig {
        music_mode: false,
        ..YeelightConfig::default()
    };
    let discovered = discover(&fake, &config).await?;
    let backend = YeelightBackend::new(config).with_quota_backoff(Duration::from_millis(200));
    backend.adopt_device(&discovered)?;
    backend.connect(&discovered.info.id).await?;
    let id = discovered.info.id;

    let green = Arc::new(vec![[0, 255, 0]]);
    assert_eq!(
        backend
            .write_colors_shared_outcome(&id, Arc::clone(&green))
            .await?,
        DeviceWriteOutcome::Sent
    );
    // Past the one-second command interval and the 200 ms pause, the host's
    // replay of the same static color must reach the light again.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    assert_eq!(
        backend
            .write_colors_shared_outcome(&id, Arc::clone(&green))
            .await?,
        DeviceWriteOutcome::Sent
    );
    let control = fake.wait_for_control(2).await;
    assert_eq!(control.len(), 2);
    assert_eq!(control[0]["params"], control[1]["params"]);
    Ok(())
}

#[tokio::test]
async fn a_closed_control_connection_fails_the_next_write() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        close_after: Some(1),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let config = YeelightConfig {
        music_mode: false,
        ..YeelightConfig::default()
    };
    let (backend, discovered) = connected(&fake, config).await?;
    let id = discovered.info.id;
    backend.write_colors(&id, &[[0, 255, 0]]).await?;
    for _ in 0..200 {
        if fake.closed_connections().0 == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    // The same color would be a duplicate; the dead connection must still
    // surface so the host reconnects.
    assert!(matches!(
        backend.write_colors(&id, &[[0, 255, 0]]).await,
        Err(DeviceError::WriteError { .. })
    ));
    Ok(())
}

/// Connect to a fake that pushes `push`, and return the write error the
/// session reports once its reader gives up, with the time that took.
async fn write_failure_under(push: ControlPush) -> TestResult<(String, Duration)> {
    let fake = FakeYeelight::spawn(FakeSpec {
        control_push: Some(push),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let config = YeelightConfig {
        music_mode: false,
        ..YeelightConfig::default()
    };
    let (backend, discovered) = connected(&fake, config).await?;
    let started = Instant::now();
    for _ in 0..400 {
        let written = backend
            .write_colors(&discovered.info.id, &[[0, 255, 0]])
            .await;
        if let Err(error @ DeviceError::WriteError { .. }) = written {
            return Ok((error.to_string(), started.elapsed()));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("the session never failed".into())
}

#[tokio::test]
async fn an_endless_control_line_fails_the_session_at_the_line_cap() -> TestResult {
    let (message, elapsed) = write_failure_under(ControlPush::EndlessLine).await?;
    assert!(message.contains("longer than 4096"), "{message}");
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    Ok(())
}

#[tokio::test]
async fn a_dripped_control_line_fails_the_session_at_the_line_deadline() -> TestResult {
    let (message, elapsed) = write_failure_under(ControlPush::Drip).await?;
    assert!(message.contains("did not finish"), "{message}");
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    Ok(())
}

/// Linux routes all of 127.0.0.0/8 to the loopback interface, so a rogue
/// host with its own address can race the light there.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn music_mode_accepts_only_the_light_itself() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        rogue_from: Some(std::net::Ipv4Addr::new(127, 0, 0, 2)),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let (backend, discovered) = connected(&fake, YeelightConfig::default()).await?;
    let id = discovered.info.id;
    let metadata = backend
        .connected_device_metadata(&id)
        .await?
        .ok_or("metadata")?;
    assert_eq!(metadata["stream_mode"], "music");
    backend.write_colors(&id, &[[255, 0, 0]]).await?;
    let music = fake.wait_for_music(1).await;
    assert_eq!(music.len(), 1, "the frame reached the light");
    assert_eq!(fake.rogue_lines(), 0, "nothing reached the rogue host");
    Ok(())
}

#[tokio::test]
async fn replies_naming_another_address_are_ignored() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        location_ip: Some(std::net::Ipv4Addr::new(192, 0, 2, 7)),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let profiles = YeelightScanner::new(YeelightScanOptions {
        unicast_targets: vec![fake.search_addr()],
        multicast: false,
        timeout: Duration::from_millis(1_000),
    })
    .scan()
    .await?
    .found;
    assert!(profiles.is_empty());
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "Windows accepts a UDP send to port 0, so it cannot fail locally"
)]
async fn a_search_that_got_no_request_out_is_an_error() {
    // Sending to port 0 is rejected locally, so this never leaves the host.
    let rejected = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
    let error = search(&[rejected], Duration::from_millis(100))
        .await
        .expect_err("a search that asked nobody could not start");
    assert!(error.to_string().contains("left this host"), "{error}");
}

#[tokio::test]
async fn one_address_yields_one_light_per_scan() -> TestResult {
    let first = FakeYeelight::spawn(FakeSpec::color_bulb()).await?;
    let second = FakeYeelight::spawn(FakeSpec {
        id: "0x00000000000000bb".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let profiles = YeelightScanner::new(YeelightScanOptions {
        unicast_targets: vec![first.search_addr(), second.search_addr()],
        multicast: false,
        timeout: Duration::from_millis(1_000),
    })
    .scan()
    .await?
    .found;
    assert_eq!(profiles.len(), 1, "two ids from one host count once");
    Ok(())
}

#[tokio::test]
async fn invalid_ids_never_become_devices() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        id: "€abcd".to_owned(),
        support: "set_bright".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let profiles = YeelightScanner::new(YeelightScanOptions {
        unicast_targets: vec![fake.search_addr()],
        multicast: false,
        timeout: Duration::from_millis(1_000),
    })
    .scan()
    .await?
    .found;
    assert!(profiles.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_control_failure_during_music_setup_fails_the_connect() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec {
        close_on_music: true,
        ..FakeSpec::color_bulb()
    })
    .await?;
    let discovered = discover(&fake, &YeelightConfig::default()).await?;
    let backend = YeelightBackend::new(YeelightConfig::default());
    backend.adopt_device(&discovered)?;
    // The closed control connection must not be reused for normal mode.
    assert!(matches!(
        backend.connect(&discovered.info.id).await,
        Err(DeviceError::ConnectionFailed { .. })
    ));
    assert!(backend.frame_sink(&discovered.info.id).is_none());
    Ok(())
}

#[tokio::test]
async fn writes_racing_disconnect_leave_no_connection_open() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec::color_bulb()).await?;
    let (backend, discovered) = connected(&fake, YeelightConfig::default()).await?;
    let id = discovered.info.id;
    let sink = backend.frame_sink(&id).ok_or("frame sink")?;
    let mut writers = Vec::new();
    for shade in 0..32_u8 {
        let sink = Arc::clone(&sink);
        writers.push(tokio::spawn(async move {
            sink.write_colors_shared(Arc::new(vec![[shade, 0, 255]]))
                .await
        }));
    }
    backend.disconnect(&id).await?;
    for writer in writers {
        let _ = writer.await?;
    }
    assert!(matches!(
        sink.write_colors_shared(Arc::new(vec![[1, 2, 3]])).await,
        Err(DeviceError::Disconnected { .. })
    ));
    for _ in 0..200 {
        if fake.closed_connections() == (1, 1) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        fake.closed_connections(),
        (1, 1),
        "the retained sink holds no socket"
    );
    Ok(())
}
