use std::path::Path;

use hypercolor_core::device::{BlocksBackend, DeviceBackend};
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

async fn serve_backend_handshake(
    socket_path: &Path,
    ack: u8,
) -> TestResult<(
    oneshot::Receiver<Vec<u8>>,
    tokio::task::JoinHandle<TestResult>,
)> {
    let listener = UnixListener::bind(socket_path)?;
    let (frame_tx, frame_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let (reader_half, mut writer_half) = stream.into_split();
        let mut reader = BufReader::new(reader_half);
        let mut line = String::new();

        reader.read_line(&mut line).await?;
        assert_eq!(line, "{\"type\":\"ping\",\"id\":\"hc\"}\n");
        writer_half
            .write_all(
                br#"{"type":"pong","version":"0.1.0","uptime_seconds":1,"device_count":1,"id":"hc"}"#,
            )
            .await?;
        writer_half.write_all(b"\n").await?;

        line.clear();
        reader.read_line(&mut line).await?;
        assert_eq!(line, "{\"type\":\"discover\",\"id\":\"hc\"}\n");
        writer_half
            .write_all(discover_response(TEST_UID).as_bytes())
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
    let (frame_rx, server_task) = serve_backend_handshake(&socket_path, 0x01).await?;

    let mut backend = BlocksBackend::new(socket_path);
    let discovered = backend.discover().await?;
    let device_id = discovered[0].id;
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
    let (_frame_rx, server_task) = serve_backend_handshake(&socket_path, 0x00).await?;

    let mut backend = BlocksBackend::new(socket_path);
    let discovered = backend.discover().await?;
    let device_id = discovered[0].id;
    backend.connect(&device_id).await?;

    backend
        .write_colors(&device_id, &[[0xFF, 0x00, 0xFF]])
        .await?;

    server_task.await??;
    Ok(())
}
