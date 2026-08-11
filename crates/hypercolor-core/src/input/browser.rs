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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use arc_swap::ArcSwap;

use crate::input::graph::{
    InputEventRead, InputPublicationRead, InputPublicationSlot, InteractionTransientTotals,
};
use crate::input::input_mono_ms;
use crate::input::routing::{
    InteractionRouteRead, InteractionRouteSlot, InteractionRouteSnapshot,
    ReusedInteractionRouteRead,
};
use crate::input::traits::{InputData, InputSource, InteractionData, MotionAggregate, PointerMode};
use crate::input::{
    InteractionSourceOrigin, LegacyWheelProjector, SourceKind, SourceStatusHandle,
    SourceStatusReporter,
};
use crate::types::event::{
    InputButtonState, InputEvent, PointerScrollPhase, PointerScrollUnit, TimedInputEvent,
};

const DEFAULT_EVENT_LIMIT: usize = 256;
const SHARED_SAMPLE_POOL_CAPACITY: usize = 2;
/// Maximum disconnected legacy children retained for compatibility releases.
pub const BROWSER_RETIRED_LEGACY_CAPACITY: usize = 256;
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
    /// Exact two-axis scroll motion.
    Scroll {
        delta_x_q16_16: i64,
        delta_y_q16_16: i64,
        unit: PointerScrollUnit,
        phase: PointerScrollPhase,
        momentum_phase: PointerScrollPhase,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum BrowserConnectionNamespace {
    Server,
    Legacy,
}

/// One server-assigned WebSocket connection lifetime.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BrowserConnectionIncarnation {
    namespace: BrowserConnectionNamespace,
    value: u64,
}

impl BrowserConnectionIncarnation {
    /// Create a server connection incarnation.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self {
            namespace: BrowserConnectionNamespace::Server,
            value,
        }
    }

    const fn legacy(value: u64) -> Self {
        Self {
            namespace: BrowserConnectionNamespace::Legacy,
            value,
        }
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
    /// The manager-owned browser source is not accepting input.
    #[error("browser input source is not active")]
    SourceInactive,
    /// The addressed child has already closed or been replaced.
    #[error("browser input child is closed")]
    ChildClosed,
}

#[derive(Default)]
struct BrowserChildState {
    pressed_keys: BTreeSet<String>,
    held_buttons: BTreeSet<String>,
    cursor: Option<(f32, f32)>,
    legacy_wheel_projector: LegacyWheelProjector,
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

    /// Manager-owned browser source health shared by this child.
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

    fn retire(&self, publish_releases: bool) {
        let mut state = lock_or_recover(&self.inner.state);
        let mut events = publish_releases.then(Vec::new).unwrap_or_default();
        if publish_releases {
            synthesize_releases(&state, &self.inner.source_id, &mut events);
        }
        state.pressed_keys.clear();
        state.held_buttons.clear();
        state.cursor = None;
        state.legacy_wheel_projector.reset();
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
            &mut events,
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
    publication_ids: Arc<[BrowserInputPublicationId]>,
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

    fn contains_publication(&self, publication_id: BrowserInputPublicationId) -> bool {
        self.publication_ids.binary_search(&publication_id).is_ok()
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
    accepting: bool,
    generation: u64,
    session_generation: u64,
    next_publication_id: u64,
    next_legacy_connection: u64,
    event_capacity: usize,
    children: BTreeMap<BrowserInputChildKey, BrowserInputChildSlot>,
    leases: BTreeMap<BrowserInputChildKey, Arc<BrowserInputLease>>,
    legacy_keys: BTreeMap<String, BrowserInputChildKey>,
    retired_legacy: VecDeque<BrowserInputChildSlot>,
}

impl BrowserInputRegistryHandle {
    fn new(status: SourceStatusHandle) -> Self {
        Self {
            inner: Arc::new(BrowserInputRegistryInner {
                latest: ArcSwap::from_pointee(BrowserInputRegistrySnapshot {
                    generation: 0,
                    children: Arc::from([]),
                    publication_ids: Arc::from([]),
                }),
                writer: Mutex::new(BrowserInputRegistryWriter {
                    accepting: false,
                    generation: 0,
                    session_generation: 0,
                    next_publication_id: 1,
                    next_legacy_connection: 1,
                    event_capacity: DEFAULT_EVENT_LIMIT,
                    children: BTreeMap::new(),
                    leases: BTreeMap::new(),
                    legacy_keys: BTreeMap::new(),
                    retired_legacy: VecDeque::new(),
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

    fn start(&self, event_capacity: usize) {
        let mut writer = lock_or_recover(&self.inner.writer);
        writer.session_generation = writer
            .session_generation
            .checked_add(1)
            .expect("browser registry session generation exhausted");
        writer.accepting = true;
        writer.event_capacity = event_capacity;
        publish_registry(&self.inner, &mut writer);
    }

    fn stop(&self) {
        let mut writer = lock_or_recover(&self.inner.writer);
        writer.accepting = false;
        let children = std::mem::take(&mut writer.children);
        let leases = std::mem::take(&mut writer.leases);
        writer.legacy_keys.clear();
        writer.retired_legacy.clear();
        for child in children.values() {
            child.deactivate();
        }
        for lease in leases.values() {
            lease.closed.store(true, Ordering::Release);
        }
        publish_registry(&self.inner, &mut writer);
        drop(writer);
        for child in children.values() {
            child.retire(false);
        }
    }

    fn attach(
        &self,
        key: BrowserInputChildKey,
        source_id: Arc<str>,
    ) -> Result<BrowserInputAttachment, BrowserInputRegistryError> {
        let mut writer = lock_or_recover(&self.inner.writer);
        if !writer.accepting {
            return Err(BrowserInputRegistryError::SourceInactive);
        }
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

    fn attach_legacy(
        &self,
        source_id: &str,
    ) -> Result<BrowserInputChildSlot, BrowserInputRegistryError> {
        let mut writer = lock_or_recover(&self.inner.writer);
        if !writer.accepting {
            return Err(BrowserInputRegistryError::SourceInactive);
        }
        if let Some(key) = writer.legacy_keys.get(source_id)
            && let Some(child) = writer.children.get(key)
        {
            return Ok(child.clone());
        }
        let connection = BrowserConnectionIncarnation::legacy(writer.next_legacy_connection);
        writer.next_legacy_connection = writer
            .next_legacy_connection
            .checked_add(1)
            .expect("legacy browser connection id exhausted");
        let key = BrowserInputChildKey::new(connection, source_id.to_owned());
        let publication_id = BrowserInputPublicationId(writer.next_publication_id);
        writer.next_publication_id = writer
            .next_publication_id
            .checked_add(1)
            .expect("browser publication id exhausted");
        let child = BrowserInputChildSlot::new(
            key.clone(),
            publication_id,
            Arc::from(source_id),
            self.inner.status.clone(),
            writer.event_capacity,
        );
        writer.legacy_keys.insert(source_id.to_owned(), key.clone());
        writer.children.insert(key, child.clone());
        publish_registry(&self.inner, &mut writer);
        Ok(child)
    }

    fn close(&self, key: &BrowserInputChildKey, publication_id: BrowserInputPublicationId) -> bool {
        self.close_inner(key, publication_id)
    }

    fn release_legacy(&self, source_id: &str) {
        let mut writer = lock_or_recover(&self.inner.writer);
        let Some(key) = writer.legacy_keys.remove(source_id) else {
            return;
        };
        let Some(child) = writer.children.remove(&key) else {
            return;
        };
        child.deactivate();
        publish_registry(&self.inner, &mut writer);
        let session_generation = writer.session_generation;
        drop(writer);
        child.retire(true);
        let mut writer = lock_or_recover(&self.inner.writer);
        if writer.accepting && writer.session_generation == session_generation {
            if writer.retired_legacy.len() == BROWSER_RETIRED_LEGACY_CAPACITY {
                writer.retired_legacy.pop_front();
            }
            writer.retired_legacy.push_back(child);
        }
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
        writer.legacy_keys.retain(|_, legacy_key| legacy_key != key);
        publish_registry(&self.inner, &mut writer);
        drop(writer);
        child.retire(false);
        true
    }

    fn take_retired_legacy(&self, output: &mut Vec<BrowserInputChildSlot>) {
        let mut writer = lock_or_recover(&self.inner.writer);
        output.extend(writer.retired_legacy.drain(..));
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

/// Cloneable control handle for the manager-owned browser registry.
#[derive(Clone)]
pub struct BrowserInputHandle {
    registry: BrowserInputRegistryHandle,
}

impl BrowserInputHandle {
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

    /// Compatibility injection for the legacy connection-wide protocol.
    pub fn inject(&self, source_id: &str, edges: impl IntoIterator<Item = BrowserInputEdge>) {
        if let Ok(child) = self.registry.attach_legacy(source_id) {
            let _ = child.inject(edges);
        }
    }

    /// Compatibility release for the legacy connection-wide protocol.
    pub fn release_source(&self, source_id: &str) {
        self.registry.release_legacy(source_id);
    }
}

/// Push-based interaction source fed by browser previews.
pub struct BrowserInputSource {
    name: String,
    running: bool,
    generation: u64,
    last_registry_generation: u64,
    registry: BrowserInputRegistryHandle,
    aggregate_event_cursors: BTreeMap<BrowserInputPublicationId, u64>,
    aggregate_transient_totals: BTreeMap<BrowserInputPublicationId, InteractionTransientTotals>,
    pending_aggregate_transients: InteractionTransientTotals,
    aggregate_latest_generations: BTreeMap<BrowserInputPublicationId, u64>,
    retained_legacy: Vec<BrowserInputChildSlot>,
    shared_samples: Vec<Arc<InputData>>,
    next_shared_sample: usize,
    event_limit: usize,
    status: SourceStatusReporter,
}

impl BrowserInputSource {
    /// Create a browser injection source.
    #[must_use]
    pub fn new() -> Self {
        let status = SourceStatusReporter::new(
            "browser_input",
            SourceKind::Interaction,
            "browser",
            true,
            true,
            true,
        );
        Self {
            name: "BrowserInput".to_owned(),
            running: false,
            generation: 0,
            last_registry_generation: 0,
            registry: BrowserInputRegistryHandle::new(status.handle()),
            aggregate_event_cursors: BTreeMap::new(),
            aggregate_transient_totals: BTreeMap::new(),
            pending_aggregate_transients: InteractionTransientTotals::default(),
            aggregate_latest_generations: BTreeMap::new(),
            retained_legacy: Vec::new(),
            shared_samples: (0..SHARED_SAMPLE_POOL_CAPACITY)
                .map(|_| Arc::new(InputData::Interaction(InteractionData::default())))
                .collect(),
            next_shared_sample: 0,
            event_limit: DEFAULT_EVENT_LIMIT,
            status,
        }
    }

    fn build_snapshot(&mut self) -> InteractionData {
        let registry = self.registry.snapshot();
        let (mut data, mut changed) = self.begin_aggregate_snapshot(&registry);
        merge_transient_delta(
            &mut data,
            std::mem::take(&mut self.pending_aggregate_transients),
        );
        let mut no_events = Vec::new();

        for child in registry.children() {
            let publication = child.read_publication_since(u64::MAX, &mut no_events);
            let transients =
                self.consume_aggregate_transients(child, publication.interaction_transients);
            self.merge_aggregate_sample(
                &mut data,
                &mut changed,
                child,
                publication.sample,
                transients,
            );
        }

        self.finish_aggregate_snapshot(data, changed)
    }

    fn drain_aggregate_events(&mut self) -> Vec<TimedInputEvent> {
        self.registry.take_retired_legacy(&mut self.retained_legacy);
        let registry = self.registry.snapshot();
        let mut retired_legacy = std::mem::take(&mut self.retained_legacy);
        let mut events = Vec::new();
        for child in registry.children() {
            let (_, child_dropped) = self.read_aggregate_child(child, &mut events);
            self.pending_aggregate_transients.dropped_events = self
                .pending_aggregate_transients
                .dropped_events
                .saturating_add(u64::from(child_dropped));
        }
        for child in &retired_legacy {
            let (publication, child_dropped) = self.read_aggregate_child(child, &mut events);
            let transients =
                self.consume_aggregate_transients(child, publication.interaction_transients);
            accumulate_transient_totals(&mut self.pending_aggregate_transients, transients);
            self.pending_aggregate_transients.dropped_events = self
                .pending_aggregate_transients
                .dropped_events
                .saturating_add(u64::from(child_dropped));
        }
        self.retain_active_aggregate_cursors(&registry);
        retired_legacy.clear();
        self.retained_legacy = retired_legacy;
        events
    }

    fn sample_and_drain_aggregate_into(
        &mut self,
        events: &mut Vec<TimedInputEvent>,
    ) -> InteractionData {
        self.registry.take_retired_legacy(&mut self.retained_legacy);
        let registry = self.registry.snapshot();
        let mut retired_legacy = std::mem::take(&mut self.retained_legacy);
        let (mut data, mut changed) = self.begin_aggregate_snapshot(&registry);
        merge_transient_delta(
            &mut data,
            std::mem::take(&mut self.pending_aggregate_transients),
        );
        let first_event = events.len();
        let mut dropped = 0_u32;

        for child in registry.children() {
            let (publication, child_dropped) = self.read_aggregate_child(child, events);
            dropped = dropped.saturating_add(child_dropped);
            let transients =
                self.consume_aggregate_transients(child, publication.interaction_transients);
            self.merge_aggregate_sample(
                &mut data,
                &mut changed,
                child,
                publication.sample,
                transients,
            );
        }
        for child in &retired_legacy {
            let (publication, child_dropped) = self.read_aggregate_child(child, events);
            dropped = dropped.saturating_add(child_dropped);
            let transients =
                self.consume_aggregate_transients(child, publication.interaction_transients);
            merge_transient_delta(&mut data, transients);
        }

        self.retain_active_aggregate_cursors(&registry);
        retired_legacy.clear();
        self.retained_legacy = retired_legacy;
        for event in &events[first_event..] {
            match &event.event {
                InputEvent::MouseWheel { delta_hi_res, .. } => {
                    data.batch.wheel_hi_res = data.batch.wheel_hi_res.saturating_add(*delta_hi_res);
                }
                InputEvent::PointerScroll {
                    delta_x_q16_16,
                    delta_y_q16_16,
                    unit,
                    ..
                } => data
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
        data.batch.dropped_events = data.batch.dropped_events.saturating_add(dropped);
        self.finish_aggregate_snapshot(data, changed)
    }

    fn begin_aggregate_snapshot(
        &mut self,
        registry: &BrowserInputRegistrySnapshot,
    ) -> (InteractionData, bool) {
        let changed = registry.generation() != self.last_registry_generation;
        self.last_registry_generation = registry.generation();
        self.aggregate_latest_generations
            .retain(|publication_id, _| registry.contains_publication(*publication_id));
        (InteractionData::default(), changed)
    }

    fn merge_aggregate_sample(
        &mut self,
        data: &mut InteractionData,
        changed: &mut bool,
        child: &BrowserInputChildSlot,
        sample: Option<Arc<InputData>>,
        transients: InteractionTransientTotals,
    ) {
        let Some(sample) = sample else {
            return;
        };
        let InputData::Interaction(snapshot) = sample.as_ref() else {
            return;
        };
        let transient_is_new = self
            .aggregate_latest_generations
            .insert(child.publication_id(), snapshot.generation)
            != Some(snapshot.generation);
        *changed |= transient_is_new;
        let mut contribution = snapshot.clone();
        contribution.batch.motion = MotionAggregate::default();
        contribution.batch.window_secs = 0.0;
        contribution.batch.dropped_events = 0;
        merge_transient_delta(&mut contribution, transients);
        if !transient_is_new {
            contribution.keyboard.recent_keys.clear();
        }
        data.merge_from(contribution);
    }

    fn finish_aggregate_snapshot(
        &mut self,
        mut data: InteractionData,
        changed: bool,
    ) -> InteractionData {
        if changed {
            self.generation = self
                .generation
                .checked_add(1)
                .expect("browser aggregate generation exhausted");
        }
        data.generation = self.generation;
        data
    }

    fn read_aggregate_child(
        &mut self,
        child: &BrowserInputChildSlot,
        events: &mut Vec<TimedInputEvent>,
    ) -> (InputPublicationRead, u32) {
        let publication_id = child.publication_id();
        let cursor = self
            .aggregate_event_cursors
            .get(&publication_id)
            .copied()
            .unwrap_or(0);
        let publication = child.read_publication_since(cursor, events);
        self.aggregate_event_cursors
            .insert(publication_id, publication.events.next_cursor);
        let dropped = u32::try_from(publication.events.dropped).unwrap_or(u32::MAX);
        (publication, dropped)
    }

    fn consume_aggregate_transients(
        &mut self,
        child: &BrowserInputChildSlot,
        current: InteractionTransientTotals,
    ) -> InteractionTransientTotals {
        let previous = self
            .aggregate_transient_totals
            .insert(child.publication_id(), current)
            .unwrap_or_default();
        current.delta_since(previous)
    }

    fn retain_active_aggregate_cursors(&mut self, registry: &BrowserInputRegistrySnapshot) {
        self.aggregate_event_cursors
            .retain(|publication_id, _| registry.contains_publication(*publication_id));
        self.aggregate_transient_totals
            .retain(|publication_id, _| registry.contains_publication(*publication_id));
    }

    fn retain_shared_sample(&mut self, snapshot: InteractionData) -> Arc<InputData> {
        for offset in 0..self.shared_samples.len() {
            let index = (self.next_shared_sample + offset) % self.shared_samples.len();
            if Arc::strong_count(&self.shared_samples[index]) != 1 {
                continue;
            }
            Arc::get_mut(&mut self.shared_samples[index])
                .expect("exclusive shared browser sample")
                .clone_from(&InputData::Interaction(snapshot));
            self.next_shared_sample = (index + 1) % self.shared_samples.len();
            return Arc::clone(&self.shared_samples[index]);
        }

        Arc::new(InputData::Interaction(snapshot))
    }

    /// A cloneable handle for WebSocket control and injection.
    #[must_use]
    pub fn handle(&self) -> BrowserInputHandle {
        BrowserInputHandle {
            registry: self.registry.clone(),
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
        let status = self.status.begin_session()?;
        self.registry.start(self.event_limit);
        self.running = true;
        if let Some(status) = status {
            status.mark_event_driven_live_without_deadline(0);
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.registry.stop();
        self.aggregate_event_cursors.clear();
        self.aggregate_transient_totals.clear();
        self.pending_aggregate_transients = InteractionTransientTotals::default();
        self.aggregate_latest_generations.clear();
        self.retained_legacy.clear();
        self.status.stop();
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if self.running {
            Ok(InputData::Interaction(self.build_snapshot()))
        } else {
            Ok(InputData::None)
        }
    }

    fn sample_and_drain_with_delta_secs(
        &mut self,
        _delta_secs: f32,
    ) -> (anyhow::Result<InputData>, Vec<TimedInputEvent>) {
        if !self.running {
            return (Ok(InputData::None), Vec::new());
        }
        let mut events = Vec::new();
        let snapshot = self.sample_and_drain_aggregate_into(&mut events);
        (Ok(InputData::Interaction(snapshot)), events)
    }

    fn sample_shared_and_drain_into(
        &mut self,
        _delta_secs: f32,
        events: &mut Vec<TimedInputEvent>,
    ) -> anyhow::Result<Option<Arc<InputData>>> {
        if !self.running {
            return Ok(None);
        }
        let snapshot = self.sample_and_drain_aggregate_into(events);
        Ok(Some(self.retain_shared_sample(snapshot)))
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

    fn interaction_source_origin(&self) -> InteractionSourceOrigin {
        InteractionSourceOrigin::BrowserCompatibilityAggregate
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
        if self.running {
            self.drain_aggregate_events()
        } else {
            Vec::new()
        }
    }
}

fn merge_transient_delta(data: &mut InteractionData, transients: InteractionTransientTotals) {
    data.batch.motion.dx += finite_f32(transients.dx);
    data.batch.motion.dy += finite_f32(transients.dy);
    data.batch.motion.distance += finite_f32(transients.distance);
    data.batch.window_secs = data
        .batch
        .window_secs
        .max(finite_f32(transients.window_secs));
    data.batch.dropped_events = data
        .batch
        .dropped_events
        .saturating_add(u32::try_from(transients.dropped_events).unwrap_or(u32::MAX))
        .saturating_add(u32::try_from(transients.invalid_events).unwrap_or(u32::MAX));
}

fn accumulate_transient_totals(
    totals: &mut InteractionTransientTotals,
    delta: InteractionTransientTotals,
) {
    totals.dx += delta.dx;
    totals.dy += delta.dy;
    totals.distance += delta.distance;
    totals.window_secs += delta.window_secs;
    totals.dropped_events = totals.dropped_events.saturating_add(delta.dropped_events);
    totals.invalid_events = totals.invalid_events.saturating_add(delta.invalid_events);
}

fn finite_f32(value: f64) -> f32 {
    value.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32
}

fn publish_registry(inner: &BrowserInputRegistryInner, writer: &mut BrowserInputRegistryWriter) {
    writer.generation = writer
        .generation
        .checked_add(1)
        .expect("browser registry generation exhausted");
    let children: Arc<[BrowserInputChildSlot]> = writer.children.values().cloned().collect();
    let mut publication_ids = children
        .iter()
        .map(BrowserInputChildSlot::publication_id)
        .collect::<Vec<_>>();
    publication_ids.sort_unstable();
    inner.latest.store(Arc::new(BrowserInputRegistrySnapshot {
        generation: writer.generation,
        children,
        publication_ids: publication_ids.into(),
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
        BrowserInputEdge::Wheel { delta_hi_res } => fold_scroll_edge(
            state,
            source_id,
            0,
            i64::from(delta_hi_res) << 16,
            PointerScrollUnit::Line120,
            PointerScrollPhase::None,
            PointerScrollPhase::None,
            at_ms,
            events,
        ),
        BrowserInputEdge::Scroll {
            delta_x_q16_16,
            delta_y_q16_16,
            unit,
            phase,
            momentum_phase,
        } => fold_scroll_edge(
            state,
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
    state: &mut BrowserChildState,
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

    if unit != PointerScrollUnit::Line120 {
        return;
    }
    let delta_hi_res = state.legacy_wheel_projector.project(delta_y_q16_16);
    if delta_hi_res == 0 {
        return;
    }
    events.push(timed_event(
        InputEvent::MouseWheel {
            source_id: source_id.to_owned(),
            delta_hi_res,
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

fn synthesize_releases(
    state: &BrowserChildState,
    source_id: &str,
    events: &mut Vec<TimedInputEvent>,
) {
    let at_ms = input_mono_ms();
    events.extend(state.pressed_keys.iter().cloned().map(|key| {
        timed_event(
            InputEvent::Key {
                source_id: source_id.to_owned(),
                key,
                state: InputButtonState::Released,
            },
            at_ms,
        )
    }));
    events.extend(state.held_buttons.iter().cloned().map(|button| {
        timed_event(
            InputEvent::MouseButton {
                source_id: source_id.to_owned(),
                button,
                state: InputButtonState::Released,
            },
            at_ms,
        )
    }));
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
    fn legacy_wheel_edges_emit_exact_scroll_then_compatibility_shadow() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        let handle = source.handle();

        handle.inject(
            "browser-1",
            [BrowserInputEdge::Wheel { delta_hi_res: -240 }],
        );
        let events = source.drain_events();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0].event,
            InputEvent::PointerScroll {
                delta_x_q16_16: 0,
                delta_y_q16_16,
                unit: PointerScrollUnit::Line120,
                phase: PointerScrollPhase::None,
                momentum_phase: PointerScrollPhase::None,
                ..
            } if delta_y_q16_16 == -240 * crate::input::Q16_16_SCALE
        ));
        assert!(matches!(
            events[1].event,
            InputEvent::MouseWheel {
                delta_hi_res: -240,
                ..
            }
        ));
    }

    #[test]
    fn exact_pixel_scroll_preserves_axes_and_phases_without_legacy_shadow() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        source.handle().inject(
            "browser-1",
            [BrowserInputEdge::Scroll {
                delta_x_q16_16: 3 * crate::input::Q16_16_SCALE,
                delta_y_q16_16: -7 * crate::input::Q16_16_SCALE,
                unit: PointerScrollUnit::Pixels,
                phase: PointerScrollPhase::Changed,
                momentum_phase: PointerScrollPhase::Began,
            }],
        );

        let (data, events) = source.sample_and_drain_with_delta_secs(0.0);
        let InputData::Interaction(data) = data.expect("sample") else {
            panic!("expected interaction data");
        };
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].event,
            InputEvent::PointerScroll {
                delta_x_q16_16,
                delta_y_q16_16,
                unit: PointerScrollUnit::Pixels,
                phase: PointerScrollPhase::Changed,
                momentum_phase: PointerScrollPhase::Began,
                ..
            } if delta_x_q16_16 == 3 * crate::input::Q16_16_SCALE
                && delta_y_q16_16 == -7 * crate::input::Q16_16_SCALE
        ));
        assert_eq!(
            data.batch.scroll.pixel_x_q16_16,
            3 * crate::input::Q16_16_SCALE
        );
        assert_eq!(
            data.batch.scroll.pixel_y_q16_16,
            -7 * crate::input::Q16_16_SCALE
        );
        assert_eq!(data.batch.wheel_hi_res, 0);
    }

    #[test]
    fn reconnect_discards_fractional_legacy_projection_state() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        let handle = source.handle();
        let half_line = BrowserInputEdge::Scroll {
            delta_x_q16_16: 0,
            delta_y_q16_16: crate::input::Q16_16_SCALE / 2,
            unit: PointerScrollUnit::Line120,
            phase: PointerScrollPhase::None,
            momentum_phase: PointerScrollPhase::None,
        };

        handle.inject("browser-1", [half_line.clone()]);
        let first = source.drain_events();
        assert_eq!(first.len(), 1);
        assert!(matches!(first[0].event, InputEvent::PointerScroll { .. }));

        handle.release_source("browser-1");
        assert!(source.drain_events().is_empty());
        handle.inject("browser-1", [half_line]);
        let reconnected = source.drain_events();
        assert_eq!(reconnected.len(), 1);
        assert!(matches!(
            reconnected[0].event,
            InputEvent::PointerScroll { .. }
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
        assert_eq!(events.len(), 4);
        assert!(matches!(
            events[0].event,
            InputEvent::PointerScroll { delta_y_q16_16, .. }
                if delta_y_q16_16 == 8 * crate::input::Q16_16_SCALE
        ));
        assert!(matches!(
            events[1].event,
            InputEvent::MouseWheel {
                delta_hi_res: 8,
                ..
            }
        ));
        assert!(matches!(
            events[2].event,
            InputEvent::PointerScroll { delta_y_q16_16, .. }
                if delta_y_q16_16 == 9 * crate::input::Q16_16_SCALE
        ));
        assert!(matches!(
            events[3].event,
            InputEvent::MouseWheel {
                delta_hi_res: 9,
                ..
            }
        ));
        assert_eq!(data.batch.dropped_events, 15);

        let next = drain_snapshot(&mut source);
        assert_eq!(next.batch.dropped_events, 0);
    }

    #[test]
    fn event_ring_reports_a_zero_limit_without_publishing() {
        let mut source = BrowserInputSource::new();
        source.event_limit = 0;
        source.start().expect("start");
        source
            .handle()
            .inject("browser-1", [BrowserInputEdge::Wheel { delta_hi_res: 120 }]);

        let (data, events) = source.sample_and_drain_with_delta_secs(0.0);
        let InputData::Interaction(data) = data.expect("sample") else {
            panic!("expected interaction data");
        };
        assert!(events.is_empty());
        assert_eq!(data.batch.dropped_events, 2);
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
    fn focus_beginning_on_repeat_establishes_held_state_without_a_recent_press() {
        let mut source = BrowserInputSource::new();
        source.start().expect("start");
        let handle = source.handle();

        handle.inject(
            "browser-1",
            [BrowserInputEdge::Key {
                key: "a".into(),
                state: InputButtonState::Repeated,
            }],
        );

        let data = drain_snapshot(&mut source);
        assert_eq!(data.keyboard.pressed_keys, vec!["a".to_owned()]);
        assert!(data.keyboard.recent_keys.is_empty());
        assert!(matches!(
            &source.drain_events()[0].event,
            InputEvent::Key {
                key,
                state: InputButtonState::Repeated,
                ..
            } if key == "a"
        ));

        handle.release_source("browser-1");
        assert!(drain_snapshot(&mut source).keyboard.pressed_keys.is_empty());
        assert!(matches!(
            &source.drain_events()[0].event,
            InputEvent::Key {
                key,
                state: InputButtonState::Released,
                ..
            } if key == "a"
        ));
    }

    #[test]
    fn stop_clears_all_state() {
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
        source.stop();
        handle.inject(
            "browser-1",
            [BrowserInputEdge::Key {
                key: "b".into(),
                state: InputButtonState::Pressed,
            }],
        );
        assert!(handle.registry().snapshot().children().is_empty());
        assert!(source.drain_events().is_empty());
        source.start().expect("restart");
        let data = drain_snapshot(&mut source);
        assert!(data.keyboard.pressed_keys.is_empty());
        handle.inject(
            "browser-1",
            [BrowserInputEdge::Key {
                key: "c".into(),
                state: InputButtonState::Pressed,
            }],
        );
        assert_eq!(source.drain_events().len(), 1);
    }
}
