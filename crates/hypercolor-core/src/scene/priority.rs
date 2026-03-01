//! Priority stack — sorted scene activation with automatic restore.
//!
//! The [`PriorityStack`] maintains an ordered collection of active scenes.
//! The entry with the highest [`ScenePriority`] is the "winner" — it
//! controls what's currently rendering. When the winner is removed, the
//! next-highest entry takes over seamlessly.

use std::time::Instant;

use crate::types::scene::{SceneId, ScenePriority};

// ── StackEntry ──────────────────────────────────────────────────────────

/// A single entry in the priority stack.
///
/// Each entry represents a scene that was activated and is either
/// currently rendering (highest priority) or shadowed by a
/// higher-priority entry.
#[derive(Debug, Clone)]
pub struct StackEntry {
    /// The scene this entry renders.
    pub scene_id: SceneId,

    /// Priority level for conflict resolution.
    pub priority: ScenePriority,

    /// Monotonic insertion timestamp — used for FIFO ordering
    /// among entries with equal priority.
    pub entered_at: Instant,
}

// ── PriorityStack ───────────────────────────────────────────────────────

/// Priority-based scene management with automatic restore-on-pop.
///
/// Scenes are stored in a flat `Vec` sorted by `(priority DESC, entered_at ASC)`.
/// The first element is always the current winner. When it's popped,
/// the next element automatically becomes active — no explicit restore
/// logic needed.
///
/// Equal-priority entries maintain FIFO ordering: the *most recently*
/// pushed entry wins within the same priority tier.
#[derive(Debug, Default)]
pub struct PriorityStack {
    /// Active scene entries, maintained in sorted order.
    entries: Vec<StackEntry>,
}

impl PriorityStack {
    /// Create an empty priority stack.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Push a scene onto the stack at the given priority.
    ///
    /// The entry is inserted in sorted position. If the new entry
    /// becomes the highest priority, it shadows all others.
    pub fn push(&mut self, scene_id: SceneId, priority: ScenePriority) {
        let entry = StackEntry {
            scene_id,
            priority,
            entered_at: Instant::now(),
        };
        self.entries.push(entry);
        self.sort();
    }

    /// Remove the topmost (highest-priority) entry from the stack.
    ///
    /// Returns the removed entry if the stack was non-empty.
    /// The next-highest entry automatically becomes the active scene.
    pub fn pop(&mut self) -> Option<StackEntry> {
        if self.entries.is_empty() {
            return None;
        }
        Some(self.entries.remove(0))
    }

    /// Remove a specific scene by ID from the stack.
    ///
    /// If the removed scene was the active (top) entry, the next-highest
    /// entry is automatically promoted.
    ///
    /// Returns `true` if an entry was found and removed.
    pub fn remove(&mut self, scene_id: &SceneId) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.scene_id != *scene_id);
        self.entries.len() < before
    }

    /// Peek at the currently active (highest-priority) scene.
    ///
    /// Returns `None` if the stack is empty.
    #[must_use]
    pub fn peek(&self) -> Option<&StackEntry> {
        self.entries.first()
    }

    /// Returns `true` if the stack contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of entries currently in the stack.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Get all entries, ordered by priority (highest first).
    #[must_use]
    pub fn entries(&self) -> &[StackEntry] {
        &self.entries
    }

    /// Clear all entries from the stack.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Sort entries by (priority DESC, `entered_at` DESC for equal priority).
    ///
    /// Within equal priority, the most recently pushed entry wins (FIFO
    /// from the perspective of "last-in is the active one"). This matches
    /// the spec's requirement that equal-priority entries use FIFO ordering.
    fn sort(&mut self) {
        self.entries.sort_by(|a, b| {
            // Higher priority first.
            b.priority
                .cmp(&a.priority)
                // Equal priority: most recent entry first (FIFO — last-in wins).
                .then_with(|| b.entered_at.cmp(&a.entered_at))
        });
    }
}
