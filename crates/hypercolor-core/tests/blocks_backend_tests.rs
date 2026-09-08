#![cfg(unix)]

use std::path::Path;

use hypercolor_core::device::{BlocksBackend, BlocksScanner};
use hypercolor_driver_api::DeviceBackend;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::oneshot;

const TEST_UID: u64 = 15_574_837_184_041_537_129;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

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

fn serve_backend_handshake(
    socket_path: &Path,
    ack: u8,
) -> TestResult<(
    oneshot::Receiver<Vec<u8>>,
    tokio::task::JoinHandle<TestResult>,
)> {
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

        let mut frame = vec![0_u8; 685];
        reader.read_exact(&mut frame).await?;
        frame_tx.send(frame).ok();

        writer_half.write_all(&[ack]).await?;
        writer_half.flush().await?;

        Ok(())
    });

    Ok((frame_rx, task))
}

#[tokio::test]
async fn blocks_backend_writes_u64_binary_frames() -> TestResult {
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let (frame_rx, server_task) = serve_backend_handshake(&socket_path, 0x01)?;

    let mut scanner = BlocksScanner::new(socket_path.clone());
    let discovered = scanner.scan().await?;
    let backend = BlocksBackend::new(socket_path);
    backend.adopt_device(&discovered[0])?;
    let device_id = discovered[0].info.id;
    backend.connect(&device_id).await?;
    backend
        .write_colors(&device_id, &[[0x12, 0x34, 0x56], [0xAB, 0xCD, 0xEF]])
        .await?;

    let frame = frame_rx.await?;
    assert_eq!(frame.len(), 685);
    assert_eq!(&frame[..2], &[0xBD, 0x01]);
    assert_eq!(u64::from_le_bytes(frame[2..10].try_into()?), TEST_UID);
    assert_eq!(&frame[10..16], &[0x12, 0x34, 0x56, 0xAB, 0xCD, 0xEF]);
    assert!(frame[16..].iter().all(|byte| *byte == 0));

    server_task.await??;
    Ok(())
}

#[tokio::test]
async fn blocks_backend_treats_binary_rejection_as_retryable() -> TestResult {
    let tempdir = tempdir()?;
    let socket_path = tempdir.path().join("blocksd.sock");
    let (_frame_rx, server_task) = serve_backend_handshake(&socket_path, 0x00)?;

    let mut scanner = BlocksScanner::new(socket_path.clone());
    let discovered = scanner.scan().await?;
    let backend = BlocksBackend::new(socket_path);
    backend.adopt_device(&discovered[0])?;
    let device_id = discovered[0].info.id;
    backend.connect(&device_id).await?;

    backend
        .write_colors(&device_id, &[[0xFF, 0x00, 0xFF]])
        .await?;

    server_task.await??;
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
async fn blocks_backend_retries_rejected_key_frame_on_same_connection() -> TestResult {
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
    backend.write_colors(&device.info.id, &key_colors()).await?;
    backend.write_colors(&device.info.id, &key_colors()).await?;
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
