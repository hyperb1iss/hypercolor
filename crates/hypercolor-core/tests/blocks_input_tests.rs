#![cfg(unix)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hypercolor_core::device::{BlocksBackend, BlocksScanner};
use hypercolor_driver_api::{DeviceBackend, DeviceInputPublisher, DeviceInputSink};
use hypercolor_types::device::DeviceId;
use hypercolor_types::device_input::{DeviceInputEdge, TouchPosition};
use hypercolor_types::event::InputButtonState;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::{Instant, sleep};

const PAD_UID: u64 = 15_574_837_184_041_537_129;
const STRANGER_UID: u64 = 42;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug, PartialEq)]
enum Record {
    Attach { device: DeviceId, lease: u64 },
    Publish { lease: u64, edge: DeviceInputEdge },
    Drop { lease: u64 },
}

#[derive(Clone, Default)]
struct RecordingSink {
    log: Arc<Mutex<Vec<Record>>>,
    next_lease: Arc<AtomicU64>,
}

impl RecordingSink {
    fn records(&self) -> Vec<Record> {
        self.log.lock().expect("record log").clone()
    }

    async fn wait_for(&self, predicate: impl Fn(&[Record]) -> bool) -> Vec<Record> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let records = self.records();
            if predicate(&records) {
                return records;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for input records: {records:?}"
            );
            sleep(Duration::from_millis(10)).await;
        }
    }
}

struct RecordingPublisher {
    lease: u64,
    log: Arc<Mutex<Vec<Record>>>,
}

impl DeviceInputSink for RecordingSink {
    fn attach(&self, device_id: DeviceId, _label: &str) -> Box<dyn DeviceInputPublisher> {
        let lease = self.next_lease.fetch_add(1, Ordering::Relaxed) + 1;
        self.log.lock().expect("record log").push(Record::Attach {
            device: device_id,
            lease,
        });
        Box::new(RecordingPublisher {
            lease,
            log: Arc::clone(&self.log),
        })
    }
}

impl DeviceInputPublisher for RecordingPublisher {
    fn publish(&self, edges: &[DeviceInputEdge]) -> bool {
        let mut log = self.log.lock().expect("record log");
        for edge in edges {
            log.push(Record::Publish {
                lease: self.lease,
                edge: edge.clone(),
            });
        }
        true
    }
}

impl Drop for RecordingPublisher {
    fn drop(&mut self) {
        self.log
            .lock()
            .expect("record log")
            .push(Record::Drop { lease: self.lease });
    }
}

fn published(records: &[Record]) -> Vec<(u64, DeviceInputEdge)> {
    records
        .iter()
        .filter_map(|record| match record {
            Record::Publish { lease, edge } => Some((*lease, edge.clone())),
            _ => None,
        })
        .collect()
}

fn attaches(records: &[Record]) -> usize {
    records
        .iter()
        .filter(|record| matches!(record, Record::Attach { .. }))
        .count()
}

fn pad() -> serde_json::Value {
    serde_json::json!({
        "uid": PAD_UID,
        "serial": "LPMJW6SWHSPD8H92",
        "block_type": "lightpad_m",
        "name": "Lightpad Block M",
        "battery_level": 31,
        "battery_charging": false,
        "grid_width": 15,
        "grid_height": 15,
        "firmware_version": "1.1.0"
    })
}

async fn read_line(reader: &mut BufReader<UnixStream>) -> TestResult<serde_json::Value> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    Ok(serde_json::from_str(&line)?)
}

async fn send(reader: &mut BufReader<UnixStream>, lines: &[&str]) -> TestResult {
    for line in lines {
        reader.get_mut().write_all(line.as_bytes()).await?;
        reader.get_mut().write_all(b"\n").await?;
    }
    reader.get_mut().flush().await?;
    Ok(())
}

/// Accept an input connection and check it subscribes to input events.
async fn accept_subscription(listener: &UnixListener) -> TestResult<BufReader<UnixStream>> {
    let (stream, _) = listener.accept().await?;
    let mut events = BufReader::new(stream);
    let subscribe = read_line(&mut events).await?;
    assert_eq!(subscribe["type"], "subscribe");
    assert_eq!(
        subscribe["events"],
        serde_json::json!(["device", "touch", "button"])
    );
    Ok(events)
}

async fn connected_backend(
    listener: &UnixListener,
    socket_path: std::path::PathBuf,
    sink: &RecordingSink,
) -> TestResult<(BlocksBackend, DeviceId, BufReader<UnixStream>)> {
    let scan = {
        let path = socket_path.clone();
        tokio::spawn(async move { BlocksScanner::new(path).scan().await })
    };
    let frames = serve_scan_and_ping_after(listener, scan).await?;
    let (discovered, frames) = frames;
    let backend = BlocksBackend::new(socket_path).with_device_input(Arc::new(sink.clone()));
    backend.adopt_device(&discovered[0])?;
    let device_id = discovered[0].info.id;
    let connect = backend.connect(&device_id);
    let (result, frames) = tokio::join!(connect, frames);
    result?;
    Ok((backend, device_id, frames?))
}

/// Run the scan against the fake server, then prepare to serve the ping.
async fn serve_scan_and_ping_after(
    listener: &UnixListener,
    scan: tokio::task::JoinHandle<anyhow::Result<Vec<hypercolor_driver_api::DiscoveredDevice>>>,
) -> TestResult<(
    Vec<hypercolor_driver_api::DiscoveredDevice>,
    impl std::future::Future<Output = TestResult<BufReader<UnixStream>>> + '_,
)> {
    let (stream, _) = listener.accept().await?;
    let mut scan_conn = BufReader::new(stream);
    assert_eq!(read_line(&mut scan_conn).await?["type"], "discover");
    let response = serde_json::json!({"type": "discover_response", "devices": [pad()]});
    send(&mut scan_conn, &[&response.to_string()]).await?;
    let discovered = scan.await??;
    let ping = async move {
        let (stream, _) = listener.accept().await?;
        let mut frames = BufReader::new(stream);
        assert_eq!(read_line(&mut frames).await?["type"], "ping");
        send(
            &mut frames,
            &[r#"{"type":"pong","version":"0.6.0","uptime_seconds":1,"device_count":1}"#],
        )
        .await?;
        Ok(frames)
    };
    Ok((discovered, ping))
}

fn touch(uid: u64, action: &str, index: u8, x: f32) -> String {
    serde_json::json!({
        "type": "touch", "uid": uid, "action": action, "index": index,
        "timestamp": 1000, "x": x, "y": 0.5, "z": 0.25,
        "vx": 0.0, "vy": 0.0, "vz": 0.0
    })
    .to_string()
}

fn at(x: f32) -> TouchPosition {
    TouchPosition {
        x,
        y: 0.5,
        pressure: 0.25,
    }
}

#[tokio::test]
async fn blocks_input_publishes_touches_and_buttons_for_adopted_devices() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let sink = RecordingSink::default();

    let (_backend, device_id, _frames) = connected_backend(&listener, socket_path, &sink).await?;
    let mut events = accept_subscription(&listener).await?;
    send(
        &mut events,
        &[
            r#"{"type":"subscribed","events":["button","device","touch"]}"#,
            &touch(STRANGER_UID, "start", 0, 0.9),
            &touch(PAD_UID, "start", 1, 0.25),
            &touch(PAD_UID, "move", 1, 0.5),
            &touch(PAD_UID, "end", 1, 0.75),
            r#"{"type":"button","uid":15574837184041537129,"action":"press","button_id":0,"button":"mode","timestamp":1}"#,
            r#"{"type":"button","uid":15574837184041537129,"action":"release","button_id":2,"timestamp":2}"#,
            r#"{"type":"future_event","uid":15574837184041537129}"#,
            // A partial device payload is harmless: devices come from adoption.
            r#"{"type":"device_added","device":{"uid":7}}"#,
        ],
    )
    .await?;

    let records = sink.wait_for(|records| published(records).len() == 5).await;
    assert_eq!(attaches(&records), 1, "no event here cancels held input");
    assert_eq!(
        records[0],
        Record::Attach {
            device: device_id,
            lease: 1
        }
    );
    assert_eq!(
        published(&records),
        vec![
            (
                1,
                DeviceInputEdge::TouchBegan {
                    contact: 1,
                    position: at(0.25)
                }
            ),
            (
                1,
                DeviceInputEdge::TouchMoved {
                    contact: 1,
                    position: at(0.5)
                }
            ),
            (
                1,
                DeviceInputEdge::TouchEnded {
                    contact: 1,
                    position: at(0.75)
                }
            ),
            (
                1,
                DeviceInputEdge::Button {
                    button: Arc::from("mode"),
                    state: InputButtonState::Pressed
                }
            ),
            (
                1,
                DeviceInputEdge::Button {
                    button: Arc::from("button2"),
                    state: InputButtonState::Released
                }
            ),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn blocks_input_cancels_holds_when_the_stream_or_device_is_lost() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let sink = RecordingSink::default();

    let (_backend, _device_id, _frames) = connected_backend(&listener, socket_path, &sink).await?;
    let mut events = accept_subscription(&listener).await?;

    // A removed block will never report its lifts.
    send(
        &mut events,
        &[&format!(
            r#"{{"type":"device_removed","uid":{PAD_UID},"reason":"disconnected"}}"#
        )],
    )
    .await?;
    sink.wait_for(|records| attaches(records) == 2).await;

    // So may a line blocksd sent that this reader could not decode.
    send(&mut events, &[r#"{"type":"touch","uid":"not a number"}"#]).await?;
    sink.wait_for(|records| attaches(records) == 3).await;

    // Losing the stream supersedes every lease, then it resubscribes.
    drop(events);
    sink.wait_for(|records| attaches(records) == 4).await;
    let mut events = accept_subscription(&listener).await?;
    send(&mut events, &[&touch(PAD_UID, "start", 0, 0.5)]).await?;
    let records = sink.wait_for(|records| published(records).len() == 1).await;
    assert_eq!(
        published(&records),
        vec![(
            4,
            DeviceInputEdge::TouchBegan {
                contact: 0,
                position: at(0.5)
            }
        )],
        "input after reconnecting flows through the newest lease"
    );
    for lease in 1..=3 {
        assert!(
            records.contains(&Record::Drop { lease }),
            "lease {lease} dropped"
        );
    }
    Ok(())
}

#[tokio::test]
async fn disconnecting_a_device_drops_its_input_lease() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let sink = RecordingSink::default();

    let (backend, device_id, _frames) = connected_backend(&listener, socket_path, &sink).await?;
    let mut events = accept_subscription(&listener).await?;

    backend.disconnect(&device_id).await?;
    assert!(sink.records().contains(&Record::Drop { lease: 1 }));
    let mut line = String::new();
    let closed = tokio::time::timeout(Duration::from_secs(1), events.read_line(&mut line)).await?;
    assert_eq!(
        closed?, 0,
        "the last disconnect closes the event connection"
    );
    Ok(())
}

#[tokio::test]
async fn an_unreachable_blocksd_does_not_churn_input_leases() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let sink = RecordingSink::default();

    let (_backend, _device_id, _frames) =
        connected_backend(&listener, socket_path.clone(), &sink).await?;
    let events = accept_subscription(&listener).await?;

    // blocksd goes away: the lost stream cancels once, and the failed
    // reconnects that follow have nothing to cancel.
    drop(listener);
    std::fs::remove_file(&socket_path)?;
    drop(events);
    sink.wait_for(|records| attaches(records) == 2).await;
    sleep(Duration::from_millis(900)).await;
    assert_eq!(attaches(&sink.records()), 2);
    Ok(())
}

#[tokio::test]
async fn blocks_backend_without_device_input_opens_no_event_stream() -> TestResult {
    let dir = tempdir()?;
    let socket_path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&socket_path)?;

    let scan = {
        let path = socket_path.clone();
        tokio::spawn(async move { BlocksScanner::new(path).scan().await })
    };
    let (discovered, ping) = serve_scan_and_ping_after(&listener, scan).await?;
    let backend = BlocksBackend::new(socket_path);
    backend.adopt_device(&discovered[0])?;
    let (connected, _frames) = tokio::join!(backend.connect(&discovered[0].info.id), ping);
    connected?;

    let extra = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
    assert!(
        extra.is_err(),
        "no input connection without a device input sink"
    );
    Ok(())
}
