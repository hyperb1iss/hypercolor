use std::collections::{HashMap, HashSet};

use hypercolor_types::spatial::DeviceZone;

use crate::compound_selection::CompoundDepth;

const MAX_HISTORY_DEPTH: usize = 100;

pub(crate) type RemovedZoneCache = HashMap<(String, Option<String>), DeviceZone>;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutEditorSnapshot {
    pub zones: Vec<DeviceZone>,
    pub selected_zone_ids: HashSet<String>,
    pub compound_depth: CompoundDepth,
    pub removed_zone_cache: RemovedZoneCache,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LayoutHistoryState {
    past: Vec<LayoutEditorSnapshot>,
    future: Vec<LayoutEditorSnapshot>,
    interaction_start: Option<LayoutEditorSnapshot>,
}

impl LayoutHistoryState {
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn reset(&mut self) {
        self.past.clear();
        self.future.clear();
        self.interaction_start = None;
    }

    pub fn begin_interaction(&mut self, snapshot: LayoutEditorSnapshot) {
        if self.interaction_start.is_none() {
            self.interaction_start = Some(snapshot);
        }
    }

    pub fn finish_interaction(&mut self, current: &LayoutEditorSnapshot) {
        let Some(start) = self.interaction_start.take() else {
            return;
        };
        self.record_snapshot(start, current);
    }

    pub fn discard_interaction(&mut self) {
        self.interaction_start = None;
    }

    pub fn record_edit(&mut self, before: LayoutEditorSnapshot, after: &LayoutEditorSnapshot) {
        if self.interaction_start.is_some() {
            return;
        }
        self.record_snapshot(before, after);
    }

    pub fn undo(&mut self, current: LayoutEditorSnapshot) -> Option<LayoutEditorSnapshot> {
        self.interaction_start = None;
        let previous = self.past.pop()?;
        Self::push_limited(&mut self.future, current);
        Some(previous)
    }

    pub fn redo(&mut self, current: LayoutEditorSnapshot) -> Option<LayoutEditorSnapshot> {
        self.interaction_start = None;
        let next = self.future.pop()?;
        Self::push_limited(&mut self.past, current);
        Some(next)
    }

    fn record_snapshot(&mut self, before: LayoutEditorSnapshot, after: &LayoutEditorSnapshot) {
        if before == *after {
            return;
        }
        Self::push_limited(&mut self.past, before);
        self.future.clear();
    }

    fn push_limited(stack: &mut Vec<LayoutEditorSnapshot>, snapshot: LayoutEditorSnapshot) {
        stack.push(snapshot);
        if stack.len() > MAX_HISTORY_DEPTH {
            let overflow = stack.len() - MAX_HISTORY_DEPTH;
            stack.drain(0..overflow);
        }
    }
}
