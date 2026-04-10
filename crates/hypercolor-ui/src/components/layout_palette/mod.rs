//! Layout palette — available devices for adding zones to layouts.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::DevicesContext;
use crate::icons::*;

mod devices;
mod offline;
mod topology;

use devices::render_online_devices_section;
use offline::render_offline_devices_section;

/// Bundle of all the signals / state a palette rendering helper needs.
///
/// Keeps parameter lists short while preserving pure-move behavior: every
/// field is already `Copy` on its own (Leptos signals, memos, contexts).
#[derive(Clone, Copy)]
pub(super) struct PaletteState {
    pub devices_ctx: DevicesContext,
    pub layout: Signal<Option<hypercolor_types::spatial::SpatialLayout>>,
    pub selected_zone_ids: Signal<std::collections::HashSet<String>>,
    pub hidden_zones: Signal<std::collections::HashSet<String>>,
    pub set_layout: WriteSignal<Option<hypercolor_types::spatial::SpatialLayout>>,
    pub set_selected_zone_ids: WriteSignal<std::collections::HashSet<String>>,
    pub set_is_dirty: WriteSignal<bool>,
    pub set_hidden_zones: WriteSignal<std::collections::HashSet<String>>,
    pub compound_depth: Signal<crate::compound_selection::CompoundDepth>,
    pub set_compound_depth: WriteSignal<crate::compound_selection::CompoundDepth>,
    pub removed_zone_cache: Signal<crate::layout_utils::ZoneCache>,
    pub set_removed_zone_cache: WriteSignal<crate::layout_utils::ZoneCache>,
    pub stable_devices: Memo<Vec<api::DeviceSummary>>,
    pub collapsed_devices: ReadSignal<std::collections::HashSet<String>>,
    pub set_collapsed_devices: WriteSignal<std::collections::HashSet<String>>,
    pub attachment_cache:
        ReadSignal<std::collections::HashMap<String, Vec<api::AttachmentBindingSummary>>>,
    pub set_attachment_cache:
        WriteSignal<std::collections::HashMap<String, Vec<api::AttachmentBindingSummary>>>,
    pub import_in_flight: ReadSignal<bool>,
    pub set_import_in_flight: WriteSignal<bool>,
}

/// Device palette for adding zones to the layout.
#[component]
pub fn LayoutPalette() -> impl IntoView {
    let devices_ctx = expect_context::<DevicesContext>();
    let ctx = expect_context::<crate::components::layout_builder::LayoutEditorContext>();

    let stable_devices = Memo::new(move |_| {
        devices_ctx
            .devices_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    });

    let (collapsed_devices, set_collapsed_devices) =
        signal(std::collections::HashSet::<String>::new());

    let (attachment_cache, set_attachment_cache) = signal(std::collections::HashMap::<
        String,
        Vec<api::AttachmentBindingSummary>,
    >::new());
    let (import_in_flight, set_import_in_flight) = signal(false);

    let state = PaletteState {
        devices_ctx,
        layout: ctx.layout,
        selected_zone_ids: ctx.selected_zone_ids,
        hidden_zones: ctx.hidden_zones,
        set_layout: ctx.set_layout,
        set_selected_zone_ids: ctx.set_selected_zone_ids,
        set_is_dirty: ctx.set_is_dirty,
        set_hidden_zones: ctx.set_hidden_zones,
        compound_depth: ctx.compound_depth,
        set_compound_depth: ctx.set_compound_depth,
        removed_zone_cache: ctx.removed_zone_cache,
        set_removed_zone_cache: ctx.set_removed_zone_cache,
        stable_devices,
        collapsed_devices,
        set_collapsed_devices,
        attachment_cache,
        set_attachment_cache,
        import_in_flight,
        set_import_in_flight,
    };

    // Auto-fetch attachments for multi-zone devices (they start expanded).
    Effect::new(move |_| {
        let devices = stable_devices.get();
        let collapsed = collapsed_devices.get();
        for dev in &devices {
            if dev.zones.len() > 1 && !collapsed.contains(&dev.layout_device_id) {
                fetch_attachments_for(state, dev.id.clone());
            }
        }
    });

    view! {
        <div class="p-3 space-y-4">
            // Devices section
            <div class="space-y-2">
                <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary flex items-center gap-1.5">
                    <Icon icon=LuCpu width="12px" height="12px" />
                    "Devices"
                </h3>
                {move || render_online_devices_section(state)}
            </div>

            {move || render_offline_devices_section(state)}
        </div>
    }
}

/// Fetch attachments for a device (if not already cached).
pub(super) fn fetch_attachments_for(state: PaletteState, device_id: String) {
    if state
        .attachment_cache
        .get_untracked()
        .contains_key(&device_id)
    {
        return;
    }
    let did = device_id.clone();
    let set_cache = state.set_attachment_cache;
    leptos::task::spawn_local(async move {
        if let Ok(profile) = api::fetch_device_attachments(&did).await {
            set_cache.update(|cache| {
                cache.insert(did, profile.bindings);
            });
        }
    });
}
