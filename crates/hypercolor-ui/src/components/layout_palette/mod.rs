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
    pub set_layout: crate::components::layout_builder::LayoutWriteHandle,
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
    /// Write-side of the master "hide all" snapshot. Individual device/zone
    /// toggles clear this so manual changes invalidate the saved state.
    pub set_master_hidden_snapshot: WriteSignal<Option<std::collections::HashSet<String>>>,
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
    let (master_hidden_snapshot, set_master_hidden_snapshot) =
        signal(None::<std::collections::HashSet<String>>);

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
        set_master_hidden_snapshot,
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

    // Are there any zones in the layout at all?
    let any_zones_in_layout = Signal::derive(move || {
        state
            .layout
            .with(|l| l.as_ref().is_some_and(|l| !l.zones.is_empty()))
    });

    // Are ALL layout zones currently hidden?
    let all_zones_hidden = Signal::derive(move || {
        let hidden = state.hidden_zones.get();
        state.layout.with(|l| {
            l.as_ref().is_some_and(|l| {
                !l.zones.is_empty() && l.zones.iter().all(|z| hidden.contains(&z.id))
            })
        })
    });

    view! {
        <div class="p-3 space-y-4">
            // Devices section
            <div class="space-y-2">
                <div class="flex items-center justify-between">
                    <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary flex items-center gap-1.5">
                        <Icon icon=LuCpu width="12px" height="12px" />
                        "Devices"
                    </h3>
                    // Master visibility toggle — hide/show all zones at once
                    {move || {
                        if !any_zones_in_layout.get() { return None; }
                        let all_hidden = all_zones_hidden.get();
                        Some(view! {
                            <button
                                class="w-5 h-5 flex items-center justify-center rounded transition-all btn-press"
                                style=if all_hidden {
                                    "color: var(--color-text-tertiary); opacity: 0.3"
                                } else {
                                    "color: var(--color-text-tertiary); opacity: 0.5"
                                }
                                title=if all_hidden { "Show all devices" } else { "Hide all devices" }
                                on:click=move |_| {
                                    if all_hidden {
                                        // Restore: use snapshot if available, otherwise show all
                                        let snapshot = master_hidden_snapshot.get_untracked();
                                        set_master_hidden_snapshot.set(None);
                                        state.set_hidden_zones.set(
                                            snapshot.unwrap_or_default()
                                        );
                                    } else {
                                        // Hide all: snapshot current state, then hide everything
                                        let current = state.hidden_zones.get_untracked();
                                        set_master_hidden_snapshot.set(Some(current));
                                        let all_ids: std::collections::HashSet<String> =
                                            state.layout.with_untracked(|l| {
                                                l.as_ref()
                                                    .map(|l| l.zones.iter().map(|z| z.id.clone()).collect())
                                                    .unwrap_or_default()
                                            });
                                        state.set_hidden_zones.set(all_ids);
                                    }
                                }
                            >
                                {if all_hidden {
                                    view! { <Icon icon=LuEyeOff width="11px" height="11px" /> }.into_any()
                                } else {
                                    view! { <Icon icon=LuEye width="11px" height="11px" /> }.into_any()
                                }}
                            </button>
                        })
                    }}
                </div>
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
