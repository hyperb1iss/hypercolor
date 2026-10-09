//! Driver-fed input from hardware that Hypercolor also lights.
//!
//! Touch surfaces, keybeds, and control buttons on driver-owned devices
//! publish through [`DeviceInputHandle`]. Each attached device is its own
//! routable child publication, so one device lifetime is one source
//! incarnation: detaching it lets the router cancel whatever it held, and a
//! busy surface can never evict another device's edges from a shared ring.
//!
//! Device input is not host keystroke capture, so `[input].enabled` does not
//! gate it. It flows only while some consumer demands interaction. Each
//! child always tracks what is physically held, but only *announces* a press
//! (publishes its begin, hold, and end) while demanded. Losing demand cancels
//! every announced hold with explicit edges, and a press that began while
//! undemanded stays invisible until it lifts, so the router never sees a hold
//! or a release it did not observe being pressed.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use arc_swap::ArcSwap;
use hypercolor_driver_api::{DeviceInputPublisher, DeviceInputSink};
use hypercolor_types::device::DeviceId;
use hypercolor_types::device_input::{DeviceInputEdge, TouchPosition};
use hypercolor_types::event::{InputButtonState, InputEvent, TimedInputEvent, TouchPhase};
use tracing::warn;

use crate::input::graph::{InputEventRead, InputPublicationRead, InputPublicationSlot};
use crate::input::input_mono_ms;
use crate::input::routing::{
    InteractionRouteRead, InteractionRouteSlot, InteractionRouteSnapshot,
    ReusedInteractionRouteRead,
};
use crate::input::traits::{DeviceButtonHold, InputData, InteractionData, TouchContact};
use crate::input::{
    Q16_16_SCALE, SourceKind, SourceSessionWriter, SourceStatusHandle, SourceStatusWriter,
};

const DEFAULT_EVENT_LIMIT: usize = 256;
/// Protocol-level contact indices on current touch hardware fit in five bits.
const MAX_CONTACTS_PER_DEVICE: usize = 32;
const MAX_HELD_BUTTONS_PER_DEVICE: usize = 16;

/// Opaque lifetime identity for one device child publication.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeviceInputPublicationId(u64);

impl DeviceInputPublicationId {
    /// Numeric identity within the device-publication namespace.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Device child registry operation failure.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum DeviceInputRegistryError {
    /// The addressed child has closed or been replaced by a newer attachment.
    #[error("device input child is closed")]
    ChildClosed,
}

struct HeldTouch {
    position: TouchPosition,
    announced: bool,
}

struct DeviceChildState {
    touches: BTreeMap<u32, HeldTouch>,
    /// Held buttons and whether each press was announced.
    buttons: BTreeMap<Arc<str>, bool>,
    publishing: bool,
    generation: u64,
}

/// Lock-free routable publication for one device.
#[derive(Clone)]
pub struct DeviceInputChildSlot {
    inner: Arc<DeviceInputChildSlotInner>,
}

struct DeviceInputChildSlotInner {
    device_id: DeviceId,
    publication_id: DeviceInputPublicationId,
    source_id: Arc<str>,
    label: Arc<str>,
    status: SourceStatusHandle,
    active: AtomicBool,
    state: Mutex<DeviceChildState>,
    publication: InputPublicationSlot,
}

impl DeviceInputChildSlot {
    fn new(
        device_id: DeviceId,
        publication_id: DeviceInputPublicationId,
        label: Arc<str>,
        status: SourceStatusHandle,
        demanded: bool,
        event_capacity: usize,
    ) -> Self {
        let publication = InputPublicationSlot::new(event_capacity);
        publication.publish_batch(
            Some(Arc::new(InputData::Interaction(InteractionData::default()))),
            &mut Vec::new(),
        );
        Self {
            inner: Arc::new(DeviceInputChildSlotInner {
                device_id,
                publication_id,
                source_id: Arc::from(format!("device:{device_id}")),
                label,
                status,
                active: AtomicBool::new(true),
                state: Mutex::new(DeviceChildState {
                    touches: BTreeMap::new(),
                    buttons: BTreeMap::new(),
                    publishing: demanded,
                    generation: 0,
                }),
                publication,
            }),
        }
    }

    /// Device whose input this child publishes.
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        self.inner.device_id
    }

    /// Opaque incarnation that changes after close and reattach.
    #[must_use]
    pub fn publication_id(&self) -> DeviceInputPublicationId {
        self.inner.publication_id
    }

    /// Source id stamped onto this child's routed events.
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.inner.source_id
    }

    /// Human-readable device label supplied by the driver.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.inner.label
    }

    /// Registry health shared by every device child.
    #[must_use]
    pub fn status(&self) -> &SourceStatusHandle {
        &self.inner.status
    }

    /// Whether this incarnation still accepts input.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.inner.active.load(Ordering::Acquire)
    }

    /// Load the latest held-state publication.
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

    fn inject(&self, edges: &[DeviceInputEdge]) -> Result<(), DeviceInputRegistryError> {
        if !self.is_active() {
            return Err(DeviceInputRegistryError::ChildClosed);
        }
        let mut state = lock_or_recover(&self.inner.state);
        if !self.is_active() {
            return Err(DeviceInputRegistryError::ChildClosed);
        }

        let mut fold = self.fold();
        for edge in edges {
            fold.apply(&mut state, edge);
        }
        self.publish_fold(&mut state, fold);
        Ok(())
    }

    /// Apply a demand change under the child lock.
    ///
    /// Losing demand cancels every announced hold with explicit edges.
    /// Regaining it announces nothing; presses already down stay invisible
    /// until they lift and land again.
    fn sync_demand(&self, demanded: bool) {
        let mut state = lock_or_recover(&self.inner.state);
        if state.publishing == demanded {
            return;
        }
        let mut fold = self.fold();
        if !demanded {
            fold.cancel_announced(&mut state);
        }
        state.publishing = demanded;
        if !demanded {
            // Publish the cancels even though the child now stays quiet.
            self.publish_locked(&mut state, fold);
        }
    }

    fn fold(&self) -> EdgeFold<'_> {
        EdgeFold {
            device_id: self.inner.device_id,
            source_id: &self.inner.source_id,
            at_ms: input_mono_ms(),
            events: Vec::new(),
            dropped: 0,
            changed: false,
        }
    }

    fn publish_fold(&self, state: &mut DeviceChildState, fold: EdgeFold<'_>) {
        if state.publishing {
            self.publish_locked(state, fold);
        }
    }

    fn publish_locked(&self, state: &mut DeviceChildState, mut fold: EdgeFold<'_>) {
        if !fold.changed && fold.dropped == 0 {
            return;
        }
        state.generation = state
            .generation
            .checked_add(1)
            .expect("device child generation exhausted");
        let snapshot = build_child_snapshot(self.inner.device_id, state, fold.dropped);
        self.inner.publication.publish_batch(
            Some(Arc::new(InputData::Interaction(snapshot))),
            &mut fold.events,
        );
    }

    fn deactivate(&self) {
        self.inner.active.store(false, Ordering::Release);
    }
}

impl InteractionRouteSlot for DeviceInputChildSlot {
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

/// Immutable view of all attached device publications.
pub struct DeviceInputRegistrySnapshot {
    generation: u64,
    children: Arc<[DeviceInputChildSlot]>,
}

impl DeviceInputRegistrySnapshot {
    /// Registry generation, incremented by every attach and detach.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Attached children in `DeviceId` order.
    #[must_use]
    pub fn children(&self) -> &[DeviceInputChildSlot] {
        &self.children
    }

    /// Resolve the attached child for one device.
    #[must_use]
    pub fn child(&self, device_id: DeviceId) -> Option<&DeviceInputChildSlot> {
        self.children
            .binary_search_by(|child| child.device_id().cmp(&device_id))
            .ok()
            .map(|index| &self.children[index])
    }
}

/// Lock-free reader of the attached device publications.
#[derive(Clone)]
pub struct DeviceInputRegistryHandle {
    inner: Arc<DeviceInputRegistryInner>,
}

struct DeviceInputRegistryInner {
    latest: ArcSwap<DeviceInputRegistrySnapshot>,
    writer: Mutex<DeviceInputRegistryWriter>,
    status: SourceStatusHandle,
    demanded: AtomicBool,
    /// Lock order: `control`, then `writer`, then a child's state.
    control: Mutex<DeviceInputStatusControl>,
}

struct DeviceInputRegistryWriter {
    generation: u64,
    next_publication_id: u64,
    event_capacity: usize,
    children: BTreeMap<DeviceId, DeviceInputChildSlot>,
}

impl DeviceInputRegistryHandle {
    fn new(status_writer: SourceStatusWriter, status: SourceStatusHandle) -> Self {
        Self {
            inner: Arc::new(DeviceInputRegistryInner {
                latest: ArcSwap::from_pointee(DeviceInputRegistrySnapshot {
                    generation: 0,
                    children: Arc::from([]),
                }),
                writer: Mutex::new(DeviceInputRegistryWriter {
                    generation: 0,
                    next_publication_id: 1,
                    event_capacity: DEFAULT_EVENT_LIMIT,
                    children: BTreeMap::new(),
                }),
                status,
                demanded: AtomicBool::new(false),
                control: Mutex::new(DeviceInputStatusControl {
                    writer: status_writer,
                    session: None,
                    graph_generation: 0,
                }),
            }),
        }
    }

    /// Load the attached children without acquiring the writer lock.
    #[must_use]
    pub fn snapshot(&self) -> Arc<DeviceInputRegistrySnapshot> {
        self.inner.latest.load_full()
    }

    fn attach(&self, device_id: DeviceId, label: Arc<str>) -> DeviceInputAttachment {
        let mut writer = lock_or_recover(&self.inner.writer);
        // A new attachment is a new session for the device. The previous
        // incarnation closes so the router cancels anything it still held.
        if let Some(previous) = writer.children.remove(&device_id) {
            previous.deactivate();
        }
        let publication_id = DeviceInputPublicationId(writer.next_publication_id);
        writer.next_publication_id = writer
            .next_publication_id
            .checked_add(1)
            .expect("device publication id exhausted");
        let child = DeviceInputChildSlot::new(
            device_id,
            publication_id,
            label,
            self.inner.status.clone(),
            self.inner.demanded.load(Ordering::Acquire),
            writer.event_capacity,
        );
        writer.children.insert(device_id, child.clone());
        publish_registry(&self.inner, &mut writer);
        drop(writer);
        self.refresh_live_resources();
        DeviceInputAttachment {
            registry: self.clone(),
            child,
            closed: AtomicBool::new(false),
        }
    }

    fn close(&self, device_id: DeviceId, publication_id: DeviceInputPublicationId) -> bool {
        let mut writer = lock_or_recover(&self.inner.writer);
        let Some(child) = writer.children.get(&device_id) else {
            return false;
        };
        if child.publication_id() != publication_id {
            return false;
        }
        child.deactivate();
        writer.children.remove(&device_id);
        publish_registry(&self.inner, &mut writer);
        drop(writer);
        self.refresh_live_resources();
        true
    }

    fn attached_count(&self) -> usize {
        self.snapshot().children().len()
    }

    /// Record demand, apply it to every child, and follow it in the shared
    /// status. Holding the writer lock serializes this with attach, so a
    /// child created mid-change still sees the new demand.
    fn set_demanded(&self, demanded: bool) {
        let mut control = lock_or_recover(&self.inner.control);
        {
            let writer = lock_or_recover(&self.inner.writer);
            if self.inner.demanded.swap(demanded, Ordering::AcqRel) == demanded {
                return;
            }
            for child in writer.children.values() {
                child.sync_demand(demanded);
            }
        }
        if let Err(error) = control.writer.set_policy(true, true, demanded) {
            warn!(%error, "device input status rejected a demand change");
        }
        if !demanded {
            control.session = None;
            return;
        }
        // Every re-demand needs a fresh session: revoking demand ended the
        // previous one.
        control.graph_generation = control.graph_generation.saturating_add(1);
        match control.writer.begin_session(control.graph_generation) {
            Ok(session) => {
                session.mark_event_driven_live_without_deadline(self.attached_count());
                control.session = Some(session);
            }
            Err(error) => warn!(%error, "device input status session did not start"),
        }
    }

    fn refresh_live_resources(&self) {
        let control = lock_or_recover(&self.inner.control);
        if let Some(session) = &control.session {
            session.mark_event_driven_live_without_deadline(self.attached_count());
        }
    }
}

/// Incarnation-fenced publisher for one attached device.
///
/// Dropping the attachment detaches the device. A later attachment for the
/// same device replaces this one, after which [`Self::inject`] reports
/// [`DeviceInputRegistryError::ChildClosed`].
pub struct DeviceInputAttachment {
    registry: DeviceInputRegistryHandle,
    child: DeviceInputChildSlot,
    closed: AtomicBool,
}

impl DeviceInputAttachment {
    /// Device bound to this attachment.
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        self.child.device_id()
    }

    /// Opaque publication incarnation bound to this attachment.
    #[must_use]
    pub fn publication_id(&self) -> DeviceInputPublicationId {
        self.child.publication_id()
    }

    /// Lock-free routable child publication.
    #[must_use]
    pub fn slot(&self) -> DeviceInputChildSlot {
        self.child.clone()
    }

    /// Fold one ordered edge batch into this device's publication.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceInputRegistryError::ChildClosed`] once this
    /// attachment has closed or a newer one replaced it.
    pub fn inject(&self, edges: &[DeviceInputEdge]) -> Result<(), DeviceInputRegistryError> {
        self.child.inject(edges)
    }

    /// Detach this incarnation without affecting a later attachment.
    pub fn close(&self) -> bool {
        if self.closed.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.registry
            .close(self.child.device_id(), self.child.publication_id())
    }
}

impl Drop for DeviceInputAttachment {
    fn drop(&mut self) {
        self.close();
    }
}

impl DeviceInputPublisher for DeviceInputAttachment {
    fn publish(&self, edges: &[DeviceInputEdge]) -> bool {
        self.inject(edges).is_ok()
    }
}

struct DeviceInputStatusControl {
    writer: SourceStatusWriter,
    session: Option<SourceSessionWriter>,
    graph_generation: u64,
}

/// Cloneable owner of the always-attached device input registry.
#[derive(Clone)]
pub struct DeviceInputHandle {
    registry: DeviceInputRegistryHandle,
}

impl DeviceInputHandle {
    /// Create an empty device input registry that starts undemanded.
    #[must_use]
    pub fn new() -> Self {
        let (writer, status) = SourceStatusWriter::new(
            "device_input",
            SourceKind::Interaction,
            "device",
            true,
            true,
            false,
        );
        Self {
            registry: DeviceInputRegistryHandle::new(writer, status),
        }
    }

    /// Attach a device, replacing any earlier attachment for it.
    #[must_use]
    pub fn attach(&self, device_id: DeviceId, label: &str) -> DeviceInputAttachment {
        self.registry.attach(device_id, Arc::from(label))
    }

    /// Clone the lock-free attached-device registry.
    #[must_use]
    pub fn registry(&self) -> DeviceInputRegistryHandle {
        self.registry.clone()
    }

    /// Whether some consumer currently demands interaction.
    #[must_use]
    pub fn is_demanded(&self) -> bool {
        self.registry.inner.demanded.load(Ordering::Acquire)
    }

    /// Follow interaction demand from the render loop.
    ///
    /// Losing demand cancels announced holds so nothing stays pressed and
    /// no device traffic reaches the bus. Presses already down when demand
    /// returns stay invisible until they lift and land again. Cheap to call
    /// every frame; unchanged demand returns immediately.
    pub fn set_demanded(&self, demanded: bool) {
        if self.is_demanded() != demanded {
            self.registry.set_demanded(demanded);
        }
    }
}

impl Default for DeviceInputHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceInputSink for DeviceInputHandle {
    fn attach(&self, device_id: DeviceId, label: &str) -> Box<dyn DeviceInputPublisher> {
        Box::new(Self::attach(self, device_id, label))
    }
}

fn publish_registry(inner: &DeviceInputRegistryInner, writer: &mut DeviceInputRegistryWriter) {
    writer.generation = writer
        .generation
        .checked_add(1)
        .expect("device registry generation exhausted");
    let children: Arc<[DeviceInputChildSlot]> = writer.children.values().cloned().collect();
    inner.latest.store(Arc::new(DeviceInputRegistrySnapshot {
        generation: writer.generation,
        children,
    }));
}

struct EdgeFold<'a> {
    device_id: DeviceId,
    source_id: &'a str,
    at_ms: u64,
    events: Vec<TimedInputEvent>,
    dropped: u32,
    changed: bool,
}

impl EdgeFold<'_> {
    fn apply(&mut self, state: &mut DeviceChildState, edge: &DeviceInputEdge) {
        let announce = state.publishing;
        match edge {
            DeviceInputEdge::TouchBegan { contact, position } => {
                let Some(position) = sanitize_position(*position, None) else {
                    self.dropped = self.dropped.saturating_add(1);
                    return;
                };
                if let Some(previous) = state.touches.remove(contact) {
                    if previous.announced {
                        self.touch(*contact, TouchPhase::Ended, previous.position);
                    }
                } else if state.touches.len() >= MAX_CONTACTS_PER_DEVICE {
                    self.dropped = self.dropped.saturating_add(1);
                    return;
                }
                state.touches.insert(
                    *contact,
                    HeldTouch {
                        position,
                        announced: announce,
                    },
                );
                if announce {
                    self.touch(*contact, TouchPhase::Began, position);
                }
            }
            DeviceInputEdge::TouchMoved { contact, position } => {
                // A contact whose begin was never seen, such as one already
                // down when the device attached, has no hold to move.
                if let Some(held) = state.touches.get_mut(contact)
                    && let Some(position) = sanitize_position(*position, Some(held.position))
                    && position != held.position
                {
                    held.position = position;
                    self.changed |= held.announced;
                }
            }
            DeviceInputEdge::TouchEnded { contact, position } => {
                // An end is never dropped; a bad final position falls back to
                // the last good one.
                if let Some(held) = state.touches.remove(contact)
                    && held.announced
                {
                    let position =
                        sanitize_position(*position, Some(held.position)).unwrap_or(held.position);
                    self.touch(*contact, TouchPhase::Ended, position);
                }
            }
            DeviceInputEdge::Button {
                button,
                state: InputButtonState::Pressed,
            } => {
                if state.buttons.contains_key(button) {
                    return;
                }
                if state.buttons.len() >= MAX_HELD_BUTTONS_PER_DEVICE {
                    self.dropped = self.dropped.saturating_add(1);
                    return;
                }
                state.buttons.insert(Arc::clone(button), announce);
                if announce {
                    self.button(button, InputButtonState::Pressed);
                }
            }
            DeviceInputEdge::Button {
                button,
                state: InputButtonState::Released,
            } => {
                if state.buttons.remove(button) == Some(true) {
                    self.button(button, InputButtonState::Released);
                }
            }
            // Device buttons report edges, not autorepeat.
            DeviceInputEdge::Button {
                state: InputButtonState::Repeated,
                ..
            } => {}
        }
    }

    /// Cancel every announced hold while keeping the physical state.
    fn cancel_announced(&mut self, state: &mut DeviceChildState) {
        for (contact, held) in &mut state.touches {
            if held.announced {
                held.announced = false;
                self.touch(*contact, TouchPhase::Cancelled, held.position);
            }
        }
        for (button, announced) in &mut state.buttons {
            if *announced {
                *announced = false;
                self.button(button, InputButtonState::Released);
            }
        }
    }

    fn touch(&mut self, contact: u32, phase: TouchPhase, position: TouchPosition) {
        self.changed = true;
        self.events.push(timed_event(
            InputEvent::Touch {
                source_id: self.source_id.to_owned(),
                device_id: self.device_id,
                contact,
                phase,
                x_q16_16: unit_to_q16_16(position.x),
                y_q16_16: unit_to_q16_16(position.y),
                pressure_q16_16: unit_to_q16_16(position.pressure),
            },
            self.at_ms,
        ));
    }

    fn button(&mut self, button: &str, state: InputButtonState) {
        self.changed = true;
        self.events.push(timed_event(
            InputEvent::DeviceButton {
                source_id: self.source_id.to_owned(),
                device_id: self.device_id,
                button: button.to_owned(),
                state,
            },
            self.at_ms,
        ));
    }
}

fn build_child_snapshot(
    device_id: DeviceId,
    state: &DeviceChildState,
    dropped: u32,
) -> InteractionData {
    let mut data = InteractionData::default();
    data.device.touches = state
        .touches
        .iter()
        .filter(|(_, held)| held.announced)
        .map(|(contact, held)| TouchContact {
            device_id,
            contact: *contact,
            x: held.position.x,
            y: held.position.y,
            pressure: held.position.pressure,
        })
        .collect();
    data.device.buttons = state
        .buttons
        .iter()
        .filter(|(_, announced)| **announced)
        .map(|(button, _)| DeviceButtonHold {
            device_id,
            button: Arc::clone(button),
        })
        .collect();
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

/// Clamp a reported position, substituting `fallback` for non-finite input.
fn sanitize_position(
    position: TouchPosition,
    fallback: Option<TouchPosition>,
) -> Option<TouchPosition> {
    if position.x.is_finite() && position.y.is_finite() && position.pressure.is_finite() {
        Some(TouchPosition {
            x: position.x.clamp(0.0, 1.0),
            y: position.y.clamp(0.0, 1.0),
            pressure: position.pressure.clamp(0.0, 1.0),
        })
    } else {
        fallback
    }
}

/// Convert a clamped unit value to Q16.16.
fn unit_to_q16_16(value: f32) -> i64 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "sanitized values lie in [0, 1], so the scaled result fits in i64"
    )]
    {
        (f64::from(value) * Q16_16_SCALE as f64).round() as i64
    }
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
