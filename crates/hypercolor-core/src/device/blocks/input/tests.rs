use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use hypercolor_driver_api::{DeviceInputPublisher, DeviceInputSink};
use hypercolor_types::device::DeviceId;
use hypercolor_types::device_input::{DeviceInputEdge, TouchPosition};

use super::super::types::BlocksEvent;
use super::{BlocksInput, EventStream, lock};

const PAD_UID: u64 = 7;

#[derive(Clone, Debug, PartialEq)]
enum Record {
    Attach { lease: u64 },
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
}

struct RecordingPublisher {
    lease: u64,
    log: Arc<Mutex<Vec<Record>>>,
}

impl DeviceInputSink for RecordingSink {
    fn attach(&self, _device_id: DeviceId, _label: &str) -> Box<dyn DeviceInputPublisher> {
        let lease = self.next_lease.fetch_add(1, Ordering::Relaxed) + 1;
        self.log
            .lock()
            .expect("record log")
            .push(Record::Attach { lease });
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

/// The stream incarnation `input` would hand its running task right now.
fn current_stream(input: &BlocksInput) -> EventStream {
    EventStream {
        sink: Arc::clone(&input.sink),
        state: Arc::clone(&input.state),
        generation: lock(&input.state).stream_generation,
    }
}

fn touch_start(x: f32) -> BlocksEvent {
    serde_json::from_value(serde_json::json!({
        "type": "touch", "uid": PAD_UID, "action": "start", "index": 0,
        "x": x, "y": 0.5, "z": 0.25
    }))
    .expect("touch event decodes")
}

#[tokio::test]
async fn an_aborted_stream_cannot_publish_into_a_reconnected_device() {
    let sink = RecordingSink::default();
    // Nothing listens here; the spawned streams never get to run anyway,
    // because this test never yields to the runtime.
    let input = BlocksInput::new(
        PathBuf::from("/nonexistent/blocksd.sock"),
        Arc::new(sink.clone()),
    );
    let device = DeviceId::new();
    input.connect(PAD_UID, device, "Lightpad Block M");
    let aborted = current_stream(&input);

    // A fast disconnect and reconnect aborts the first stream and starts a
    // second, while the first still holds events it decoded before its abort
    // could land at an `.await`.
    input.disconnect(device);
    input.connect(PAD_UID, device, "Lightpad Block M");
    assert!(
        !aborted.handle_event(touch_start(0.9)),
        "the aborted stream learns it was superseded"
    );
    assert!(!aborted.reattach_all());
    assert!(!aborted.handle_event(BlocksEvent::DeviceRemoved { uid: PAD_UID }));
    assert_eq!(
        sink.records(),
        vec![
            Record::Attach { lease: 1 },
            Record::Drop { lease: 1 },
            Record::Attach { lease: 2 },
        ],
        "nothing from the aborted stream reaches the new lease"
    );

    let replacement = current_stream(&input);
    assert!(replacement.handle_event(touch_start(0.25)));
    assert_eq!(
        sink.records().last(),
        Some(&Record::Publish {
            lease: 2,
            edge: DeviceInputEdge::TouchBegan {
                contact: 0,
                position: TouchPosition {
                    x: 0.25,
                    y: 0.5,
                    pressure: 0.25,
                },
            },
        }),
        "the replacement stream still publishes"
    );
}

#[tokio::test]
async fn a_dropped_backend_retires_its_stream() {
    let sink = RecordingSink::default();
    let input = BlocksInput::new(
        PathBuf::from("/nonexistent/blocksd.sock"),
        Arc::new(sink.clone()),
    );
    input.connect(PAD_UID, DeviceId::new(), "Lightpad Block M");
    let aborted = current_stream(&input);

    // A replacement backend may already hold a lease for the same device,
    // which a stream that outlived its backend must not supersede.
    drop(input);
    assert!(!aborted.reattach_all());
    assert!(!aborted.handle_event(touch_start(0.5)));
    assert_eq!(
        sink.records(),
        vec![Record::Attach { lease: 1 }],
        "the stream of a dropped backend attaches and publishes nothing"
    );
}
