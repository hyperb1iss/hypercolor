//! Deterministic per-consumer routing for interaction snapshots and events.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use hypercolor_types::config::InteractionRoutePolicy;

use super::{
    InputData, InputEventRead, InputSourceSlot, InteractionData, InteractionSourceOrigin,
    InteractionTransientTotals, SourceStatusHandle,
};
use crate::types::event::{InputButtonState, InputEvent, TimedInputEvent};

/// Stable identity for one interaction consumer lifetime.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConsumerIncarnation(u64);

impl ConsumerIncarnation {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SourceNamespace {
    Host,
    Browser,
    CompatibilityAggregate,
}

/// Stable identity for one routable source lifetime.
///
/// Host manager slots and browser child publications occupy disjoint identity
/// namespaces, so equal backend-local counters cannot alias one another.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceIncarnation {
    namespace: SourceNamespace,
    value: u64,
}

impl SourceIncarnation {
    #[must_use]
    pub const fn host_slot(value: u64) -> Self {
        Self {
            namespace: SourceNamespace::Host,
            value,
        }
    }

    #[must_use]
    pub const fn browser_child(value: u64) -> Self {
        Self {
            namespace: SourceNamespace::Browser,
            value,
        }
    }

    #[must_use]
    pub const fn compatibility_aggregate(value: u64) -> Self {
        Self {
            namespace: SourceNamespace::CompatibilityAggregate,
            value,
        }
    }

    #[must_use]
    pub const fn is_browser(self) -> bool {
        matches!(self.namespace, SourceNamespace::Browser)
    }
}

/// Source category used for exact policy selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionRouteSourceClass {
    Host,
    Browser,
    /// Legacy union published for old consumers, never a selectable route.
    CompatibilityAggregate,
}

/// Zero-copy interaction snapshot from a graph or specialized slot.
#[derive(Clone)]
pub enum InteractionRouteSnapshot {
    InputData(Arc<InputData>),
    Interaction(Arc<InteractionData>),
}

impl InteractionRouteSnapshot {
    #[must_use]
    pub fn as_interaction(&self) -> &InteractionData {
        match self {
            Self::InputData(sample) => match sample.as_ref() {
                InputData::Interaction(interaction) => interaction,
                _ => unreachable!("route snapshots only wrap interaction input data"),
            },
            Self::Interaction(interaction) => interaction,
        }
    }
}

/// Read-only interaction publication consumed by the pure router.
pub trait InteractionRouteSlot: Send + Sync {
    /// Atomically read held state and events from one publication revision.
    ///
    /// `u64::MAX` attaches at the current tail without replaying history.
    fn read_interaction_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> InteractionRouteRead;

    /// Read into retained storage and report the initialized event prefix.
    fn read_interaction_reusing_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> ReusedInteractionRouteRead {
        output.clear();
        let publication = self.read_interaction_since(cursor, output);
        ReusedInteractionRouteRead {
            publication,
            event_count: output.len(),
        }
    }

    /// Health handle bound to this source lifetime, when one exists.
    fn status_handle(&self) -> Option<SourceStatusHandle> {
        None
    }
}

/// Coherent held-state and event read from one source publication revision.
pub struct InteractionRouteRead {
    pub snapshot: Option<InteractionRouteSnapshot>,
    pub events: InputEventRead,
    pub interaction_transients: InteractionTransientTotals,
}

/// One route publication plus the valid prefix in retained event storage.
pub struct ReusedInteractionRouteRead {
    pub publication: InteractionRouteRead,
    pub event_count: usize,
}

impl InteractionRouteSlot for InputSourceSlot {
    fn read_interaction_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> InteractionRouteRead {
        let publication = self.read_publication_since(cursor, output);
        InteractionRouteRead {
            snapshot: publication.sample.and_then(|sample| {
                matches!(sample.as_ref(), InputData::Interaction(_))
                    .then_some(InteractionRouteSnapshot::InputData(sample))
            }),
            events: publication.events,
            interaction_transients: publication.interaction_transients,
        }
    }

    fn read_interaction_reusing_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> ReusedInteractionRouteRead {
        let (publication, event_count) = self.read_publication_reusing_since(cursor, output, 0);
        ReusedInteractionRouteRead {
            publication: InteractionRouteRead {
                snapshot: publication.sample.and_then(|sample| {
                    matches!(sample.as_ref(), InputData::Interaction(_))
                        .then_some(InteractionRouteSnapshot::InputData(sample))
                }),
                events: publication.events,
                interaction_transients: publication.interaction_transients,
            },
            event_count,
        }
    }

    fn status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status().clone())
    }
}

/// One exact source offered to a route resolution.
#[derive(Clone)]
pub struct InteractionRouteSource {
    pub incarnation: SourceIncarnation,
    pub descriptor: Arc<str>,
    pub class: InteractionRouteSourceClass,
    pub availability_revision: u64,
    slot: Arc<dyn InteractionRouteSlot>,
}

impl InteractionRouteSource {
    #[must_use]
    pub fn new(
        incarnation: SourceIncarnation,
        descriptor: impl Into<Arc<str>>,
        class: InteractionRouteSourceClass,
        availability_revision: u64,
        slot: Arc<dyn InteractionRouteSlot>,
    ) -> Self {
        let namespace_matches = matches!(
            (incarnation.namespace, class),
            (SourceNamespace::Host, InteractionRouteSourceClass::Host)
                | (
                    SourceNamespace::Browser,
                    InteractionRouteSourceClass::Browser
                )
                | (
                    SourceNamespace::CompatibilityAggregate,
                    InteractionRouteSourceClass::CompatibilityAggregate
                )
        );
        assert!(
            namespace_matches,
            "source incarnation namespace must match its route class"
        );
        Self {
            incarnation,
            descriptor: descriptor.into(),
            class,
            availability_revision,
            slot,
        }
    }

    #[must_use]
    pub fn manager_slot(
        descriptor: impl Into<Arc<str>>,
        availability_revision: u64,
        slot: InputSourceSlot,
    ) -> Option<Self> {
        let (incarnation, class) = match slot.interaction_origin()? {
            InteractionSourceOrigin::Host => (
                SourceIncarnation::host_slot(slot.id()),
                InteractionRouteSourceClass::Host,
            ),
            InteractionSourceOrigin::BrowserCompatibilityAggregate => (
                SourceIncarnation::compatibility_aggregate(slot.id()),
                InteractionRouteSourceClass::CompatibilityAggregate,
            ),
        };
        Some(Self::new(
            incarnation,
            descriptor,
            class,
            availability_revision,
            Arc::new(slot),
        ))
    }
}

/// Exact policy request for one consumer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InteractionRouteRequest {
    pub policy: InteractionRoutePolicy,
    pub browser_source: Option<SourceIncarnation>,
}

impl InteractionRouteRequest {
    #[must_use]
    pub const fn host() -> Self {
        Self {
            policy: InteractionRoutePolicy::Host,
            browser_source: None,
        }
    }
}

/// Generations attached to one coherent route publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InteractionRouteContext {
    pub config_generation: u64,
    pub source_graph_generation: u64,
    pub browser_registry_generation: u64,
    pub now_ms: u64,
}

/// Source identity and revisions participating in frame reuse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionRouteReuseSource {
    pub incarnation: SourceIncarnation,
    pub interaction_generation: Option<u64>,
    pub availability_revision: u64,
}

/// Complete interaction-specific frame reuse identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionRouteReuseKey {
    pub route_generation: u64,
    pub output_generation: u64,
    pub sources: Vec<InteractionRouteReuseSource>,
}

/// Immutable source entry in a route diagnostics publication.
#[derive(Clone)]
pub struct InteractionRouteDiagnosticSource {
    pub incarnation: SourceIncarnation,
    pub descriptor: Arc<str>,
    pub availability_revision: u64,
    pub status: Option<SourceStatusHandle>,
}

/// Generation-tagged drop counters for one consumer route.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InteractionRouteDropMetrics {
    pub route_generation: u64,
    pub overwritten_events: u64,
    pub producer_dropped_events: u64,
    pub invalid_events: u64,
}

/// One coherent immutable diagnostic model for a consumer route.
#[derive(Clone)]
pub struct InteractionRouteDiagnostics {
    pub consumer: ConsumerIncarnation,
    pub policy: InteractionRoutePolicy,
    pub selected: Arc<[InteractionRouteDiagnosticSource]>,
    pub suppressed: Arc<[Arc<str>]>,
    pub metrics: InteractionRouteDropMetrics,
    pub route_generation: u64,
    pub config_generation: u64,
    pub source_graph_generation: u64,
    pub browser_registry_generation: u64,
}

/// Routed snapshot, ordered events, and immutable cache/diagnostic identities.
pub struct RoutedInteraction {
    pub interaction: InteractionData,
    pub reuse_key: InteractionRouteReuseKey,
    pub diagnostics: Arc<InteractionRouteDiagnostics>,
}

impl RoutedInteraction {
    /// Allocate lane-owned buffers for repeated route resolutions.
    #[must_use]
    pub fn new(consumer: ConsumerIncarnation) -> Self {
        Self {
            interaction: InteractionData::default(),
            reuse_key: InteractionRouteReuseKey {
                route_generation: 0,
                output_generation: 0,
                sources: Vec::new(),
            },
            diagnostics: Arc::new(InteractionRouteDiagnostics {
                consumer,
                policy: InteractionRoutePolicy::Host,
                selected: Arc::from([]),
                suppressed: Arc::from([]),
                metrics: InteractionRouteDropMetrics::default(),
                route_generation: 0,
                config_generation: 0,
                source_graph_generation: 0,
                browser_registry_generation: 0,
            }),
        }
    }
}

/// Pure interaction router with independent state for every consumer.
#[derive(Default)]
pub struct InteractionRouter {
    consumers: BTreeMap<ConsumerIncarnation, ConsumerRouteState>,
    selected_scratch: Vec<InteractionRouteSource>,
}

impl InteractionRouter {
    /// Resolve one consumer into reusable lane-owned storage.
    pub fn resolve_into(
        &mut self,
        consumer: ConsumerIncarnation,
        request: InteractionRouteRequest,
        sources: &[InteractionRouteSource],
        context: InteractionRouteContext,
        output: &mut RoutedInteraction,
    ) {
        select_sources_into(request, sources, &mut self.selected_scratch);
        let state = self.consumers.entry(consumer).or_default();
        state.resolve_into(
            consumer,
            request.policy,
            &self.selected_scratch,
            sources,
            context,
            output,
        );
    }

    /// Remove a consumer and return release edges for its observed controls.
    #[must_use]
    pub fn remove_consumer(
        &mut self,
        consumer: ConsumerIncarnation,
        now_ms: u64,
    ) -> Vec<TimedInputEvent> {
        self.consumers
            .remove(&consumer)
            .map_or_else(Vec::new, |state| state.release_all(now_ms))
    }
}

fn select_sources_into(
    request: InteractionRouteRequest,
    sources: &[InteractionRouteSource],
    selected: &mut Vec<InteractionRouteSource>,
) {
    selected.clear();
    if matches!(
        request.policy,
        InteractionRoutePolicy::Host | InteractionRoutePolicy::Merge
    ) {
        for source in sources {
            if source.class == InteractionRouteSourceClass::Host
                && !selected
                    .iter()
                    .any(|selected_source| selected_source.incarnation == source.incarnation)
            {
                selected.push(source.clone());
            }
        }
    }
    if matches!(
        request.policy,
        InteractionRoutePolicy::Browser | InteractionRoutePolicy::Merge
    ) && let Some(browser_source) = request.browser_source.filter(|source| source.is_browser())
        && let Some(source) = sources.iter().find(|source| {
            source.class == InteractionRouteSourceClass::Browser
                && source.incarnation == browser_source
        })
        && !selected
            .iter()
            .any(|selected_source| selected_source.incarnation == source.incarnation)
    {
        selected.push(source.clone());
    }
}

#[derive(Default)]
struct ConsumerRouteState {
    policy: Option<InteractionRoutePolicy>,
    route_generation: u64,
    output_generation: u64,
    source_states: BTreeMap<SourceIncarnation, ConsumerSourceState>,
    selected_order: Vec<SourceIncarnation>,
    overwritten_events: u64,
    producer_dropped_events: u64,
    invalid_events: u64,
    selection_scratch: Vec<SourceIncarnation>,
    removed_scratch: Vec<SourceIncarnation>,
    missing_scratch: Vec<InteractionControl>,
    event_sort_scratch: Vec<(TimedInputEvent, usize)>,
    cached_diagnostics: Option<Arc<InteractionRouteDiagnostics>>,
}

impl ConsumerRouteState {
    fn resolve_into(
        &mut self,
        consumer: ConsumerIncarnation,
        policy: InteractionRoutePolicy,
        selected: &[InteractionRouteSource],
        all_sources: &[InteractionRouteSource],
        context: InteractionRouteContext,
        output: &mut RoutedInteraction,
    ) {
        self.selection_scratch.clear();
        self.selection_scratch
            .extend(selected.iter().map(|source| source.incarnation));
        let route_changed =
            self.policy != Some(policy) || self.selected_order != self.selection_scratch;
        let mut events = std::mem::take(&mut output.interaction.batch.events);
        events.extend(self.event_sort_scratch.drain(..).map(|(event, _)| event));
        let mut event_count = 0;
        if route_changed {
            self.bump_route_generation();
            self.transition_sources(selected, context.now_ms, &mut events, &mut event_count);
            self.policy = Some(policy);
            self.selected_order.clear();
            self.selected_order
                .extend_from_slice(&self.selection_scratch);
        } else {
            self.refresh_sources(selected);
        }

        let overwritten_before = self.overwritten_events;
        let producer_dropped_before = self.producer_dropped_events;
        let invalid_before = self.invalid_events;
        let interaction_changed = self.read_selected_publications(
            selected,
            context.now_ms,
            &mut events,
            &mut event_count,
        );
        let snapshot_changed =
            self.reconcile_snapshots(selected, context.now_ms, &mut events, &mut event_count);
        if route_changed || interaction_changed || snapshot_changed || event_count != 0 {
            self.output_generation = self
                .output_generation
                .checked_add(1)
                .expect("routed interaction generation exhausted");
        }

        stable_sort_events_reused(&mut events, event_count, &mut self.event_sort_scratch);
        self.build_interaction_into(selected, events, &mut output.interaction);
        output.interaction.generation = self.output_generation;
        output.interaction.batch.dropped_events = u32::try_from(
            self.overwritten_events
                .saturating_sub(overwritten_before)
                .saturating_add(
                    self.producer_dropped_events
                        .saturating_sub(producer_dropped_before),
                )
                .saturating_add(self.invalid_events.saturating_sub(invalid_before)),
        )
        .unwrap_or(u32::MAX);

        self.refresh_reuse_key(selected, &mut output.reuse_key);
        self.refresh_diagnostics(consumer, policy, selected, all_sources, context);
        output.diagnostics = Arc::clone(
            self.cached_diagnostics
                .as_ref()
                .expect("route diagnostics initialized by resolution"),
        );
    }

    fn bump_route_generation(&mut self) {
        self.route_generation = self
            .route_generation
            .checked_add(1)
            .expect("interaction route generation exhausted");
        self.overwritten_events = 0;
        self.producer_dropped_events = 0;
        self.invalid_events = 0;
    }

    fn transition_sources(
        &mut self,
        selected: &[InteractionRouteSource],
        now_ms: u64,
        events: &mut Vec<TimedInputEvent>,
        event_count: &mut usize,
    ) {
        self.removed_scratch.clear();
        self.removed_scratch.extend(
            self.source_states
                .keys()
                .filter(|incarnation| !self.selection_scratch.contains(incarnation))
                .copied(),
        );
        for index in 0..self.removed_scratch.len() {
            let incarnation = self.removed_scratch[index];
            if let Some(source) = self.source_states.remove(&incarnation) {
                self.synthesize_removed_source(source, now_ms, events, event_count);
            }
        }
        for source in selected {
            match self.source_states.entry(source.incarnation) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(ConsumerSourceState::attach(source));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().slot = Arc::clone(&source.slot);
                    entry.get_mut().descriptor = Arc::clone(&source.descriptor);
                }
            }
        }
    }

    fn refresh_sources(&mut self, selected: &[InteractionRouteSource]) {
        for source in selected {
            if let Some(state) = self.source_states.get_mut(&source.incarnation) {
                state.slot = Arc::clone(&source.slot);
                state.descriptor = Arc::clone(&source.descriptor);
            }
        }
    }

    fn read_selected_publications(
        &mut self,
        selected: &[InteractionRouteSource],
        now_ms: u64,
        output: &mut Vec<TimedInputEvent>,
        output_count: &mut usize,
    ) -> bool {
        let mut changed = false;
        for source in selected {
            let (attaching, publication, captured_count) = {
                let state = self
                    .source_states
                    .get_mut(&source.incarnation)
                    .expect("selected route source has consumer state");
                state.transient_delta = InteractionTransientTotals::default();
                let attaching = state.cursor == u64::MAX;
                let read = state
                    .slot
                    .read_interaction_reusing_since(state.cursor, &mut state.captured);
                let publication = read.publication;
                state.cursor = publication.events.next_cursor;
                (attaching, publication, read.event_count)
            };
            if attaching {
                let state = self
                    .source_states
                    .get_mut(&source.incarnation)
                    .expect("attached route source has consumer state");
                state.latest_snapshot = publication.snapshot;
                state.interaction_transients = publication.interaction_transients;
                replace_quarantine_from_snapshot(state);
                state.last_interaction_generation = state
                    .latest_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.as_interaction().generation);
                debug_assert!(captured_count == 0, "tail attach cannot replay events");
                continue;
            }
            if publication.events.dropped > 0 {
                self.overwritten_events = self
                    .overwritten_events
                    .saturating_add(publication.events.dropped);
                let (recovery, recovery_count) = {
                    let state = self
                        .source_states
                        .get_mut(&source.incarnation)
                        .expect("overflowed route source has consumer state");
                    let read = state
                        .slot
                        .read_interaction_reusing_since(u64::MAX, &mut state.captured);
                    let recovery = read.publication;
                    state.cursor = recovery.events.next_cursor;
                    (recovery, read.event_count)
                };
                let transient_delta = {
                    let state = self
                        .source_states
                        .get_mut(&source.incarnation)
                        .expect("overflowed route source has consumer state");
                    debug_assert!(
                        recovery_count == 0,
                        "overflow recovery cannot replay events"
                    );
                    state.latest_snapshot = recovery.snapshot;
                    apply_interaction_transients(state, recovery.interaction_transients)
                };
                self.record_transient_delta(transient_delta);
                self.reset_source_after_loss(source.incarnation, now_ms, output, output_count);
                changed = true;
                continue;
            }
            let (captured, transient_delta) = {
                let state = self
                    .source_states
                    .get_mut(&source.incarnation)
                    .expect("selected route source has consumer state");
                state.latest_snapshot = publication.snapshot;
                let transient_delta =
                    apply_interaction_transients(state, publication.interaction_transients);
                (std::mem::take(&mut state.captured), transient_delta)
            };
            changed |= self.record_transient_delta(transient_delta);
            for event in &captured[..captured_count] {
                changed |= self.route_event(source.incarnation, event, output, output_count);
            }
            self.source_states
                .get_mut(&source.incarnation)
                .expect("selected route source has consumer state")
                .captured = captured;
        }
        changed
    }

    fn record_transient_delta(&mut self, delta: InteractionTransientTotals) -> bool {
        self.producer_dropped_events = self
            .producer_dropped_events
            .saturating_add(delta.dropped_events);
        self.invalid_events = self.invalid_events.saturating_add(delta.invalid_events);
        delta != InteractionTransientTotals::default()
    }

    fn reset_source_after_loss(
        &mut self,
        incarnation: SourceIncarnation,
        now_ms: u64,
        output: &mut Vec<TimedInputEvent>,
        output_count: &mut usize,
    ) {
        let Some(mut source) = self.source_states.remove(&incarnation) else {
            return;
        };
        let observed = std::mem::take(&mut source.observed);
        for (control, press) in observed {
            if !self.control_observed_elsewhere(&control) {
                push_owned_event_reused(output, output_count, synthetic_release(&press, now_ms));
            }
        }
        replace_quarantine_from_snapshot(&mut source);
        self.source_states.insert(incarnation, source);
    }

    fn route_event(
        &mut self,
        incarnation: SourceIncarnation,
        event: &TimedInputEvent,
        output: &mut Vec<TimedInputEvent>,
        output_count: &mut usize,
    ) -> bool {
        if event.repeat_count == 0 {
            self.invalid_events = self.invalid_events.saturating_add(1);
            return false;
        }
        let Some((control, state)) = event_control(&event) else {
            push_event_reused(output, output_count, event);
            return true;
        };
        let source = self
            .source_states
            .get_mut(&incarnation)
            .expect("routed event source has consumer state");
        if source.quarantined.contains(&control) {
            if state == InputButtonState::Released {
                source.quarantined.remove(&control);
            }
            return false;
        }
        match state {
            InputButtonState::Pressed => {
                source.observed.insert(control, event.clone());
                push_event_reused(output, output_count, event);
                true
            }
            InputButtonState::Repeated => {
                if source.observed.contains_key(&control) {
                    push_event_reused(output, output_count, event);
                    true
                } else {
                    self.invalid_events = self.invalid_events.saturating_add(1);
                    false
                }
            }
            InputButtonState::Released => {
                if source.observed.remove(&control).is_none() {
                    self.invalid_events = self.invalid_events.saturating_add(1);
                    return false;
                }
                if !self.control_observed_elsewhere(&control) {
                    push_event_reused(output, output_count, event);
                }
                true
            }
        }
    }

    fn reconcile_snapshots(
        &mut self,
        selected: &[InteractionRouteSource],
        now_ms: u64,
        output: &mut Vec<TimedInputEvent>,
        output_count: &mut usize,
    ) -> bool {
        let mut changed = false;
        for source in selected {
            let snapshot = self
                .source_states
                .get(&source.incarnation)
                .and_then(|state| state.latest_snapshot.clone());
            self.missing_scratch.clear();
            self.missing_scratch.extend(
                self.source_states
                    .get(&source.incarnation)
                    .expect("selected route source has consumer state")
                    .observed
                    .keys()
                    .filter(|control| {
                        snapshot.as_ref().is_none_or(|snapshot| {
                            !snapshot_holds(snapshot.as_interaction(), control)
                        })
                    })
                    .cloned(),
            );
            for index in 0..self.missing_scratch.len() {
                let control = self.missing_scratch[index].clone();
                let press = self
                    .source_states
                    .get_mut(&source.incarnation)
                    .and_then(|state| state.observed.remove(&control));
                if let Some(press) = press {
                    if !self.control_observed_elsewhere(&control) {
                        push_owned_event_reused(
                            output,
                            output_count,
                            synthetic_release(&press, now_ms),
                        );
                    }
                    changed = true;
                }
            }
            let state = self
                .source_states
                .get_mut(&source.incarnation)
                .expect("selected route source has consumer state");
            let Some(snapshot) = snapshot else {
                changed |= state.last_interaction_generation.take().is_some();
                changed |= !state.quarantined.is_empty();
                state.quarantined.clear();
                continue;
            };
            let snapshot = snapshot.as_interaction();
            if state.last_interaction_generation != Some(snapshot.generation) {
                changed = true;
            }
            for key in &snapshot.keyboard.pressed_keys {
                if !contains_key_control(&state.observed, key)
                    && !contains_key_control_set(&state.quarantined, key)
                {
                    changed |= state
                        .quarantined
                        .insert(InteractionControl::Key(key.clone()));
                }
            }
            for button in &snapshot.mouse.buttons {
                if !contains_mouse_control(&state.observed, button)
                    && !contains_mouse_control_set(&state.quarantined, button)
                {
                    changed |= state
                        .quarantined
                        .insert(InteractionControl::MouseButton(button.clone()));
                }
            }
            state.last_interaction_generation = Some(snapshot.generation);
        }
        changed
    }

    fn build_interaction_into(
        &self,
        selected: &[InteractionRouteSource],
        events: Vec<TimedInputEvent>,
        interaction: &mut InteractionData,
    ) {
        let mut key_count = 0;
        let mut button_count = 0;
        interaction.mouse.x = 0;
        interaction.mouse.y = 0;
        interaction.mouse.norm_x = 0.0;
        interaction.mouse.norm_y = 0.0;
        interaction.mouse.mode = super::PointerMode::None;
        interaction.mouse.injected = false;
        interaction.batch.wheel_hi_res = 0;
        interaction.batch.scroll = super::ScrollAggregate::default();
        interaction.batch.motion = super::MotionAggregate::default();
        interaction.batch.window_secs = 0.0;
        interaction.batch.dropped_events = 0;
        for source in selected {
            let state = self
                .source_states
                .get(&source.incarnation)
                .expect("selected route source has consumer state");
            let Some(snapshot) = state.latest_snapshot.as_ref() else {
                continue;
            };
            let snapshot = snapshot.as_interaction();
            for key in &snapshot.keyboard.pressed_keys {
                if contains_key_control(&state.observed, key) {
                    push_unique_reused(&mut interaction.keyboard.pressed_keys, &mut key_count, key);
                }
            }
            for button in &snapshot.mouse.buttons {
                if contains_mouse_control(&state.observed, button) {
                    push_unique_reused(&mut interaction.mouse.buttons, &mut button_count, button);
                }
            }
            if snapshot.mouse.mode != super::PointerMode::None
                && (interaction.mouse.mode == super::PointerMode::None
                    || snapshot.mouse.injected && !interaction.mouse.injected)
            {
                interaction.mouse.x = snapshot.mouse.x;
                interaction.mouse.y = snapshot.mouse.y;
                interaction.mouse.norm_x = snapshot.mouse.norm_x;
                interaction.mouse.norm_y = snapshot.mouse.norm_y;
                interaction.mouse.mode = snapshot.mouse.mode;
                interaction.mouse.injected = snapshot.mouse.injected;
            }
            interaction.batch.motion.dx += transient_f32(state.transient_delta.dx);
            interaction.batch.motion.dy += transient_f32(state.transient_delta.dy);
            interaction.batch.motion.distance += transient_f32(state.transient_delta.distance);
            interaction.batch.window_secs += transient_f32(state.transient_delta.window_secs);
        }
        interaction.keyboard.pressed_keys.truncate(key_count);
        interaction.mouse.buttons.truncate(button_count);
        interaction.mouse.down = button_count > 0;
        let mut recent_count = 0;
        for event in &events {
            match &event.event {
                InputEvent::Key {
                    key,
                    state: InputButtonState::Pressed,
                    ..
                } => push_reused(
                    &mut interaction.keyboard.recent_keys,
                    &mut recent_count,
                    key,
                ),
                InputEvent::MouseWheel { delta_hi_res, .. } => {
                    interaction.batch.wheel_hi_res =
                        interaction.batch.wheel_hi_res.saturating_add(*delta_hi_res);
                }
                InputEvent::PointerScroll {
                    delta_x_q16_16,
                    delta_y_q16_16,
                    unit,
                    ..
                } => interaction
                    .batch
                    .scroll
                    .accumulate(*unit, *delta_x_q16_16, *delta_y_q16_16),
                InputEvent::Key { .. }
                | InputEvent::MouseButton { .. }
                | InputEvent::MidiNote { .. }
                | InputEvent::MidiControlChange { .. }
                | InputEvent::MidiPitchBend { .. }
                | InputEvent::MidiRealtime { .. } => {}
            }
        }
        interaction.keyboard.recent_keys.truncate(recent_count);
        interaction.batch.events = events;
    }

    fn refresh_reuse_key(
        &self,
        selected: &[InteractionRouteSource],
        reuse_key: &mut InteractionRouteReuseKey,
    ) {
        reuse_key.route_generation = self.route_generation;
        reuse_key.output_generation = self.output_generation;
        reuse_key.sources.clear();
        reuse_key.sources.extend(selected.iter().map(|source| {
            InteractionRouteReuseSource {
                incarnation: source.incarnation,
                interaction_generation: self
                    .source_states
                    .get(&source.incarnation)
                    .and_then(|state| state.latest_snapshot.as_ref())
                    .map(|snapshot| snapshot.as_interaction().generation),
                availability_revision: source.availability_revision,
            }
        }));
    }

    fn refresh_diagnostics(
        &mut self,
        consumer: ConsumerIncarnation,
        policy: InteractionRoutePolicy,
        selected: &[InteractionRouteSource],
        all_sources: &[InteractionRouteSource],
        context: InteractionRouteContext,
    ) {
        let metrics = InteractionRouteDropMetrics {
            route_generation: self.route_generation,
            overwritten_events: self.overwritten_events,
            producer_dropped_events: self.producer_dropped_events,
            invalid_events: self.invalid_events,
        };
        let current = self.cached_diagnostics.as_ref().is_some_and(|diagnostics| {
            diagnostics.consumer == consumer
                && diagnostics.policy == policy
                && diagnostics.metrics == metrics
                && diagnostics.route_generation == self.route_generation
                && diagnostics.config_generation == context.config_generation
                && diagnostics.source_graph_generation == context.source_graph_generation
                && diagnostics.browser_registry_generation == context.browser_registry_generation
                && diagnostics.selected.len() == selected.len()
                && diagnostics
                    .selected
                    .iter()
                    .zip(selected)
                    .all(|(entry, source)| {
                        entry.incarnation == source.incarnation
                            && entry.descriptor == source.descriptor
                            && entry.availability_revision == source.availability_revision
                    })
                && suppressed_descriptors_match(diagnostics, selected, all_sources)
        });
        if current {
            return;
        }
        self.cached_diagnostics = Some(Arc::new(InteractionRouteDiagnostics {
            consumer,
            policy,
            selected: selected
                .iter()
                .map(|source| InteractionRouteDiagnosticSource {
                    incarnation: source.incarnation,
                    descriptor: Arc::clone(&source.descriptor),
                    availability_revision: source.availability_revision,
                    status: source.slot.status_handle(),
                })
                .collect(),
            suppressed: all_sources
                .iter()
                .filter(|source| {
                    !selected
                        .iter()
                        .any(|selected_source| selected_source.incarnation == source.incarnation)
                })
                .map(|source| Arc::clone(&source.descriptor))
                .collect(),
            metrics,
            route_generation: self.route_generation,
            config_generation: context.config_generation,
            source_graph_generation: context.source_graph_generation,
            browser_registry_generation: context.browser_registry_generation,
        }));
    }

    fn synthesize_removed_source(
        &self,
        source: ConsumerSourceState,
        now_ms: u64,
        output: &mut Vec<TimedInputEvent>,
        output_count: &mut usize,
    ) {
        for (control, press) in source.observed {
            if !self.control_observed_elsewhere(&control) {
                push_owned_event_reused(output, output_count, synthetic_release(&press, now_ms));
            }
        }
    }

    fn control_observed_elsewhere(&self, control: &InteractionControl) -> bool {
        self.source_states
            .values()
            .any(|source| source.observed.contains_key(control))
    }

    fn release_all(self, now_ms: u64) -> Vec<TimedInputEvent> {
        let mut released = BTreeSet::new();
        let mut events = Vec::new();
        for source in self.source_states.into_values() {
            for (control, press) in source.observed {
                if released.insert(control) {
                    events.push(synthetic_release(&press, now_ms));
                }
            }
        }
        events
    }
}

struct ConsumerSourceState {
    slot: Arc<dyn InteractionRouteSlot>,
    descriptor: Arc<str>,
    cursor: u64,
    captured: Vec<TimedInputEvent>,
    latest_snapshot: Option<InteractionRouteSnapshot>,
    interaction_transients: InteractionTransientTotals,
    transient_delta: InteractionTransientTotals,
    observed: BTreeMap<InteractionControl, TimedInputEvent>,
    quarantined: BTreeSet<InteractionControl>,
    last_interaction_generation: Option<u64>,
}

fn suppressed_descriptors_match(
    diagnostics: &InteractionRouteDiagnostics,
    selected: &[InteractionRouteSource],
    all_sources: &[InteractionRouteSource],
) -> bool {
    let mut suppressed = diagnostics.suppressed.iter();
    for source in all_sources.iter().filter(|source| {
        !selected
            .iter()
            .any(|selected_source| selected_source.incarnation == source.incarnation)
    }) {
        if suppressed
            .next()
            .is_none_or(|descriptor| descriptor != &source.descriptor)
        {
            return false;
        }
    }
    suppressed.next().is_none()
}

impl ConsumerSourceState {
    fn attach(source: &InteractionRouteSource) -> Self {
        Self {
            slot: Arc::clone(&source.slot),
            descriptor: Arc::clone(&source.descriptor),
            cursor: u64::MAX,
            captured: Vec::new(),
            latest_snapshot: None,
            interaction_transients: InteractionTransientTotals::default(),
            transient_delta: InteractionTransientTotals::default(),
            observed: BTreeMap::new(),
            quarantined: BTreeSet::new(),
            last_interaction_generation: None,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum InteractionControl {
    Key(String),
    MouseButton(String),
}

fn apply_interaction_transients(
    state: &mut ConsumerSourceState,
    current: InteractionTransientTotals,
) -> InteractionTransientTotals {
    state.transient_delta = current.delta_since(state.interaction_transients);
    state.interaction_transients = current;
    state.transient_delta
}

#[expect(clippy::cast_possible_truncation)]
fn transient_f32(value: f64) -> f32 {
    value as f32
}

fn replace_quarantine_from_snapshot(state: &mut ConsumerSourceState) {
    state.quarantined.clear();
    let Some(snapshot) = state.latest_snapshot.as_ref() else {
        return;
    };
    let snapshot = snapshot.as_interaction();
    state.quarantined.extend(
        snapshot
            .keyboard
            .pressed_keys
            .iter()
            .cloned()
            .map(InteractionControl::Key)
            .chain(
                snapshot
                    .mouse
                    .buttons
                    .iter()
                    .cloned()
                    .map(InteractionControl::MouseButton),
            ),
    );
}

fn snapshot_holds(snapshot: &InteractionData, control: &InteractionControl) -> bool {
    match control {
        InteractionControl::Key(key) => snapshot.keyboard.pressed_keys.contains(key),
        InteractionControl::MouseButton(button) => snapshot.mouse.buttons.contains(button),
    }
}

fn contains_key_control<T>(controls: &BTreeMap<InteractionControl, T>, key: &str) -> bool {
    controls
        .keys()
        .any(|control| matches!(control, InteractionControl::Key(value) if value == key))
}

fn contains_key_control_set(controls: &BTreeSet<InteractionControl>, key: &str) -> bool {
    controls
        .iter()
        .any(|control| matches!(control, InteractionControl::Key(value) if value == key))
}

fn contains_mouse_control<T>(controls: &BTreeMap<InteractionControl, T>, button: &str) -> bool {
    controls
        .keys()
        .any(|control| matches!(control, InteractionControl::MouseButton(value) if value == button))
}

fn contains_mouse_control_set(controls: &BTreeSet<InteractionControl>, button: &str) -> bool {
    controls
        .iter()
        .any(|control| matches!(control, InteractionControl::MouseButton(value) if value == button))
}

fn push_unique_reused(output: &mut Vec<String>, length: &mut usize, value: &str) {
    if output[..*length].iter().any(|existing| existing == value) {
        return;
    }
    push_reused(output, length, value);
}

fn push_reused(output: &mut Vec<String>, length: &mut usize, value: &str) {
    if let Some(slot) = output.get_mut(*length) {
        slot.clear();
        slot.push_str(value);
    } else {
        output.push(value.to_owned());
    }
    *length += 1;
}

fn push_event_reused(
    output: &mut Vec<TimedInputEvent>,
    length: &mut usize,
    event: &TimedInputEvent,
) {
    if let Some(slot) = output.get_mut(*length) {
        slot.clone_from(event);
    } else {
        output.push(event.clone());
    }
    *length += 1;
}

fn push_owned_event_reused(
    output: &mut Vec<TimedInputEvent>,
    length: &mut usize,
    mut event: TimedInputEvent,
) {
    if let Some(slot) = output.get_mut(*length) {
        std::mem::swap(slot, &mut event);
    } else {
        output.push(event);
    }
    *length += 1;
}

fn stable_sort_events_reused(
    events: &mut Vec<TimedInputEvent>,
    event_count: usize,
    scratch: &mut Vec<(TimedInputEvent, usize)>,
) {
    if event_count == 0 {
        scratch.extend(
            events
                .drain(..)
                .enumerate()
                .map(|(ordinal, event)| (event, ordinal)),
        );
        return;
    }

    events.truncate(event_count);
    scratch.extend(
        events
            .drain(..)
            .enumerate()
            .map(|(ordinal, event)| (event, ordinal)),
    );
    scratch.sort_unstable_by_key(|(event, ordinal)| (event.at_ms, event.seq, *ordinal));
    events.extend(scratch.drain(..).map(|(event, _)| event));
}

fn event_control(event: &TimedInputEvent) -> Option<(InteractionControl, InputButtonState)> {
    match &event.event {
        InputEvent::Key { key, state, .. } => Some((InteractionControl::Key(key.clone()), *state)),
        InputEvent::MouseButton { button, state, .. } => {
            Some((InteractionControl::MouseButton(button.clone()), *state))
        }
        _ => None,
    }
}

fn synthetic_release(press: &TimedInputEvent, now_ms: u64) -> TimedInputEvent {
    let mut release = press.clone();
    match &mut release.event {
        InputEvent::Key { state, .. }
        | InputEvent::MouseButton { state, .. }
        | InputEvent::MidiNote { state, .. } => *state = InputButtonState::Released,
        InputEvent::MouseWheel { .. }
        | InputEvent::PointerScroll { .. }
        | InputEvent::MidiControlChange { .. }
        | InputEvent::MidiPitchBend { .. }
        | InputEvent::MidiRealtime { .. } => {
            unreachable!("only held controls have synthesized releases")
        }
    }
    release.at_ms = now_ms;
    release.seq = 0;
    release.repeat_count = 1;
    release
}
