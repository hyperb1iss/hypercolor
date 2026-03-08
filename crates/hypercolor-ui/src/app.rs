use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use hypercolor_types::effect::{ControlDefinition, ControlValue};

use crate::api;
use crate::components::shell::Shell;
use crate::pages::dashboard::DashboardPage;
use crate::pages::devices::DevicesPage;
use crate::pages::effects::EffectsPage;
use crate::pages::layout::LayoutPage;
use crate::pages::settings::SettingsPage;
use crate::ws::{
    AudioLevel, BackpressureNotice, CanvasFrame, ConnectionState, DeviceEventHint,
    PerformanceMetrics, WsManager,
};

#[derive(Debug, Clone, PartialEq)]
pub struct IndexedEffect {
    pub effect: api::EffectSummary,
    search_text: String,
}

impl IndexedEffect {
    fn new(effect: api::EffectSummary) -> Self {
        let name_aliases = effect_name_aliases(&effect.name);
        let search_text = [
            effect.name.to_lowercase(),
            effect.description.to_lowercase(),
            effect.author.to_lowercase(),
            effect.category.to_lowercase(),
            effect.tags.join(" ").to_lowercase(),
            name_aliases.join(" "),
        ]
        .join(" ");

        Self {
            effect,
            search_text,
        }
    }

    pub fn matches_search(&self, term: &str) -> bool {
        term.is_empty() || self.search_text.contains(term)
    }
}

fn effect_name_aliases(name: &str) -> Vec<String> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Vec::new();
    }

    let mut aliases = vec![
        normalized.replace([' ', '-'], "_"),
        normalized.replace([' ', '_'], "-"),
        normalized.replace([' ', '_', '-'], ""),
    ];
    aliases.retain(|alias| !alias.is_empty() && alias != &normalized);
    aliases.sort();
    aliases.dedup();
    aliases
}

/// Global WebSocket state provided via Leptos context.
#[derive(Clone, Copy)]
pub struct WsContext {
    pub canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub connection_state: ReadSignal<ConnectionState>,
    pub preview_fps: ReadSignal<f32>,
    pub preview_target_fps: ReadSignal<u32>,
    pub set_preview_cap: WriteSignal<u32>,
    pub metrics: ReadSignal<Option<PerformanceMetrics>>,
    pub backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    pub active_effect: ReadSignal<Option<String>>,
    pub last_device_event: ReadSignal<Option<DeviceEventHint>>,
    pub audio_level: ReadSignal<AudioLevel>,
}

/// Shared active-effect state — accessible from sidebar, effects page, etc.
#[derive(Clone, Copy)]
pub struct EffectsContext {
    pub effects_index: Memo<Vec<IndexedEffect>>,
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
    fn effect_summary(&self, id: &str) -> Option<api::EffectSummary> {
        self.effects_index.with_untracked(|effects| {
            effects
                .iter()
                .find(|entry| entry.effect.id == id)
                .map(|entry| entry.effect.clone())
        })
    }

    pub fn refresh_active_effect(&self) {
        let ctx = *self;
        leptos::task::spawn_local(async move {
            match api::fetch_active_effect().await {
                Ok(Some(active)) => {
                    apply_active_effect_snapshot(
                        &ctx,
                        active.id.clone(),
                        active.name,
                        active.controls,
                        active.control_values,
                        active.active_preset_id,
                    );
                }
                Ok(None) => clear_active_effect_state(&ctx),
                Err(_) => {}
            }
        });
    }

    /// Apply an effect by ID — sets local state + calls API.
    pub fn apply_effect(&self, id: String) {
        // Skip if already the active effect
        if self.active_effect_id.get().as_deref() == Some(&id) {
            return;
        }

        let selected_effect = self.effect_summary(&id);
        self.set_active_effect_id.set(Some(id.clone()));
        self.set_active_effect_name
            .set(selected_effect.as_ref().map(|effect| effect.name.clone()));
        self.set_active_effect_category.set(
            selected_effect
                .as_ref()
                .map(|effect| effect.category.clone())
                .unwrap_or_default(),
        );
        self.set_active_controls.set(Vec::new());
        self.set_active_control_values.set(HashMap::new());
        self.set_active_preset_id.set(None);

        let ctx = *self;
        leptos::task::spawn_local(async move {
            if api::apply_effect(&id).await.is_ok() {
                ctx.refresh_active_effect();
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
        clear_active_effect_state(self);
        let ctx = *self;
        leptos::task::spawn_local(async move {
            if api::stop_effect().await.is_err() {
                ctx.refresh_active_effect();
            }
        });
    }
}

fn apply_active_effect_snapshot(
    ctx: &EffectsContext,
    id: String,
    name: String,
    controls: Vec<ControlDefinition>,
    control_values: HashMap<String, ControlValue>,
    active_preset_id: Option<String>,
) {
    let category = ctx
        .effect_summary(&id)
        .map(|effect| effect.category)
        .unwrap_or_default();

    ctx.set_active_effect_id.set(Some(id));
    ctx.set_active_effect_name.set(Some(name));
    ctx.set_active_effect_category.set(category);
    ctx.set_active_controls.set(controls);
    ctx.set_active_control_values.set(control_values);
    ctx.set_active_preset_id.set(active_preset_id);
}

fn clear_active_effect_state(ctx: &EffectsContext) {
    ctx.set_active_effect_id.set(None);
    ctx.set_active_effect_name.set(None);
    ctx.set_active_controls.set(Vec::new());
    ctx.set_active_control_values.set(HashMap::new());
    ctx.set_active_effect_category.set(String::new());
    ctx.set_active_preset_id.set(None);
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
        preview_fps: ws.preview_fps,
        preview_target_fps: ws.preview_target_fps,
        set_preview_cap: ws.set_preview_cap,
        metrics: ws.metrics,
        backpressure_notice: ws.backpressure_notice,
        active_effect: ws.active_effect,
        last_device_event: ws.last_device_event,
        audio_level: ws.audio_level,
    };
    provide_context(ws_ctx);

    // Global effects state — shared between sidebar player + effects page
    let effects_resource = LocalResource::new(api::fetch_effects);
    let effects_index = Memo::new(move |_| {
        effects_resource
            .get()
            .and_then(Result::ok)
            .map(|effects| effects.into_iter().map(IndexedEffect::new).collect())
            .unwrap_or_default()
    });
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
        effects_index,
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

    // Refresh devices reactively from daemon lifecycle events instead of
    // rebuilding the grid on a fixed timer.
    Effect::new(move |_| {
        let Some(event) = ws_ctx.last_device_event.get() else {
            return;
        };

        let current_devices = devices_resource
            .get()
            .and_then(|result| result.ok())
            .unwrap_or_default();

        let should_refetch = match event.event_type.as_str() {
            "device_discovered" => event.device_id.as_ref().is_some_and(|device_id| {
                !current_devices.iter().any(|device| &device.id == device_id)
            }),
            "device_disconnected" | "device_state_changed" => {
                event.device_id.as_ref().is_some_and(|device_id| {
                    current_devices.iter().any(|device| &device.id == device_id)
                })
            }
            "device_discovery_completed" => {
                current_devices.is_empty() && event.found_count.is_some_and(|count| count > 0)
            }
            _ => false,
        };

        if should_refetch {
            devices_resource.refetch();
        }
    });

    // Initialize active effect from API on load
    Effect::new(move |_| {
        if let Some(Ok(Some(active))) = active_resource.get() {
            apply_active_effect_snapshot(
                &effects_ctx,
                active.id,
                active.name,
                active.controls,
                active.control_values,
                active.active_preset_id,
            );
        } else if let Some(Ok(None)) = active_resource.get() {
            clear_active_effect_state(&effects_ctx);
        }
    });

    view! {
        <Meta charset="UTF-8" />
        <Meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <Title text="Hypercolor" />

        <Router>
            <Shell>
                <Routes fallback=|| view! { <p class="text-fg-tertiary p-8">"Not found"</p> }>
                    <Route path=path!("/") view=DashboardPage />
                    <Route path=path!("/effects") view=EffectsPage />
                    <Route path=path!("/effects/:id") view=EffectsPage />
                    <Route path=path!("/layout") view=LayoutPage />
                    <Route path=path!("/devices") view=DevicesPage />
                    <Route path=path!("/settings") view=SettingsPage />
                </Routes>
            </Shell>
        </Router>

        <leptoaster::Toaster />
    }
}
