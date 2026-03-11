//! Linux keyboard event capture via `/dev/input/event*`.
//!
//! This source complements [`InteractionInput`](super::InteractionInput): the
//! existing interaction source polls aggregate state for effect rendering,
//! while this source exposes a true event stream for keyboard-driven
//! automation, WebSocket subscribers, and future MIDI-style mappings.

use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Context;
use evdev::{Device, EventSummary, InputEvent as EvdevInputEvent, KeyCode, enumerate};
use tracing::{debug, trace, warn};

use crate::input::traits::{InputData, InputSource};
use crate::types::event::{InputButtonState, InputEvent};

const POLL_INTERVAL: Duration = Duration::from_millis(8);
const READY_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_EVENT_LIMIT: usize = 256;

#[derive(Default)]
struct SharedState {
    events: Vec<InputEvent>,
}

struct KeyboardDevice {
    source_id: String,
    label: String,
    device: Device,
}

/// Linux host keyboard event source backed by `evdev`.
pub struct EvdevKeyboardInput {
    name: String,
    running: bool,
    event_limit: usize,
    shared: Arc<Mutex<SharedState>>,
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl EvdevKeyboardInput {
    /// Create a new `evdev` keyboard event source.
    #[must_use]
    pub fn new() -> Self {
        Self {
            name: "EvdevKeyboard".to_owned(),
            running: false,
            event_limit: DEFAULT_EVENT_LIMIT,
            shared: Arc::new(Mutex::new(SharedState::default())),
            stop_flag: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    fn stop_worker(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl InputSource for EvdevKeyboardInput {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }

        self.stop_flag.store(false, Ordering::Release);
        let shared = Arc::clone(&self.shared);
        let stop_flag = Arc::clone(&self.stop_flag);
        let event_limit = self.event_limit;
        let source_name = self.name.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let worker = thread::Builder::new()
            .name("hypercolor-evdev-input".to_owned())
            .spawn(move || {
                let mut devices = discover_keyboard_devices();
                if devices.is_empty() {
                    warn!(
                        source = %source_name,
                        "No readable evdev keyboard devices found; host keyboard events will stay idle"
                    );
                } else {
                    debug!(
                        source = %source_name,
                        devices = ?devices.iter().map(|device| device.label.as_str()).collect::<Vec<_>>(),
                        "Opened evdev keyboard devices"
                    );
                }
                let _ = ready_tx.send(());

                while !stop_flag.load(Ordering::Acquire) {
                    let mut stale = Vec::new();
                    let mut pending_events = Vec::new();

                    for (idx, device) in devices.iter_mut().enumerate() {
                        match device.device.fetch_events() {
                            Ok(events) => {
                                pending_events.extend(
                                    events.filter_map(|event| map_keyboard_event(&device.source_id, event)),
                                );
                            }
                            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                            Err(error) => {
                                warn!(
                                    source = %source_name,
                                    device = %device.label,
                                    %error,
                                    "Evdev keyboard device stopped producing events"
                                );
                                stale.push(idx);
                            }
                        }
                    }

                    for idx in stale.into_iter().rev() {
                        devices.remove(idx);
                    }

                    if !pending_events.is_empty()
                        && let Ok(mut guard) = shared.lock()
                    {
                        extend_events(&mut guard.events, pending_events, event_limit);
                    }

                    thread::sleep(POLL_INTERVAL);
                }
            })
            .context("failed to spawn evdev keyboard input worker")?;

        ready_rx
            .recv_timeout(READY_TIMEOUT)
            .context("timed out waiting for evdev keyboard worker readiness")?;

        self.worker = Some(worker);
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_worker();
        if let Ok(mut guard) = self.shared.lock() {
            guard.events.clear();
        }
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn drain_events(&mut self) -> Vec<InputEvent> {
        if let Ok(mut guard) = self.shared.lock() {
            return std::mem::take(&mut guard.events);
        }

        Vec::new()
    }
}

impl Default for EvdevKeyboardInput {
    fn default() -> Self {
        Self::new()
    }
}

fn discover_keyboard_devices() -> Vec<KeyboardDevice> {
    enumerate()
        .filter_map(|(path, device)| {
            if !is_keyboard_device(&device) {
                return None;
            }

            if let Err(error) = device.set_nonblocking(true) {
                warn!(
                    path = %path.display(),
                    %error,
                    "Skipping evdev keyboard device because nonblocking mode failed"
                );
                return None;
            }

            Some(KeyboardDevice {
                source_id: path.display().to_string(),
                label: keyboard_label(&path, &device),
                device,
            })
        })
        .collect()
}

fn is_keyboard_device(device: &Device) -> bool {
    let Some(keys) = device.supported_keys() else {
        return false;
    };

    let looks_like_keyboard = keys.contains(KeyCode::KEY_A)
        || keys.contains(KeyCode::KEY_Z)
        || keys.contains(KeyCode::KEY_ENTER)
        || keys.contains(KeyCode::KEY_SPACE);

    let looks_like_pointer = device.supported_relative_axes().is_some_and(|axes| {
        axes.contains(evdev::RelativeAxisCode::REL_X)
            || axes.contains(evdev::RelativeAxisCode::REL_Y)
    });

    looks_like_keyboard && !looks_like_pointer
}

fn keyboard_label(path: &Path, device: &Device) -> String {
    device.name().map_or_else(
        || path.display().to_string(),
        |name| format!("{name} ({})", path.display()),
    )
}

fn map_keyboard_event(source_id: &str, event: EvdevInputEvent) -> Option<InputEvent> {
    let EventSummary::Key(_, code, value) = event.destructure() else {
        return None;
    };

    let state = match value {
        0 => InputButtonState::Released,
        1 => InputButtonState::Pressed,
        2 => InputButtonState::Repeated,
        _ => {
            trace!(source_id, code = ?code, value, "Ignoring unsupported evdev key event value");
            return None;
        }
    };

    Some(InputEvent::Key {
        source_id: source_id.to_owned(),
        key: canonical_evdev_key_name(code),
        state,
    })
}

fn extend_events(target: &mut Vec<InputEvent>, mut recent: Vec<InputEvent>, limit: usize) {
    target.append(&mut recent);
    if target.len() > limit {
        let overflow = target.len() - limit;
        target.drain(..overflow);
    }
}

fn canonical_evdev_key_name(code: KeyCode) -> String {
    match code {
        KeyCode::KEY_0 => "0".to_owned(),
        KeyCode::KEY_1 => "1".to_owned(),
        KeyCode::KEY_2 => "2".to_owned(),
        KeyCode::KEY_3 => "3".to_owned(),
        KeyCode::KEY_4 => "4".to_owned(),
        KeyCode::KEY_5 => "5".to_owned(),
        KeyCode::KEY_6 => "6".to_owned(),
        KeyCode::KEY_7 => "7".to_owned(),
        KeyCode::KEY_8 => "8".to_owned(),
        KeyCode::KEY_9 => "9".to_owned(),
        KeyCode::KEY_A => "a".to_owned(),
        KeyCode::KEY_B => "b".to_owned(),
        KeyCode::KEY_C => "c".to_owned(),
        KeyCode::KEY_D => "d".to_owned(),
        KeyCode::KEY_E => "e".to_owned(),
        KeyCode::KEY_F => "f".to_owned(),
        KeyCode::KEY_G => "g".to_owned(),
        KeyCode::KEY_H => "h".to_owned(),
        KeyCode::KEY_I => "i".to_owned(),
        KeyCode::KEY_J => "j".to_owned(),
        KeyCode::KEY_K => "k".to_owned(),
        KeyCode::KEY_L => "l".to_owned(),
        KeyCode::KEY_M => "m".to_owned(),
        KeyCode::KEY_N => "n".to_owned(),
        KeyCode::KEY_O => "o".to_owned(),
        KeyCode::KEY_P => "p".to_owned(),
        KeyCode::KEY_Q => "q".to_owned(),
        KeyCode::KEY_R => "r".to_owned(),
        KeyCode::KEY_S => "s".to_owned(),
        KeyCode::KEY_T => "t".to_owned(),
        KeyCode::KEY_U => "u".to_owned(),
        KeyCode::KEY_V => "v".to_owned(),
        KeyCode::KEY_W => "w".to_owned(),
        KeyCode::KEY_X => "x".to_owned(),
        KeyCode::KEY_Y => "y".to_owned(),
        KeyCode::KEY_Z => "z".to_owned(),
        KeyCode::KEY_UP => "ArrowUp".to_owned(),
        KeyCode::KEY_DOWN => "ArrowDown".to_owned(),
        KeyCode::KEY_LEFT => "ArrowLeft".to_owned(),
        KeyCode::KEY_RIGHT => "ArrowRight".to_owned(),
        KeyCode::KEY_LEFTCTRL => "ControlLeft".to_owned(),
        KeyCode::KEY_RIGHTCTRL => "ControlRight".to_owned(),
        KeyCode::KEY_LEFTSHIFT => "ShiftLeft".to_owned(),
        KeyCode::KEY_RIGHTSHIFT => "ShiftRight".to_owned(),
        KeyCode::KEY_LEFTALT => "AltLeft".to_owned(),
        KeyCode::KEY_RIGHTALT => "AltRight".to_owned(),
        KeyCode::KEY_LEFTMETA => "MetaLeft".to_owned(),
        KeyCode::KEY_RIGHTMETA => "MetaRight".to_owned(),
        KeyCode::KEY_SPACE => "Space".to_owned(),
        KeyCode::KEY_ENTER => "Enter".to_owned(),
        KeyCode::KEY_ESC => "Escape".to_owned(),
        KeyCode::KEY_TAB => "Tab".to_owned(),
        KeyCode::KEY_BACKSPACE => "Backspace".to_owned(),
        KeyCode::KEY_MINUS => "-".to_owned(),
        KeyCode::KEY_EQUAL => "=".to_owned(),
        KeyCode::KEY_LEFTBRACE => "[".to_owned(),
        KeyCode::KEY_RIGHTBRACE => "]".to_owned(),
        KeyCode::KEY_BACKSLASH => "\\".to_owned(),
        KeyCode::KEY_SEMICOLON => ";".to_owned(),
        KeyCode::KEY_APOSTROPHE => "'".to_owned(),
        KeyCode::KEY_GRAVE => "`".to_owned(),
        KeyCode::KEY_COMMA => ",".to_owned(),
        KeyCode::KEY_DOT => ".".to_owned(),
        KeyCode::KEY_SLASH => "/".to_owned(),
        other => format!("{other:?}"),
    }
}
