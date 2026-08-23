//! `/studio` — the unified surface-centric composition workspace (Spec 65).
//!
//! Two columns: the zone tree on the left and the center Stage.
//! Selecting a zone drives the Stage preview. The composition panel —
//! effect and layer editing — slides in over the Stage on demand rather
//! than occupying a permanent rail.

mod composition_panel;
pub mod device_assignment;
mod device_card;
mod face_composition;
mod scene_selector;
mod stage;
pub use crate::zones::surface;
mod zone_add_device;
mod zone_controls;
mod zone_tree;

use std::collections::{HashMap, HashSet};

use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::api::ComponentBindingSummary;
use crate::apply_target::ApplyTarget;
use crate::components::layout_builder::ZoneLayoutProvider;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::page_search_bar::PageSearchBar;
use crate::components::resize_handle::ResizeHandle;
use crate::icons::*;
use crate::storage;

use crate::zones::surface::{UNASSIGNED_SURFACE_ID, surfaces_from_zones};
use composition_panel::CompositionPanel;
use scene_selector::SceneSelector;
use stage::Stage;
use zone_tree::ZoneTree;

const TREE_WIDTH_KEY: &str = "hc-studio-tree-width";
const TREE_WIDTH_RANGE: (f64, f64) = (240.0, 460.0);
/// localStorage key for the per-(scene, zone) hidden-outputs map.
const HIDDEN_OUTPUTS_KEY: &str = "hc-studio-hidden-outputs";

/// Build the storage-side key used to address the hidden-outputs entry
/// for one (scene, zone) pair. Lives at the module root so the device
/// card and any future zone-scoped UI agree on the format.
pub fn hidden_outputs_storage_key(scene_id: &str, zone_id: &str) -> String {
    format!("{scene_id}::{zone_id}")
}

/// The zone half of a [`hidden_outputs_storage_key`].
pub fn zone_id_from_storage_key(key: &str) -> Option<String> {
    key.split_once("::").map(|(_, zone)| zone.to_owned())
}

fn load_hidden_outputs() -> HashMap<String, HashSet<String>> {
    storage::get(HIDDEN_OUTPUTS_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_hidden_outputs(map: &HashMap<String, HashSet<String>>) {
    if let Ok(json) = serde_json::to_string(map) {
        storage::set(HIDDEN_OUTPUTS_KEY, &json);
    }
}

/// An empty layer stack at revision 0 — the resource value for a selection
/// that has no per-zone layer endpoint (none selected, or the synthetic
/// Unassigned entry).
fn empty_layer_stack() -> api::LayerStackResponse {
    api::LayerStackResponse {
        items: Vec::new(),
        revision: 0,
    }
}

/// Shared Studio state — the selected surface and the active scene —
/// provided to the columns so surface selection is one source of truth.
#[derive(Clone, Copy)]
pub struct StudioContext {
    pub selected_surface_id: RwSignal<Option<String>>,
    pub active_scene: Signal<Option<api::SceneDocument>>,
    /// Re-fetch the active scene. Zone mutations call this so the tree and
    /// Stage pick up the new zone set and scene revision.
    pub refresh_scene: Callback<()>,
    /// Whether the composition slide-over is open. The now-playing chip
    /// toggles it; the panel and its scrim read it.
    pub composition_open: RwSignal<bool>,
    /// Per-(scene, zone) sets of `Output` ids the user has hidden from
    /// the zone's device card. Keys are built by
    /// [`hidden_outputs_storage_key`]. Client UI state only; never
    /// mirrored to the daemon's `layout_auto_exclusions` (Plan 55 §8).
    pub hidden_outputs: RwSignal<HashMap<String, HashSet<String>>>,
    /// Output ids the rail has highlighted by clicking a device card or one
    /// of its channels. The Stage bridges this into the canvas selection so
    /// a rail click lights the matching boxes on the spatial view.
    pub selected_output_ids: RwSignal<HashSet<String>>,
    /// Output ids the rail is hovering — a single channel or a whole
    /// device's outputs. Bridged into the canvas as a transient highlight,
    /// distinct from the persistent click selection.
    pub hovered_output_ids: RwSignal<HashSet<String>>,
    /// Cache of component (attachment) bindings per physical device id.
    /// Each device card lazily fills its own entry; channel rows read it
    /// to surface live binding labels without re-fetching per render.
    pub attachment_cache: RwSignal<HashMap<String, Vec<ComponentBindingSummary>>>,
    /// Header search term. Filters the zone tree's device rows by name,
    /// filling the header toolbar the way every other page's search does.
    pub device_search: Signal<String>,
    /// The daemon's render canvas extent. Zone layouts carry placements
    /// only, so anything fitting a footprint to the canvas reads this.
    pub render_canvas_size: Memo<(u32, u32)>,
    /// Which rail disclosure (zone menu, device kebab, add-device picker)
    /// is open, by key. The rail rebuilds on scene events and search
    /// keystrokes, so openness must outlive the DOM that renders it; one
    /// shared slot also means opening a menu closes the previous one.
    pub rail_disclosure: RwSignal<Option<String>>,
    /// An in-flight zone rename: `(zone id, draft text)`. Lives here so a
    /// rail rebuild mid-typing does not eat the field or the text.
    pub zone_rename_draft: RwSignal<Option<(String, String)>>,
    /// The output box under the pointer on the canvas, mirrored from the
    /// editor so the rail can softly highlight the card that owns it.
    pub pointer_output_id: RwSignal<Option<String>>,
}

/// A local open/closed signal whose truth lives in a shared keyed slot,
/// so it survives its component being rebuilt. Opening claims the slot
/// (closing whichever disclosure held it); closing releases it.
pub(crate) fn keyed_disclosure(slot: RwSignal<Option<String>>, key: String) -> RwSignal<bool> {
    let open = RwSignal::new(slot.with_untracked(|held| held.as_deref() == Some(key.as_str())));
    Effect::new({
        let key = key.clone();
        move |_| {
            let is_open = open.get();
            if is_open {
                if slot.with_untracked(|held| held.as_deref() != Some(key.as_str())) {
                    slot.set(Some(key.clone()));
                }
            } else if slot.with_untracked(|held| held.as_deref() == Some(key.as_str())) {
                slot.set(None);
            }
        }
    });
    Effect::new(move |_| {
        let foreign = slot.with(|held| held.as_deref() != Some(key.as_str()));
        if foreign && open.get_untracked() {
            open.set(false);
        }
    });
    open
}

#[component]
pub fn StudioPage() -> impl IntoView {
    let (layers_tick, set_layers_tick) = signal(0_u64);

    // The active scene is the app-wide shared resource — WS scene events
    // keep it fresh, so zone changes made from other pages, other
    // clients, or the CLI land here without a Studio-local refetch.
    let zones_ctx = expect_context::<crate::zones::ZonesContext>();
    let active_scene: Signal<Option<api::SceneDocument>> = zones_ctx.active_scene.into();

    let selected_surface_id = RwSignal::new(None::<String>);

    // Keep the selection on a still-present zone, defaulting to the first
    // LED zone so Studio always opens on a Light.
    Effect::new(move |_| {
        let Some(scene) = active_scene.get() else {
            if selected_surface_id.get_untracked().is_some() {
                selected_surface_id.set(None);
            }
            return;
        };
        let current = selected_surface_id.get_untracked();
        // The synthetic Unassigned entry has no zone; it is "present"
        // while the scene is genuinely multi-zone (§9.4).
        let multi_zone = surface::led_zone_count(&scene.zones) > 1;
        let still_present = current.as_ref().is_some_and(|id| {
            (id == UNASSIGNED_SURFACE_ID && multi_zone)
                || scene.zones.iter().any(|zone| zone.id.to_string() == *id)
        });
        if still_present {
            return;
        }
        let next = scene
            .zones
            .iter()
            .find(|zone| zone.role != ZoneRole::Display)
            .or_else(|| scene.zones.first())
            .map(|zone| zone.id.to_string());
        selected_surface_id.set(next);
    });

    // Studio's selected LED zone is the app-wide effect apply-target
    // (Wave B3): a quick-apply from anywhere lands in the zone being
    // composed. A Screen or the Unassigned entry is not an apply target.
    let effects_ctx = expect_context::<crate::app::EffectsContext>();
    Effect::new(move |_| {
        let Some(scene) = active_scene.get() else {
            return;
        };
        let selected_led_zone = selected_surface_id.get().filter(|id| {
            id != UNASSIGNED_SURFACE_ID
                && scene
                    .zones
                    .iter()
                    .any(|zone| zone.id.to_string() == *id && zone.role != ZoneRole::Display)
        });
        if let Some(zone_id) = selected_led_zone {
            zones_ctx.focused_zone.set(Some(zone_id.clone()));
            effects_ctx.apply_target.set(ApplyTarget::Zone(zone_id));
        } else if matches!(
            effects_ctx.apply_target.get_untracked(),
            ApplyTarget::Zone(ref target)
                if !scene
                    .zones
                    .iter()
                    .any(|zone| zone.id.to_string() == target.as_str())
        ) {
            // A Screen / Unassigned selection holding a target left over
            // from a no-longer-active scene falls back to the default zone.
            zones_ctx.focused_zone.set(None);
            effects_ctx.apply_target.set(ApplyTarget::Primary);
        }
    });

    let layers_resource = api::daemon_resource(move || {
        let _ = layers_tick.get();
        let scene = active_scene.get();
        let zone_id = selected_surface_id.get();
        async move {
            match (scene, zone_id) {
                // The Unassigned entry is not a surface — it has no layer
                // stack, so it never hits the per-zone layer endpoint.
                (_, Some(zone_id)) if zone_id == UNASSIGNED_SURFACE_ID => Ok(empty_layer_stack()),
                (Some(_), Some(zone_id)) => api::list_layers(&zone_id).await,
                _ => Ok(empty_layer_stack()),
            }
        }
    });

    let on_layers_mutated = Callback::new(move |()| {
        set_layers_tick.update(|tick| *tick = tick.wrapping_add(1));
        zones_ctx.refresh.run(());
    });
    let refresh_scene = zones_ctx.refresh;

    // The zone tree owns selection, so the layer panel shows the selected
    // surface's name in its header rather than a redundant zone selector.
    let surface_label = Signal::derive(move || {
        let id = selected_surface_id.get()?;
        let scene = active_scene.get()?;
        surfaces_from_zones(&scene.zones)
            .into_iter()
            .find(|surface| surface.id == id)
            .map(|surface| surface.name)
    });

    // The zone-tree column width persists per browser; the Stage
    // takes the space that is left.
    let tree_width = RwSignal::new(storage::get_clamped(
        TREE_WIDTH_KEY,
        280.0,
        TREE_WIDTH_RANGE.0,
        TREE_WIDTH_RANGE.1,
    ));
    Effect::new(move |_| {
        storage::set(TREE_WIDTH_KEY, &tree_width.get().to_string());
    });
    let tree_drag_start = StoredValue::new(0.0_f64);

    // Narrow viewports collapse the tree into a slide-over drawer (§13).
    let tree_drawer = RwSignal::new(false);
    // Picking a zone from the drawer reveals the Stage behind it.
    Effect::new(move |_| {
        let _ = selected_surface_id.get();
        if tree_drawer.get_untracked() {
            tree_drawer.set(false);
        }
    });

    let composition_open = RwSignal::new(false);

    // Hidden-output state is the palette card's `hidden_zones` model
    // re-scoped per zone, persisted across sessions so a deliberately
    // hidden output stays hidden between visits.
    let hidden_outputs = RwSignal::new(load_hidden_outputs());
    Effect::new(move |_| {
        let snapshot = hidden_outputs.get();
        save_hidden_outputs(&snapshot);
    });

    let attachment_cache = RwSignal::new(HashMap::<String, Vec<ComponentBindingSummary>>::new());

    // Rail-driven canvas highlight state. Switching surfaces clears it so a
    // stale highlight from the previous zone never lingers on the new one.
    // Only a genuine switch clears: a card click re-asserts the surface it
    // already lives in, and `set` notifies on equal values, so comparing
    // against the previous id is what keeps that click's selection alive.
    let selected_output_ids = RwSignal::new(HashSet::<String>::new());
    let hovered_output_ids = RwSignal::new(HashSet::<String>::new());
    Effect::new(move |previous: Option<Option<String>>| {
        let current = selected_surface_id.get();
        if previous.is_some_and(|previous| previous != current) {
            selected_output_ids.set(HashSet::new());
            hovered_output_ids.set(HashSet::new());
        }
        current
    });

    let (device_search, set_device_search) = signal(String::new());
    let render_canvas_size = crate::render_canvas::use_render_canvas_size();
    let rail_disclosure = RwSignal::new(None::<String>);
    let zone_rename_draft = RwSignal::new(None::<(String, String)>);
    let pointer_output_id = RwSignal::new(None::<String>);

    provide_context(StudioContext {
        selected_surface_id,
        active_scene,
        refresh_scene,
        composition_open,
        hidden_outputs,
        selected_output_ids,
        hovered_output_ids,
        attachment_cache,
        device_search: device_search.into(),
        render_canvas_size,
        rail_disclosure,
        zone_rename_draft,
        pointer_output_id,
    });

    view! {
        <div class="flex h-full flex-col overflow-hidden">
            <PageHeader
                icon=LuLayoutTemplate
                title="Studio"
                accent=PageAccent::Coral
            >
                <HeaderTrailing slot>
                    <SceneSelector />
                </HeaderTrailing>
                <HeaderToolbar slot>
                    <PageSearchBar
                        placeholder="Search devices...".to_string()
                        value=device_search
                        set_value=set_device_search
                    />
                </HeaderToolbar>
            </PageHeader>
            // Narrow-viewport drawer toggle; the tree sits beside the Stage
            // on `lg` and up, so this strip is hidden there.
            <div class="flex shrink-0 items-center border-b border-edge-subtle/70 bg-surface-raised/40 px-3 py-2 lg:hidden">
                <button
                    type="button"
                    class="inline-flex items-center gap-1.5 rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-2.5 py-1.5 text-[11px] font-medium text-fg-secondary btn-press"
                    on:click=move |_| tree_drawer.set(true)
                >
                    <Icon icon=LuLightbulb width="13px" height="13px" />
                    "Zones"
                </button>
            </div>

            <div class="relative flex min-h-0 flex-1 overflow-hidden">
                <div
                    // z-index 45: the drawer must paint over the mobile
                    // bottom nav (40, later in the DOM) and add its
                    // safe-area padding, since as a fixed overlay it
                    // deliberately covers the bar instead of stacking above
                    // it. Keep bracket class syntax out of comments; the
                    // Tailwind scanner reads them and emits dead rules.
                    class="w-80 shrink-0 overflow-hidden lg:w-[var(--tree-w)] max-lg:fixed max-lg:inset-y-0 max-lg:left-0 max-lg:z-[45] max-lg:border-r max-lg:border-edge-subtle/70 max-lg:transition-transform max-lg:duration-200 max-md:pb-[env(safe-area-inset-bottom)]"
                    class=("max-lg:-translate-x-full", move || !tree_drawer.get())
                    style:--tree-w=move || format!("{}px", tree_width.get())
                >
                    <ZoneTree />
                </div>
                <div class="hidden lg:contents">
                    <ResizeHandle
                        on_drag_start=Callback::new(move |()| {
                            tree_drag_start.set_value(tree_width.get_untracked());
                        })
                        on_drag=Callback::new(move |delta: f64| {
                            let next = (tree_drag_start.get_value() + delta)
                                .clamp(TREE_WIDTH_RANGE.0, TREE_WIDTH_RANGE.1);
                            tree_width.set(next);
                        })
                        on_drag_end=Callback::new(|()| {})
                    />
                </div>
                <div class="relative min-w-0 flex-1">
                    <ZoneLayoutProvider
                        active_scene=active_scene
                        selected_zone_id=selected_surface_id
                        refresh_scene=refresh_scene
                    >
                        <Stage />
                    </ZoneLayoutProvider>
                    <CompositionPanel
                        active_scene=active_scene
                        selected_zone_id=selected_surface_id.read_only()
                        set_selected_zone_id=selected_surface_id.write_only()
                        surface_label=surface_label
                        layers_resource=layers_resource
                        on_layers_mutated=on_layers_mutated
                    />
                </div>

                <Show when=move || tree_drawer.get()>
                    <div
                        // Above the mobile bottom nav (40), below the drawer
                        // (45): while the drawer is open a tap anywhere
                        // outside it, nav bar included, closes it instead of
                        // navigating away.
                        class="fixed inset-0 z-[42] bg-black/55 lg:hidden"
                        on:click=move |_| tree_drawer.set(false)
                    />
                </Show>
            </div>
        </div>
    }
}
