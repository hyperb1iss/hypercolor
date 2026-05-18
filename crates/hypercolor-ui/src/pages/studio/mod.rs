//! `/studio` — the unified surface-centric composition workspace (Spec 65).
//!
//! Three rails: the Lights & Screens surface list, the center Stage, and
//! the reused layer-stack editor. Selecting a surface loads its layer
//! stack and preview. The per-surface model is identical for one zone or
//! twelve, so multi-zone (Waves 9-10) fills in the rail rather than
//! rebuilding the workspace.

mod stage;
mod stage_view;
mod surface;
mod surface_rail;

use hypercolor_types::scene::RenderGroupRole;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::components::layer_panel::LayerPanel;
use crate::components::resize_handle::ResizeHandle;
use crate::icons::*;
use crate::storage;

use stage::Stage;
use surface::surfaces_from_groups;
use surface_rail::SurfaceRail;

const SURFACE_WIDTH_KEY: &str = "hc-studio-surface-width";
const LAYERS_WIDTH_KEY: &str = "hc-studio-layers-width";
const SURFACE_WIDTH_RANGE: (f64, f64) = (200.0, 440.0);
const LAYERS_WIDTH_RANGE: (f64, f64) = (300.0, 540.0);

/// Shared Studio state — the selected surface and the active scene —
/// provided to the rails so surface selection is one source of truth.
#[derive(Clone, Copy)]
pub struct StudioContext {
    pub selected_surface_id: RwSignal<Option<String>>,
    pub active_scene: Signal<Option<api::ActiveSceneResponse>>,
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
        let still_present = current
            .as_ref()
            .is_some_and(|id| scene.groups.iter().any(|group| group.id.to_string() == *id));
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
                (Some(scene), Some(group_id)) => api::list_layers(&scene.id, &group_id).await,
                _ => Ok(api::LayerStackResponse {
                    items: Vec::new(),
                    layers_version: 0,
                }),
            }
        }
    });

    let on_layers_mutated = Callback::new(move |()| {
        set_layers_tick.update(|tick| *tick = tick.wrapping_add(1));
        set_scene_tick.update(|tick| *tick = tick.wrapping_add(1));
    });

    // The surface rail owns selection, so the layer panel shows the
    // selected surface's name in its header instead of a redundant
    // group selector.
    let surface_label = Signal::derive(move || {
        let id = selected_surface_id.get()?;
        let scene = active_scene.get()?;
        surfaces_from_groups(&scene.groups)
            .into_iter()
            .find(|surface| surface.id == id)
            .map(|surface| surface.name)
    });

    // Rail widths persist per browser; the Stage takes the space between.
    let surface_width = RwSignal::new(storage::get_clamped(
        SURFACE_WIDTH_KEY,
        264.0,
        SURFACE_WIDTH_RANGE.0,
        SURFACE_WIDTH_RANGE.1,
    ));
    let layers_width = RwSignal::new(storage::get_clamped(
        LAYERS_WIDTH_KEY,
        380.0,
        LAYERS_WIDTH_RANGE.0,
        LAYERS_WIDTH_RANGE.1,
    ));
    Effect::new(move |_| {
        storage::set(SURFACE_WIDTH_KEY, &surface_width.get().to_string());
    });
    Effect::new(move |_| {
        storage::set(LAYERS_WIDTH_KEY, &layers_width.get().to_string());
    });
    let surface_drag_start = StoredValue::new(0.0_f64);
    let layers_drag_start = StoredValue::new(0.0_f64);

    // Narrow viewports collapse the rails into slide-over drawers (§13).
    let surface_drawer = RwSignal::new(false);
    let layers_drawer = RwSignal::new(false);
    // Picking a surface from the drawer reveals the Stage behind it.
    Effect::new(move |_| {
        let _ = selected_surface_id.get();
        if surface_drawer.get_untracked() {
            surface_drawer.set(false);
        }
    });

    provide_context(StudioContext {
        selected_surface_id,
        active_scene,
    });

    view! {
        <div class="flex h-full flex-col overflow-hidden">
            // Narrow-viewport drawer toggles; the three rails are side by
            // side on `lg` and up, so this strip is hidden there.
            <div class="flex shrink-0 items-center gap-2 border-b border-edge-subtle/70 bg-surface-raised/40 px-3 py-2 lg:hidden">
                <button
                    type="button"
                    class="inline-flex items-center gap-1.5 rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-2.5 py-1.5 text-[11px] font-medium text-fg-secondary btn-press"
                    on:click=move |_| {
                        layers_drawer.set(false);
                        surface_drawer.set(true);
                    }
                >
                    <Icon icon=LuLightbulb width="13px" height="13px" />
                    "Lights & Screens"
                </button>
                <div class="flex-1" />
                <button
                    type="button"
                    class="inline-flex items-center gap-1.5 rounded-lg border border-edge-subtle/70 bg-surface-overlay/40 px-2.5 py-1.5 text-[11px] font-medium text-fg-secondary btn-press"
                    on:click=move |_| {
                        surface_drawer.set(false);
                        layers_drawer.set(true);
                    }
                >
                    "Layers"
                    <Icon icon=LuLayers width="13px" height="13px" />
                </button>
            </div>

            <div class="relative flex min-h-0 flex-1 overflow-hidden">
                <div
                    class="w-80 shrink-0 overflow-hidden lg:w-[var(--surface-w)] max-lg:fixed max-lg:inset-y-0 max-lg:left-0 max-lg:z-40 max-lg:border-r max-lg:border-edge-subtle/70 max-lg:transition-transform max-lg:duration-200"
                    class=("max-lg:-translate-x-full", move || !surface_drawer.get())
                    style:--surface-w=move || format!("{}px", surface_width.get())
                >
                    <SurfaceRail />
                </div>
                <div class="hidden lg:contents">
                    <ResizeHandle
                        on_drag_start=Callback::new(move |()| {
                            surface_drag_start.set_value(surface_width.get_untracked());
                        })
                        on_drag=Callback::new(move |delta: f64| {
                            let next = (surface_drag_start.get_value() + delta)
                                .clamp(SURFACE_WIDTH_RANGE.0, SURFACE_WIDTH_RANGE.1);
                            surface_width.set(next);
                        })
                        on_drag_end=Callback::new(|()| {})
                    />
                </div>
                <div class="min-w-0 flex-1">
                    <Stage />
                </div>
                <div class="hidden lg:contents">
                    <ResizeHandle
                        on_drag_start=Callback::new(move |()| {
                            layers_drag_start.set_value(layers_width.get_untracked());
                        })
                        on_drag=Callback::new(move |delta: f64| {
                            let next = (layers_drag_start.get_value() - delta)
                                .clamp(LAYERS_WIDTH_RANGE.0, LAYERS_WIDTH_RANGE.1);
                            layers_width.set(next);
                        })
                        on_drag_end=Callback::new(|()| {})
                    />
                </div>
                <aside
                    class="scrollbar-none w-80 shrink-0 overflow-y-auto border-l border-edge-subtle/70 bg-surface-sunken/35 px-4 pb-6 lg:w-[var(--layers-w)] max-lg:fixed max-lg:inset-y-0 max-lg:right-0 max-lg:z-40 max-lg:transition-transform max-lg:duration-200"
                    class=("max-lg:translate-x-full", move || !layers_drawer.get())
                    style:--layers-w=move || format!("{}px", layers_width.get())
                >
                    <LayerPanel
                        active_scene=active_scene
                        selected_group_id=selected_surface_id.read_only()
                        set_selected_group_id=selected_surface_id.write_only()
                        surface_label=surface_label
                        layers_resource=layers_resource
                        on_layers_mutated=on_layers_mutated
                    />
                </aside>

                <Show when=move || surface_drawer.get() || layers_drawer.get()>
                    <div
                        class="fixed inset-0 z-30 bg-black/55 lg:hidden"
                        on:click=move |_| {
                            surface_drawer.set(false);
                            layers_drawer.set(false);
                        }
                    />
                </Show>
            </div>
        </div>
    }
}
