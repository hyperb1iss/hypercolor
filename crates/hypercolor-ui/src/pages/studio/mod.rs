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
mod zone_assignment;

use hypercolor_types::scene::RenderGroupRole;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::{CapabilitiesContext, WsContext};
use crate::components::layer_panel::LayerPanel;
use crate::components::resize_handle::ResizeHandle;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;
use crate::storage;

use stage::Stage;
use stage_view::StageView;
use surface::{SurfaceKind, UNASSIGNED_SURFACE_ID, surfaces_from_groups};
use surface_rail::SurfaceRail;

const LAYERS_COLLAPSED_KEY: &str = "hc-studio-layers-collapsed";

/// An empty layer stack at version 0 — the resource value for a selection
/// that has no per-group layer endpoint (none selected, or the synthetic
/// Unassigned entry).
fn empty_layer_stack() -> api::LayerStackResponse {
    api::LayerStackResponse {
        items: Vec::new(),
        layers_version: 0,
    }
}

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
    /// Re-fetch the active scene. Zone mutations call this so the rail and
    /// Stage pick up the new group set and `groups_revision`.
    pub refresh_scene: Callback<()>,
    /// The Stage's requested view. Lifted to the context so the workspace
    /// can react — the Layers rail steps aside while the Layout view owns
    /// the Stage, since layout editing is not layer compositing.
    pub stage_view: RwSignal<StageView>,
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

    // The Unassigned entry has no layer stack; the rail still renders, but
    // the Layers rail swaps the panel for an explanatory note.
    let is_unassigned = Signal::derive(move || {
        selected_surface_id.get().as_deref() == Some(UNASSIGNED_SURFACE_ID)
    });

    // Per-zone preview frames (§9.5) are streamed only while Studio shows a
    // genuinely multi-zone scene and the daemon advertises the capability;
    // a single-zone scene's per-zone canvas is just the composited canvas,
    // so there is nothing extra to stream.
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

    // The Stage view, lifted so the workspace can collapse the Layers rail
    // while Layout owns the Stage.
    let stage_view = RwSignal::new(StageView::default());

    // The Layers rail is manually collapsible (persisted per browser) and
    // also steps aside automatically while the Layout view is active for a
    // Light — layout editing is not layer compositing, so the rail would
    // only be noise there.
    let layers_collapsed = RwSignal::new(
        storage::get_parsed::<bool>(LAYERS_COLLAPSED_KEY).unwrap_or(false),
    );
    Effect::new(move |_| {
        storage::set(LAYERS_COLLAPSED_KEY, &layers_collapsed.get().to_string());
    });
    let selected_is_light = Signal::derive(move || {
        let (Some(id), Some(scene)) = (selected_surface_id.get(), active_scene.get()) else {
            return false;
        };
        surfaces_from_groups(&scene.groups)
            .into_iter()
            .find(|surface| surface.id == id)
            .is_some_and(|surface| surface.kind == SurfaceKind::Light)
    });
    let layout_active =
        Signal::derive(move || stage_view.get() == StageView::Layout && selected_is_light.get());
    // The rail shows for a real surface that is not in Layout mode and not
    // manually collapsed. The Unassigned entry keeps its own note.
    let layers_visible =
        Signal::derive(move || !layout_active.get() && !layers_collapsed.get());

    provide_context(StudioContext {
        selected_surface_id,
        active_scene,
        refresh_scene,
        stage_view,
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
                        layers_collapsed.set(false);
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
                // Layers resize handle — desktop only, and gone while the
                // rail is collapsed or stepped aside for Layout.
                <Show when=move || layers_visible.get()>
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
                </Show>
                <aside
                    class="scrollbar-none relative w-80 shrink-0 overflow-y-auto border-l border-edge-subtle/70 bg-surface-sunken/35 px-4 pb-6 lg:w-[var(--layers-w)] max-lg:fixed max-lg:inset-y-0 max-lg:right-0 max-lg:z-40 max-lg:transition-transform max-lg:duration-200"
                    class=("max-lg:translate-x-full", move || !layers_drawer.get())
                    class=("lg:hidden", move || !layers_visible.get())
                    style:--layers-w=move || format!("{}px", layers_width.get())
                >
                    // Desktop collapse control — reclaims the Stage width.
                    <button
                        type="button"
                        class="absolute left-1 top-3 z-10 hidden rounded-md p-1 text-fg-tertiary transition-colors hover:text-fg-primary lg:block"
                        title="Hide the layers panel"
                        on:click=move |_| layers_collapsed.set(true)
                    >
                        <Icon icon=LuChevronRight width="14px" height="14px" />
                    </button>
                    {move || {
                        if is_unassigned.get() {
                            view! { <UnassignedLayersNote /> }.into_any()
                        } else {
                            view! {
                                <LayerPanel
                                    active_scene=active_scene
                                    selected_group_id=selected_surface_id.read_only()
                                    set_selected_group_id=selected_surface_id.write_only()
                                    surface_label=surface_label
                                    layers_resource=layers_resource
                                    on_layers_mutated=on_layers_mutated
                                />
                            }
                                .into_any()
                        }
                    }}
                </aside>
                // Collapsed re-open strip — desktop only, while the rail is
                // manually collapsed (Layout mode hides it with no strip).
                <Show when=move || layers_collapsed.get() && !layout_active.get()>
                    <button
                        type="button"
                        class="hidden shrink-0 items-center justify-center border-l border-edge-subtle/70 bg-surface-raised/40 px-2 text-fg-tertiary transition-colors hover:text-fg-primary lg:flex"
                        title="Show the layers panel"
                        on:click=move |_| layers_collapsed.set(false)
                    >
                        <Icon icon=LuLayers width="15px" height="15px" />
                    </button>
                </Show>

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

/// The Layers rail content shown while the synthetic Unassigned entry is
/// selected. Unassigned device outputs are not a surface (§9.4): they have
/// no layer stack, so the rail explains that rather than rendering an
/// empty editor. Assigning outputs into a zone happens in the Stage
/// Layout view.
#[component]
fn UnassignedLayersNote() -> impl IntoView {
    view! {
        <div class="pt-4">
            <div class="mb-3">
                <span class=label_class(LabelSize::Section, LabelTone::Strong)>"Layers"</span>
            </div>
            <div class="rounded-xl border border-dashed border-edge-subtle/55 bg-surface-overlay/30 px-4 py-6 text-center">
                <div class="mx-auto mb-3 flex h-9 w-9 items-center justify-center rounded-lg bg-surface-sunken/70">
                    <Icon
                        icon=LuBan
                        width="16px"
                        height="16px"
                        style="color: rgba(241, 250, 140, 0.75)"
                    />
                </div>
                <div class="text-sm font-medium text-fg-secondary">"No layer stack"</div>
                <div class="mt-1.5 text-[12px] leading-5 text-fg-tertiary/70">
                    "Unassigned lights belong to no zone, so there is nothing to
                     compose here. Assign their outputs to a zone in the Stage
                     Layout view to give them a layer stack."
                </div>
            </div>
        </div>
    }
}
