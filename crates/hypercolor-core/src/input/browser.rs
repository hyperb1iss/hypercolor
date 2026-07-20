//! Browser-preview input injection.
//!
//! A focused effect preview in the web UI can drive an effect's input
//! without any host-capture permission: the browser posts pointer and key
//! edges over an authorized WebSocket message, and this source folds them
//! into the same [`InteractionData`] contract the host backends produce.
//!
//! Injected sources are always per-connection: the caller assigns a stable
//! `source_id` per socket so browser pointers never implicitly merge with
//! each other or with host input.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::input::input_mono_ms;
use crate::input::traits::{InputData, InputSource, InteractionData, MotionAggregate, PointerMode};
use crate::types::event::{InputButtonState, InputEvent, TimedInputEvent};

const DEFAULT_EVENT_LIMIT: usize = 256;

/// One injected edge from a browser preview, already normalized.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserInputEdge {
    /// A key changed state. `key` is a browser-style code (`"a"`, `"Space"`).
    Key {
        key: String,
        state: InputButtonState,
    },
    /// A pointer button changed state (`"left"`, `"right"`, `"middle"`).
    Button {
        button: String,
        state: InputButtonState,
    },
    /// The pointer moved to a normalized `[0, 1]²` position.
    Move { norm_x: f32, norm_y: f32 },
    /// The wheel moved, in 1/120-notch hi-res units.
    Wheel { delta_hi_res: i32 },
}

#[derive(Default)]
struct SharedState {
    events: Vec<TimedInputEvent>,
    dropped: u32,
    pressed_keys: BTreeMap<String, BTreeSet<String>>,
    held_buttons: BTreeMap<String, BTreeSet<String>>,
    recent_keys: Vec<String>,
    cursor: BTreeMap<String, (f32, f32)>,
    /// Source id of the most recently moved pointer, so the primary pointer
    /// tracks real activity rather than lexical source order.
    active_pointer: Option<String>,
    motion: MotionAggregate,
    generation_dirty: bool,
}

impl SharedState {
    fn union_pressed(&self) -> Vec<String> {
        let mut keys: BTreeSet<&String> = BTreeSet::new();
        for held in self.pressed_keys.values() {
            keys.extend(held.iter());
        }
        keys.into_iter().cloned().collect()
    }

    fn union_buttons(&self) -> Vec<String> {
        let mut buttons: BTreeSet<&String> = BTreeSet::new();
        for held in self.held_buttons.values() {
            buttons.extend(held.iter());
        }
        buttons.into_iter().cloned().collect()
    }

    fn primary_cursor(&self) -> Option<(f32, f32)> {
        // The pointer that moved most recently, falling back to any known
        // cursor so a still-but-present pointer still reports a position.
        self.active_pointer
            .as_ref()
            .and_then(|source| self.cursor.get(source))
            .or_else(|| self.cursor.values().next_back())
            .copied()
    }
}

/// Cloneable push handle for a [`BrowserInputSource`].
///
/// Held by the WebSocket layer; every clone feeds the same source, so a
/// socket can inject without touching the input-manager lock.
#[derive(Clone)]
pub struct BrowserInputHandle {
    shared: Arc<Mutex<SharedState>>,
    event_limit: usize,
}

impl BrowserInputHandle {
    /// Inject a batch of edges from one browser connection.
    ///
    /// `source_id` scopes held state to the connection so releases and
    /// pointer positions never cross-contaminate between sockets.
    pub fn inject(&self, source_id: &str, edges: impl IntoIterator<Item = BrowserInputEdge>) {
        let Ok(mut guard) = self.shared.lock() else {
            return;
        };
        let at_ms = input_mono_ms();
        for edge in edges {
            self.fold(&mut guard, source_id, edge, at_ms);
        }
    }

    /// Drop all state a disconnecting connection still held.
    ///
    /// Synthesizes release edges for its keys and buttons so effects see
    /// clean edges and nothing sticks after the socket closes.
    pub fn release_source(&self, source_id: &str) {
        let Ok(mut guard) = self.shared.lock() else {
            return;
        };
        let at_ms = input_mono_ms();
        if let Some(keys) = guard.pressed_keys.remove(source_id) {
            for key in keys {
                push_event(
                    &mut guard,
                    TimedInputEvent {
                        event: InputEvent::Key {
                            source_id: source_id.to_owned(),
                            key,
                            state: InputButtonState::Released,
                        },
                        at_ms,
                        seq: 0,
                    },
                    self.event_limit,
                );
            }
        }
        if let Some(buttons) = guard.held_buttons.remove(source_id) {
            for button in buttons {
                push_event(
                    &mut guard,
                    TimedInputEvent {
                        event: InputEvent::MouseButton {
                            source_id: source_id.to_owned(),
                            button,
                            state: InputButtonState::Released,
                        },
                        at_ms,
                        seq: 0,
                    },
                    self.event_limit,
                );
            }
        }
        guard.cursor.remove(source_id);
        if guard.active_pointer.as_deref() == Some(source_id) {
            guard.active_pointer = guard.cursor.keys().next_back().cloned();
        }
        guard.generation_dirty = true;
    }

    fn fold(&self, state: &mut SharedState, source_id: &str, edge: BrowserInputEdge, at_ms: u64) {
        match edge {
            BrowserInputEdge::Key { key, state: st } => {
                match st {
                    InputButtonState::Pressed => {
                        state
                            .pressed_keys
                            .entry(source_id.to_owned())
                            .or_default()
                            .insert(key.clone());
                        state.recent_keys.push(key.clone());
                        cap_recent(&mut state.recent_keys, self.event_limit);
                    }
                    InputButtonState::Released => {
                        if let Some(held) = state.pressed_keys.get_mut(source_id) {
                            held.remove(&key);
                        }
                    }
                    InputButtonState::Repeated => {}
                }
                state.generation_dirty = true;
                push_event(
                    state,
                    TimedInputEvent {
                        event: InputEvent::Key {
                            source_id: source_id.to_owned(),
                            key,
                            state: st,
                        },
                        at_ms,
                        seq: 0,
                    },
                    self.event_limit,
                );
            }
            BrowserInputEdge::Button { button, state: st } => {
                match st {
                    InputButtonState::Pressed => {
                        state
                            .held_buttons
                            .entry(source_id.to_owned())
                            .or_default()
                            .insert(button.clone());
                    }
                    InputButtonState::Released => {
                        if let Some(held) = state.held_buttons.get_mut(source_id) {
                            held.remove(&button);
                        }
                    }
                    InputButtonState::Repeated => {}
                }
                state.generation_dirty = true;
                push_event(
                    state,
                    TimedInputEvent {
                        event: InputEvent::MouseButton {
                            source_id: source_id.to_owned(),
                            button,
                            state: st,
                        },
                        at_ms,
                        seq: 0,
                    },
                    self.event_limit,
                );
            }
            BrowserInputEdge::Move { norm_x, norm_y } => {
                let nx = sanitize_unit(norm_x);
                let ny = sanitize_unit(norm_y);
                if let Some((px, py)) = state.cursor.get(source_id).copied() {
                    let dx = nx - px;
                    let dy = ny - py;
                    state.motion.dx += dx;
                    state.motion.dy += dy;
                    state.motion.distance += dx.hypot(dy);
                }
                state.cursor.insert(source_id.to_owned(), (nx, ny));
                state.active_pointer = Some(source_id.to_owned());
                state.generation_dirty = true;
            }
            BrowserInputEdge::Wheel { delta_hi_res } => {
                push_event(
                    state,
                    TimedInputEvent {
                        event: InputEvent::MouseWheel {
                            source_id: source_id.to_owned(),
                            delta_hi_res,
                        },
                        at_ms,
                        seq: 0,
                    },
                    self.event_limit,
                );
            }
        }
    }
}

/// Push-based interaction source fed by browser previews.
pub struct BrowserInputSource {
    name: String,
    running: bool,
    generation: u64,
    shared: Arc<Mutex<SharedState>>,
    event_limit: usize,
}

impl BrowserInputSource {
    /// Create a browser injection source.
    #[must_use]
    pub fn new() -> Self {
        Self {
            name: "BrowserInput".to_owned(),
            running: false,
            generation: 0,
            shared: Arc::new(Mutex::new(SharedState::default())),
            event_limit: DEFAULT_EVENT_LIMIT,
        }
    }

    fn build_snapshot(&mut self, guard: &mut SharedState) -> InteractionData {
        let mut data = InteractionData::default();
        data.keyboard.pressed_keys = guard.union_pressed();
        data.keyboard.recent_keys = std::mem::take(&mut guard.recent_keys);
        data.mouse.buttons = guard.union_buttons();
        data.mouse.down = !data.mouse.buttons.is_empty();
        if let Some((nx, ny)) = guard.primary_cursor() {
            data.mouse.mode = PointerMode::Absolute;
            data.mouse.norm_x = nx;
            data.mouse.norm_y = ny;
        }
        data.batch.motion = std::mem::take(&mut guard.motion);
        data.batch.dropped_events = std::mem::take(&mut guard.dropped);

        if guard.generation_dirty || !data.keyboard.recent_keys.is_empty() {
            self.generation = self.generation.wrapping_add(1);
            guard.generation_dirty = false;
        }
        data.generation = self.generation;
        data
    }

    /// A cloneable handle for the WebSocket layer to push edges through.
    #[must_use]
    pub fn handle(&self) -> BrowserInputHandle {
        BrowserInputHandle {
            shared: Arc::clone(&self.shared),
            event_limit: self.event_limit,
        }
    }
}

impl Default for BrowserInputSource {
    fn default() -> Self {
        Self::new()
    }
}

impl InputSource for BrowserInputSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        if let Ok(mut guard) = self.shared.lock() {
            *guard = SharedState::default();
        }
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if !self.running {
            return Ok(InputData::None);
        }

        let shared = Arc::clone(&self.shared);
        let Ok(mut guard) = shared.lock() else {
            return Ok(InputData::None);
        };
        let snapshot = self.build_snapshot(&mut guard);
        Ok(InputData::Interaction(snapshot))
    }

    fn sample_and_drain_with_delta_secs(
        &mut self,
        _delta_secs: f32,
    ) -> (anyhow::Result<InputData>, Vec<TimedInputEvent>) {
        if !self.running {
            return (Ok(InputData::None), Vec::new());
        }

        let shared = Arc::clone(&self.shared);
        let Ok(mut guard) = shared.lock() else {
            return (Ok(InputData::None), Vec::new());
        };
        // One lock for both halves so batch and held state stay coherent.
        let events = std::mem::take(&mut guard.events);
        let snapshot = self.build_snapshot(&mut guard);
        (Ok(InputData::Interaction(snapshot)), events)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_interaction_source(&self) -> bool {
        true
    }

    fn interaction_diagnostics(&self) -> Option<crate::input::InteractionDiagnostics> {
        Some(crate::input::InteractionDiagnostics {
            backend: "browser",
            host_capture: false,
            capturing: self.running,
            devices_opened: 0,
            devices_denied: 0,
        })
    }

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        if let Ok(mut guard) = self.shared.lock() {
            return std::mem::take(&mut guard.events);
        }
        Vec::new()
    }
}

fn push_event(state: &mut SharedState, event: TimedInputEvent, limit: usize) {
    state.events.push(event);
    if state.events.len() > limit {
        let overflow = state.events.len() - limit;
        state.events.drain(..overflow);
        state.dropped = state
            .dropped
            .saturating_add(u32::try_from(overflow).unwrap_or(u32::MAX));
    }
}

fn cap_recent(recent: &mut Vec<String>, limit: usize) {
    if recent.len() > limit {
        let overflow = recent.len() - limit;
        recent.drain(..overflow);
    }
}

fn sanitize_unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain_snapshot(source: &mut BrowserInputSource) -> InteractionData {
        match source.sample().expect("sample") {
            InputData::Interaction(data) => data,
            _ => panic!("expected interaction data"),
        }
    }

    #[test]
    fn injected_keys_fold_into_pressed_state_and_events() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        let handle = source.handle();

        handle.inject(
            "browser-1",
            [BrowserInputEdge::Key {
                key: "a".into(),
                state: InputButtonState::Pressed,
            }],
        );

        let events = source.drain_events();
        assert_eq!(events.len(), 1);
        let data = drain_snapshot(&mut source);
        assert_eq!(data.keyboard.pressed_keys, vec!["a".to_owned()]);
        assert_eq!(data.keyboard.recent_keys, vec!["a".to_owned()]);

        handle.inject(
            "browser-1",
            [BrowserInputEdge::Key {
                key: "a".into(),
                state: InputButtonState::Released,
            }],
        );
        let _ = source.drain_events();
        let data = drain_snapshot(&mut source);
        assert!(data.keyboard.pressed_keys.is_empty());
    }

    #[test]
    fn per_connection_state_does_not_cross_contaminate() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        let handle = source.handle();

        handle.inject(
            "browser-a",
            [BrowserInputEdge::Key {
                key: "x".into(),
                state: InputButtonState::Pressed,
            }],
        );
        handle.inject(
            "browser-b",
            [BrowserInputEdge::Key {
                key: "x".into(),
                state: InputButtonState::Pressed,
            }],
        );
        handle.release_source("browser-a");

        let _ = source.drain_events();
        let data = drain_snapshot(&mut source);
        assert_eq!(
            data.keyboard.pressed_keys,
            vec!["x".to_owned()],
            "releasing one connection keeps the other's hold"
        );
    }

    #[test]
    fn pointer_move_sets_absolute_position_and_motion() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        let handle = source.handle();

        handle.inject(
            "browser-1",
            [
                BrowserInputEdge::Move {
                    norm_x: 0.2,
                    norm_y: 0.2,
                },
                BrowserInputEdge::Move {
                    norm_x: 0.5,
                    norm_y: 0.6,
                },
            ],
        );

        let data = drain_snapshot(&mut source);
        assert_eq!(data.mouse.mode, PointerMode::Absolute);
        assert!((data.mouse.norm_x - 0.5).abs() < 1e-6);
        assert!(data.batch.motion.distance > 0.0);
    }

    #[test]
    fn wheel_edges_carry_hi_res_delta() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        let handle = source.handle();

        handle.inject(
            "browser-1",
            [BrowserInputEdge::Wheel { delta_hi_res: -240 }],
        );
        let events = source.drain_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].event,
            InputEvent::MouseWheel {
                delta_hi_res: -240,
                ..
            }
        ));
    }

    #[test]
    fn stop_clears_all_state() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        source.handle().inject(
            "browser-1",
            [BrowserInputEdge::Key {
                key: "a".into(),
                state: InputButtonState::Pressed,
            }],
        );
        source.stop();
        source.start().expect("restart");
        let data = drain_snapshot(&mut source);
        assert!(data.keyboard.pressed_keys.is_empty());
    }
}
