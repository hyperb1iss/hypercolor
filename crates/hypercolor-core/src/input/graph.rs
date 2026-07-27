//! Immutable input-graph publications for render-plane consumers.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use arc_swap::{ArcSwap, ArcSwapOption};

use super::{InputData, SourceKind};
use crate::types::event::TimedInputEvent;

/// Maximum events retained per source between consumer reads.
pub const INPUT_EVENT_RING_CAPACITY: usize = 256;

/// Stable latest-value and event publication for one registered source.
#[derive(Clone)]
pub struct InputSourceSlot {
    inner: Arc<InputSourceSlotInner>,
}

struct InputSourceSlotInner {
    id: u64,
    kind: Option<SourceKind>,
    latest: ArcSwapOption<InputData>,
    event_writer: Mutex<InputEventWriter>,
    events: ArcSwap<InputEventSnapshot>,
}

struct InputEventWriter {
    entries: VecDeque<InputEventEntry>,
    next_cursor: u64,
    dropped_total: u64,
}

#[derive(Clone)]
struct InputEventEntry {
    cursor: u64,
    event: TimedInputEvent,
}

/// Immutable bounded event publication for one source.
pub struct InputEventSnapshot {
    entries: Arc<[InputEventEntry]>,
    oldest_cursor: u64,
    next_cursor: u64,
    dropped_total: u64,
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

impl InputSourceSlot {
    pub(crate) fn new(id: u64, kind: Option<SourceKind>) -> Self {
        Self {
            inner: Arc::new(InputSourceSlotInner {
                id,
                kind,
                latest: ArcSwapOption::empty(),
                event_writer: Mutex::new(InputEventWriter {
                    entries: VecDeque::with_capacity(INPUT_EVENT_RING_CAPACITY),
                    next_cursor: 0,
                    dropped_total: 0,
                }),
                events: ArcSwap::from_pointee(InputEventSnapshot::empty()),
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

    /// Load the latest sample without borrowing the input manager.
    #[must_use]
    pub fn latest(&self) -> Option<Arc<InputData>> {
        self.inner.latest.load_full()
    }

    pub(crate) fn publish_sample(&self, sample: Option<Arc<InputData>>) {
        if sample.is_none()
            && self.inner.latest.load().as_ref().is_some_and(|latest| {
                matches!(latest.as_ref(), InputData::Media(_) | InputData::Net(_))
            })
        {
            return;
        }
        self.inner.latest.store(sample);
    }

    pub(crate) fn publish_events(&self, events: &mut Vec<TimedInputEvent>) {
        if events.is_empty() {
            return;
        }

        let mut writer = lock_event_writer(&self.inner.event_writer);
        for event in events.drain(..) {
            if writer.entries.len() == INPUT_EVENT_RING_CAPACITY {
                writer.entries.pop_front();
                writer.dropped_total = writer.dropped_total.saturating_add(1);
            }
            let cursor = writer.next_cursor;
            writer.next_cursor = writer
                .next_cursor
                .checked_add(1)
                .expect("input event cursor exhausted");
            writer.entries.push_back(InputEventEntry { cursor, event });
        }

        let oldest_cursor = writer
            .entries
            .front()
            .map_or(writer.next_cursor, |entry| entry.cursor);
        let snapshot = InputEventSnapshot {
            entries: writer.entries.iter().cloned().collect(),
            oldest_cursor,
            next_cursor: writer.next_cursor,
            dropped_total: writer.dropped_total,
        };
        self.inner.events.store(Arc::new(snapshot));
    }

    /// Append events newer than `cursor` and advance it monotonically.
    pub fn read_events_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> InputEventRead {
        let snapshot = self.inner.events.load_full();
        let effective_cursor = cursor.min(snapshot.next_cursor);
        let dropped = snapshot.oldest_cursor.saturating_sub(effective_cursor);
        let readable_cursor = effective_cursor.max(snapshot.oldest_cursor);
        output.extend(
            snapshot
                .entries
                .iter()
                .filter(|entry| entry.cursor >= readable_cursor)
                .map(|entry| entry.event.clone()),
        );
        InputEventRead {
            next_cursor: snapshot.next_cursor,
            dropped,
            dropped_total: snapshot.dropped_total,
        }
    }

    /// Cursor immediately after the newest retained event.
    #[must_use]
    pub fn event_cursor(&self) -> u64 {
        self.inner.events.load().next_cursor
    }
}

impl InputEventSnapshot {
    fn empty() -> Self {
        Self {
            entries: Arc::from([]),
            oldest_cursor: 0,
            next_cursor: 0,
            dropped_total: 0,
        }
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

fn lock_event_writer(writer: &Mutex<InputEventWriter>) -> MutexGuard<'_, InputEventWriter> {
    match writer.lock() {
        Ok(writer) => writer,
        Err(poisoned) => poisoned.into_inner(),
    }
}
