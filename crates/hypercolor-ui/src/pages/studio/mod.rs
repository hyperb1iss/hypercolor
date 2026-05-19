//! `/studio` — the unified surface-centric composition workspace (Spec 65).
//!
//! Two columns: the Zones & Devices tree on the left and the center Stage.
//! Selecting a zone drives the Stage preview. The composition panel —
//! effect and layer editing — slides in over the Stage on demand rather
//! than occupying a permanent rail.

mod composition_panel;
mod device_card;
mod device_grouping;
mod stage;
mod surface;
mod zone_assignment;
mod zone_controls;
mod zone_tree;

use hypercolor_types::scene::RenderGroupRole;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::{CapabilitiesContext, WsContext};
use crate::components::layout_builder::LayoutEditorProvider;
use crate::components::resize_handle::ResizeHandle;
use crate::icons::*;
use crate::storage;

use composition_panel::CompositionPanel;
use stage::Stage;
use surface::{UNASSIGNED_SURFACE_ID, surfaces_from_groups};
use zone_tree::ZoneTree;

const TREE_WIDTH_KEY: &str = "hc-studio-tree-width";
const TREE_WIDTH_RANGE: (f64, f64) = (240.0, 460.0);

/// An empty layer stack at version 0 — the resource value for a selection
/// that has no per-group layer endpoint (none selected, or the synthetic
/// Unassigned entry).
fn empty_layer_stack() -> api::LayerStackResponse {
    api::LayerStackResponse {
        items: Vec::new(),
        layers_version: 0,
    }
}

/// Shared Studio state — the selected surface and the active scene —
/// provided to the columns so surface selection is one source of truth.
#[derive(Clone, Copy)]
pub struct StudioContext {
    pub selected_surface_id: RwSignal<Option<String>>,
    pub active_scene: Signal<Option<api::ActiveSceneResponse>>,
    /// Re-fetch the active scene. Zone mutations call this so the tree and
    /// Stage pick up the new group set and `groups_revision`.
    pub refresh_scene: Callback<()>,
    /// Whether the composition slide-over is open. The now-playing chip
    /// toggles it; the panel and its scrim read it.
    pub composition_open: RwSignal<bool>,
}

#[component]
pub fn StudioPage() -> impl IntoView {
    let (scene_tick, set_scene_tick) = signal(0_u64);
    let (layers_tick, set_layers_tick) = signal(0_u64);

    let scene_resource = LocalResource::new(move || {
        let _ = scene_tick.get();
        async move { api::fetch_active_scene().await }
    });
    let active_scene =
        Signal::derive(move || scene_resource.get().and_then(Result::ok).flatten());

    let selected_surface_id = RwSignal::new(None::<String>);

    // Keep the selection on a still-present group, defaulting to the first
    // LED group so Studio always opens on a Light.
    Effect::new(move |_| {
        let Some(scene) = active_scene.get() else {
            if selected_surface_id.get_untracked().is_some() {
                selected_surface_id.set(None);
            }
            return;
        };
        let current = selected_surface_id.get_untracked();
        // The synthetic Unassigned entry has no group; it is "present"
        // while the scene is genuinely multi-zone (§9.4).
        let multi_zone = surface::led_zone_count(&scene.groups) > 1;
        let still_present = current.as_ref().is_some_and(|id| {
            (id == UNASSIGNED_SURFACE_ID && multi_zone)
                || scene.groups.iter().any(|group| group.id.to_string() == *id)
        });
        if still_present {
            return;
        }
        let next = scene
            .groups
            .iter()
            .find(|group| group.role != RenderGroupRole::Display)
            .or_else(|| scene.groups.first())
            .map(|group| group.id.to_string());
        selected_surface_id.set(next);
    });

    let layers_resource = LocalResource::new(move || {
        let _ = layers_tick.get();
        let scene = active_scene.get();
        let group_id = selected_surface_id.get();
        async move {
            match (scene, group_id) {
                // The Unassigned entry is not a surface — it has no layer
                // stack, so it never hits the per-group layer endpoint.
                (_, Some(group_id)) if group_id == UNASSIGNED_SURFACE_ID => {
                    Ok(empty_layer_stack())
                }
                (Some(scene), Some(group_id)) => api::list_layers(&scene.id, &group_id).await,
                _ => Ok(empty_layer_stack()),
            }
        }
    });

    let on_layers_mutated = Callback::new(move |()| {
        set_layers_tick.update(|tick| *tick = tick.wrapping_add(1));
        set_scene_tick.update(|tick| *tick = tick.wrapping_add(1));
    });
    let refresh_scene = Callback::new(move |()| {
        set_scene_tick.update(|tick| *tick = tick.wrapping_add(1));
    });

    // The zone tree owns selection, so the layer panel shows the selected
    // surface's name in its header rather than a redundant group selector.
    let surface_label = Signal::derive(move || {
        let id = selected_surface_id.get()?;
        let scene = active_scene.get()?;
        surfaces_from_groups(&scene.groups)
            .into_iter()
            .find(|surface| surface.id == id)
            .map(|surface| surface.name)
    });

    // The Zones & Devices column width persists per browser; the Stage
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

    // Per-zone preview frames (§9.5) are streamed only while Studio shows a
    // genuinely multi-zone scene and the daemon advertises the capability;
    // a single-zone scene's per-zone canvas is just the composited canvas.
    let ws = expect_context::<WsContext>();
    let caps = expect_context::<CapabilitiesContext>();
    Effect::new(move |_| {
        let multi_zone = active_scene
            .get()
            .is_some_and(|scene| surface::led_zone_count(&scene.groups) > 1);
        ws.set_zone_preview_active
            .set(multi_zone && caps.has("zone-preview-frames"));
    });
    on_cleanup(move || ws.set_zone_preview_active.set(false));

    let composition_open = RwSignal::new(false);

    provide_context(StudioContext {
        selected_surface_id,
        active_scene,
        refresh_scene,
        composition_open,
    });

    view! {
        <div class="flex h-full flex-col overflow-hidden">
            // Narrow-viewport drawer toggle; the tree sits beside the Stage
            // on `lg` and up, so this strip is hidden there.
            <div class="flex shrink-0 items-center border-b border-edge-subtle/70 bg-surface-raised/40 px-3 py-2 lg:hidden">
                <button
                    type="button"
                    class="inline-flex items-center gap-1.5 rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-2.5 py-1.5 text-[11px] font-medium text-fg-secondary btn-press"
                    on:click=move |_| tree_drawer.set(true)
                >
                    <Icon icon=LuLightbulb width="13px" height="13px" />
                    "Zones & Devices"
                </button>
            </div>

            <div class="relative flex min-h-0 flex-1 overflow-hidden">
                <div
                    class="w-80 shrink-0 overflow-hidden lg:w-[var(--tree-w)] max-lg:fixed max-lg:inset-y-0 max-lg:left-0 max-lg:z-40 max-lg:border-r max-lg:border-edge-subtle/70 max-lg:transition-transform max-lg:duration-200"
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
                    <LayoutEditorProvider>
                        <Stage />
                    </LayoutEditorProvider>
                    <CompositionPanel
                        active_scene=active_scene
                        selected_group_id=selected_surface_id.read_only()
                        set_selected_group_id=selected_surface_id.write_only()
                        surface_label=surface_label
                        layers_resource=layers_resource
                        on_layers_mutated=on_layers_mutated
                    />
                </div>

                <Show when=move || tree_drawer.get()>
                    <div
                        class="fixed inset-0 z-30 bg-black/55 lg:hidden"
                        on:click=move |_| tree_drawer.set(false)
                    />
                </Show>
            </div>
        </div>
    }
}
