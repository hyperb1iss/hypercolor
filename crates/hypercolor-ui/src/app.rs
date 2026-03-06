use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use hypercolor_types::effect::{ControlDefinition, ControlValue};

use crate::api;
use crate::components::shell::Shell;
use crate::pages::dashboard::DashboardPage;
use crate::pages::devices::DevicesPage;
use crate::pages::effects::EffectsPage;
use crate::ws::{CanvasFrame, ConnectionState, WsManager};

/// Global WebSocket state provided via Leptos context.
#[derive(Clone, Copy)]
pub struct WsContext {
    pub canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub connection_state: ReadSignal<ConnectionState>,
    pub fps: ReadSignal<f32>,
    pub active_effect: ReadSignal<Option<String>>,
}

/// Shared active-effect state — accessible from sidebar, effects page, etc.
#[derive(Clone, Copy)]
pub struct EffectsContext {
    pub effects_resource: LocalResource<Result<Vec<api::EffectSummary>, String>>,
    pub active_effect_id: ReadSignal<Option<String>>,
    pub set_active_effect_id: WriteSignal<Option<String>>,
    pub active_effect_name: ReadSignal<Option<String>>,
    pub set_active_effect_name: WriteSignal<Option<String>>,
    pub active_effect_category: ReadSignal<String>,
    pub set_active_effect_category: WriteSignal<String>,
    pub active_controls: ReadSignal<Vec<ControlDefinition>>,
    pub set_active_controls: WriteSignal<Vec<ControlDefinition>>,
    pub active_control_values: ReadSignal<HashMap<String, ControlValue>>,
    pub set_active_control_values: WriteSignal<HashMap<String, ControlValue>>,
    pub active_preset_id: ReadSignal<Option<String>>,
    pub set_active_preset_id: WriteSignal<Option<String>>,
    pub favorite_ids: ReadSignal<HashSet<String>>,
    pub set_favorite_ids: WriteSignal<HashSet<String>>,
}

/// Shared device + layout state — accessible from devices page and layout builder.
#[derive(Clone, Copy)]
pub struct DevicesContext {
    pub devices_resource: LocalResource<Result<Vec<api::DeviceSummary>, String>>,
    pub layouts_resource: LocalResource<Result<Vec<api::LayoutSummary>, String>>,
}

impl EffectsContext {
    /// Apply an effect by ID — sets local state + calls API.
    pub fn apply_effect(&self, id: String) {
        // Skip if already the active effect
        if self.active_effect_id.get().as_deref() == Some(&id) {
            return;
        }
        let category = self
            .effects_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|effects| {
                effects
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.category.clone())
            })
            .unwrap_or_default();
        self.set_active_effect_id.set(Some(id.clone()));
        self.set_active_effect_category.set(category);
        self.set_active_controls.set(Vec::new());
        self.set_active_control_values.set(HashMap::new());
        self.set_active_preset_id.set(None);

        let set_name = self.set_active_effect_name;
        let set_controls = self.set_active_controls;
        let set_values = self.set_active_control_values;
        let set_preset = self.set_active_preset_id;

        leptos::task::spawn_local(async move {
            let _ = api::apply_effect(&id).await;
            if let Ok(Some(active)) = api::fetch_active_effect().await {
                set_name.set(Some(active.name));
                set_controls.set(active.controls);
                set_values.set(active.control_values);
                set_preset.set(active.active_preset_id);
            }
        });
    }

    /// Toggle an effect's favorite status.
    pub fn toggle_favorite(&self, effect_id: String) {
        let is_fav = self.favorite_ids.get().contains(&effect_id);
        let set_favorites = self.set_favorite_ids;

        if is_fav {
            set_favorites.update(|ids| {
                ids.remove(&effect_id);
            });
            leptos::task::spawn_local(async move {
                let _ = api::remove_favorite(&effect_id).await;
            });
        } else {
            set_favorites.update({
                let id = effect_id.clone();
                move |ids| {
                    ids.insert(id);
                }
            });
            leptos::task::spawn_local(async move {
                let _ = api::add_favorite(&effect_id).await;
            });
        }
    }

    /// Stop the active effect.
    pub fn stop_effect(&self) {
        self.set_active_effect_id.set(None);
        self.set_active_effect_name.set(None);
        self.set_active_controls.set(Vec::new());
        self.set_active_control_values.set(HashMap::new());
        self.set_active_effect_category.set(String::new());
        self.set_active_preset_id.set(None);
        leptos::task::spawn_local(async move {
            let _ = api::stop_effect().await;
        });
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    leptoaster::provide_toaster();

    // Global WebSocket connection
    let ws = WsManager::new();
    let ws_ctx = WsContext {
        canvas_frame: ws.canvas_frame,
        connection_state: ws.connection_state,
        fps: ws.fps,
        active_effect: ws.active_effect,
    };
    provide_context(ws_ctx);

    // Global effects state — shared between sidebar player + effects page
    let effects_resource = LocalResource::new(api::fetch_effects);
    let active_resource = LocalResource::new(api::fetch_active_effect);
    let favorites_resource = LocalResource::new(api::fetch_favorites);
    let (active_effect_id, set_active_effect_id) = signal(None::<String>);
    let (active_effect_name, set_active_effect_name) = signal(None::<String>);
    let (active_effect_category, set_active_effect_category) = signal(String::new());
    let (active_controls, set_active_controls) = signal(Vec::<ControlDefinition>::new());
    let (active_control_values, set_active_control_values) =
        signal(HashMap::<String, ControlValue>::new());
    let (active_preset_id, set_active_preset_id) = signal(None::<String>);
    let (favorite_ids, set_favorite_ids) = signal(HashSet::<String>::new());

    let effects_ctx = EffectsContext {
        effects_resource,
        active_effect_id,
        set_active_effect_id,
        active_effect_name,
        set_active_effect_name,
        active_effect_category,
        set_active_effect_category,
        active_controls,
        set_active_controls,
        active_control_values,
        set_active_control_values,
        active_preset_id,
        set_active_preset_id,
        favorite_ids,
        set_favorite_ids,
    };
    provide_context(effects_ctx);

    // Initialize favorites from API on load
    Effect::new(move |_| {
        if let Some(Ok(favorites)) = favorites_resource.get() {
            let ids: HashSet<String> = favorites.iter().map(|f| f.effect_id.clone()).collect();
            set_favorite_ids.set(ids);
        }
    });

    // Global devices + layouts state
    let devices_resource = LocalResource::new(api::fetch_devices);
    let layouts_resource = LocalResource::new(api::fetch_layouts);
    provide_context(DevicesContext {
        devices_resource,
        layouts_resource,
    });

    // Keep devices list fresh so startup discovery results appear in the UI
    // without requiring a manual refresh/scan click.
    Effect::new(move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let devices_resource = devices_resource;
        let callback = Closure::<dyn FnMut()>::new(move || {
            devices_resource.refetch();
        });

        let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            5_000,
        );
        callback.forget();
    });

    // Initialize active effect from API on load
    Effect::new(move |_| {
        if let Some(Ok(Some(active))) = active_resource.get() {
            let active_id = active.id.clone();
            set_active_effect_id.set(Some(active.id));
            set_active_effect_name.set(Some(active.name));
            set_active_controls.set(active.controls);
            set_active_control_values.set(active.control_values);
            set_active_preset_id.set(active.active_preset_id);
            if let Some(Ok(effects)) = effects_resource.get() {
                if let Some(e) = effects.iter().find(|e| e.id == active_id) {
                    set_active_effect_category.set(e.category.clone());
                }
            }
        } else if let Some(Ok(None)) = active_resource.get() {
            set_active_effect_id.set(None);
            set_active_effect_name.set(None);
            set_active_controls.set(Vec::new());
            set_active_control_values.set(HashMap::new());
            set_active_effect_category.set(String::new());
            set_active_preset_id.set(None);
        }
    });

    view! {
        <Meta charset="UTF-8" />
        <Meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <Title text="Hypercolor" />

        <Router>
            <Shell>
                <Routes fallback=|| view! { <p class="text-text-tertiary p-8">"Not found"</p> }>
                    <Route path=path!("/") view=DashboardPage />
                    <Route path=path!("/effects") view=EffectsPage />
                    <Route path=path!("/effects/:id") view=EffectsPage />
                    <Route path=path!("/devices") view=DevicesPage />
                </Routes>
            </Shell>
        </Router>

        <leptoaster::Toaster />
    }
}
