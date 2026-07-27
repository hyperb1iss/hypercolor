//! Immutable input-graph publications for render-plane consumers.

use std::hint::spin_loop;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use arc_swap::{ArcSwap, ArcSwapOption};

use super::{InputData, SourceKind, SourceStatusHandle};
use crate::types::event::TimedInputEvent;

/// Maximum events retained per source between consumer reads.
pub const INPUT_EVENT_RING_CAPACITY: usize = 256;

/// Typed routing origin for one interaction source publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionSourceOrigin {
    /// Host keyboard and pointer capture.
    Host,
    /// Manager-owned compatibility aggregate over browser child slots.
    BrowserCompatibilityAggregate,
}

/// Stable latest-value and event publication for one registered source.
#[derive(Clone)]
pub struct InputSourceSlot {
    inner: Arc<InputSourceSlotInner>,
}

struct InputSourceSlotInner {
    id: u64,
    kind: Option<SourceKind>,
    interaction_origin: Option<InteractionSourceOrigin>,
    status: SourceStatusHandle,
    publication: InputPublicationSlot,
}

/// Shared latest-value and bounded-event publication primitive.
#[derive(Clone)]
pub(crate) struct InputPublicationSlot {
    inner: Arc<InputPublicationSlotInner>,
}

struct InputPublicationSlotInner {
    latest: ArcSwapOption<InputData>,
    event_slots: Box<[ArcSwapOption<InputEventEntry>]>,
    writer: Mutex<InputPublicationWriter>,
    revision: AtomicU64,
    next_cursor: AtomicU64,
    dropped_total: AtomicU64,
    interaction_transients: AtomicInteractionTransientTotals,
}

struct InputPublicationWriter {
    next_cursor: u64,
    dropped_total: u64,
    interaction_transients: InteractionTransientTotals,
}

#[derive(Default)]
struct AtomicInteractionTransientTotals {
    dx: AtomicU64,
    dy: AtomicU64,
    distance: AtomicU64,
    window_secs: AtomicU64,
    dropped_events: AtomicU64,
    invalid_events: AtomicU64,
}

#[derive(Clone)]
struct InputEventEntry {
    cursor: u64,
    event: TimedInputEvent,
}

/// Cursor result returned after reading one source's bounded event history.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InputEventRead {
    /// Cursor to use for the next read.
    pub next_cursor: u64,
    /// Events overwritten since the supplied cursor.
    pub dropped: u64,
    /// Total events dropped by this source slot since construction.
    pub dropped_total: u64,
}

/// Cumulative interaction transients for one stable source-slot lifetime.
///
/// Consumers retain their own prior totals and subtract them from the next
/// coherent publication, so superseded latest-value samples never lose motion.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InteractionTransientTotals {
    /// Cumulative horizontal pointer motion.
    pub dx: f64,
    /// Cumulative vertical pointer motion.
    pub dy: f64,
    /// Cumulative pointer path length.
    pub distance: f64,
    /// Cumulative wall-clock span covered by pointer motion.
    pub window_secs: f64,
    /// Cumulative events dropped by the producer before graph publication.
    pub dropped_events: u64,
    /// Cumulative structurally invalid events rejected by graph publication.
    pub invalid_events: u64,
}

/// One coherent latest sample, bounded-event cursor, and transient revision.
pub struct InputPublicationRead {
    /// Latest sample in this publication revision.
    pub sample: Option<Arc<InputData>>,
    /// Bounded discrete-event read metadata.
    pub events: InputEventRead,
    /// Cumulative interaction transients in this publication revision.
    pub interaction_transients: InteractionTransientTotals,
}

impl InputSourceSlot {
    pub(crate) fn new(
        id: u64,
        kind: Option<SourceKind>,
        interaction_origin: Option<InteractionSourceOrigin>,
        status: SourceStatusHandle,
    ) -> Self {
        Self {
            inner: Arc::new(InputSourceSlotInner {
                id,
                kind,
                interaction_origin,
                status,
                publication: InputPublicationSlot::new(INPUT_EVENT_RING_CAPACITY),
            }),
        }
    }

    /// Stable identity that never aliases a replacement source.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.inner.id
    }

    /// Declared source kind used to build typed route caches.
    #[must_use]
    pub fn kind(&self) -> Option<SourceKind> {
        self.inner.kind
    }

    /// Typed interaction origin, absent for non-interaction sources.
    #[must_use]
    pub fn interaction_origin(&self) -> Option<InteractionSourceOrigin> {
        self.inner.interaction_origin
    }

    /// Health handle bound to the same source generation as this slot.
    #[must_use]
    pub fn status(&self) -> &SourceStatusHandle {
        &self.inner.status
    }

    /// Load the latest sample without borrowing the input manager.
    #[must_use]
    pub fn latest(&self) -> Option<Arc<InputData>> {
        self.inner.publication.latest()
    }

    pub(crate) fn publish_batch(
        &self,
        sample: Option<Arc<InputData>>,
        events: &mut Vec<TimedInputEvent>,
    ) {
        self.inner.publication.publish_batch(sample, events);
    }

    /// Atomically read one latest sample and its bounded event revision.
    ///
    /// `u64::MAX` starts at this exact revision's event tail.
    pub fn read_publication_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> InputPublicationRead {
        self.inner
            .publication
            .read_publication_since(cursor, output)
    }

    /// Append events newer than `cursor` and advance it monotonically.
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
}

impl InputPublicationSlot {
    pub(crate) fn new(event_capacity: usize) -> Self {
        let event_slots = (0..event_capacity)
            .map(|_| ArcSwapOption::empty())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            inner: Arc::new(InputPublicationSlotInner {
                latest: ArcSwapOption::empty(),
                event_slots,
                writer: Mutex::new(InputPublicationWriter {
                    next_cursor: 0,
                    dropped_total: 0,
                    interaction_transients: InteractionTransientTotals::default(),
                }),
                revision: AtomicU64::new(0),
                next_cursor: AtomicU64::new(0),
                dropped_total: AtomicU64::new(0),
                interaction_transients: AtomicInteractionTransientTotals::default(),
            }),
        }
    }

    pub(crate) fn latest(&self) -> Option<Arc<InputData>> {
        self.inner.latest.load_full()
    }

    pub(crate) fn publish_batch(
        &self,
        sample: Option<Arc<InputData>>,
        events: &mut Vec<TimedInputEvent>,
    ) {
        let mut writer = lock_publication_writer(&self.inner.writer);
        let _revision = PublicationRevisionGuard::begin(&self.inner.revision);
        let latest = self.inner.latest.load_full();
        if let Some(InputData::Interaction(interaction)) = sample.as_deref() {
            writer.interaction_transients.absorb(interaction);
        }
        let retain_change_only_sample = sample.is_none()
            && latest.as_ref().is_some_and(|latest| {
                matches!(latest.as_ref(), InputData::Media(_) | InputData::Net(_))
            });
        if !retain_change_only_sample {
            self.inner.latest.store(sample);
        }

        let capacity = self.inner.event_slots.len();
        for event in events.drain(..) {
            if event.repeat_count == 0 {
                writer.dropped_total = writer.dropped_total.saturating_add(1);
                writer.interaction_transients.invalid_events = writer
                    .interaction_transients
                    .invalid_events
                    .saturating_add(1);
                continue;
            }
            if capacity == 0 {
                writer.dropped_total = writer.dropped_total.saturating_add(1);
                writer.interaction_transients.dropped_events = writer
                    .interaction_transients
                    .dropped_events
                    .saturating_add(1);
                continue;
            }

            let cursor = writer.next_cursor;
            writer.next_cursor = writer
                .next_cursor
                .checked_add(1)
                .expect("input event cursor exhausted");
            if cursor >= capacity as u64 {
                writer.dropped_total = writer.dropped_total.saturating_add(1);
            }
            let index = usize::try_from(cursor % capacity as u64)
                .expect("input event slot index fits usize");
            self.inner.event_slots[index].store(Some(Arc::new(InputEventEntry { cursor, event })));
        }

        self.inner
            .next_cursor
            .store(writer.next_cursor, Ordering::Release);
        self.inner
            .dropped_total
            .store(writer.dropped_total, Ordering::Release);
        self.inner
            .interaction_transients
            .store(writer.interaction_transients);
    }

    pub(crate) fn read_publication_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> InputPublicationRead {
        loop {
            let revision = self.inner.revision.load(Ordering::Acquire);
            if revision & 1 != 0 {
                spin_loop();
                continue;
            }

            let original_len = output.len();
            let sample = self.inner.latest.load_full();
            let next_cursor = self.inner.next_cursor.load(Ordering::Acquire);
            let dropped_total = self.inner.dropped_total.load(Ordering::Acquire);
            let interaction_transients = self.inner.interaction_transients.load();
            let capacity = self.inner.event_slots.len() as u64;
            let effective_cursor = if cursor == u64::MAX {
                next_cursor
            } else {
                cursor.min(next_cursor)
            };
            let oldest_cursor = next_cursor.saturating_sub(capacity.min(next_cursor));
            let dropped = oldest_cursor.saturating_sub(effective_cursor);
            let readable_cursor = effective_cursor.max(oldest_cursor);
            let mut valid = true;

            for expected_cursor in readable_cursor..next_cursor {
                let index = usize::try_from(expected_cursor % capacity)
                    .expect("input event slot index fits usize");
                let entry = self.inner.event_slots[index].load_full();
                let Some(entry) = entry.filter(|entry| entry.cursor == expected_cursor) else {
                    valid = false;
                    break;
                };
                output.push(entry.event.clone());
            }

            let final_revision = self.inner.revision.load(Ordering::Acquire);
            if valid && revision == final_revision && final_revision & 1 == 0 {
                return InputPublicationRead {
                    sample,
                    events: InputEventRead {
                        next_cursor,
                        dropped,
                        dropped_total,
                    },
                    interaction_transients,
                };
            }

            output.truncate(original_len);
            spin_loop();
        }
    }

    pub(crate) fn read_events_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> InputEventRead {
        self.read_publication_since(cursor, output).events
    }

    pub(crate) fn event_cursor(&self) -> u64 {
        self.inner.next_cursor.load(Ordering::Acquire)
    }
}

impl InteractionTransientTotals {
    /// Difference from one earlier total in the same source-slot lifetime.
    #[must_use]
    pub fn delta_since(self, earlier: Self) -> Self {
        Self {
            dx: self.dx - earlier.dx,
            dy: self.dy - earlier.dy,
            distance: self.distance - earlier.distance,
            window_secs: self.window_secs - earlier.window_secs,
            dropped_events: self.dropped_events.saturating_sub(earlier.dropped_events),
            invalid_events: self.invalid_events.saturating_sub(earlier.invalid_events),
        }
    }

    fn absorb(&mut self, interaction: &super::InteractionData) {
        add_finite(&mut self.dx, interaction.batch.motion.dx);
        add_finite(&mut self.dy, interaction.batch.motion.dy);
        add_finite(&mut self.distance, interaction.batch.motion.distance);
        add_finite(&mut self.window_secs, interaction.batch.window_secs);
        self.dropped_events = self
            .dropped_events
            .saturating_add(u64::from(interaction.batch.dropped_events));
    }
}

impl AtomicInteractionTransientTotals {
    fn load(&self) -> InteractionTransientTotals {
        InteractionTransientTotals {
            dx: f64::from_bits(self.dx.load(Ordering::Relaxed)),
            dy: f64::from_bits(self.dy.load(Ordering::Relaxed)),
            distance: f64::from_bits(self.distance.load(Ordering::Relaxed)),
            window_secs: f64::from_bits(self.window_secs.load(Ordering::Relaxed)),
            dropped_events: self.dropped_events.load(Ordering::Relaxed),
            invalid_events: self.invalid_events.load(Ordering::Relaxed),
        }
    }

    fn store(&self, totals: InteractionTransientTotals) {
        self.dx.store(totals.dx.to_bits(), Ordering::Relaxed);
        self.dy.store(totals.dy.to_bits(), Ordering::Relaxed);
        self.distance
            .store(totals.distance.to_bits(), Ordering::Relaxed);
        self.window_secs
            .store(totals.window_secs.to_bits(), Ordering::Relaxed);
        self.dropped_events
            .store(totals.dropped_events, Ordering::Relaxed);
        self.invalid_events
            .store(totals.invalid_events, Ordering::Relaxed);
    }
}

fn add_finite(total: &mut f64, value: f32) {
    if value.is_finite() {
        let next = *total + f64::from(value);
        if next.is_finite() {
            *total = next;
        }
    }
}

struct PublicationRevisionGuard<'a>(&'a AtomicU64);

impl<'a> PublicationRevisionGuard<'a> {
    fn begin(revision: &'a AtomicU64) -> Self {
        let previous = revision.fetch_add(1, Ordering::AcqRel);
        debug_assert_eq!(previous & 1, 0, "publication writers must be serialized");
        Self(revision)
    }
}

impl Drop for PublicationRevisionGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}

/// Cloneable lock-free view of the active input graph.
#[derive(Clone)]
pub struct InputGraphHandle {
    latest: Arc<ArcSwap<InputGraphSnapshot>>,
}

impl InputGraphHandle {
    pub(crate) fn new() -> Self {
        Self {
            latest: Arc::new(ArcSwap::from_pointee(InputGraphSnapshot {
                generation: 0,
                slots: Arc::from([]),
            })),
        }
    }

    /// Load one coherent graph generation and its stable source slots.
    #[must_use]
    pub fn snapshot(&self) -> Arc<InputGraphSnapshot> {
        self.latest.load_full()
    }

    pub(crate) fn publish(&self, generation: u64, slots: Arc<[InputSourceSlot]>) {
        self.latest
            .store(Arc::new(InputGraphSnapshot { generation, slots }));
    }
}

impl Default for InputGraphHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable input graph used as the route-cache identity.
pub struct InputGraphSnapshot {
    generation: u64,
    slots: Arc<[InputSourceSlot]>,
}

impl InputGraphSnapshot {
    /// Canonical manager-owned graph generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Source slots in stable manager registration order.
    #[must_use]
    pub fn slots(&self) -> &[InputSourceSlot] {
        &self.slots
    }
}

fn lock_publication_writer(
    writer: &Mutex<InputPublicationWriter>,
) -> MutexGuard<'_, InputPublicationWriter> {
    match writer.lock() {
        Ok(writer) => writer,
        Err(poisoned) => poisoned.into_inner(),
    }
}
