//! Touch and button input from blocksd.
//!
//! Input travels on its own subscribe-only connection. The frame connection
//! pairs each request with the next line or byte it reads, so an event
//! interleaved there would be misread as a frame acknowledgement.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_driver_api::{DeviceInputPublisher, DeviceInputSink};
use hypercolor_types::device::DeviceId;
use hypercolor_types::device_input::{DeviceInputEdge, TouchPosition};
use hypercolor_types::event::InputButtonState;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tracing::{debug, warn};

use super::types::{BlocksButton, BlocksButtonAction, BlocksEvent, BlocksTouch, BlocksTouchAction};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const RECONNECT_INITIAL: Duration = Duration::from_millis(250);
const RECONNECT_MAX: Duration = Duration::from_secs(5);
const SUBSCRIBE_REQUEST: &[u8] =
    b"{\"type\":\"subscribe\",\"events\":[\"device\",\"touch\",\"button\"]}\n";

/// Input side of one Blocks backend.
///
/// Holds a publisher lease for every connected device and runs the event
/// stream while any device is connected.
pub(super) struct BlocksInput {
    socket_path: PathBuf,
    sink: Arc<dyn DeviceInputSink>,
    /// Lock order: `state`, then `stream`.
    state: Arc<Mutex<InputState>>,
    stream: Mutex<Option<JoinHandle<()>>>,
}

struct InputState {
    devices: HashMap<u64, InputDevice>,
    /// Whether a stream task owns the event connection. Changed only under
    /// this lock, so a connect racing the stream's exit always restarts it.
    streaming: bool,
}

struct InputDevice {
    device_id: DeviceId,
    label: Arc<str>,
    publisher: Box<dyn DeviceInputPublisher>,
}

impl BlocksInput {
    pub(super) fn new(socket_path: PathBuf, sink: Arc<dyn DeviceInputSink>) -> Self {
        Self {
            socket_path,
            sink,
            state: Arc::new(Mutex::new(InputState {
                devices: HashMap::new(),
                streaming: false,
            })),
            stream: Mutex::new(None),
        }
    }

    /// Start publishing input for a connected device.
    ///
    /// Only devices the host adopted and connected are attached, under the
    /// host's canonical `DeviceId`, so input never lands on an id no tracked
    /// device has.
    pub(super) fn connect(&self, uid: u64, device_id: DeviceId, label: &str) {
        let mut state = lock(&self.state);
        let already_attached = state
            .devices
            .get(&uid)
            .is_some_and(|device| device.device_id == device_id);
        if !already_attached {
            let label: Arc<str> = Arc::from(label);
            let publisher = self.sink.attach(device_id, &label);
            state.devices.insert(
                uid,
                InputDevice {
                    device_id,
                    label,
                    publisher,
                },
            );
        }
        if !state.streaming {
            state.streaming = true;
            *lock(&self.stream) = Some(tokio::spawn(run_stream(
                self.socket_path.clone(),
                Arc::clone(&self.sink),
                Arc::clone(&self.state),
            )));
        }
    }

    /// Stop publishing input for a device, cancelling anything it held, and
    /// close the event connection once no device remains.
    pub(super) fn disconnect(&self, device_id: DeviceId) {
        let mut state = lock(&self.state);
        state
            .devices
            .retain(|_, device| device.device_id != device_id);
        if state.devices.is_empty() && state.streaming {
            state.streaming = false;
            if let Some(task) = lock(&self.stream).take() {
                task.abort();
            }
        }
    }
}

impl Drop for BlocksInput {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.stream).take() {
            task.abort();
        }
    }
}

/// Follow blocksd events until no device remains connected.
async fn run_stream(
    socket_path: PathBuf,
    sink: Arc<dyn DeviceInputSink>,
    state: Arc<Mutex<InputState>>,
) {
    let mut backoff = RECONNECT_INITIAL;
    loop {
        {
            let mut state = lock(&state);
            if state.devices.is_empty() {
                state.streaming = false;
                return;
            }
        }
        let mut delivered = false;
        match BlocksEventConnection::subscribe(&socket_path).await {
            Ok(mut connection) => {
                debug!(path = %socket_path.display(), "blocksd input stream connected");
                backoff = RECONNECT_INITIAL;
                delivered = true;
                let mut warned = false;
                loop {
                    match connection.next_item().await {
                        Ok(Some(StreamItem::Event(event))) => handle_event(&sink, &state, event),
                        Ok(Some(StreamItem::Undecodable(error))) => {
                            if !warned {
                                warn!(%error, "blocksd sent an undecodable input event");
                                warned = true;
                            }
                            // The skipped line could have been a touch end.
                            reattach_all(&sink, &state);
                        }
                        Ok(None) => break,
                        Err(error) => {
                            debug!(%error, "blocksd input stream read failed");
                            break;
                        }
                    }
                }
            }
            Err(error) => debug!(%error, "blocksd input stream unavailable"),
        }
        // Touch ends may have been lost with the connection. A connection
        // that never opened delivered nothing, so there is nothing to cancel.
        if delivered {
            reattach_all(&sink, &state);
        }
        sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

fn handle_event(sink: &Arc<dyn DeviceInputSink>, state: &Mutex<InputState>, event: BlocksEvent) {
    match event {
        BlocksEvent::Touch(touch) => publish(state, touch.uid, touch_edge(&touch)),
        BlocksEvent::Button(button) => publish(state, button.uid, button_edge(&button)),
        // A block that left the topology will never report its lifts.
        BlocksEvent::DeviceRemoved { uid } => {
            if let Some(device) = lock(state).devices.get_mut(&uid) {
                reattach(sink, device);
            }
        }
        BlocksEvent::Error { message } => warn!(%message, "blocksd closed the input stream"),
        BlocksEvent::Subscribed { .. } | BlocksEvent::DeviceAdded { .. } | BlocksEvent::Other => {}
    }
}

fn publish(state: &Mutex<InputState>, uid: u64, edge: DeviceInputEdge) {
    // Blocks that were never adopted, or have disconnected, are ignored.
    if let Some(device) = lock(state).devices.get(&uid) {
        device.publisher.publish(std::slice::from_ref(&edge));
    }
}

fn touch_edge(touch: &BlocksTouch) -> DeviceInputEdge {
    let contact = u32::from(touch.index);
    let position = TouchPosition {
        x: touch.x,
        y: touch.y,
        pressure: touch.z,
    };
    match touch.action {
        BlocksTouchAction::Start => DeviceInputEdge::TouchBegan { contact, position },
        BlocksTouchAction::Move => DeviceInputEdge::TouchMoved { contact, position },
        BlocksTouchAction::End => DeviceInputEdge::TouchEnded { contact, position },
    }
}

fn button_edge(button: &BlocksButton) -> DeviceInputEdge {
    // Older daemons report neither the SDK function nor the protocol index,
    // so every button on such a block shares one name.
    let name: Arc<str> = match (&button.button, button.button_id) {
        (Some(name), _) => Arc::from(name.as_str()),
        (None, Some(id)) => Arc::from(format!("button{id}")),
        (None, None) => Arc::from("button"),
    };
    DeviceInputEdge::Button {
        button: name,
        state: match button.action {
            BlocksButtonAction::Press => InputButtonState::Pressed,
            BlocksButtonAction::Release => InputButtonState::Released,
        },
    }
}

fn reattach_all(sink: &Arc<dyn DeviceInputSink>, state: &Mutex<InputState>) {
    for device in lock(state).devices.values_mut() {
        reattach(sink, device);
    }
}

/// Supersede a device's lease so the host cancels whatever it held.
fn reattach(sink: &Arc<dyn DeviceInputSink>, device: &mut InputDevice) {
    device.publisher = sink.attach(device.device_id, &device.label);
}

enum StreamItem {
    Event(BlocksEvent),
    Undecodable(serde_json::Error),
}

struct BlocksEventConnection {
    reader: BufReader<UnixStream>,
    line: String,
}

impl BlocksEventConnection {
    async fn subscribe(path: &Path) -> Result<Self> {
        let mut stream = timeout(CONNECT_TIMEOUT, UnixStream::connect(path))
            .await
            .context("blocksd input connect timeout")?
            .with_context(|| format!("failed to connect to blocksd at {}", path.display()))?;
        stream
            .write_all(SUBSCRIBE_REQUEST)
            .await
            .context("blocksd subscribe failed")?;
        Ok(Self {
            reader: BufReader::new(stream),
            line: String::with_capacity(512),
        })
    }

    /// Read the next line, or `None` once blocksd closes the stream.
    ///
    /// # Errors
    ///
    /// Fails when the socket read fails.
    async fn next_item(&mut self) -> Result<Option<StreamItem>> {
        self.line.clear();
        if self.reader.read_line(&mut self.line).await? == 0 {
            return Ok(None);
        }
        Ok(Some(match serde_json::from_str(&self.line) {
            Ok(event) => StreamItem::Event(event),
            Err(error) => StreamItem::Undecodable(error),
        }))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
