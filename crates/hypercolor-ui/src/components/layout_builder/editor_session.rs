use std::collections::{HashMap, HashSet};

use leptos::prelude::*;

use crate::api;
use crate::compound_selection::CompoundDepth;
use crate::layout_history::LayoutHistoryState;
use crate::layout_utils::ZoneCache;
use hypercolor_types::spatial::SpatialLayout;

use super::LayoutWriteHandle;

/// Shared layout editor state — provided via context to palette, canvas, and zone properties.
#[derive(Clone, Copy)]
pub(crate) struct LayoutEditorContext {
    pub layout: Signal<Option<SpatialLayout>>,
    pub selected_zone_ids: Signal<HashSet<String>>,
    pub hidden_zones: Signal<HashSet<String>>,
    /// Outputs the host is transiently highlighting (e.g. the Studio rail
    /// hovering a device or channel). Rendered as a soft ring, separate
    /// from the persistent click selection.
    pub hovered_zone_ids: Signal<HashSet<String>>,
    pub keep_aspect_ratio: Signal<bool>,
    pub set_layout: LayoutWriteHandle,
    pub set_selected_zone_ids: WriteSignal<HashSet<String>>,
    pub set_hovered_zone_ids: WriteSignal<HashSet<String>>,
    pub compound_depth: Signal<CompoundDepth>,
    pub set_compound_depth: WriteSignal<CompoundDepth>,
    pub set_is_dirty: WriteSignal<bool>,
    pub set_hidden_zones: WriteSignal<HashSet<String>>,
    pub set_keep_aspect_ratio: WriteSignal<bool>,
    pub removed_zone_cache: Signal<ZoneCache>,
    pub set_removed_zone_cache: WriteSignal<ZoneCache>,
    /// Push the current in-flight layout to the daemon's preview engine.
    /// Used by the canvas during drag to keep the LED preview live without
    /// committing intermediate state to the layout signal.
    pub push_preview: Callback<SpatialLayout>,
    /// Whether the editor history has an undoable / redoable step — lets a
    /// host header drive its undo / redo buttons off this same context.
    pub can_undo: Signal<bool>,
    pub can_redo: Signal<bool>,
}

#[derive(Clone, Copy)]
pub(crate) struct LayoutZoneDisplayContext {
    pub attachment_profiles: LocalResource<HashMap<String, api::DeviceComponentsResponse>>,
}

pub(super) fn attachment_profiles_resource(
    layout: ReadSignal<Option<SpatialLayout>>,
    devices_resource: LocalResource<Result<Vec<api::DeviceSummary>, String>>,
) -> LocalResource<HashMap<String, api::DeviceComponentsResponse>> {
    LocalResource::new(move || {
        let current_layout = layout.get();
        let devices = devices_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default();

        async move {
            let mut device_ids = HashMap::<String, String>::new();
            if let Some(current_layout) = current_layout {
                for zone in current_layout.zones {
                    if zone.attachment.is_none() {
                        continue;
                    }
                    if let Some(device) = devices
                        .iter()
                        .find(|device| device.layout_device_id == zone.device_id)
                    {
                        device_ids.insert(zone.device_id, device.id.clone());
                    }
                }
            }

            let mut profiles = HashMap::new();
            for (layout_device_id, device_id) in device_ids {
                if let Ok(profile) = api::fetch_device_attachments(&device_id).await {
                    profiles.insert(layout_device_id, profile);
                }
            }
            profiles
        }
    })
}

#[derive(Clone, Copy)]
pub(super) struct LayoutEditorSession {
    pub(super) layout: ReadSignal<Option<SpatialLayout>>,
    pub(super) saved_layout: ReadSignal<Option<SpatialLayout>>,
    pub(super) set_saved_layout: WriteSignal<Option<SpatialLayout>>,
    pub(super) selected_zone_ids: ReadSignal<HashSet<String>>,
    pub(super) set_selected_zone_ids: WriteSignal<HashSet<String>>,
    pub(super) compound_depth: ReadSignal<CompoundDepth>,
    pub(super) set_compound_depth: WriteSignal<CompoundDepth>,
    pub(super) keep_aspect_ratio: ReadSignal<bool>,
    pub(super) set_keep_aspect_ratio: WriteSignal<bool>,
    pub(super) hidden_zones: ReadSignal<HashSet<String>>,
    pub(super) set_hidden_zones: WriteSignal<HashSet<String>>,
    pub(super) hovered_zone_ids: ReadSignal<HashSet<String>>,
    pub(super) set_hovered_zone_ids: WriteSignal<HashSet<String>>,
    pub(super) removed_zone_cache: ReadSignal<ZoneCache>,
    pub(super) set_removed_zone_cache: WriteSignal<ZoneCache>,
    pub(super) write: LayoutWriteHandle,
    pub(super) layout_signal: Signal<Option<SpatialLayout>>,
    pub(super) can_undo: Signal<bool>,
    pub(super) can_redo: Signal<bool>,
    pub(super) is_dirty: Signal<bool>,
}

impl LayoutEditorSession {
    pub(super) fn new(keep_aspect_ratio_initial: bool) -> Self {
        let (layout, set_layout_signal) = signal(None::<SpatialLayout>);
        let (saved_layout, set_saved_layout) = signal(None::<SpatialLayout>);
        let (selected_zone_ids, set_selected_zone_ids) = signal(HashSet::<String>::new());
        let (compound_depth, set_compound_depth) = signal(CompoundDepth::Root);
        let (keep_aspect_ratio, set_keep_aspect_ratio) = signal(keep_aspect_ratio_initial);
        let (hidden_zones, set_hidden_zones) = signal(HashSet::<String>::new());
        let (hovered_zone_ids, set_hovered_zone_ids) = signal(HashSet::<String>::new());
        let (removed_zone_cache, set_removed_zone_cache) = signal(ZoneCache::new());
        let (dirty, set_is_dirty) = signal(false);
        let history = RwSignal::new(LayoutHistoryState::default());
        let write = LayoutWriteHandle {
            layout,
            set_layout: set_layout_signal,
            selected_zone_ids,
            set_selected_zone_ids,
            compound_depth,
            set_compound_depth,
            removed_zone_cache,
            set_removed_zone_cache,
            history,
            set_dirty: set_is_dirty,
        };

        Self {
            layout,
            saved_layout,
            set_saved_layout,
            selected_zone_ids,
            set_selected_zone_ids,
            compound_depth,
            set_compound_depth,
            keep_aspect_ratio,
            set_keep_aspect_ratio,
            hidden_zones,
            set_hidden_zones,
            hovered_zone_ids,
            set_hovered_zone_ids,
            removed_zone_cache,
            set_removed_zone_cache,
            write,
            layout_signal: Signal::derive(move || layout.get()),
            can_undo: Signal::derive(move || history.get().can_undo()),
            can_redo: Signal::derive(move || history.get().can_redo()),
            is_dirty: Signal::derive(move || dirty.get()),
        }
    }

    pub(super) fn provide_editor_context(self, push_preview: Callback<SpatialLayout>) {
        provide_context(LayoutEditorContext {
            layout: self.layout_signal,
            selected_zone_ids: Signal::derive(move || self.selected_zone_ids.get()),
            hidden_zones: Signal::derive(move || self.hidden_zones.get()),
            hovered_zone_ids: Signal::derive(move || self.hovered_zone_ids.get()),
            keep_aspect_ratio: Signal::derive(move || self.keep_aspect_ratio.get()),
            set_layout: self.write,
            set_selected_zone_ids: self.set_selected_zone_ids,
            set_hovered_zone_ids: self.set_hovered_zone_ids,
            set_is_dirty: self.write.set_dirty,
            set_hidden_zones: self.set_hidden_zones,
            set_keep_aspect_ratio: self.set_keep_aspect_ratio,
            compound_depth: Signal::derive(move || self.compound_depth.get()),
            set_compound_depth: self.set_compound_depth,
            removed_zone_cache: Signal::derive(move || self.removed_zone_cache.get()),
            set_removed_zone_cache: self.set_removed_zone_cache,
            push_preview,
            can_undo: self.can_undo,
            can_redo: self.can_redo,
        });
    }
}
