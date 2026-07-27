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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::input::input_mono_ms;
use crate::input::traits::{InputData, InputSource, InteractionData, MotionAggregate, PointerMode};
use crate::input::{SourceKind, SourceStatusHandle, SourceStatusReporter};
use crate::types::event::{InputButtonState, InputEvent, TimedInputEvent};

const DEFAULT_EVENT_LIMIT: usize = 256;
const MAX_HELD_KEYS_PER_SOURCE: usize = 128;
const MAX_HELD_BUTTONS_PER_SOURCE: usize = 16;

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
    events: VecDeque<TimedInputEvent>,
    dropped: u32,
    pressed_keys: BTreeMap<String, BTreeSet<String>>,
    held_buttons: BTreeMap<String, BTreeSet<String>>,
    recent_keys: VecDeque<String>,
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
                        physical_code: None,
                        repeat_count: 1,
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
                        physical_code: None,
                        repeat_count: 1,
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
                        if !try_press(
                            &mut state.pressed_keys,
                            source_id,
                            &key,
                            MAX_HELD_KEYS_PER_SOURCE,
                        ) {
                            state.dropped = state.dropped.saturating_add(1);
                            return;
                        }
                        state.recent_keys.push_back(key.clone());
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
                        physical_code: None,
                        repeat_count: 1,
                    },
                    self.event_limit,
                );
            }
            BrowserInputEdge::Button { button, state: st } => {
                match st {
                    InputButtonState::Pressed => {
                        if !try_press(
                            &mut state.held_buttons,
                            source_id,
                            &button,
                            MAX_HELD_BUTTONS_PER_SOURCE,
                        ) {
                            state.dropped = state.dropped.saturating_add(1);
                            return;
                        }
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
                        physical_code: None,
                        repeat_count: 1,
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
                        physical_code: None,
                        repeat_count: 1,
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
    status: SourceStatusReporter,
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
            status: SourceStatusReporter::new(
                "browser_input",
                SourceKind::Interaction,
                "browser",
                true,
                true,
                true,
            ),
        }
    }

    fn build_snapshot(&mut self, guard: &mut SharedState) -> InteractionData {
        let mut data = InteractionData::default();
        data.keyboard.pressed_keys = guard.union_pressed();
        data.keyboard.recent_keys = guard.recent_keys.drain(..).collect();
        data.mouse.buttons = guard.union_buttons();
        data.mouse.down = !data.mouse.buttons.is_empty();
        if let Some((nx, ny)) = guard.primary_cursor() {
            data.mouse.mode = PointerMode::Absolute;
            data.mouse.norm_x = nx;
            data.mouse.norm_y = ny;
            // The user is driving this pointer over the preview canvas, so it
            // outranks the host cursor in `merge_from`. Without the flag, host
            // capture — which registers first — would shadow it and an
            // interactive effect previewed in the UI would track the desktop.
            data.mouse.injected = true;
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
        if self.running {
            return Ok(());
        }
        self.running = true;
        if let Some(status) = self.status.begin_session()? {
            status.mark_event_driven_live_without_deadline(0);
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        if let Ok(mut guard) = self.shared.lock() {
            *guard = SharedState::default();
        }
        self.status.stop();
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
        let events = drain_events(&mut guard.events);
        let snapshot = self.build_snapshot(&mut guard);
        (Ok(InputData::Interaction(snapshot)), events)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
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
            degraded: None,
        })
    }

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        if let Ok(mut guard) = self.shared.lock() {
            return drain_events(&mut guard.events);
        }
        Vec::new()
    }
}

fn push_event(state: &mut SharedState, event: TimedInputEvent, limit: usize) {
    if limit == 0 {
        state.dropped = state
            .dropped
            .saturating_add(u32::try_from(state.events.len()).unwrap_or(u32::MAX))
            .saturating_add(1);
        state.events.clear();
        return;
    }
    while state.events.len() >= limit {
        state.events.pop_front();
        state.dropped = state.dropped.saturating_add(1);
    }
    state.events.push_back(event);
}

fn try_press(
    held: &mut BTreeMap<String, BTreeSet<String>>,
    source_id: &str,
    name: &str,
    limit: usize,
) -> bool {
    let entries = held.entry(source_id.to_owned()).or_default();
    entries.contains(name) || (entries.len() < limit && entries.insert(name.to_owned()))
}

fn cap_recent(recent: &mut VecDeque<String>, limit: usize) {
    while recent.len() > limit {
        recent.pop_front();
    }
}

fn drain_events(events: &mut VecDeque<TimedInputEvent>) -> Vec<TimedInputEvent> {
    events.drain(..).collect()
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
    fn bounded_rings_keep_newest_events_in_order_and_report_overflow() {
        let mut source = BrowserInputSource::new();
        source.event_limit = 4;
        source.start().expect("start");
        let handle = source.handle();

        handle.inject(
            "browser-1",
            (0..10).map(|delta_hi_res| BrowserInputEdge::Wheel { delta_hi_res }),
        );

        let (data, events) = source.sample_and_drain_with_delta_secs(0.0);
        let InputData::Interaction(data) = data.expect("sample") else {
            panic!("expected interaction data");
        };
        let deltas = events
            .into_iter()
            .map(|timed| match timed.event {
                InputEvent::MouseWheel { delta_hi_res, .. } => delta_hi_res,
                other => panic!("expected wheel event, got {other:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![6, 7, 8, 9]);
        assert_eq!(data.batch.dropped_events, 6);

        let next = drain_snapshot(&mut source);
        assert_eq!(next.batch.dropped_events, 0);
    }

    #[test]
    fn event_ring_enforces_a_lowered_or_zero_limit() {
        let mut state = SharedState::default();
        let event = TimedInputEvent {
            event: InputEvent::MouseWheel {
                source_id: "browser-1".to_owned(),
                delta_hi_res: 120,
            },
            at_ms: 1,
            seq: 0,
            physical_code: None,
            repeat_count: 1,
        };
        for _ in 0..4 {
            push_event(&mut state, event.clone(), 4);
        }

        push_event(&mut state, event.clone(), 2);
        assert_eq!(state.events.len(), 2);
        assert_eq!(state.dropped, 3);

        push_event(&mut state, event, 0);
        assert!(state.events.is_empty());
        assert_eq!(state.dropped, 6);
    }

    #[test]
    fn held_state_cardinality_is_bounded_per_source() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        let handle = source.handle();

        handle.inject(
            "browser-1",
            (0..=MAX_HELD_KEYS_PER_SOURCE).map(|index| BrowserInputEdge::Key {
                key: format!("key-{index}"),
                state: InputButtonState::Pressed,
            }),
        );

        let (data, events) = source.sample_and_drain_with_delta_secs(0.0);
        let InputData::Interaction(data) = data.expect("sample") else {
            panic!("expected interaction data");
        };
        assert_eq!(data.keyboard.pressed_keys.len(), MAX_HELD_KEYS_PER_SOURCE);
        assert_eq!(data.keyboard.recent_keys.len(), MAX_HELD_KEYS_PER_SOURCE);
        assert_eq!(events.len(), MAX_HELD_KEYS_PER_SOURCE);
        assert_eq!(data.batch.dropped_events, 1);
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
