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

/// Longest event line the reader buffers, excluding its newline.
///
/// The largest event on the subscribed categories is `device_added`, a few
/// hundred bytes of device metadata, and touches and buttons are smaller
/// still. 64 KiB leaves two orders of magnitude for payload growth while
/// bounding what a peer that never sends a newline can make this reader
/// hold.
const MAX_EVENT_LINE: usize = 64 * 1024;

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
    /// Incarnation of the stream task that owns the event connection, bumped
    /// whenever a stream starts or is aborted. An abort lands only at the
    /// stream's next `.await`, so an aborted stream can still hold a decoded
    /// event; it checks this under the lock and drops the event rather than
    /// publish into the leases of a device that reconnected meanwhile.
    stream_generation: u64,
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
                stream_generation: 0,
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
            state.stream_generation = state.stream_generation.wrapping_add(1);
            let stream = EventStream {
                sink: Arc::clone(&self.sink),
                state: Arc::clone(&self.state),
                generation: state.stream_generation,
            };
            *lock(&self.stream) = Some(tokio::spawn(run_stream(self.socket_path.clone(), stream)));
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
            state.stream_generation = state.stream_generation.wrapping_add(1);
            if let Some(task) = lock(&self.stream).take() {
                task.abort();
            }
        }
    }
}

impl Drop for BlocksInput {
    fn drop(&mut self) {
        // Retire the stream's generation too, so a stream mid-poll cannot
        // supersede a lease that a replacement backend attached for the
        // same device.
        let mut state = lock(&self.state);
        state.streaming = false;
        state.stream_generation = state.stream_generation.wrapping_add(1);
        if let Some(task) = lock(&self.stream).take() {
            task.abort();
        }
    }
}

/// One incarnation of the event stream.
///
/// Every state access goes through [`EventStream::lock_current`], so once a
/// newer stream (or an abort) has bumped the generation this one touches
/// nothing and exits at its next check.
struct EventStream {
    sink: Arc<dyn DeviceInputSink>,
    state: Arc<Mutex<InputState>>,
    generation: u64,
}

impl EventStream {
    /// Lock the shared state if this stream still owns the event connection.
    fn lock_current(&self) -> Option<MutexGuard<'_, InputState>> {
        let state = lock(&self.state);
        (state.stream_generation == self.generation).then_some(state)
    }

    /// Route one event. Returns `false` once this stream is superseded.
    fn handle_event(&self, event: BlocksEvent) -> bool {
        match event {
            BlocksEvent::Touch(touch) => self.publish(touch.uid, &touch_edge(&touch)),
            BlocksEvent::Button(button) => self.publish(button.uid, &button_edge(&button)),
            // A block that left the topology will never report its lifts.
            BlocksEvent::DeviceRemoved { uid } => {
                let Some(mut state) = self.lock_current() else {
                    return false;
                };
                if let Some(device) = state.devices.get_mut(&uid) {
                    reattach(&self.sink, device);
                }
                true
            }
            BlocksEvent::Error { message } => {
                warn!(%message, "blocksd closed the input stream");
                true
            }
            BlocksEvent::Subscribed { .. }
            | BlocksEvent::DeviceAdded { .. }
            | BlocksEvent::Other => true,
        }
    }

    /// Publish one edge for `uid`. Returns `false` once this stream is
    /// superseded.
    fn publish(&self, uid: u64, edge: &DeviceInputEdge) -> bool {
        let Some(state) = self.lock_current() else {
            return false;
        };
        // Blocks that were never adopted, or have disconnected, are ignored.
        if let Some(device) = state.devices.get(&uid) {
            device.publisher.publish(std::slice::from_ref(edge));
        }
        true
    }

    /// Supersede every lease so the host cancels whatever it held. Returns
    /// `false` once this stream is superseded.
    fn reattach_all(&self) -> bool {
        let Some(mut state) = self.lock_current() else {
            return false;
        };
        for device in state.devices.values_mut() {
            reattach(&self.sink, device);
        }
        true
    }
}

/// Follow blocksd events until no device remains connected, or until a newer
/// stream supersedes this one.
async fn run_stream(socket_path: PathBuf, stream: EventStream) {
    let mut backoff = RECONNECT_INITIAL;
    loop {
        {
            let Some(mut state) = stream.lock_current() else {
                return;
            };
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
                let mut warned_oversized = false;
                loop {
                    let current = match connection.next_item().await {
                        Ok(Some(StreamItem::Event(event))) => stream.handle_event(event),
                        Ok(Some(StreamItem::Undecodable(error))) => {
                            if !warned {
                                warn!(%error, "blocksd sent an undecodable input event");
                                warned = true;
                            }
                            // The skipped line could have been a touch end.
                            stream.reattach_all()
                        }
                        Ok(Some(StreamItem::Oversized)) => {
                            if !warned_oversized {
                                warn!(
                                    limit = MAX_EVENT_LINE,
                                    "blocksd sent an input event line over the length limit"
                                );
                                warned_oversized = true;
                            }
                            // So could a line too long to read.
                            stream.reattach_all()
                        }
                        Ok(None) => break,
                        Err(error) => {
                            debug!(%error, "blocksd input stream read failed");
                            break;
                        }
                    };
                    if !current {
                        return;
                    }
                }
            }
            Err(error) => debug!(%error, "blocksd input stream unavailable"),
        }
        // Touch ends may have been lost with the connection. A connection
        // that never opened delivered nothing, so there is nothing to cancel.
        if delivered && !stream.reattach_all() {
            return;
        }
        sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
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

/// Supersede a device's lease so the host cancels whatever it held.
fn reattach(sink: &Arc<dyn DeviceInputSink>, device: &mut InputDevice) {
    device.publisher = sink.attach(device.device_id, &device.label);
}

enum StreamItem {
    Event(BlocksEvent),
    Undecodable(serde_json::Error),
    /// A line longer than [`MAX_EVENT_LINE`], discarded through its newline.
    Oversized,
}

struct BlocksEventConnection {
    reader: BufReader<UnixStream>,
    line: Vec<u8>,
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
            line: Vec::with_capacity(512),
        })
    }

    /// Read the next line, or `None` once blocksd closes the stream.
    ///
    /// A line longer than [`MAX_EVENT_LINE`] is skipped through its newline
    /// without being buffered, so the stream stays framed and a peer that
    /// never sends a newline cannot grow the buffer without bound.
    ///
    /// # Errors
    ///
    /// Fails when the socket read fails.
    async fn next_item(&mut self) -> Result<Option<StreamItem>> {
        self.line.clear();
        let mut oversized = false;
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                // blocksd closed the stream. A partial last line is still
                // decoded.
                if self.line.is_empty() && !oversized {
                    return Ok(None);
                }
                break;
            }
            let newline = available.iter().position(|&byte| byte == b'\n');
            let content = newline.unwrap_or(available.len());
            if !oversized {
                if self.line.len() + content > MAX_EVENT_LINE {
                    oversized = true;
                    self.line.clear();
                } else {
                    self.line.extend_from_slice(&available[..content]);
                }
            }
            self.reader.consume(newline.map_or(content, |at| at + 1));
            if newline.is_some() {
                break;
            }
        }
        if oversized {
            return Ok(Some(StreamItem::Oversized));
        }
        Ok(Some(match serde_json::from_slice(&self.line) {
            Ok(event) => StreamItem::Event(event),
            Err(error) => StreamItem::Undecodable(error),
        }))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
