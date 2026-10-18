#![cfg(unix)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use hypercolor_core::device::BlocksBackend;
use hypercolor_driver_api::DriverRuntimeActions;
use hypercolor_types::device::DeviceId;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::{Instant, sleep, timeout};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Longer than the watcher's coalescing window, so a request that was going
/// to follow a change has had time to land.
const SETTLE: Duration = Duration::from_millis(500);

#[derive(Default)]
struct RecordingRuntime {
    requests: Mutex<Vec<String>>,
}

impl RecordingRuntime {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("request log").clone()
    }

    async fn wait_for_requests(&self, count: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let requests = self.requests();
            if requests.len() >= count {
                return requests;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {count} discovery requests: {requests:?}"
            );
            sleep(Duration::from_millis(10)).await;
        }
    }
}

#[async_trait]
impl DriverRuntimeActions for RecordingRuntime {
    async fn activate_device(
        &self,
        _device_id: DeviceId,
        _backend_id: &str,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn disconnect_device(
        &self,
        _device_id: DeviceId,
        _backend_id: &str,
        _will_retry: bool,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn request_discovery(&self, driver_id: &str) -> anyhow::Result<()> {
        self.requests
            .lock()
            .expect("request log")
            .push(driver_id.to_owned());
        Ok(())
    }
}

fn watched_backend(socket_path: &Path, runtime: &Arc<RecordingRuntime>) -> BlocksBackend {
    let actions: Arc<dyn DriverRuntimeActions> = Arc::<RecordingRuntime>::clone(runtime);
    BlocksBackend::new(socket_path.to_path_buf()).with_discovery_requests(actions, "blocks")
}

/// Accept the watcher's connection and check it subscribes to device
/// events only.
async fn accept_watcher(listener: &UnixListener) -> TestResult<BufReader<UnixStream>> {
    let (stream, _) = timeout(Duration::from_secs(10), listener.accept()).await??;
    let mut events = BufReader::new(stream);
    let mut line = String::new();
    events.read_line(&mut line).await?;
    let subscribe: serde_json::Value = serde_json::from_str(&line)?;
    assert_eq!(subscribe["type"], "subscribe");
    assert_eq!(subscribe["events"], serde_json::json!(["device"]));
    Ok(events)
}

async fn send(events: &mut BufReader<UnixStream>, lines: &[&str]) -> TestResult {
    let mut batch = String::new();
    for line in lines {
        batch.push_str(line);
        batch.push('\n');
    }
    events.get_mut().write_all(batch.as_bytes()).await?;
    events.get_mut().flush().await?;
    Ok(())
}

const SUBSCRIBED: &str = r#"{"type":"subscribed","events":["device"]}"#;

fn device_added(uid: u64) -> String {
    serde_json::json!({
        "type": "device_added",
        "device": {"uid": uid, "serial": "LPMJW6SWHSPD8H92", "block_type": "lightpad_m"}
    })
    .to_string()
}

fn device_removed(uid: u64) -> String {
    serde_json::json!({"type": "device_removed", "uid": uid, "reason": "disconnected"}).to_string()
}

#[tokio::test]
async fn blocks_watcher_requests_discovery_when_blocks_arrive_or_leave() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let runtime = Arc::new(RecordingRuntime::default());
    let _backend = watched_backend(&socket_path, &runtime);

    let mut events = accept_watcher(&listener).await?;
    // The subscription itself resyncs, since nobody heard earlier events.
    send(&mut events, &[SUBSCRIBED]).await?;
    assert_eq!(runtime.wait_for_requests(1).await, vec!["blocks"]);

    send(&mut events, &[&device_added(7)]).await?;
    runtime.wait_for_requests(2).await;

    send(&mut events, &[&device_removed(7)]).await?;
    runtime.wait_for_requests(3).await;

    // Touches, buttons, and unknown events never trigger discovery.
    send(
        &mut events,
        &[
            r#"{"type":"touch","uid":7,"action":"start","index":0,"x":0.5,"y":0.5,"z":0.5,"vx":0.0,"vy":0.0,"vz":0.0}"#,
            r#"{"type":"button","uid":7,"action":"press"}"#,
            r#"{"type":"future_event","uid":7}"#,
        ],
    )
    .await?;
    sleep(SETTLE).await;
    assert_eq!(runtime.requests(), vec!["blocks"; 3]);
    Ok(())
}

#[tokio::test]
async fn blocks_watcher_coalesces_a_burst_into_one_request() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let runtime = Arc::new(RecordingRuntime::default());
    let _backend = watched_backend(&socket_path, &runtime);

    let mut events = accept_watcher(&listener).await?;
    // A plug-in: the subscription ack and every block of the new topology
    // land together, and a flapping block leaves and returns.
    send(
        &mut events,
        &[
            SUBSCRIBED,
            &device_added(1),
            &device_added(2),
            &device_removed(2),
            &device_added(2),
        ],
    )
    .await?;
    runtime.wait_for_requests(1).await;
    sleep(SETTLE).await;
    assert_eq!(runtime.requests(), vec!["blocks"]);

    // A later change is its own request, not swallowed by the first.
    send(&mut events, &[&device_added(3)]).await?;
    runtime.wait_for_requests(2).await;
    sleep(SETTLE).await;
    assert_eq!(runtime.requests().len(), 2);
    Ok(())
}

#[tokio::test]
async fn blocks_watcher_follows_blocksd_across_a_restart() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let runtime = Arc::new(RecordingRuntime::default());
    let _backend = watched_backend(&socket_path, &runtime);

    let mut events = accept_watcher(&listener).await?;
    send(&mut events, &[SUBSCRIBED]).await?;
    runtime.wait_for_requests(1).await;

    // blocksd exits: the connection closes and the socket disappears.
    drop(events);
    drop(listener);
    std::fs::remove_file(&socket_path)?;
    sleep(Duration::from_millis(300)).await;

    let listener = UnixListener::bind(&socket_path)?;
    let mut events = accept_watcher(&listener).await?;
    send(&mut events, &[SUBSCRIBED]).await?;
    runtime.wait_for_requests(2).await;

    send(&mut events, &[&device_added(9)]).await?;
    assert_eq!(runtime.wait_for_requests(3).await, vec!["blocks"; 3]);
    Ok(())
}

#[tokio::test]
async fn blocks_watcher_finds_blocksd_that_starts_later() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let runtime = Arc::new(RecordingRuntime::default());
    let _backend = watched_backend(&socket_path, &runtime);

    sleep(Duration::from_millis(300)).await;
    assert!(runtime.requests().is_empty());

    let listener = UnixListener::bind(&socket_path)?;
    let mut events = accept_watcher(&listener).await?;
    send(&mut events, &[SUBSCRIBED]).await?;
    assert_eq!(runtime.wait_for_requests(1).await, vec!["blocks"]);
    Ok(())
}

#[tokio::test]
async fn blocks_watcher_stops_with_its_backend() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let runtime = Arc::new(RecordingRuntime::default());
    let backend = watched_backend(&socket_path, &runtime);

    let mut events = accept_watcher(&listener).await?;
    drop(backend);

    let mut line = String::new();
    let read = timeout(Duration::from_secs(3), events.read_line(&mut line)).await??;
    assert_eq!(read, 0, "the watcher closes its connection when dropped");

    // Nothing is left to hear a late change.
    send(&mut events, &[&device_added(5)]).await.ok();
    sleep(SETTLE).await;
    assert!(runtime.requests().is_empty());
    Ok(())
}

#[test]
fn blocks_backend_without_a_runtime_skips_the_watcher() {
    let runtime = Arc::new(RecordingRuntime::default());
    let backend = watched_backend(Path::new("/nonexistent/blocksd.sock"), &runtime);
    drop(backend);
    assert!(runtime.requests().is_empty());
}
