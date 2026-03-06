//! Host keyboard and mouse capture for interactive LightScript effects.
//!
//! The capture backend runs on a dedicated polling thread so the public input
//! source stays `Send` even when the platform device handle is not.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Context;
use device_query::{DeviceQuery, DeviceState, Keycode};
use tracing::warn;

use crate::input::traits::{InputData, InputSource, InteractionData, KeyboardData, MouseData};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const READY_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_RECENT_KEY_LIMIT: usize = 32;

#[derive(Default)]
struct SharedInteractionState {
    interaction: InteractionData,
}

/// Global host input source for LightScript keyboard and mouse helpers.
pub struct InteractionInput {
    name: String,
    running: bool,
    recent_key_limit: usize,
    shared: Arc<Mutex<SharedInteractionState>>,
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl InteractionInput {
    /// Create a new host input capture source.
    #[must_use]
    pub fn new() -> Self {
        Self {
            name: "HostInput".to_owned(),
            running: false,
            recent_key_limit: DEFAULT_RECENT_KEY_LIMIT,
            shared: Arc::new(Mutex::new(SharedInteractionState::default())),
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

impl InputSource for InteractionInput {
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
        let recent_key_limit = self.recent_key_limit;
        let source_name = self.name.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let worker = thread::Builder::new()
            .name("hypercolor-host-input".to_owned())
            .spawn(move || {
                let Some(device_state) = try_create_device_state() else {
                    warn!(
                        source = %source_name,
                        "Host input capture unavailable; interactive LightScript input will stay idle"
                    );
                    let _ = ready_tx.send(());
                    return;
                };

                let _ = ready_tx.send(());
                let mut previous_keys: Vec<Keycode> = Vec::new();

                while !stop_flag.load(Ordering::Acquire) {
                    let current_keys = sorted_keys(device_state.get_keys());
                    let mouse_state = device_state.get_mouse();

                    let recent_keys = current_keys
                        .iter()
                        .filter(|key| !previous_keys.contains(key))
                        .map(|key| canonical_key_name(*key))
                        .collect::<Vec<String>>();
                    previous_keys = current_keys.clone();

                    let keyboard = KeyboardData {
                        pressed_keys: current_keys
                            .into_iter()
                            .map(canonical_key_name)
                            .collect(),
                        recent_keys,
                    };
                    let mouse = mouse_data_from_state(mouse_state);

                    if let Ok(mut guard) = shared.lock() {
                        guard.interaction.keyboard.pressed_keys = keyboard.pressed_keys;
                        extend_recent_keys(
                            &mut guard.interaction.keyboard.recent_keys,
                            keyboard.recent_keys,
                            recent_key_limit,
                        );
                        guard.interaction.mouse = mouse;
                    }

                    thread::sleep(POLL_INTERVAL);
                }
            })
            .context("failed to spawn host input capture worker")?;

        ready_rx
            .recv_timeout(READY_TIMEOUT)
            .context("timed out waiting for host input capture worker readiness")?;

        self.worker = Some(worker);
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_worker();
        if let Ok(mut guard) = self.shared.lock() {
            guard.interaction = InteractionData::default();
        }
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if !self.running {
            return Ok(InputData::None);
        }

        let snapshot = if let Ok(mut guard) = self.shared.lock() {
            let mut snapshot = guard.interaction.clone();
            snapshot.keyboard.recent_keys =
                std::mem::take(&mut guard.interaction.keyboard.recent_keys);
            snapshot
        } else {
            InteractionData::default()
        };

        Ok(InputData::Interaction(snapshot))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl Default for InteractionInput {
    fn default() -> Self {
        Self::new()
    }
}

fn sorted_keys(mut keys: Vec<Keycode>) -> Vec<Keycode> {
    keys.sort_by_key(Keycode::to_string);
    keys
}

fn extend_recent_keys(target: &mut Vec<String>, mut recent: Vec<String>, limit: usize) {
    target.append(&mut recent);
    if target.len() > limit {
        let overflow = target.len() - limit;
        target.drain(..overflow);
    }
}

fn mouse_data_from_state(mouse_state: device_query::MouseState) -> MouseData {
    let buttons = mouse_state
        .button_pressed
        .iter()
        .enumerate()
        .filter_map(|(idx, pressed)| (*pressed).then(|| mouse_button_name(idx)))
        .collect::<Vec<String>>();
    let (x, y) = mouse_state.coords;
    MouseData {
        x,
        y,
        down: !buttons.is_empty(),
        buttons,
    }
}

fn mouse_button_name(index: usize) -> String {
    match index {
        1 => "left",
        2 => "middle",
        3 => "right",
        4 => "button4",
        5 => "button5",
        _ => "button",
    }
    .to_owned()
}

fn canonical_key_name(key: Keycode) -> String {
    match key {
        Keycode::Key0 => "0",
        Keycode::Key1 => "1",
        Keycode::Key2 => "2",
        Keycode::Key3 => "3",
        Keycode::Key4 => "4",
        Keycode::Key5 => "5",
        Keycode::Key6 => "6",
        Keycode::Key7 => "7",
        Keycode::Key8 => "8",
        Keycode::Key9 => "9",
        Keycode::A => "a",
        Keycode::B => "b",
        Keycode::C => "c",
        Keycode::D => "d",
        Keycode::E => "e",
        Keycode::F => "f",
        Keycode::G => "g",
        Keycode::H => "h",
        Keycode::I => "i",
        Keycode::J => "j",
        Keycode::K => "k",
        Keycode::L => "l",
        Keycode::M => "m",
        Keycode::N => "n",
        Keycode::O => "o",
        Keycode::P => "p",
        Keycode::Q => "q",
        Keycode::R => "r",
        Keycode::S => "s",
        Keycode::T => "t",
        Keycode::U => "u",
        Keycode::V => "v",
        Keycode::W => "w",
        Keycode::X => "x",
        Keycode::Y => "y",
        Keycode::Z => "z",
        Keycode::Up => "ArrowUp",
        Keycode::Down => "ArrowDown",
        Keycode::Left => "ArrowLeft",
        Keycode::Right => "ArrowRight",
        Keycode::LControl => "ControlLeft",
        Keycode::RControl => "ControlRight",
        Keycode::LShift => "ShiftLeft",
        Keycode::RShift => "ShiftRight",
        Keycode::LAlt => "AltLeft",
        Keycode::RAlt => "AltRight",
        Keycode::LMeta => "MetaLeft",
        Keycode::RMeta => "MetaRight",
        Keycode::Command => "MetaLeft",
        Keycode::RCommand => "MetaRight",
        Keycode::LOption => "AltLeft",
        Keycode::ROption => "AltRight",
        other => match other {
            Keycode::Grave => "`",
            Keycode::Minus => "-",
            Keycode::Equal => "=",
            Keycode::LeftBracket => "[",
            Keycode::RightBracket => "]",
            Keycode::BackSlash => "\\",
            Keycode::Semicolon => ";",
            Keycode::Apostrophe => "'",
            Keycode::Comma => ",",
            Keycode::Dot => ".",
            Keycode::Slash => "/",
            _ => return other.to_string(),
        },
    }
    .to_owned()
}

fn try_create_device_state() -> Option<DeviceState> {
    #[cfg(target_os = "linux")]
    {
        DeviceState::checked_new()
    }

    #[cfg(not(target_os = "linux"))]
    {
        std::panic::catch_unwind(DeviceState::new).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_query::Keycode;

    #[test]
    fn canonical_key_names_follow_browser_style_for_common_keys() {
        assert_eq!(canonical_key_name(Keycode::A), "a");
        assert_eq!(canonical_key_name(Keycode::Escape), "Escape");
        assert_eq!(canonical_key_name(Keycode::Left), "ArrowLeft");
        assert_eq!(canonical_key_name(Keycode::Space), "Space");
        assert_eq!(canonical_key_name(Keycode::LControl), "ControlLeft");
    }

    #[test]
    fn extend_recent_keys_caps_queue_size() {
        let mut recent = vec!["a".to_owned(), "b".to_owned()];
        extend_recent_keys(&mut recent, vec!["c".to_owned(), "d".to_owned()], 3);
        assert_eq!(recent, vec!["b", "c", "d"]);
    }

    #[test]
    fn mouse_state_maps_common_buttons() {
        let mouse = mouse_data_from_state(device_query::MouseState {
            coords: (12, 34),
            button_pressed: vec![false, true, false, true],
        });
        assert_eq!(mouse.x, 12);
        assert_eq!(mouse.y, 34);
        assert!(mouse.down);
        assert_eq!(mouse.buttons, vec!["left", "right"]);
    }
}
