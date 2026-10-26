//! `/studio` — the unified surface-centric composition workspace (Spec 65).
//!
//! The page header's second row is the zone tab strip: one tab per LED
//! zone, then a Screens tab. Below it, two columns: the device pane on
//! the left and the center Stage, both about the active tab. The
//! composition panel (effect and layer editing) slides in over the Stage
//! on demand rather than occupying a permanent rail.

mod composition_panel;
pub mod device_assignment;
mod device_card;
mod device_pane;
mod drag;
pub mod drag_model;
mod face_composition;
pub mod history;
pub(crate) mod move_menu;
mod moves;
mod offline_device_card;
pub mod pane_model;
pub mod placement;
mod screen_list;
mod stage;
pub use crate::zones::surface;
pub mod tab_model;
mod undo_notice;
mod zone_controls;
mod zone_strip;

use std::collections::{HashMap, HashSet};

use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::api::ComponentBindingSummary;
use crate::app::WsContext;
use crate::components::layout_builder::ZoneLayoutProvider;
use crate::components::page_header::{HeaderToolbar, HeaderTrailing, PageAccent, PageHeader};
use crate::components::resize_handle::ResizeHandle;
use crate::components::scene_picker::ScenePicker;
use crate::icons::*;
use crate::pointer_gesture::PointerEnd;
use crate::storage;

use crate::zones::surface::{selected_screen_device_id, surfaces_from_zones};
use composition_panel::CompositionPanel;
use device_pane::DevicePane;
use drag::{DragController, DragLayer};
use stage::Stage;
use undo_notice::{UndoNotice, UndoNoticeHost};
use zone_strip::{ZONE_PANEL_ID, ZoneStrip, active_tab_dom_id};

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

/// An empty layer stack at revision 0 — the resource value while nothing
/// is selected.
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
    pub(crate) history: history::StudioHistory,
    pub selected_surface_id: RwSignal<Option<String>>,
    pub active_scene: Signal<Option<api::SceneDocument>>,
    /// Re-fetch the active scene. Zone mutations call this so the tree and
    /// Stage pick up the new zone set and scene revision.
    pub refresh_scene: Callback<()>,
    /// Whether the composition slide-over is open. The now-playing chip
    /// toggles it; the panel and its scrim read it.
    pub composition_open: RwSignal<bool>,
    /// The selected Screen's live face: the scene's own assignment when it
    /// has one, otherwise the display's stored default. `None` for Lights,
    /// for Screens with no bound display, and for screens showing nothing.
    pub screen_face: Signal<Option<api::DisplayFaceResponse>>,
    /// Re-fetch the selected Screen's face after a face write.
    pub refresh_screen_face: Callback<()>,
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
    /// Display labels for Screens, keyed by zone id. Repeated device
    /// names ("Lian Li Uni Fan TL Wireless LCD" three times) resolve to
    /// the per-device label the canvas shows ("TL LCD 2").
    pub screen_labels: Memo<HashMap<String, String>>,
    /// The daemon's render canvas extent. Zone layouts carry placements
    /// only, so anything fitting a footprint to the canvas reads this.
    pub render_canvas_size: Memo<(u32, u32)>,
    /// Which device-pane disclosure (a card's kebab) is open, by key. The
    /// pane rebuilds on scene events and search keystrokes, so openness
    /// must outlive the DOM that renders it; one shared slot also means
    /// opening a menu closes the previous one.
    pub rail_disclosure: RwSignal<Option<String>>,
    /// An in-flight zone rename: `(zone id, draft text)`. Lives here so a
    /// tab strip rebuild mid-typing does not eat the field or the text.
    pub zone_rename_draft: RwSignal<Option<(String, String)>>,
    /// Other zones expanded in the device pane. Session state: rows start
    /// collapsed, a split device's hint opens its other zone, and a new
    /// zone opens every row so its first devices are in view.
    pub pane_expanded: RwSignal<HashSet<String>>,
    /// The rows a new zone opened on its own, `(new zone id, row ids)`.
    /// They close again once that zone gets its first device or its tab
    /// is left; a row the user toggles since is theirs and stays.
    pub pane_auto_expanded: RwSignal<Option<(String, HashSet<String>)>>,
    /// The offer to undo the last destructive zone action, if still open.
    pub undo_notice: RwSignal<Option<UndoNotice>>,
    /// Whether the narrow-viewport device drawer is open.
    pub pane_drawer: RwSignal<bool>,
    /// The output box under the pointer on the canvas, mirrored from the
    /// editor so the rail can softly highlight the card that owns it.
    pub pointer_output_id: RwSignal<Option<String>>,
    /// The active named layout: where the rig's hardware physically sits.
    /// Outputs joining a zone are placed from it, and the device pane
    /// lists saved controllers from it.
    pub rig_layout: Signal<Option<hypercolor_types::spatial::SpatialLayout>>,
    /// Re-fetch [`Self::rig_layout`] after a write that changes it.
    pub refresh_rig_layout: Callback<()>,
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
    // The active scene is the app-wide shared resource — WS scene events
    // keep it fresh, so zone changes made from other pages, other
    // clients, or the CLI land here without a Studio-local refetch.
    let zones_ctx = expect_context::<crate::zones::ZonesContext>();
    let active_scene: Signal<Option<api::SceneDocument>> = zones_ctx.active_scene.into();

    let selected_surface_id = RwSignal::new(None::<String>);

    // Keep the selection on a still-present zone. A fresh selection opens
    // the zone the rest of the app targets, else the first LED zone, so
    // Studio always opens on a Light.
    Effect::new(move |_| {
        let Some(scene) = active_scene.get() else {
            if selected_surface_id.get_untracked().is_some() {
                selected_surface_id.set(None);
            }
            return;
        };
        let current = selected_surface_id.get_untracked();
        let still_present = current
            .as_ref()
            .is_some_and(|id| scene.zones.iter().any(|zone| zone.id.to_string() == *id));
        if still_present {
            return;
        }
        let focused = zones_ctx.focused_zone.get_untracked();
        let led_zones = || {
            scene
                .zones
                .iter()
                .filter(|zone| zone.role != ZoneRole::Display)
        };
        let next = led_zones()
            .find(|zone| focused.as_deref() == Some(zone.id.to_string().as_str()))
            .or_else(|| led_zones().next())
            .or_else(|| scene.zones.first())
            .map(|zone| zone.id.to_string());
        selected_surface_id.set(next);
    });

    // Studio's selected LED zone is the app's target zone, so an apply
    // from anywhere lands in the zone being composed. A Screen is not a
    // target and leaves the focus alone; a focus left over from another
    // scene already resolves to the primary zone.
    Effect::new(move |_| {
        let Some(scene) = active_scene.get() else {
            return;
        };
        let selected_led_zone = selected_surface_id.get().filter(|id| {
            scene
                .zones
                .iter()
                .any(|zone| zone.id.to_string() == *id && zone.role != ZoneRole::Display)
        });
        if let Some(zone_id) = selected_led_zone
            && zones_ctx.focused_zone.get_untracked().as_deref() != Some(zone_id.as_str())
        {
            zones_ctx.focused_zone.set(Some(zone_id));
        }
    });

    let ws = expect_context::<WsContext>();
    // The shared scene already contains each zone's layers and revision.
    // Project its stack without fetching the same document again.
    let layers_resource = LocalResource::new(move || {
        let zone_id = selected_surface_id.get();
        let stack = active_scene.with(|scene| match (scene.as_ref(), zone_id.as_deref()) {
            (Some(scene), Some(zone_id)) => scene
                .zones
                .iter()
                .find(|zone| zone.id.to_string() == zone_id)
                .map(|zone| api::LayerStackResponse {
                    items: zone.layers.clone(),
                    revision: scene.revision,
                })
                .ok_or_else(|| {
                    api::ApiError::Parse(format!("Zone {zone_id} is not present in the live scene"))
                }),
            _ => Ok(empty_layer_stack()),
        });
        async move { stack }
    });

    let on_layers_mutated = Callback::new(move |()| {
        zones_ctx.refresh.run(());
    });
    let refresh_scene = zones_ctx.refresh;

    // The selected Screen's face rides the display face endpoint, which
    // reports the scene layer or the stored default, whichever is live.
    // It refetches on selection, after every face write from this page,
    // and on structural scene events (the daemon publishes zone_changed for
    // default-face writes too), so it never needs a timer.
    let (face_tick, set_face_tick) = signal(0_u64);
    let face_scene_generation = Memo::new(move |_| {
        ws.last_scene_event
            .with(|hint| hint.as_ref().map_or(0, |hint| hint.structural_generation))
    });
    let selected_screen_device = Memo::new(move |_| {
        let selected = selected_surface_id.get()?;
        let scene = active_scene.get()?;
        selected_screen_device_id(&scene.zones, &selected)
    });
    let screen_face_resource = api::daemon_resource(move || {
        let _ = face_tick.get();
        let _ = face_scene_generation.get();
        let device_id = selected_screen_device.get();
        async move {
            match device_id {
                Some(device_id) => api::fetch_display_face(&device_id).await,
                None => Ok(None),
            }
        }
    });
    // The resource keeps the previous screen's face while the next fetch is
    // in flight; only a face for the currently selected display counts, so
    // a fresh selection never wears, or acts on, its predecessor's face.
    let screen_face = Signal::derive(move || {
        let selected = selected_screen_device.get()?;
        screen_face_resource
            .get()
            .and_then(Result::ok)
            .flatten()
            .filter(|face| face.device_id == selected)
    });
    let refresh_screen_face = Callback::new(move |()| {
        set_face_tick.update(|tick| *tick = tick.wrapping_add(1));
    });

    let devices_ctx = expect_context::<crate::app::DevicesContext>();
    let screen_labels = Memo::new(move |_| {
        let Some(scene) = active_scene.get() else {
            return HashMap::new();
        };
        let layout_ids: HashMap<String, String> = devices_ctx
            .devices_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|device| (device.id, device.layout_device_id))
            .collect();
        tab_model::screen_label_map(&scene.zones, &layout_ids)
    });

    // The tab strip owns selection, so the layer panel shows the selected
    // surface's name in its header rather than a redundant zone selector.
    let surface_label = Signal::derive(move || {
        let id = selected_surface_id.get()?;
        if let Some(label) = screen_labels.with(|labels| labels.get(&id).cloned()) {
            return Some(label);
        }
        let scene = active_scene.get()?;
        surfaces_from_zones(&scene.zones)
            .into_iter()
            .find(|surface| surface.id == id)
            .map(|surface| surface.name)
    });

    // The device pane's width persists per browser; the Stage takes the
    // space that is left.
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

    // Narrow viewports collapse the pane into a slide-over drawer (§13).
    let tree_drawer = RwSignal::new(false);
    // Picking a zone or a Screen reveals the Stage behind the drawer.
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

    let render_canvas_size = crate::render_canvas::use_render_canvas_size();
    // Layout events, device events, and membership changes (a primary
    // zone that follows the active layout writes through to it) can all
    // change the active layout.
    let scene_members = Memo::new(move |_| {
        active_scene.with(|scene| {
            scene.as_ref().map(|scene| {
                (
                    scene.id,
                    scene
                        .zones
                        .iter()
                        .map(|zone| (zone.id, zone.members.clone()))
                        .collect::<Vec<_>>(),
                )
            })
        })
    });
    let rig_layout_resource = api::daemon_resource(move || {
        let _ = scene_members.get();
        let _ = ws.layout_generation.get();
        let _ = ws.last_device_event.get();
        async move { api::fetch_active_layout().await.ok() }
    });
    let rig_layout = Signal::derive(move || rig_layout_resource.get().flatten());
    let refresh_rig_layout = Callback::new(move |()| rig_layout_resource.refetch());
    let pane_expanded = RwSignal::new(HashSet::<String>::new());
    let pane_auto_expanded = RwSignal::new(None::<(String, HashSet<String>)>);
    Effect::new(move |_| {
        let Some((zone_id, rows)) = pane_auto_expanded.get() else {
            return;
        };
        let open = selected_surface_id.get();
        let members = active_scene.with(|scene| {
            scene
                .as_ref()
                .and_then(|scene| {
                    scene
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == zone_id)
                })
                .map_or(0, |zone| zone.members.len())
        });
        if pane_model::auto_expansion_ends(open.as_deref(), &zone_id, members) {
            pane_expanded.update(|expanded| expanded.retain(|id| !rows.contains(id)));
            pane_auto_expanded.set(None);
        }
    });
    let rail_disclosure = RwSignal::new(None::<String>);
    let zone_rename_draft = RwSignal::new(None::<(String, String)>);
    let pointer_output_id = RwSignal::new(None::<String>);

    let history = history::StudioHistory::new(active_scene, selected_surface_id, refresh_scene);
    provide_context(history);

    let studio = StudioContext {
        history,
        selected_surface_id,
        active_scene,
        refresh_scene,
        composition_open,
        screen_face,
        refresh_screen_face,
        hidden_outputs,
        selected_output_ids,
        hovered_output_ids,
        attachment_cache,
        screen_labels,
        render_canvas_size,
        rail_disclosure,
        zone_rename_draft,
        pane_expanded,
        pane_auto_expanded,
        undo_notice: RwSignal::new(None),
        pane_drawer: tree_drawer,
        pointer_output_id,
        rig_layout,
        refresh_rig_layout,
    };
    provide_context(studio);
    let root_ref = NodeRef::<leptos::html::Div>::new();
    let drag = DragController::new(studio, devices_ctx, root_ref);
    provide_context(drag);
    let dragging = Memo::new(move |_| drag.payload.with(Option::is_some));

    let on_screens = Memo::new(move |_| {
        let selected = selected_surface_id.get();
        active_scene.with(|scene| {
            scene.as_ref().is_some_and(|scene| {
                scene.zones.iter().any(|zone| {
                    zone.role == ZoneRole::Display
                        && selected.as_deref() == Some(zone.id.to_string().as_str())
                })
            })
        })
    });
    let panel_label =
        move || active_tab_dom_id(selected_surface_id.get().as_deref(), on_screens.get());

    // Drag sources capture their pointer, so every gesture event bubbles up
    // here, and a live drag captures on this root so it survives the pane
    // rebuilding beneath it.
    view! {
        <div
            node_ref=root_ref
            class="flex h-full flex-col overflow-hidden"
            class=("cursor-grabbing", move || dragging.get())
            class=("select-none", move || dragging.get())
            on:pointermove=move |ev| drag.on_move(&ev)
            on:pointerup=move |ev| drag.on_end(&ev, PointerEnd::Up)
            on:pointercancel=move |ev| drag.on_end(&ev, PointerEnd::Cancel)
            on:lostpointercapture=move |ev| drag.on_lost_capture(&ev)
            on:contextmenu=move |ev: web_sys::MouseEvent| {
                if drag.holds_touch() {
                    ev.prevent_default();
                }
            }
        >
            <DragLayer />
            <PageHeader
                icon=LuLayoutTemplate
                title="Studio"
                accent=PageAccent::Coral
            >
                <HeaderTrailing slot>
                    <ScenePicker />
                </HeaderTrailing>
                <HeaderToolbar slot>
                    <ZoneStrip />
                </HeaderToolbar>
            </PageHeader>
            // Narrow-viewport drawer toggle; the pane sits beside the Stage
            // on `lg` and up, so this strip is hidden there.
            <div class="flex shrink-0 items-center border-b border-edge-subtle/70 bg-surface-raised/40 px-3 py-2 lg:hidden">
                <button
                    type="button"
                    class="inline-flex items-center gap-1.5 rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-2.5 py-1.5 text-[11px] font-medium text-fg-secondary btn-press"
                    on:click=move |_| tree_drawer.set(true)
                >
                    <Icon icon=LuLightbulb width="13px" height="13px" />
                    "Devices"
                </button>
            </div>

            <div
                class="relative flex min-h-0 flex-1 overflow-hidden"
                role="tabpanel"
                id=ZONE_PANEL_ID
                aria-labelledby=panel_label
            >
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
                    <DevicePane />
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
                    <UndoNoticeHost />
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
