//! `/studio` — the unified surface-centric composition workspace (Spec 65).
//!
//! The page header's second row is the zone tab strip: one tab per zone.
//! Below it, two columns: the device pane on the left and the center
//! Stage, both about the active tab. A screen is one of its zone's
//! outputs: a box on the Stage and a card in the pane, and its face opens
//! from either. The composition panel (effect and layer editing) slides
//! in over the Stage on demand rather than occupying a permanent rail.

mod composition_panel;
pub mod device_assignment;
mod device_card;
mod device_pane;
mod drag;
pub mod drag_model;
pub mod history;
pub(crate) mod layer_target;
pub(crate) mod move_menu;
mod moves;
mod offline_device_card;
pub mod pane_model;
pub mod phone_summary;
pub mod phone_view;
pub mod placement;
pub mod screen_model;
pub(crate) mod screens;
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

use crate::zones::surface::surfaces_from_zones;
use composition_panel::CompositionPanel;
use device_pane::DevicePane;
use drag::{DragController, DragLayer};
use phone_view::{SURFACE_PARAM, StudioPhoneView, VIEW_PARAM};
use stage::Stage;
use undo_notice::{UndoNotice, UndoNoticeHost};
use zone_strip::{ZONE_PANEL_ID, ZoneStrip, active_tab_dom_id};

/// localStorage key for the device pane's width. The value keeps the
/// pane's old name so a width people already set survives.
const PANE_WIDTH_KEY: &str = "hc-studio-tree-width";
const PANE_WIDTH_RANGE: (f64, f64) = (240.0, 460.0);
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

/// An empty layer stack at revision 0: the resource value while nothing
/// is selected, or while the selection names a zone the scene lacks.
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
    /// Re-fetch the active scene. Zone mutations call this so the device
    /// pane and Stage pick up the new zone set and scene revision.
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
    /// The daemon's render canvas extent. Zone layouts carry placements
    /// only, so anything fitting a footprint to the canvas reads this.
    pub render_canvas_size: Memo<(u32, u32)>,
    /// Which device-pane disclosure (a card's kebab) is open, by key. The
    /// pane rebuilds on scene events and search keystrokes, so openness
    /// must outlive the DOM that renders it; one shared slot also means
    /// opening a menu closes the previous one.
    pub rail_disclosure: RwSignal<Option<String>>,
    /// Device cards whose folded output list is open, by the list's
    /// element id. Kept here for the same reason as `rail_disclosure`.
    pub open_output_lists: RwSignal<HashSet<String>>,
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
    /// What layers apply to: the open zone's outputs, the target the next
    /// layer takes, and the editor and canvas pick that choose one.
    pub(crate) targets: layer_target::LayerTargets,
    /// The scene's screens, the ones selected, and what they show.
    pub(crate) screens: screens::Screens,
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

/// `/studio`: the spatial workspace. Phones open on a zone summary with
/// per-zone effect switching and reach the workspace at `?view=editor`
/// (see [`phone_view`]).
#[component]
pub fn StudioPage() -> impl IntoView {
    let phone = crate::viewport::is_phone_layout();
    let query = leptos_router::hooks::use_query_map();
    let summary = Memo::new(move |_| {
        phone.get()
            && StudioPhoneView::from_query(query.with(|query| query.get(VIEW_PARAM)).as_deref())
                == StudioPhoneView::Summary
    });
    view! {
        {move || {
            if summary.get() {
                view! { <phone_summary::StudioPhoneSummary /> }.into_any()
            } else {
                view! { <StudioWorkspace /> }.into_any()
            }
        }}
    }
}

#[component]
fn StudioWorkspace() -> impl IntoView {
    // The active scene is the app-wide shared resource — WS scene events
    // keep it fresh, so zone changes made from other pages, other
    // clients, or the CLI land here without a Studio-local refetch. A
    // failed read of it leaves Studio on the scene it last read.
    let zones_ctx = expect_context::<crate::zones::ZonesContext>();
    let active_scene: Signal<Option<api::SceneDocument>> =
        history::held_document(zones_ctx.active_scene).into();

    let selected_surface_id = RwSignal::new(None::<String>);
    let wants_screens = StoredValue::new(
        leptos_router::hooks::use_query_map()
            .with_untracked(|query| phone_view::wants_screens(query.get(SURFACE_PARAM).as_deref())),
    );

    // Keep the selection on a zone the scene still holds. A fresh
    // selection opens the zone the rest of the app targets, else the
    // first zone. A screen's view is no zone to open: its screen sits in
    // the zone that places it.
    Effect::new(move |_| {
        let Some(scene) = active_scene.get() else {
            if selected_surface_id.get_untracked().is_some() {
                selected_surface_id.set(None);
            }
            return;
        };
        let zones = || {
            scene
                .zones
                .iter()
                .filter(|zone| zone.role != ZoneRole::Display)
        };
        let current = selected_surface_id.get_untracked();
        let still_present = current
            .as_ref()
            .is_some_and(|id| zones().any(|zone| zone.id.to_string() == *id));
        if still_present {
            return;
        }
        let focused = zones_ctx.focused_zone.get_untracked();
        let next = zones()
            .find(|zone| focused.as_deref() == Some(zone.id.to_string().as_str()))
            .or_else(|| zones().next())
            .map(|zone| zone.id.to_string());
        selected_surface_id.set(next);
    });

    // Picks outside Studio also select its tab. Only a new focus moves
    // the tab.
    Effect::new(move |_| {
        let Some(focused) = zones_ctx.focused_zone.get() else {
            return;
        };
        let is_zone = active_scene.with_untracked(|scene| {
            scene.as_ref().is_some_and(|scene| {
                scene
                    .zones
                    .iter()
                    .any(|zone| zone.id.to_string() == focused && zone.role != ZoneRole::Display)
            })
        });
        if is_zone && selected_surface_id.get_untracked().as_ref() != Some(&focused) {
            selected_surface_id.set(Some(focused));
        }
    });

    // Studio's selected zone is the app's target zone, so an apply from
    // anywhere lands in the zone being composed. A focus left over from
    // another scene already resolves to the primary zone.
    Effect::new(move |_| {
        let Some(scene) = active_scene.get() else {
            return;
        };
        let selected_zone = selected_surface_id.get().filter(|id| {
            scene
                .zones
                .iter()
                .any(|zone| zone.id.to_string() == *id && zone.role != ZoneRole::Display)
        });
        if let Some(zone_id) = selected_zone
            && zones_ctx.focused_zone.get_untracked().as_deref() != Some(zone_id.as_str())
        {
            zones_ctx.focused_zone.set(Some(zone_id));
        }
    });

    let ws = expect_context::<WsContext>();
    // The shared scene already contains each zone's layers and revision.
    // Project its stack without fetching the same document again. A
    // selection the scene does not hold yet, or any longer, is a moment of
    // catch-up the selection settles on its own, so it shows no layers
    // rather than an error.
    let layers_resource = LocalResource::new(move || {
        let zone_id = selected_surface_id.get();
        let stack = active_scene.with(|scene| {
            scene
                .as_ref()
                .zip(zone_id.as_deref())
                .and_then(|(scene, zone_id)| {
                    scene
                        .zones
                        .iter()
                        .find(|zone| zone.id.to_string() == zone_id)
                        .map(|zone| api::LayerStackResponse {
                            items: zone.layers.clone(),
                            revision: scene.revision,
                        })
                })
                .unwrap_or_else(empty_layer_stack)
        });
        async move { Ok(stack) }
    });

    let on_layers_mutated = Callback::new(move |()| {
        zones_ctx.refresh.run(());
    });
    let refresh_scene = zones_ctx.refresh;

    let devices_ctx = expect_context::<crate::app::DevicesContext>();

    // The tab strip owns selection, so the layer panel shows the selected
    // zone's name in its header rather than a redundant zone selector.
    let surface_label = Signal::derive(move || {
        let id = selected_surface_id.get()?;
        let scene = active_scene.get()?;
        surfaces_from_zones(&scene.zones)
            .into_iter()
            .find(|surface| surface.id == id)
            .map(|surface| surface.name)
    });

    // The device pane's width persists per browser; the Stage takes the
    // space that is left.
    let pane_width = RwSignal::new(storage::get_clamped(
        PANE_WIDTH_KEY,
        280.0,
        PANE_WIDTH_RANGE.0,
        PANE_WIDTH_RANGE.1,
    ));
    Effect::new(move |_| {
        storage::set(PANE_WIDTH_KEY, &pane_width.get().to_string());
    });
    let pane_drag_start = StoredValue::new(0.0_f64);

    // Narrow viewports collapse the pane into a slide-over drawer (§13).
    let pane_drawer = RwSignal::new(false);
    // Picking a zone reveals the Stage behind the drawer.
    Effect::new(move |_| {
        let _ = selected_surface_id.get();
        if pane_drawer.get_untracked() {
            pane_drawer.set(false);
        }
    });

    let composition_open = RwSignal::new(false);

    // Which outputs are hidden, kept per zone and persisted across
    // sessions so a deliberately hidden output stays hidden between
    // visits.
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
    let open_output_lists = RwSignal::new(HashSet::<String>::new());
    let zone_rename_draft = RwSignal::new(None::<(String, String)>);
    let pointer_output_id = RwSignal::new(None::<String>);

    let history = history::StudioHistory::new(
        active_scene,
        selected_surface_id,
        refresh_scene,
        render_canvas_size,
    );
    provide_context(history);
    // A zone paused from the sidebar is an undo step here too, the same
    // one the tab menu's pause records.
    let studio_zone_patch = Callback::new(move |(zone_id, request)| {
        history.patch_zone(zone_id, request, "Couldn't update the zone");
    });
    zones_ctx
        .studio_zone_patch
        .set_value(Some(studio_zone_patch));
    on_cleanup(move || {
        // A Studio mounted since keeps its own registration.
        zones_ctx.studio_zone_patch.try_update_value(|slot| {
            if slot
                .as_ref()
                .is_some_and(|current| current.matches(&studio_zone_patch))
            {
                *slot = None;
            }
        });
    });

    let screens = screens::Screens::new(active_scene, selected_surface_id, selected_output_ids);
    // A link that asked for the screens lands on the first one, with its
    // face open; the ask is spent once it has been honoured. The tab
    // switch clears the last zone's selection, so the screen is selected
    // on the frame after it.
    Effect::new(move |_| {
        if !wants_screens.get_value() {
            return;
        }
        let Some(first) = screens.placed.with(|placed| placed.first().cloned()) else {
            return;
        };
        wants_screens.set_value(false);
        selected_surface_id.set(Some(first.zone_id));
        request_animation_frame(move || {
            selected_output_ids.try_set(HashSet::from([first.output_id]));
            composition_open.try_set(true);
        });
    });

    let studio = StudioContext {
        history,
        selected_surface_id,
        active_scene,
        refresh_scene,
        composition_open,
        hidden_outputs,
        selected_output_ids,
        hovered_output_ids,
        attachment_cache,
        render_canvas_size,
        rail_disclosure,
        open_output_lists,
        zone_rename_draft,
        pane_expanded,
        pane_auto_expanded,
        undo_notice: RwSignal::new(None),
        pane_drawer,
        pointer_output_id,
        rig_layout,
        refresh_rig_layout,
        targets: layer_target::LayerTargets::new(
            active_scene,
            selected_surface_id,
            selected_output_ids,
            screens,
        ),
        screens,
    };
    provide_context(studio);
    screens.follow_placements(studio);
    let root_ref = NodeRef::<leptos::html::Div>::new();
    let drag = DragController::new(studio, devices_ctx, root_ref);
    provide_context(drag);
    history.attach_announcer(Some(history::Announcer {
        say: Callback::new(move |words| drag.announce(words)),
        device: Callback::new(move |device_id: String| {
            moves::registry_device(drag, &device_id)
                .map(|device| (device.name.clone(), moves::light_segment_count(&device)))
        }),
    }));
    on_cleanup(move || history.attach_announcer(None));
    let dragging = Memo::new(move |_| drag.payload.with(Option::is_some));
    // Restart the short content settle only for a different tab. Scene
    // refreshes and repeated clicks keep the live canvas undisturbed.
    let content_switch = RwSignal::new(0_u8);
    Effect::new(move |previous: Option<Option<String>>| {
        let selected = selected_surface_id.get();
        if dragging.get() {
            content_switch.set(0);
        } else if previous.is_some_and(|previous| previous != selected) {
            content_switch.update(|phase| *phase = if *phase == 1 { 2 } else { 1 });
        }
        selected
    });

    let panel_label = move || active_tab_dom_id(selected_surface_id.get().as_deref());

    // Drag sources capture their pointer, so every gesture event bubbles up
    // here, and a live drag captures on this root so it survives the pane
    // rebuilding beneath it.
    view! {
        <div
            node_ref=root_ref
            class="studio-workspace flex h-full flex-col overflow-hidden"
            data-switch=move || content_switch.get().to_string()
            data-dragging=move || dragging.get().to_string()
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
            <div class="flex shrink-0 items-center justify-between gap-2 border-b border-edge-subtle/70 bg-surface-raised/40 px-3 py-2 lg:hidden">
                <button
                    type="button"
                    class="inline-flex items-center gap-1.5 rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-2.5 py-1.5 text-[11px] font-medium text-fg-secondary btn-press max-md:min-h-11 max-md:px-3.5 max-md:text-xs"
                    on:click=move |_| pane_drawer.set(true)
                >
                    <Icon icon=LuLightbulb width="13px" height="13px" />
                    "Devices"
                </button>
                // Phones entered the editor from the zone summary; this is
                // the way back without reaching for the browser.
                <a
                    href=crate::route_ui::route_href(StudioPhoneView::Summary.href())
                    class="hidden min-h-11 items-center gap-1.5 rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-3.5 text-xs font-medium text-fg-secondary btn-press max-md:inline-flex"
                >
                    <Icon icon=LuLayers width="13px" height="13px" />
                    "Zone summary"
                </a>
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
                    class="studio-device-drawer w-80 shrink-0 overflow-hidden lg:w-[var(--pane-w)] max-lg:fixed max-lg:inset-y-0 max-lg:left-0 max-lg:z-[45] max-lg:max-w-[85vw] max-lg:bg-surface-base max-lg:border-r max-lg:border-edge-subtle/70 max-md:pb-[env(safe-area-inset-bottom)]"
                    class=("max-lg:-translate-x-full", move || !pane_drawer.get())
                    style:--pane-w=move || format!("{}px", pane_width.get())
                >
                    <DevicePane />
                </div>
                <div class="hidden lg:contents">
                    <ResizeHandle
                        on_drag_start=Callback::new(move |()| {
                            pane_drag_start.set_value(pane_width.get_untracked());
                        })
                        on_drag=Callback::new(move |delta: f64| {
                            let next = (pane_drag_start.get_value() + delta)
                                .clamp(PANE_WIDTH_RANGE.0, PANE_WIDTH_RANGE.1);
                            pane_width.set(next);
                        })
                        on_drag_end=Callback::new(|()| {})
                    />
                </div>
                <div class="relative min-w-0 flex-1 overflow-hidden">
                    // On wide layouts the open composition panel takes its
                    // width from the Stage instead of covering it, so the
                    // canvas or face being tuned stays whole and in view.
                    <div
                        class="h-full transition-[padding] duration-200 motion-reduce:transition-none"
                        class=("xl:pr-[420px]", move || composition_open.get())
                    >
                        <ZoneLayoutProvider
                            selected_zone_id=selected_surface_id
                            refresh_scene=refresh_scene
                        >
                            <Stage />
                        </ZoneLayoutProvider>
                    </div>
                    <CompositionPanel
                        active_scene=active_scene
                        selected_zone_id=selected_surface_id.read_only()
                        set_selected_zone_id=selected_surface_id.write_only()
                        surface_label=surface_label
                        layers_resource=layers_resource
                        on_layers_mutated=on_layers_mutated
                    />
                    <UndoNoticeHost />
                    <layer_target::TargetEditorHost />
                    <screens::CopyEverywhereHost />
                </div>

                <Show when=move || pane_drawer.get()>
                    <div
                        // Above the mobile bottom nav (40), below the drawer
                        // (45): while the drawer is open a tap anywhere
                        // outside it, nav bar included, closes it instead of
                        // navigating away.
                        class="fixed inset-0 z-[42] bg-black/55 lg:hidden"
                        on:click=move |_| pane_drawer.set(false)
                    />
                </Show>
            </div>
        </div>
    }
}
