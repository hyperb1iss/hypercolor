//! Browser-preview input injection.
//!
//! A focused effect preview in the web UI can drive an effect's input
//! without any host-capture permission: the browser posts pointer and key
//! edges over an authorized WebSocket message, and this source folds them
//! into the same [`InteractionData`] contract the host backends produce.
//!
//! Injected sources are addressed by a server-assigned connection incarnation
//! and preview ID so browser pointers never implicitly merge with each other
//! or with host input.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use arc_swap::ArcSwap;

use crate::input::graph::{InputEventRead, InputPublicationRead, InputPublicationSlot};
use crate::input::input_mono_ms;
use crate::input::routing::{
    InteractionRouteRead, InteractionRouteSlot, InteractionRouteSnapshot,
    ReusedInteractionRouteRead,
};
use crate::input::traits::{InputData, InteractionData, MotionAggregate, PointerMode};
use crate::input::{SourceKind, SourceStatusHandle, SourceStatusWriter};
use hypercolor_types::event::{
    InputButtonState, InputEvent, PointerScrollPhase, PointerScrollUnit, TimedInputEvent,
};

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
    /// Exact two-axis scroll motion.
    Scroll {
        delta_x_q16_16: i64,
        delta_y_q16_16: i64,
        unit: PointerScrollUnit,
        phase: PointerScrollPhase,
        momentum_phase: PointerScrollPhase,
    },
}

/// One server-assigned WebSocket connection lifetime.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BrowserConnectionIncarnation {
    value: u64,
}

impl BrowserConnectionIncarnation {
    /// Create a server connection incarnation.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self { value }
    }

    /// Numeric connection token supplied by the server.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.value
    }
}

/// Client-chosen preview identity scoped to one WebSocket connection.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BrowserPreviewId(Arc<str>);

impl BrowserPreviewId {
    /// Wrap one opaque client preview id.
    #[must_use]
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }

    /// Borrow the opaque client value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for BrowserPreviewId {
    fn from(value: String) -> Self {
        Self::new(Arc::<str>::from(value))
    }
}

/// Address of one interactive preview publication.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BrowserInputChildKey {
    connection: BrowserConnectionIncarnation,
    preview_id: BrowserPreviewId,
}

impl BrowserInputChildKey {
    /// Build a connection-scoped preview key.
    #[must_use]
    pub fn new(
        connection: BrowserConnectionIncarnation,
        preview_id: impl Into<BrowserPreviewId>,
    ) -> Self {
        Self {
            connection,
            preview_id: preview_id.into(),
        }
    }

    /// Server connection lifetime in this key.
    #[must_use]
    pub const fn connection(&self) -> BrowserConnectionIncarnation {
        self.connection
    }

    /// Client preview id in this key.
    #[must_use]
    pub fn preview_id(&self) -> &BrowserPreviewId {
        &self.preview_id
    }
}

/// Opaque lifetime identity for one child publication.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BrowserInputPublicationId(u64);

impl BrowserInputPublicationId {
    /// Numeric identity within the browser-publication namespace.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Browser child registry operation failure.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum BrowserInputRegistryError {
    /// The addressed child has already closed or been replaced.
    #[error("browser input child is closed")]
    ChildClosed,
}

#[derive(Default)]
struct BrowserChildState {
    pressed_keys: BTreeSet<String>,
    held_buttons: BTreeSet<String>,
    cursor: Option<(f32, f32)>,
    generation: u64,
}

/// Lock-free routable publication for one browser preview.
#[derive(Clone)]
pub struct BrowserInputChildSlot {
    inner: Arc<BrowserInputChildSlotInner>,
}

struct BrowserInputChildSlotInner {
    key: BrowserInputChildKey,
    publication_id: BrowserInputPublicationId,
    source_id: Arc<str>,
    status: SourceStatusHandle,
    active: AtomicBool,
    state: Mutex<BrowserChildState>,
    publication: InputPublicationSlot,
}

impl BrowserInputChildSlot {
    fn new(
        key: BrowserInputChildKey,
        publication_id: BrowserInputPublicationId,
        source_id: Arc<str>,
        status: SourceStatusHandle,
        event_capacity: usize,
    ) -> Self {
        let publication = InputPublicationSlot::new(event_capacity);
        publication.publish_batch(
            Some(Arc::new(InputData::Interaction(InteractionData::default()))),
            &mut Vec::new(),
        );
        Self {
            inner: Arc::new(BrowserInputChildSlotInner {
                key,
                publication_id,
                source_id,
                status,
                active: AtomicBool::new(true),
                state: Mutex::new(BrowserChildState::default()),
                publication,
            }),
        }
    }

    /// Structured connection and preview address.
    #[must_use]
    pub fn key(&self) -> &BrowserInputChildKey {
        &self.inner.key
    }

    /// Opaque incarnation that changes after close and reattach.
    #[must_use]
    pub fn publication_id(&self) -> BrowserInputPublicationId {
        self.inner.publication_id
    }

    /// Diagnostic source id stamped onto discrete events.
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.inner.source_id
    }

    /// Always-live browser registry health shared by this child.
    #[must_use]
    pub fn status(&self) -> &SourceStatusHandle {
        &self.inner.status
    }

    /// Whether this incarnation still accepts input.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.inner.active.load(Ordering::Acquire)
    }

    /// Load the latest held-state and motion publication.
    #[must_use]
    pub fn latest(&self) -> Option<Arc<InputData>> {
        self.inner.publication.latest()
    }

    /// Append retained events newer than `cursor` without consuming them.
    pub fn read_events_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> InputEventRead {
        self.inner.publication.read_events_since(cursor, output)
    }

    /// Cursor immediately after the newest retained event.
    #[must_use]
    pub fn event_cursor(&self) -> u64 {
        self.inner.publication.event_cursor()
    }

    /// Atomically read one held-state and bounded-event revision.
    pub fn read_publication_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> InputPublicationRead {
        self.inner
            .publication
            .read_publication_since(cursor, output)
    }

    fn inject(
        &self,
        edges: impl IntoIterator<Item = BrowserInputEdge>,
    ) -> Result<(), BrowserInputRegistryError> {
        if !self.is_active() {
            return Err(BrowserInputRegistryError::ChildClosed);
        }
        let mut state = lock_or_recover(&self.inner.state);
        if !self.is_active() {
            return Err(BrowserInputRegistryError::ChildClosed);
        }

        let mut events = Vec::new();
        let mut recent_keys = Vec::new();
        let mut motion = MotionAggregate::default();
        let mut dropped = 0_u32;
        let at_ms = input_mono_ms();
        let mut edge_count = 0_usize;
        for edge in edges {
            edge_count = edge_count.saturating_add(1);
            fold_child_edge(
                &mut state,
                &self.inner.source_id,
                edge,
                at_ms,
                &mut events,
                &mut recent_keys,
                &mut motion,
                &mut dropped,
            );
        }
        if edge_count == 0 {
            return Ok(());
        }

        state.generation = state
            .generation
            .checked_add(1)
            .expect("browser child generation exhausted");
        let snapshot = build_child_snapshot(&state, recent_keys, motion, dropped);
        self.inner.publication.publish_batch(
            Some(Arc::new(InputData::Interaction(snapshot))),
            &mut events,
        );
        Ok(())
    }

    fn deactivate(&self) {
        self.inner.active.store(false, Ordering::Release);
    }

    fn retire(&self) {
        let mut state = lock_or_recover(&self.inner.state);
        state.pressed_keys.clear();
        state.held_buttons.clear();
        state.cursor = None;
        state.generation = state
            .generation
            .checked_add(1)
            .expect("browser child generation exhausted");
        self.inner.publication.publish_batch(
            Some(Arc::new(InputData::Interaction(build_child_snapshot(
                &state,
                Vec::new(),
                MotionAggregate::default(),
                0,
            )))),
            &mut Vec::new(),
        );
    }
}

impl InteractionRouteSlot for BrowserInputChildSlot {
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
        let (publication, event_count) = self
            .inner
            .publication
            .read_publication_reusing_since(cursor, output, 0);
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

/// Immutable view of all active browser child publications.
pub struct BrowserInputRegistrySnapshot {
    generation: u64,
    children: Arc<[BrowserInputChildSlot]>,
}

impl BrowserInputRegistrySnapshot {
    /// Registry generation, incremented by every attach and detach.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Active children in deterministic key order.
    #[must_use]
    pub fn children(&self) -> &[BrowserInputChildSlot] {
        &self.children
    }

    /// Resolve one exact connection-scoped preview.
    #[must_use]
    pub fn child(&self, key: &BrowserInputChildKey) -> Option<&BrowserInputChildSlot> {
        self.children
            .binary_search_by(|child| child.key().cmp(key))
            .ok()
            .map(|index| &self.children[index])
    }
}

#[derive(Clone)]
pub struct BrowserInputRegistryHandle {
    inner: Arc<BrowserInputRegistryInner>,
}

struct BrowserInputRegistryInner {
    latest: ArcSwap<BrowserInputRegistrySnapshot>,
    writer: Mutex<BrowserInputRegistryWriter>,
    status: SourceStatusHandle,
}

struct BrowserInputRegistryWriter {
    generation: u64,
    next_publication_id: u64,
    event_capacity: usize,
    children: BTreeMap<BrowserInputChildKey, BrowserInputChildSlot>,
    leases: BTreeMap<BrowserInputChildKey, Arc<BrowserInputLease>>,
}

impl BrowserInputRegistryHandle {
    fn new(status: SourceStatusHandle) -> Self {
        Self {
            inner: Arc::new(BrowserInputRegistryInner {
                latest: ArcSwap::from_pointee(BrowserInputRegistrySnapshot {
                    generation: 0,
                    children: Arc::from([]),
                }),
                writer: Mutex::new(BrowserInputRegistryWriter {
                    generation: 0,
                    next_publication_id: 1,
                    event_capacity: DEFAULT_EVENT_LIMIT,
                    children: BTreeMap::new(),
                    leases: BTreeMap::new(),
                }),
                status,
            }),
        }
    }

    /// Load the active child registry without acquiring its writer lock.
    #[must_use]
    pub fn snapshot(&self) -> Arc<BrowserInputRegistrySnapshot> {
        self.inner.latest.load_full()
    }

    fn attach(
        &self,
        key: BrowserInputChildKey,
        source_id: Arc<str>,
    ) -> Result<BrowserInputAttachment, BrowserInputRegistryError> {
        let mut writer = lock_or_recover(&self.inner.writer);
        if let Some(lease) = writer.leases.get(&key) {
            return lease
                .try_acquire()
                .then(|| BrowserInputAttachment::from_acquired(Arc::clone(lease)))
                .ok_or(BrowserInputRegistryError::ChildClosed);
        }
        let publication_id = BrowserInputPublicationId(writer.next_publication_id);
        writer.next_publication_id = writer
            .next_publication_id
            .checked_add(1)
            .expect("browser publication id exhausted");
        let child = BrowserInputChildSlot::new(
            key.clone(),
            publication_id,
            source_id,
            self.inner.status.clone(),
            writer.event_capacity,
        );
        let attachment = BrowserInputAttachment::new(self.clone(), child.clone());
        writer.children.insert(key.clone(), child);
        writer.leases.insert(key, Arc::clone(&attachment.lease));
        publish_registry(&self.inner, &mut writer);
        Ok(attachment)
    }

    fn close(&self, key: &BrowserInputChildKey, publication_id: BrowserInputPublicationId) -> bool {
        self.close_inner(key, publication_id)
    }

    fn close_inner(
        &self,
        key: &BrowserInputChildKey,
        publication_id: BrowserInputPublicationId,
    ) -> bool {
        let mut writer = lock_or_recover(&self.inner.writer);
        let Some(child) = writer.children.get(key) else {
            return false;
        };
        if child.publication_id() != publication_id {
            return false;
        }
        child.deactivate();
        let child = writer
            .children
            .remove(key)
            .expect("browser child existed during incarnation-fenced close");
        if let Some(lease) = writer.leases.remove(key) {
            lease.closed.store(true, Ordering::Release);
        }
        publish_registry(&self.inner, &mut writer);
        drop(writer);
        child.retire();
        true
    }
}

struct BrowserInputLease {
    registry: BrowserInputRegistryHandle,
    child: BrowserInputChildSlot,
    owners: AtomicUsize,
    closed: AtomicBool,
}

impl BrowserInputLease {
    fn try_acquire(&self) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        let mut owners = self.owners.load(Ordering::Acquire);
        loop {
            if owners == 0 || self.closed.load(Ordering::Acquire) {
                return false;
            }
            match self.owners.compare_exchange_weak(
                owners,
                owners
                    .checked_add(1)
                    .expect("browser attachment owner count exhausted"),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => owners = current,
            }
        }
    }

    fn release(&self) {
        let previous = self.owners.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "browser attachment owner count underflow");
        if previous == 1 && !self.closed.swap(true, Ordering::AcqRel) {
            self.registry
                .close(self.child.key(), self.child.publication_id());
        }
    }
}

/// Incarnation-fenced owner and writer for one active browser preview.
pub struct BrowserInputAttachment {
    lease: Arc<BrowserInputLease>,
}

impl BrowserInputAttachment {
    fn new(registry: BrowserInputRegistryHandle, child: BrowserInputChildSlot) -> Self {
        Self {
            lease: Arc::new(BrowserInputLease {
                registry,
                child,
                owners: AtomicUsize::new(1),
                closed: AtomicBool::new(false),
            }),
        }
    }

    fn from_acquired(lease: Arc<BrowserInputLease>) -> Self {
        Self { lease }
    }

    /// Child key bound to this attachment.
    #[must_use]
    pub fn key(&self) -> &BrowserInputChildKey {
        self.lease.child.key()
    }

    /// Opaque publication incarnation bound to this attachment.
    #[must_use]
    pub fn publication_id(&self) -> BrowserInputPublicationId {
        self.lease.child.publication_id()
    }

    /// Lock-free routable child publication.
    #[must_use]
    pub fn slot(&self) -> BrowserInputChildSlot {
        self.lease.child.clone()
    }

    /// Publish one edge batch directly to this child.
    pub fn inject(
        &self,
        edges: impl IntoIterator<Item = BrowserInputEdge>,
    ) -> Result<(), BrowserInputRegistryError> {
        self.lease.child.inject(edges)
    }

    /// Close this incarnation without affecting a later attachment at the same key.
    pub fn close(&self) -> bool {
        if self.lease.closed.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.lease.registry.close(self.key(), self.publication_id())
    }
}

impl Clone for BrowserInputAttachment {
    fn clone(&self) -> Self {
        self.lease
            .owners
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |owners| {
                Some(
                    owners
                        .checked_add(1)
                        .expect("browser attachment owner count exhausted"),
                )
            })
            .expect("browser attachment owner count updates cannot fail");
        Self {
            lease: Arc::clone(&self.lease),
        }
    }
}

impl Drop for BrowserInputAttachment {
    fn drop(&mut self) {
        self.lease.release();
    }
}

/// Cloneable control handle for the always-live browser registry.
#[derive(Clone)]
pub struct BrowserInputHandle {
    registry: BrowserInputRegistryHandle,
}

impl BrowserInputHandle {
    /// Create an always-live browser input registry.
    #[must_use]
    pub fn new() -> Self {
        let (status_writer, status) = SourceStatusWriter::new(
            "browser_input",
            SourceKind::Interaction,
            "browser",
            true,
            true,
            true,
        );
        let session = status_writer
            .begin_session(1)
            .expect("browser registry status generation starts at one");
        assert!(
            session.mark_event_driven_live_without_deadline(0),
            "browser registry session is current"
        );
        Self {
            registry: BrowserInputRegistryHandle::new(status),
        }
    }

    /// Attach or idempotently resolve one active connection-scoped preview.
    pub fn attach(
        &self,
        key: BrowserInputChildKey,
    ) -> Result<BrowserInputAttachment, BrowserInputRegistryError> {
        let source_id: Arc<str> = Arc::from(format!(
            "browser:{}:{}",
            key.connection().get(),
            key.preview_id().as_str()
        ));
        self.registry.attach(key, source_id)
    }

    /// Clone the lock-free active-child registry.
    #[must_use]
    pub fn registry(&self) -> BrowserInputRegistryHandle {
        self.registry.clone()
    }
}

impl Default for BrowserInputHandle {
    fn default() -> Self {
        Self::new()
    }
}

fn publish_registry(inner: &BrowserInputRegistryInner, writer: &mut BrowserInputRegistryWriter) {
    writer.generation = writer
        .generation
        .checked_add(1)
        .expect("browser registry generation exhausted");
    let children: Arc<[BrowserInputChildSlot]> = writer.children.values().cloned().collect();
    inner.latest.store(Arc::new(BrowserInputRegistrySnapshot {
        generation: writer.generation,
        children,
    }));
}

#[expect(clippy::too_many_arguments)]
fn fold_child_edge(
    state: &mut BrowserChildState,
    source_id: &str,
    edge: BrowserInputEdge,
    at_ms: u64,
    events: &mut Vec<TimedInputEvent>,
    recent_keys: &mut Vec<String>,
    motion: &mut MotionAggregate,
    dropped: &mut u32,
) {
    match edge {
        BrowserInputEdge::Key {
            key,
            state: edge_state,
        } => {
            match edge_state {
                InputButtonState::Pressed | InputButtonState::Repeated => {
                    if !try_press(&mut state.pressed_keys, &key, MAX_HELD_KEYS_PER_SOURCE) {
                        *dropped = dropped.saturating_add(1);
                        return;
                    }
                    if edge_state == InputButtonState::Pressed {
                        recent_keys.push(key.clone());
                    }
                }
                InputButtonState::Released => {
                    state.pressed_keys.remove(&key);
                }
            }
            events.push(timed_event(
                InputEvent::Key {
                    source_id: source_id.to_owned(),
                    key,
                    state: edge_state,
                },
                at_ms,
            ));
        }
        BrowserInputEdge::Button {
            button,
            state: edge_state,
        } => {
            match edge_state {
                InputButtonState::Pressed => {
                    if !try_press(
                        &mut state.held_buttons,
                        &button,
                        MAX_HELD_BUTTONS_PER_SOURCE,
                    ) {
                        *dropped = dropped.saturating_add(1);
                        return;
                    }
                }
                InputButtonState::Released => {
                    state.held_buttons.remove(&button);
                }
                InputButtonState::Repeated => {}
            }
            events.push(timed_event(
                InputEvent::MouseButton {
                    source_id: source_id.to_owned(),
                    button,
                    state: edge_state,
                },
                at_ms,
            ));
        }
        BrowserInputEdge::Move { norm_x, norm_y } => {
            let position = (sanitize_unit(norm_x), sanitize_unit(norm_y));
            if let Some(previous) = state.cursor {
                let dx = position.0 - previous.0;
                let dy = position.1 - previous.1;
                motion.dx += dx;
                motion.dy += dy;
                motion.distance += dx.hypot(dy);
            }
            state.cursor = Some(position);
        }
        BrowserInputEdge::Scroll {
            delta_x_q16_16,
            delta_y_q16_16,
            unit,
            phase,
            momentum_phase,
        } => fold_scroll_edge(
            source_id,
            delta_x_q16_16,
            delta_y_q16_16,
            unit,
            phase,
            momentum_phase,
            at_ms,
            events,
        ),
    }
}

#[expect(clippy::too_many_arguments)]
fn fold_scroll_edge(
    source_id: &str,
    delta_x_q16_16: i64,
    delta_y_q16_16: i64,
    unit: PointerScrollUnit,
    phase: PointerScrollPhase,
    momentum_phase: PointerScrollPhase,
    at_ms: u64,
    events: &mut Vec<TimedInputEvent>,
) {
    events.push(timed_event(
        InputEvent::PointerScroll {
            source_id: source_id.to_owned(),
            delta_x_q16_16,
            delta_y_q16_16,
            unit,
            phase,
            momentum_phase,
        },
        at_ms,
    ));
}

fn build_child_snapshot(
    state: &BrowserChildState,
    recent_keys: Vec<String>,
    motion: MotionAggregate,
    dropped: u32,
) -> InteractionData {
    let mut data = InteractionData::default();
    data.keyboard.pressed_keys = state.pressed_keys.iter().cloned().collect();
    data.keyboard.recent_keys = recent_keys;
    data.mouse.buttons = state.held_buttons.iter().cloned().collect();
    data.mouse.down = !data.mouse.buttons.is_empty();
    if let Some((norm_x, norm_y)) = state.cursor {
        data.mouse.mode = PointerMode::Absolute;
        data.mouse.norm_x = norm_x;
        data.mouse.norm_y = norm_y;
        data.mouse.injected = true;
    }
    data.batch.motion = motion;
    data.batch.dropped_events = dropped;
    data.generation = state.generation;
    data
}

fn timed_event(event: InputEvent, at_ms: u64) -> TimedInputEvent {
    TimedInputEvent {
        event,
        at_ms,
        seq: 0,
        physical_code: None,
        repeat_count: 1,
    }
}

fn try_press(held: &mut BTreeSet<String>, name: &str, limit: usize) -> bool {
    held.contains(name) || (held.len() < limit && held.insert(name.to_owned()))
}

fn sanitize_unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
