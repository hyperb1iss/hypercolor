#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::device::{
    BackendManager, BlocksBackend, BlocksScanner, DeviceOutputStatistics,
};
use hypercolor_driver_api::{
    DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceDeliveryStatus, DeviceWriteOutcome,
};
use hypercolor_types::device::{BLOCKS_OUTPUT_BACKEND_ID, DeviceError, DeviceId};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::oneshot;

const TEST_UID: u64 = 15_574_837_184_041_537_129;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
/// Every binary frame the fake blocksd read, and the task serving them.
type FakeBlocksd = (
    oneshot::Receiver<Vec<Vec<u8>>>,
    tokio::task::JoinHandle<TestResult>,
);

fn discover_response(uid: u64) -> String {
    format!(
        concat!(
            r#"{{"type":"discover_response","id":"hc","devices":["#,
            r#"{{"uid":{uid},"serial":"LPMJW6SWHSPD8H92","block_type":"lightpad_m","#,
            r#""name":"Lightpad Block M","battery_level":31,"battery_charging":false,"#,
            r#""grid_width":15,"grid_height":15,"firmware_version":"0.4.2"}}"#,
            r#"]}}"#
        ),
        uid = uid
    )
}

/// Serve discovery and one backend connection that answers each binary frame
/// with the next byte of `acks`, then report every frame it read.
fn serve_backend_handshake(socket_path: &Path, acks: &'static [u8]) -> TestResult<FakeBlocksd> {
    let listener = UnixListener::bind(socket_path)?;
    let (frame_tx, frame_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        let (scanner_stream, _) = listener.accept().await?;
        let (scanner_reader, mut scanner_writer) = scanner_stream.into_split();
        let mut scanner_reader = BufReader::new(scanner_reader);
        let mut line = String::new();

        scanner_reader.read_line(&mut line).await?;
        assert_eq!(line, "{\"type\":\"discover\",\"id\":\"hc\"}\n");
        scanner_writer
            .write_all(discover_response(TEST_UID).as_bytes())
            .await?;
        scanner_writer.write_all(b"\n").await?;
        drop(scanner_reader);
        drop(scanner_writer);

        let (backend_stream, _) = listener.accept().await?;
        let (reader_half, mut writer_half) = backend_stream.into_split();
        let mut reader = BufReader::new(reader_half);

        line.clear();
        reader.read_line(&mut line).await?;
        assert_eq!(line, "{\"type\":\"ping\",\"id\":\"hc\"}\n");
        writer_half
            .write_all(
                br#"{"type":"pong","version":"0.1.0","uptime_seconds":1,"device_count":1,"id":"hc"}"#,
            )
            .await?;
        writer_half.write_all(b"\n").await?;

        let mut frames = Vec::with_capacity(acks.len());
        for ack in acks {
            let mut frame = vec![0_u8; 685];
            reader.read_exact(&mut frame).await?;
            frames.push(frame);
            writer_half.write_all(&[*ack]).await?;
            writer_half.flush().await?;
        }
        frame_tx.send(frames).ok();

        Ok(())
    });

    Ok((frame_rx, task))
}

#[tokio::test]
async fn blocks_backend_writes_u64_binary_frames() -> TestResult {
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let (frame_rx, server_task) = serve_backend_handshake(&socket_path, &[0x01])?;

    let mut scanner = BlocksScanner::new(socket_path.clone());
    let discovered = scanner.scan().await?;
    let backend = BlocksBackend::new(socket_path);
    backend.adopt_device(&discovered[0])?;
    let device_id = discovered[0].info.id;
    backend.connect(&device_id).await?;
    backend
        .write_colors(&device_id, &[[0x12, 0x34, 0x56], [0xAB, 0xCD, 0xEF]])
        .await?;

    let frames = frame_rx.await?;
    let frame = &frames[0];
    assert_eq!(frame.len(), 685);
    assert_eq!(&frame[..2], &[0xBD, 0x01]);
    assert_eq!(u64::from_le_bytes(frame[2..10].try_into()?), TEST_UID);
    assert_eq!(&frame[10..16], &[0x12, 0x34, 0x56, 0xAB, 0xCD, 0xEF]);
    assert!(frame[16..].iter().all(|byte| *byte == 0));

    server_task.await??;
    Ok(())
}

/// Observer for direct backend deliveries; these tests read the terminal ack.
struct IgnoreTransportStart;

impl DeviceDeliveryObserver for IgnoreTransportStart {
    fn transport_started(&self, _id: DeviceDeliveryId) {}
}

async fn deliver(
    backend: &BlocksBackend,
    device_id: &DeviceId,
    sequence: u64,
    colors: Vec<[u8; 3]>,
) -> DeviceDeliveryAck {
    let id = DeviceDeliveryId {
        queue_generation: 1,
        sequence,
    };
    let ack = backend
        .deliver_colors_shared_observed(
            device_id,
            id,
            Arc::new(colors),
            Arc::new(IgnoreTransportStart),
        )
        .await;
    assert_eq!(ack.id, id);
    ack
}

fn assert_declined(ack: &DeviceDeliveryAck) {
    assert_eq!(ack.status, DeviceDeliveryStatus::Declined);
    assert!(ack.transport_started, "the frame reached blocksd");
    assert_eq!(
        ack.completed_payload_bytes, 0,
        "a declined frame is not sent"
    );
    assert_eq!(ack.error, None, "a decline is not a transport failure");
}

#[tokio::test]
async fn blocks_backend_reports_a_declined_grid_frame_and_keeps_the_connection() -> TestResult {
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let (frame_rx, server_task) = serve_backend_handshake(&socket_path, &[0x00, 0x00, 0x01])?;

    let mut scanner = BlocksScanner::new(socket_path.clone());
    let discovered = scanner.scan().await?;
    let backend = BlocksBackend::new(socket_path);
    backend.adopt_device(&discovered[0])?;
    let device_id = discovered[0].info.id;
    backend.connect(&device_id).await?;

    let declined = deliver(&backend, &device_id, 1, vec![[0xFF, 0x00, 0xFF]]).await;
    assert_declined(&declined);
    assert_eq!(
        backend
            .write_colors_shared_outcome(&device_id, Arc::new(vec![[0xFF, 0x00, 0xFF]]))
            .await?,
        DeviceWriteOutcome::Declined
    );
    let accepted = deliver(&backend, &device_id, 2, vec![[0x00, 0xFF, 0x00]; 225]).await;
    assert_eq!(accepted.status, DeviceDeliveryStatus::Completed);
    assert_eq!(accepted.completed_payload_bytes, 675);

    // All three frames arrived on the one connection the fake serves, so the
    // declines did not drop it.
    let frames = frame_rx.await?;
    assert_eq!(frames.len(), 3);
    assert_eq!(&frames[2][10..13], &[0x00, 0xFF, 0x00]);
    server_task.await??;
    Ok(())
}

#[tokio::test]
async fn blocks_backend_fails_an_unknown_grid_ack_byte_and_reconnects() -> TestResult {
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let task = tokio::spawn(async move {
        let legacy: serde_json::Value = serde_json::from_str(&discover_response(TEST_UID))?;
        serve_discovery(&listener, &[legacy["devices"][0].clone()]).await?;
        let mut frame = [0_u8; 685];
        // blocksd only ever answers 0x01 or 0x00; 0x7F is neither.
        let mut reader = serve_ping(&listener).await?;
        reader.read_exact(&mut frame).await?;
        reader.get_mut().write_all(&[0x7F]).await?;
        drop(reader);
        let mut reader = serve_ping(&listener).await?;
        reader.read_exact(&mut frame).await?;
        reader.get_mut().write_all(&[0x01]).await?;
        TestResult::Ok(())
    });
    let discovered = BlocksScanner::new(socket_path.clone()).scan().await?;
    let backend = BlocksBackend::new(socket_path);
    let device_id = discovered[0].info.id;
    backend.adopt_device(&discovered[0])?;
    backend.connect(&device_id).await?;

    let garbled = deliver(&backend, &device_id, 1, vec![[1, 2, 3]]).await;
    assert_eq!(garbled.status, DeviceDeliveryStatus::Failed);
    let error = garbled.error.ok_or("an unknown ack byte fails the frame")?;
    assert!(
        error.to_string().contains("acknowledgement byte 0x7f"),
        "the error names the byte: {error}"
    );

    let dropped = deliver(&backend, &device_id, 2, vec![[1, 2, 3]]).await;
    assert!(
        matches!(dropped.error, Some(DeviceError::Disconnected { .. })),
        "the failure dropped the connection: {:?}",
        dropped.error
    );

    backend.connect(&device_id).await?;
    let accepted = deliver(&backend, &device_id, 3, vec![[1, 2, 3]]).await;
    assert_eq!(accepted.status, DeviceDeliveryStatus::Completed);
    task.await??;
    Ok(())
}

fn lumi_device() -> serde_json::Value {
    serde_json::json!({
        "uid": TEST_UID - 1,
        "serial": "LKBC9PZSOH978HOE",
        "block_type": "lumi_keys",
        "name": "LUMI Keys",
        "battery_level": 90,
        "battery_charging": false,
        "grid_width": 0,
        "grid_height": 0,
        "key_count": 24,
        "firmware_version": "1.3.9"
    })
}

async fn serve_discovery(listener: &UnixListener, devices: &[serde_json::Value]) -> TestResult {
    let (stream, _) = listener.accept().await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line)?["type"],
        "discover"
    );
    let response = serde_json::json!({"type": "discover_response", "devices": devices});
    reader
        .get_mut()
        .write_all(response.to_string().as_bytes())
        .await?;
    reader.get_mut().write_all(b"\n").await?;
    Ok(())
}

async fn serve_ping(listener: &UnixListener) -> TestResult<BufReader<tokio::net::UnixStream>> {
    let (stream, _) = listener.accept().await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line)?["type"],
        "ping"
    );
    reader
        .get_mut()
        .write_all(
            b"{\"type\":\"pong\",\"version\":\"0.5.0\",\"uptime_seconds\":1,\"device_count\":2}\n",
        )
        .await?;
    Ok(reader)
}

async fn assert_key_request(reader: &mut BufReader<tokio::net::UnixStream>) -> TestResult {
    use base64::Engine;
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let request: serde_json::Value = serde_json::from_str(&line)?;
    assert_eq!(request["type"], "key_frame");
    assert_eq!(request["uid"].as_u64(), Some(TEST_UID - 1));
    let pixels = base64::engine::general_purpose::STANDARD
        .decode(request["pixels"].as_str().ok_or("missing pixels")?)?;
    assert_eq!(pixels, key_colors().as_flattened());
    Ok(())
}

fn key_colors() -> [[u8; 3]; 24] {
    let mut colors = [[0; 3]; 24];
    for (index, color) in (0_u8..24).zip(&mut colors) {
        *color = [index, 80 + index, 200 + index];
    }
    colors
}

fn key_ack(accepted: bool) -> String {
    serde_json::json!({"type": "key_frame_ack", "uid": TEST_UID - 1, "accepted": accepted})
        .to_string()
        + "\n"
}

#[tokio::test]
async fn blocks_discovery_filters_unsupported_and_invalid_surfaces() -> TestResult {
    use hypercolor_types::device::DeviceTopologyHint;
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let legacy: serde_json::Value = serde_json::from_str(&discover_response(TEST_UID))?;
    let grid = legacy["devices"][0].clone();
    let keys = lumi_device();
    let mut devices = vec![grid.clone(), keys.clone()];
    for (field, value) in [
        ("key_count", serde_json::json!(0)),
        ("key_count", serde_json::json!(23)),
        ("grid_width", serde_json::json!(15)),
        ("block_type", serde_json::json!("seaboard")),
        ("block_type", serde_json::json!("unknown")),
    ] {
        let mut invalid = keys.clone();
        invalid[field] = value;
        devices.push(invalid);
    }
    let mut legacy_keys = keys.clone();
    legacy_keys
        .as_object_mut()
        .ok_or("device is not an object")?
        .remove("key_count");
    devices.push(legacy_keys);
    for (width, height) in [(0, 0), (14, 15), (u32::MAX, u32::MAX)] {
        let mut invalid = grid.clone();
        invalid["grid_width"] = width.into();
        invalid["grid_height"] = height.into();
        devices.push(invalid);
    }
    let task = tokio::spawn(async move { serve_discovery(&listener, &devices).await });
    let discovered = BlocksScanner::new(socket_path).scan().await?;
    assert_eq!(discovered.len(), 2);
    assert_eq!(discovered[0].info.capabilities.led_count, 225);
    assert_eq!(
        discovered[0].info.segments[0].topology,
        DeviceTopologyHint::Matrix { rows: 15, cols: 15 }
    );
    assert_eq!(discovered[1].info.capabilities.led_count, 24);
    assert_eq!(
        discovered[1].info.segments[0].topology,
        DeviceTopologyHint::Strip
    );
    assert_eq!(discovered[1].metadata["uid"], (TEST_UID - 1).to_string());
    task.await??;
    Ok(())
}

#[tokio::test]
async fn blocks_backend_interleaves_key_json_and_grid_binary_frames() -> TestResult {
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let task = tokio::spawn(async move {
        let legacy: serde_json::Value = serde_json::from_str(&discover_response(TEST_UID))?;
        serve_discovery(&listener, &[legacy["devices"][0].clone(), lumi_device()]).await?;
        let mut reader = serve_ping(&listener).await?;
        assert_key_request(&mut reader).await?;
        // Split the JSON response across writes to exercise buffered framing.
        let ack = key_ack(true);
        reader.get_mut().write_all(&ack.as_bytes()[..7]).await?;
        reader.get_mut().write_all(&ack.as_bytes()[7..]).await?;
        let mut frame = [0_u8; 685];
        reader.read_exact(&mut frame).await?;
        assert_eq!(&frame[..2], &[0xBD, 0x01]);
        assert_eq!(u64::from_le_bytes(frame[2..10].try_into()?), TEST_UID);
        assert_eq!(&frame[10..13], &[10, 20, 30]);
        reader.get_mut().write_all(&[1]).await?;
        assert_key_request(&mut reader).await?;
        reader.get_mut().write_all(key_ack(true).as_bytes()).await?;
        TestResult::Ok(())
    });
    let discovered = BlocksScanner::new(socket_path.clone()).scan().await?;
    let backend = BlocksBackend::new(socket_path);
    for device in &discovered {
        backend.adopt_device(device)?;
        backend.connect(&device.info.id).await?;
    }
    backend
        .write_colors(&discovered[1].info.id, &key_colors())
        .await?;
    backend
        .write_colors(&discovered[0].info.id, &[[10, 20, 30]; 225])
        .await?;
    backend
        .write_colors(&discovered[1].info.id, &key_colors())
        .await?;
    task.await??;
    Ok(())
}

#[tokio::test]
async fn blocks_backend_reports_a_declined_key_frame_and_retries_on_the_same_connection()
-> TestResult {
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let task = tokio::spawn(async move {
        serve_discovery(&listener, &[lumi_device()]).await?;
        let mut reader = serve_ping(&listener).await?;
        for accepted in [false, true] {
            assert_key_request(&mut reader).await?;
            reader
                .get_mut()
                .write_all(key_ack(accepted).as_bytes())
                .await?;
        }
        TestResult::Ok(())
    });
    let discovered = BlocksScanner::new(socket_path.clone()).scan().await?;
    let backend = BlocksBackend::new(socket_path);
    let device = &discovered[0];
    backend.adopt_device(device)?;
    backend.connect(&device.info.id).await?;
    for colors in [vec![], vec![[1, 2, 3]; 23], vec![[1, 2, 3]; 25]] {
        assert!(
            backend
                .write_colors(&device.info.id, &colors)
                .await
                .is_err()
        );
    }
    let declined = deliver(&backend, &device.info.id, 1, key_colors().to_vec()).await;
    assert_declined(&declined);
    let accepted = deliver(&backend, &device.info.id, 2, key_colors().to_vec()).await;
    assert_eq!(accepted.status, DeviceDeliveryStatus::Completed);
    assert_eq!(accepted.completed_payload_bytes, 72);
    task.await??;
    Ok(())
}

#[tokio::test]
async fn blocks_backend_rejects_malformed_key_acks_and_reconnects() -> TestResult {
    let valid: serde_json::Value = serde_json::from_str(&key_ack(true))?;
    let mut responses = vec!["not json\n".to_owned(), "\n".to_owned()];
    for (field, value) in [
        ("type", serde_json::json!("frame_ack")),
        ("uid", serde_json::json!(TEST_UID)),
        ("uid", serde_json::json!((TEST_UID - 1).to_string())),
        ("accepted", serde_json::json!("true")),
        ("accepted", serde_json::Value::Null),
    ] {
        let mut response = valid.clone();
        response[field] = value;
        responses.push(response.to_string() + "\n");
    }
    for field in ["type", "uid", "accepted"] {
        let mut response = valid.clone();
        response
            .as_object_mut()
            .ok_or("ack is not an object")?
            .remove(field);
        responses.push(response.to_string() + "\n");
    }
    for response in responses {
        let tempdir = tempdir()?;
        let socket_path = tempdir.path().join("blocksd.sock");
        let listener = UnixListener::bind(&socket_path)?;
        let task = tokio::spawn(async move {
            serve_discovery(&listener, &[lumi_device()]).await?;
            let mut reader = serve_ping(&listener).await?;
            assert_key_request(&mut reader).await?;
            reader.get_mut().write_all(response.as_bytes()).await?;
            drop(reader);
            let mut reader = serve_ping(&listener).await?;
            assert_key_request(&mut reader).await?;
            reader.get_mut().write_all(key_ack(true).as_bytes()).await?;
            TestResult::Ok(())
        });
        let discovered = BlocksScanner::new(socket_path.clone()).scan().await?;
        let backend = BlocksBackend::new(socket_path);
        let device = &discovered[0];
        backend.adopt_device(device)?;
        backend.connect(&device.info.id).await?;
        assert!(
            backend
                .write_colors(&device.info.id, &key_colors())
                .await
                .is_err()
        );
        backend.connect(&device.info.id).await?;
        backend.write_colors(&device.info.id, &key_colors()).await?;
        task.await??;
    }
    Ok(())
}

#[tokio::test]
async fn blocks_backend_fails_an_oversized_reply_and_reconnects() -> TestResult {
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let task = tokio::spawn(async move {
        serve_discovery(&listener, &[lumi_device()]).await?;
        let mut reader = serve_ping(&listener).await?;
        assert_key_request(&mut reader).await?;
        // A valid acknowledgement, padded to twice the 1 MiB reply cap.
        let mut ack: serde_json::Value = serde_json::from_str(&key_ack(true))?;
        ack["pad"] = "x".repeat(2 * 1024 * 1024).into();
        reader
            .get_mut()
            .write_all((ack.to_string() + "\n").as_bytes())
            .await?;
        drop(reader);
        let mut reader = serve_ping(&listener).await?;
        assert_key_request(&mut reader).await?;
        reader.get_mut().write_all(key_ack(true).as_bytes()).await?;
        TestResult::Ok(())
    });
    let discovered = BlocksScanner::new(socket_path.clone()).scan().await?;
    let backend = BlocksBackend::new(socket_path);
    let device = &discovered[0];
    backend.adopt_device(device)?;
    backend.connect(&device.info.id).await?;
    let error = backend
        .write_colors(&device.info.id, &key_colors())
        .await
        .expect_err("an oversized acknowledgement fails the frame");
    assert!(
        error.to_string().contains("exceeded"),
        "the error names the cap: {error}"
    );
    backend.connect(&device.info.id).await?;
    backend.write_colors(&device.info.id, &key_colors()).await?;
    task.await??;
    Ok(())
}

fn pad_layout(layout_device_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: "blocks-layout".into(),
        name: "Blocks Layout".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![Output {
            id: "pad".into(),
            name: "pad".into(),
            device_id: layout_device_id.into(),
            zone_name: None,
            position: NormalizedPosition { x: 0.5, y: 0.5 },
            size: NormalizedPosition { x: 1.0, y: 1.0 },
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 4,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn pad_frame(color: [u8; 3]) -> Vec<ZoneColors> {
    vec![ZoneColors {
        zone_id: "pad".into(),
        colors: vec![color; 4],
    }]
}

async fn wait_for_stats(
    manager: &BackendManager,
    ready: impl Fn(&DeviceOutputStatistics) -> bool,
) -> TestResult<DeviceOutputStatistics> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let stats = manager.device_output_statistics();
        if let Some(stats) = stats.into_iter().next().filter(|stats| ready(stats)) {
            return Ok(stats);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("output queue did not reach the expected state".into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn blocks_output_queue_counts_a_declined_frame_as_declined_not_sent() -> TestResult {
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let (frame_rx, server_task) = serve_backend_handshake(&socket_path, &[0x00, 0x01])?;

    let discovered = BlocksScanner::new(socket_path.clone()).scan().await?;
    let backend = BlocksBackend::new(socket_path);
    backend.adopt_device(&discovered[0])?;
    let device_id = discovered[0].info.id;
    let mut manager = BackendManager::new();
    manager.register_backend(Arc::new(backend));
    manager
        .connect_device(BLOCKS_OUTPUT_BACKEND_ID, device_id, "blocks:pad")
        .await?;
    let layout = pad_layout("blocks:pad");

    manager.write_frame(&pad_frame([200, 40, 120]), &layout);
    let declined = wait_for_stats(&manager, |stats| stats.transport_declined == 1).await?;
    assert_eq!(declined.transport_started, 1);
    assert_eq!(declined.frames_sent, 0, "a declined frame is not delivered");
    assert_eq!(declined.transport_completed, 0);
    assert_eq!(declined.bytes_sent, 0);
    assert_eq!(declined.transport_failed, 0, "a decline is not a failure");
    assert_eq!(declined.errors_total, 0);
    assert_eq!(declined.last_error, None);
    assert!(
        manager.async_write_failures().is_empty(),
        "a decline never reaches device lifecycle, so nothing disconnects"
    );

    manager.write_frame(&pad_frame([40, 200, 120]), &layout);
    let accepted = wait_for_stats(&manager, |stats| stats.frames_sent == 1).await?;
    assert_eq!(accepted.transport_started, 2);
    assert_eq!(accepted.transport_completed, 1);
    assert_eq!(accepted.transport_declined, 1);
    assert_eq!(accepted.bytes_sent, 12);
    assert_eq!(accepted.transport_failed, 0);
    assert_eq!(
        accepted.transport_started,
        accepted.transport_completed + accepted.transport_failed + accepted.transport_declined,
        "every Blocks attempt starts transport, so each ends completed, failed, or declined"
    );
    assert!(manager.async_write_failures().is_empty());

    assert_eq!(frame_rx.await?.len(), 2, "both frames share one connection");
    server_task.await??;
    Ok(())
}
