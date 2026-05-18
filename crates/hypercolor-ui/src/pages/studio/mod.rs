//! `/studio` — the unified surface-centric composition workspace (Spec 65).
//!
//! Three rails: the Lights & Screens surface list, the center Stage, and
//! the reused layer-stack editor. Selecting a surface loads its layer
//! stack and preview. The per-surface model is identical for one zone or
//! twelve, so multi-zone (Waves 9-10) fills in the rail rather than
//! rebuilding the workspace.

mod stage;
mod surface;
mod surface_rail;

use hypercolor_types::scene::RenderGroupRole;
use leptos::prelude::*;

use crate::api;
use crate::components::layer_panel::LayerPanel;
use crate::components::resize_handle::ResizeHandle;
use crate::storage;

use stage::Stage;
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

    provide_context(StudioContext {
        selected_surface_id,
        active_scene,
    });

    view! {
        <div class="flex h-full overflow-hidden">
            <div
                class="shrink-0 overflow-hidden"
                style:width=move || format!("{}px", surface_width.get())
            >
                <SurfaceRail />
            </div>
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
            <div class="min-w-0 flex-1">
                <Stage />
            </div>
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
            <aside
                class="scrollbar-none shrink-0 overflow-y-auto border-l border-edge-subtle/70 bg-surface-sunken/35 px-4 pb-6"
                style:width=move || format!("{}px", layers_width.get())
            >
                <LayerPanel
                    active_scene=active_scene
                    selected_group_id=selected_surface_id.read_only()
                    set_selected_group_id=selected_surface_id.write_only()
                    layers_resource=layers_resource
                    on_layers_mutated=on_layers_mutated
                />
            </aside>
        </div>
    }
}
