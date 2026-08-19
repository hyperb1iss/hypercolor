//! Shared host keyboard and pointer state folding.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use hypercolor_types::event::{InputButtonState, InputEvent, PointerScrollUnit, TimedInputEvent};
use hypercolor_types::host_input::{
    HostInputBatch, HostInputCapabilities, HostInputDevice, HostInputEvent, HostKeyIdentity,
    HostKeySignal, HostPointerMotion, HostPointerSnapshot, HostRepeatEvidence,
};

use super::{
    InteractionBatch, InteractionData, LegacyWheelProjector, MotionAggregate, PointerMode,
};

/// Result of publishing a native batch into the active fold session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostInputPublishOutcome {
    Published,
    Stale,
}

/// Monotonic diagnostics produced by the shared fold.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostInputFoldDiagnostics {
    pub state_gaps: u64,
    pub impossible_key_edges: u64,
    pub impossible_button_edges: u64,
    pub coordinate_space_resets: u64,
    pub device_catalog_generation: u64,
}

/// One frame-ready snapshot and its ordered discrete events.
pub struct HostInputSample {
    pub interaction: InteractionData,
    pub events: Vec<TimedInputEvent>,
}

/// Session-scoped publisher handed to one native acquisition backend.
#[derive(Clone)]
pub struct HostInputSink {
    shared: Arc<Mutex<HostInputFoldState>>,
    source_id: Arc<str>,
    epoch: u64,
}

impl HostInputSink {
    /// Fold one ordered native publication into the active host state.
    #[must_use]
    pub fn publish(&self, batch: HostInputBatch<'_>) -> HostInputPublishOutcome {
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.epoch != self.epoch {
            return HostInputPublishOutcome::Stale;
        }
        state.fold_batch(&self.source_id, batch);
        HostInputPublishOutcome::Published
    }
}

/// Canonical held-state, repeat, pointer, gap, and snapshot fold.
pub struct HostInputFold {
    shared: Arc<Mutex<HostInputFoldState>>,
    generation: u64,
    last_state_key: Option<VisibleStateKey>,
}

impl HostInputFold {
    /// Construct a fold with a bounded final event history.
    #[must_use]
    pub fn new(event_capacity: usize) -> Self {
        Self {
            shared: Arc::new(Mutex::new(HostInputFoldState::new(event_capacity))),
            generation: 0,
            last_state_key: None,
        }
    }

    /// Start a new acquisition session and invalidate every older publisher.
    #[must_use]
    pub fn begin_session(
        &mut self,
        source_id: impl Into<Arc<str>>,
        capabilities: HostInputCapabilities,
    ) -> HostInputSink {
        let source_id = source_id.into();
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.epoch = state.epoch.wrapping_add(1).max(1);
        state.clear_session();
        state.capabilities = capabilities;
        let epoch = state.epoch;
        drop(state);
        self.last_state_key = None;
        HostInputSink {
            shared: Arc::clone(&self.shared),
            source_id,
            epoch,
        }
    }

    /// End the active session and make its publishers inert.
    pub fn end_session(&mut self) {
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.epoch = state.epoch.wrapping_add(1).max(1);
        state.clear_session();
        drop(state);
        self.last_state_key = None;
    }

    /// Drain transient state into one frame snapshot.
    #[must_use]
    pub fn sample_and_drain(&mut self) -> HostInputSample {
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut interaction = InteractionData::default();
        interaction.keyboard.pressed_keys = state.union_pressed_keys();
        interaction.keyboard.recent_keys = state
            .recent_keys
            .drain(..)
            .map(|key| key.to_string())
            .collect();
        interaction.mouse.buttons = state.union_pressed_buttons();
        interaction.mouse.down = !interaction.mouse.buttons.is_empty();
        state.apply_pointer_snapshot(&mut interaction);
        interaction.batch.motion = std::mem::take(&mut state.motion);
        interaction.batch.dropped_events = std::mem::take(&mut state.dropped_events);

        let state_key = VisibleStateKey::from_interaction(&interaction);
        if self.last_state_key.as_ref() != Some(&state_key)
            || !interaction.keyboard.recent_keys.is_empty()
        {
            self.generation = self.generation.wrapping_add(1);
            self.last_state_key = Some(state_key);
        }
        interaction.generation = self.generation;
        let events = state.events.drain(..).collect();
        HostInputSample {
            interaction,
            events,
        }
    }

    /// Read the current fold diagnostics without draining state.
    #[must_use]
    pub fn diagnostics(&self) -> HostInputFoldDiagnostics {
        self.shared.lock().map_or_else(
            |error| error.into_inner().diagnostics,
            |state| state.diagnostics,
        )
    }

    /// Number of native devices in the current catalog.
    #[must_use]
    pub fn device_count(&self) -> usize {
        self.shared.lock().map_or_else(
            |error| error.into_inner().devices.len(),
            |state| state.devices.len(),
        )
    }
}

impl Default for HostInputFold {
    fn default() -> Self {
        Self::new(InteractionBatch::MAX_EVENTS)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VisibleStateKey {
    keys: Vec<String>,
    buttons: Vec<String>,
    pointer_mode: PointerMode,
    pointer_x: i32,
    pointer_y: i32,
    pointer_norm_x: i32,
    pointer_norm_y: i32,
}

impl VisibleStateKey {
    fn from_interaction(interaction: &InteractionData) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::as_conversions,
            reason = "normalized coordinates are clamped before fixed-point conversion"
        )]
        let (pointer_norm_x, pointer_norm_y) = (
            (interaction.mouse.norm_x.clamp(0.0, 1.0) * 10_000.0) as i32,
            (interaction.mouse.norm_y.clamp(0.0, 1.0) * 10_000.0) as i32,
        );
        Self {
            keys: interaction.keyboard.pressed_keys.clone(),
            buttons: interaction.mouse.buttons.clone(),
            pointer_mode: interaction.mouse.mode,
            pointer_x: interaction.mouse.x,
            pointer_y: interaction.mouse.y,
            pointer_norm_x,
            pointer_norm_y,
        }
    }
}

struct HostInputFoldState {
    epoch: u64,
    capabilities: HostInputCapabilities,
    event_capacity: usize,
    events: VecDeque<TimedInputEvent>,
    dropped_events: u32,
    pressed_keys: BTreeMap<Arc<str>, BTreeSet<Arc<str>>>,
    recent_keys: VecDeque<Arc<str>>,
    pressed_buttons: BTreeMap<Arc<str>, BTreeSet<Arc<str>>>,
    devices: BTreeMap<Arc<str>, Arc<HostInputDevice>>,
    motion: MotionAggregate,
    virtual_cursor_x: f32,
    virtual_cursor_y: f32,
    pointer: Option<HostPointerSnapshot>,
    pointer_present: bool,
    coordinate_space_generation: Option<u64>,
    absolute_baselines: BTreeMap<Arc<str>, (f32, f32, u64)>,
    wheel_projectors: BTreeMap<Arc<str>, LegacyWheelProjector>,
    diagnostics: HostInputFoldDiagnostics,
}

impl HostInputFoldState {
    fn new(event_capacity: usize) -> Self {
        Self {
            epoch: 0,
            capabilities: HostInputCapabilities::default(),
            event_capacity,
            events: VecDeque::with_capacity(event_capacity),
            dropped_events: 0,
            pressed_keys: BTreeMap::new(),
            recent_keys: VecDeque::with_capacity(event_capacity),
            pressed_buttons: BTreeMap::new(),
            devices: BTreeMap::new(),
            motion: MotionAggregate::default(),
            virtual_cursor_x: 0.0,
            virtual_cursor_y: 0.0,
            pointer: None,
            pointer_present: false,
            coordinate_space_generation: None,
            absolute_baselines: BTreeMap::new(),
            wheel_projectors: BTreeMap::new(),
            diagnostics: HostInputFoldDiagnostics::default(),
        }
    }

    fn clear_session(&mut self) {
        self.events.clear();
        self.dropped_events = 0;
        self.pressed_keys.clear();
        self.recent_keys.clear();
        self.pressed_buttons.clear();
        self.devices.clear();
        self.motion = MotionAggregate::default();
        self.pointer = None;
        self.pointer_present = false;
        self.coordinate_space_generation = None;
        self.absolute_baselines.clear();
        self.wheel_projectors.clear();
        self.diagnostics = HostInputFoldDiagnostics::default();
    }

    fn fold_batch(&mut self, source_id: &Arc<str>, batch: HostInputBatch<'_>) {
        self.diagnostics.device_catalog_generation = batch.device_catalog_generation;
        if let Some(pointer) = batch.pointer {
            self.observe_coordinate_space(pointer.coordinate_space_generation);
            self.pointer = Some(pointer);
            self.pointer_present = true;
        }
        for event in batch.events {
            self.fold_event(source_id, event, batch.at_ms);
        }
        if !self.devices.is_empty() {
            self.pointer_present = self
                .devices
                .values()
                .any(|device| device.capabilities.pointer)
                || self.pointer.is_some();
        }
    }

    fn fold_event(&mut self, fallback_source: &Arc<str>, event: &HostInputEvent, at_ms: u64) {
        match event {
            HostInputEvent::Key {
                device,
                identity,
                signal,
            } => {
                let source_id = event_source_id(device.as_ref(), fallback_source);
                self.fold_key(source_id, identity, signal, at_ms);
            }
            HostInputEvent::Button {
                device,
                button,
                pressed,
                physical_code,
            } => {
                let source_id = event_source_id(device.as_ref(), fallback_source);
                self.fold_button(source_id, button.as_str(), *pressed, physical_code, at_ms);
            }
            HostInputEvent::Motion { device, motion } => {
                let source_id = event_source_id(device.as_ref(), fallback_source);
                self.fold_motion(source_id, *motion);
            }
            HostInputEvent::Scroll {
                device,
                delta_x_q16_16,
                delta_y_q16_16,
                unit,
                phase,
                momentum_phase,
                physical_code,
            } => {
                let source_id = event_source_id(device.as_ref(), fallback_source);
                self.fold_scroll(
                    source_id,
                    *delta_x_q16_16,
                    *delta_y_q16_16,
                    *unit,
                    *phase,
                    *momentum_phase,
                    physical_code,
                    at_ms,
                );
            }
            HostInputEvent::DeviceArrived { device } => {
                self.devices
                    .insert(Arc::clone(&device.source_id), Arc::clone(device));
                self.pointer_present |= device.capabilities.pointer;
                self.absolute_baselines.remove(&device.source_id);
                self.wheel_projectors.remove(&device.source_id);
            }
            HostInputEvent::StateGap { device, .. } => {
                self.diagnostics.state_gaps = self.diagnostics.state_gaps.saturating_add(1);
                if let Some(device) = device {
                    self.synthesize_releases(&device.source_id, at_ms);
                    self.absolute_baselines.remove(&device.source_id);
                    self.wheel_projectors.remove(&device.source_id);
                } else {
                    self.synthesize_all_releases(at_ms);
                    self.absolute_baselines.clear();
                    self.wheel_projectors.clear();
                }
            }
            HostInputEvent::DeviceRemoved { device } => {
                self.devices.remove(&device.source_id);
                self.synthesize_releases(&device.source_id, at_ms);
                self.absolute_baselines.remove(&device.source_id);
                self.wheel_projectors.remove(&device.source_id);
                self.pointer_present = self
                    .devices
                    .values()
                    .any(|entry| entry.capabilities.pointer)
                    || self.pointer.is_some();
            }
        }
    }

    fn fold_key(
        &mut self,
        source_id: &Arc<str>,
        identity: &HostKeyIdentity,
        signal: &HostKeySignal,
        at_ms: u64,
    ) {
        let held = self
            .pressed_keys
            .get(source_id)
            .is_some_and(|keys| keys.contains(&identity.key));
        let edge = match signal {
            HostKeySignal::Edge { pressed, repeat } => Some((*pressed, *repeat)),
            HostKeySignal::AggregateState {
                active,
                active_counterpart,
            } if *active != held => Some((*active, HostRepeatEvidence::NotRepeat)),
            HostKeySignal::AggregateState {
                active: true,
                active_counterpart: Some(counterpart),
            } if self
                .pressed_keys
                .get(source_id)
                .is_some_and(|keys| keys.contains(counterpart)) =>
            {
                Some((false, HostRepeatEvidence::NotRepeat))
            }
            HostKeySignal::AggregateState {
                active_counterpart: None,
                ..
            } => None,
            HostKeySignal::AggregateState { .. } => {
                self.diagnostics.impossible_key_edges =
                    self.diagnostics.impossible_key_edges.saturating_add(1);
                None
            }
        };
        let Some((pressed, repeat)) = edge else {
            return;
        };

        let state = if pressed {
            if held || repeat == HostRepeatEvidence::Repeat {
                if !held {
                    self.pressed_keys
                        .entry(Arc::clone(source_id))
                        .or_default()
                        .insert(Arc::clone(&identity.key));
                }
                InputButtonState::Repeated
            } else {
                self.pressed_keys
                    .entry(Arc::clone(source_id))
                    .or_default()
                    .insert(Arc::clone(&identity.key));
                self.recent_keys.push_back(Arc::clone(&identity.key));
                while self.recent_keys.len() > self.event_capacity {
                    self.recent_keys.pop_front();
                }
                InputButtonState::Pressed
            }
        } else {
            if !held {
                self.diagnostics.impossible_key_edges =
                    self.diagnostics.impossible_key_edges.saturating_add(1);
            }
            if let Some(keys) = self.pressed_keys.get_mut(source_id) {
                keys.remove(&identity.key);
            }
            InputButtonState::Released
        };

        self.push_event(TimedInputEvent {
            event: InputEvent::Key {
                source_id: source_id.to_string(),
                key: identity.key.to_string(),
                state,
            },
            at_ms,
            seq: 0,
            physical_code: Some(identity.physical_code.to_string()),
            repeat_count: 1,
        });
    }

    fn fold_button(
        &mut self,
        source_id: &Arc<str>,
        button: &str,
        pressed: bool,
        physical_code: &Arc<str>,
        at_ms: u64,
    ) {
        let button: Arc<str> = Arc::from(button);
        let changed = if pressed {
            self.pressed_buttons
                .entry(Arc::clone(source_id))
                .or_default()
                .insert(Arc::clone(&button))
        } else {
            self.pressed_buttons
                .get_mut(source_id)
                .is_some_and(|buttons| buttons.remove(&button))
        };
        if !changed {
            self.diagnostics.impossible_button_edges =
                self.diagnostics.impossible_button_edges.saturating_add(1);
        }
        self.push_event(TimedInputEvent {
            event: InputEvent::MouseButton {
                source_id: source_id.to_string(),
                button: button.to_string(),
                state: if pressed {
                    InputButtonState::Pressed
                } else {
                    InputButtonState::Released
                },
            },
            at_ms,
            seq: 0,
            physical_code: Some(physical_code.to_string()),
            repeat_count: 1,
        });
    }

    fn fold_motion(&mut self, source_id: &Arc<str>, motion: HostPointerMotion) {
        match motion {
            HostPointerMotion::Relative {
                delta_x,
                delta_y,
                units_per_x,
                units_per_y,
            } => {
                if !delta_x.is_finite()
                    || !delta_y.is_finite()
                    || !units_per_x.is_finite()
                    || !units_per_y.is_finite()
                    || units_per_x <= 0.0
                    || units_per_y <= 0.0
                {
                    return;
                }
                let dx = (delta_x / units_per_x) as f32;
                let dy = (delta_y / units_per_y) as f32;
                self.virtual_cursor_x = (self.virtual_cursor_x + dx).clamp(0.0, 1.0);
                self.virtual_cursor_y = (self.virtual_cursor_y + dy).clamp(0.0, 1.0);
                self.motion.dx += dx;
                self.motion.dy += dy;
                self.motion.distance += dx.hypot(dy);
                self.pointer_present = true;
            }
            HostPointerMotion::Absolute {
                norm_x,
                norm_y,
                coordinate_space_generation,
            } => {
                self.observe_coordinate_space(coordinate_space_generation);
                let next = (
                    norm_x.clamp(0.0, 1.0),
                    norm_y.clamp(0.0, 1.0),
                    coordinate_space_generation,
                );
                if let Some((previous_x, previous_y, previous_generation)) =
                    self.absolute_baselines.insert(Arc::clone(source_id), next)
                    && previous_generation == coordinate_space_generation
                {
                    let dx = next.0 - previous_x;
                    let dy = next.1 - previous_y;
                    self.motion.dx += dx;
                    self.motion.dy += dy;
                    self.motion.distance += dx.hypot(dy);
                }
                self.pointer_present = true;
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the neutral scroll record is one atomic event"
    )]
    fn fold_scroll(
        &mut self,
        source_id: &Arc<str>,
        delta_x_q16_16: i64,
        delta_y_q16_16: i64,
        unit: PointerScrollUnit,
        phase: hypercolor_types::event::PointerScrollPhase,
        momentum_phase: hypercolor_types::event::PointerScrollPhase,
        physical_code: &Arc<str>,
        at_ms: u64,
    ) {
        self.push_event(TimedInputEvent {
            event: InputEvent::PointerScroll {
                source_id: source_id.to_string(),
                delta_x_q16_16,
                delta_y_q16_16,
                unit,
                phase,
                momentum_phase,
            },
            at_ms,
            seq: 0,
            physical_code: Some(physical_code.to_string()),
            repeat_count: 1,
        });
        if unit != PointerScrollUnit::Line120 {
            return;
        }
        let legacy_delta = self
            .wheel_projectors
            .entry(Arc::clone(source_id))
            .or_default()
            .project(delta_y_q16_16);
        if legacy_delta == 0 {
            return;
        }
        self.push_event(TimedInputEvent {
            event: InputEvent::MouseWheel {
                source_id: source_id.to_string(),
                delta_hi_res: legacy_delta,
            },
            at_ms,
            seq: 0,
            physical_code: Some("host:legacy-wheel-shadow".to_owned()),
            repeat_count: 1,
        });
    }

    fn observe_coordinate_space(&mut self, generation: u64) {
        if self.coordinate_space_generation == Some(generation) {
            return;
        }
        if self
            .coordinate_space_generation
            .replace(generation)
            .is_some()
        {
            self.diagnostics.coordinate_space_resets =
                self.diagnostics.coordinate_space_resets.saturating_add(1);
        }
        self.absolute_baselines.clear();
    }

    fn synthesize_all_releases(&mut self, at_ms: u64) {
        let mut sources = BTreeSet::new();
        sources.extend(self.pressed_keys.keys().cloned());
        sources.extend(self.pressed_buttons.keys().cloned());
        for source_id in sources {
            self.synthesize_releases(&source_id, at_ms);
        }
    }

    fn synthesize_releases(&mut self, source_id: &Arc<str>, at_ms: u64) {
        if let Some(keys) = self.pressed_keys.remove(source_id) {
            for key in keys {
                self.push_event(TimedInputEvent {
                    event: InputEvent::Key {
                        source_id: source_id.to_string(),
                        key: key.to_string(),
                        state: InputButtonState::Released,
                    },
                    at_ms,
                    seq: 0,
                    physical_code: None,
                    repeat_count: 1,
                });
            }
        }
        if let Some(buttons) = self.pressed_buttons.remove(source_id) {
            for button in buttons {
                self.push_event(TimedInputEvent {
                    event: InputEvent::MouseButton {
                        source_id: source_id.to_string(),
                        button: button.to_string(),
                        state: InputButtonState::Released,
                    },
                    at_ms,
                    seq: 0,
                    physical_code: None,
                    repeat_count: 1,
                });
            }
        }
    }

    fn push_event(&mut self, event: TimedInputEvent) {
        if self.event_capacity == 0 {
            self.dropped_events = self
                .dropped_events
                .saturating_add(u32::try_from(self.events.len()).unwrap_or(u32::MAX))
                .saturating_add(1);
            self.events.clear();
            return;
        }
        while self.events.len() >= self.event_capacity {
            self.events.pop_front();
            self.dropped_events = self.dropped_events.saturating_add(1);
        }
        self.events.push_back(event);
    }

    fn union_pressed_keys(&self) -> Vec<String> {
        self.pressed_keys
            .values()
            .flat_map(BTreeSet::iter)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(ToString::to_string)
            .collect()
    }

    fn union_pressed_buttons(&self) -> Vec<String> {
        self.pressed_buttons
            .values()
            .flat_map(BTreeSet::iter)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(ToString::to_string)
            .collect()
    }

    fn apply_pointer_snapshot(&self, interaction: &mut InteractionData) {
        if let Some(pointer) = self.pointer {
            interaction.mouse.mode = PointerMode::Absolute;
            interaction.mouse.x = saturating_i32(pointer.x);
            interaction.mouse.y = saturating_i32(pointer.y);
            interaction.mouse.norm_x = pointer.norm_x.clamp(0.0, 1.0);
            interaction.mouse.norm_y = pointer.norm_y.clamp(0.0, 1.0);
        } else if self.pointer_present && self.capabilities.pointer {
            interaction.mouse.mode = PointerMode::Virtual;
            interaction.mouse.norm_x = self.virtual_cursor_x;
            interaction.mouse.norm_y = self.virtual_cursor_y;
        }
    }
}

fn event_source_id<'a>(
    device: Option<&'a Arc<HostInputDevice>>,
    fallback: &'a Arc<str>,
) -> &'a Arc<str> {
    device.map_or(fallback, |device| &device.source_id)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "finite native coordinates saturate at the public i32 boundary"
)]
fn saturating_i32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}
