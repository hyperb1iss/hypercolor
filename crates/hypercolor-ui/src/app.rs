use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use hypercolor_types::effect::{ControlDefinition, ControlValue};

use crate::api;
use crate::components::preset_matching::controls_to_json;
use crate::components::shell::Shell;
use crate::pages::dashboard::DashboardPage;
use crate::pages::devices::DevicesPage;
use crate::pages::displays::DisplaysPage;
use crate::pages::effects::EffectsPage;
use crate::pages::layout::LayoutPage;
use crate::pages::settings::SettingsPage;
use crate::preferences::{EffectPreferences, PreferencesStore};
use crate::preview_telemetry::{PreviewPresenterTelemetry, PreviewTelemetryContext};
use crate::thumbnails::{self, ThumbnailStore};
use crate::ws::{
    AudioLevel, BackpressureNotice, CanvasFrame, ConnectionState, DeviceEventHint,
    PerformanceMetrics, WsManager,
};

#[derive(Debug, Clone, PartialEq)]
struct ActiveEffectSnapshot {
    id: Option<String>,
    name: Option<String>,
    category: String,
    controls: Vec<ControlDefinition>,
    control_values: HashMap<String, ControlValue>,
    preset_id: Option<String>,
}

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
    pub screen_canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub web_viewport_canvas_frame: ReadSignal<Option<CanvasFrame>>,
    /// Latest per-display JPEG frame from the `display_preview` WS
    /// channel. Cleared when the selected display changes (handled by
    /// `set_display_preview_device`).
    pub display_preview_frame: ReadSignal<Option<CanvasFrame>>,
    /// Set to `Some(device_id)` to subscribe the live preview stream to
    /// that display, or `None` to unsubscribe. Setting the same id twice
    /// is safe — subsequent subscribes retarget without dropping the
    /// existing relay task on the server.
    pub set_display_preview_device: WriteSignal<Option<String>>,
    pub connection_state: ReadSignal<ConnectionState>,
    pub preview_fps: ReadSignal<f32>,
    pub preview_target_fps: ReadSignal<u32>,
    pub set_preview_cap: WriteSignal<u32>,
    pub set_preview_consumers: WriteSignal<u32>,
    pub set_screen_preview_consumers: WriteSignal<u32>,
    pub set_web_viewport_preview_consumers: WriteSignal<u32>,
    pub metrics: ReadSignal<Option<PerformanceMetrics>>,
    pub backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    pub active_effect: ReadSignal<Option<String>>,
    pub last_device_event: ReadSignal<Option<DeviceEventHint>>,
    pub audio_level: ReadSignal<AudioLevel>,
    pub audio_enabled: ReadSignal<bool>,
    pub set_audio_enabled: WriteSignal<bool>,
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
    pub is_playing: ReadSignal<bool>,
    pub set_is_playing: WriteSignal<bool>,
    pub favorite_ids: ReadSignal<HashSet<String>>,
    pub set_favorite_ids: WriteSignal<HashSet<String>>,
    /// Per-effect preferences (preset + control-value snapshot). Embedded
    /// on the context rather than looked up via `use_context` so the
    /// save/restore path that runs inside spawned async tasks doesn't
    /// depend on the reactive owner being live.
    pub preferences: PreferencesStore,
    /// Effect IDs we've already checked against preferences this
    /// session. `apply_active_effect_snapshot` consults this before
    /// running the restore path so user modifications to the current
    /// effect (preset selection, control tweaks) only trigger a save
    /// — without it, every refresh after a user action would see
    /// "daemon != stored" and mis-fire a restore that clobbers the
    /// change. Cleared for an effect when `apply_effect(id)` is called,
    /// so switching away and coming back re-triggers the restore.
    pub restored_effects: StoredValue<HashSet<String>>,
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
                Ok(None) => ctx.set_is_playing.set(false),
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

        // Drop this effect from the "already checked for restore" set —
        // the next snapshot for this ID should run through the restore
        // path so the user's saved preferences get re-applied on top of
        // whatever defaults the daemon loads.
        self.restored_effects.update_value(|set| {
            set.remove(&id);
        });

        let previous = capture_active_effect_state(self);
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
            } else {
                restore_active_effect_state(&ctx, previous);
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
            let revert_id = effect_id.clone();
            leptos::task::spawn_local(async move {
                if api::remove_favorite(&effect_id).await.is_err() {
                    set_favorites.update(|ids| {
                        ids.insert(revert_id);
                    });
                }
            });
        } else {
            set_favorites.update({
                let id = effect_id.clone();
                move |ids| {
                    ids.insert(id);
                }
            });
            let revert_id = effect_id.clone();
            leptos::task::spawn_local(async move {
                if api::add_favorite(&effect_id).await.is_err() {
                    set_favorites.update(|ids| {
                        ids.remove(&revert_id);
                    });
                }
            });
        }
    }

    /// Stop the active effect (keeps metadata visible for the sidebar).
    pub fn stop_effect(&self) {
        self.set_is_playing.set(false);
        let ctx = *self;
        leptos::task::spawn_local(async move {
            if api::stop_effect().await.is_err() {
                ctx.refresh_active_effect();
            }
        });
    }

    /// Resume the previously stopped effect.
    pub fn resume_effect(&self) {
        if let Some(id) = self.active_effect_id.get_untracked() {
            self.set_is_playing.set(true);
            let ctx = *self;
            leptos::task::spawn_local(async move {
                if api::apply_effect(&id).await.is_ok() {
                    ctx.refresh_active_effect();
                } else {
                    ctx.set_is_playing.set(false);
                }
            });
        }
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

    ctx.set_active_effect_name.set(Some(name));
    ctx.set_active_effect_category.set(category);
    ctx.set_active_controls.set(controls);
    ctx.set_active_control_values.set(control_values.clone());
    ctx.set_active_preset_id.set(active_preset_id.clone());
    ctx.set_is_playing.set(true);
    if ctx.active_effect_id.get_untracked().as_deref() != Some(id.as_str()) {
        ctx.set_active_effect_id.set(Some(id.clone()));
    }

    // ── Per-effect preferences: restore or save ───────────────────────
    //
    // Two paths:
    //
    //   1. First snapshot after a switch → RESTORE. The daemon has just
    //      loaded defaults; if our stored preferences differ, re-apply
    //      the saved state to the daemon.
    //
    //   2. Any follow-up snapshot (user picked a preset, tweaked a
    //      control, etc.) → SAVE. The daemon is already in the state
    //      the user just asked for; we just need to capture it.
    //
    // The `restored_effects` set gates this. It's cleared for an effect
    // ID when `apply_effect(id)` is called, so we re-check on each
    // genuine switch, and marked after the first check so subsequent
    // refreshes for the same effect fall through to save.
    let store = ctx.preferences;
    let already_checked = ctx
        .restored_effects
        .with_value(|set| set.contains(id.as_str()));
    if !already_checked {
        ctx.restored_effects.update_value(|set| {
            set.insert(id.clone());
        });

        if let Some(prefs) = store.get(&id) {
            // Compare through the same lossy JSON serializer we use to
            // send controls to the daemon — colors hex-encode to 256
            // steps, so a naive `HashMap` equality would mis-fire
            // thanks to float precision drift on round-trip.
            let stored_json = controls_to_json(&prefs.control_values);
            let daemon_json = controls_to_json(&control_values);
            let needs_restore = prefs.preset_id != active_preset_id || stored_json != daemon_json;
            if needs_restore {
                restore_effect_preferences(*ctx, id, prefs);
                return;
            }
        }
    }

    // Save path — either this was the first snapshot with nothing to
    // restore, or it's a follow-up after user modification. In both
    // cases, capture whatever the daemon just confirmed so switching
    // away and coming back lands us in the same place.
    store.save(
        id,
        EffectPreferences {
            preset_id: active_preset_id,
            control_values,
        },
    );
}

/// Re-applies a remembered preset + control snapshot on top of the
/// daemon's defaults. Runs fully asynchronously — apply_preset first (if
/// any), then update_controls, then a final refresh so the signals
/// reflect the restored state. Bails at every step if the user has
/// switched effects in the meantime, since a late-landing restore from
/// effect A would trample a freshly-activated effect B.
/// Re-applies a remembered preset + control snapshot on top of the
/// daemon's defaults. Fully async — apply_preset first (if any), then
/// update_controls using the same hex-colour-encoding serializer the
/// preset picker uses (the daemon silently ignores `ControlValue`'s
/// default tagged JSON), then a final refresh so the UI mirrors the
/// restored daemon state. Bails at every step if the user has switched
/// effects in the meantime — a late-landing restore from effect A would
/// otherwise trample a freshly-activated effect B.
fn restore_effect_preferences(ctx: EffectsContext, effect_id: String, prefs: EffectPreferences) {
    leptos::task::spawn_local(async move {
        if ctx.active_effect_id.get_untracked().as_deref() != Some(effect_id.as_str()) {
            return;
        }

        if let Some(preset_id) = prefs.preset_id.as_ref() {
            let _ = api::apply_preset(preset_id).await;
            if ctx.active_effect_id.get_untracked().as_deref() != Some(effect_id.as_str()) {
                return;
            }
        }

        if !prefs.control_values.is_empty() {
            let controls_json = serde_json::Value::Object(controls_to_json(&prefs.control_values));
            let _ = api::update_controls(&controls_json).await;
            if ctx.active_effect_id.get_untracked().as_deref() != Some(effect_id.as_str()) {
                return;
            }
        }

        // Surface the restored daemon state in the UI. This re-enters
        // `apply_active_effect_snapshot`, but with the effect already
        // present in `restored_effects` so the save branch fires.
        ctx.refresh_active_effect();
    });
}

fn clear_active_effect_state(ctx: &EffectsContext) {
    ctx.set_active_effect_id.set(None);
    ctx.set_active_effect_name.set(None);
    ctx.set_active_controls.set(Vec::new());
    ctx.set_active_control_values.set(HashMap::new());
    ctx.set_active_effect_category.set(String::new());
    ctx.set_active_preset_id.set(None);
    ctx.set_is_playing.set(false);
}

fn capture_active_effect_state(ctx: &EffectsContext) -> ActiveEffectSnapshot {
    ActiveEffectSnapshot {
        id: ctx.active_effect_id.get_untracked(),
        name: ctx.active_effect_name.get_untracked(),
        category: ctx.active_effect_category.get_untracked(),
        controls: ctx.active_controls.get_untracked(),
        control_values: ctx.active_control_values.get_untracked(),
        preset_id: ctx.active_preset_id.get_untracked(),
    }
}

fn restore_active_effect_state(ctx: &EffectsContext, snapshot: ActiveEffectSnapshot) {
    match snapshot.id {
        Some(id) => {
            ctx.set_active_effect_id.set(Some(id));
            ctx.set_active_effect_name.set(snapshot.name);
            ctx.set_active_effect_category.set(snapshot.category);
            ctx.set_active_controls.set(snapshot.controls);
            ctx.set_active_control_values.set(snapshot.control_values);
            ctx.set_active_preset_id.set(snapshot.preset_id);
        }
        None => clear_active_effect_state(ctx),
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    leptoaster::provide_toaster();

    // Global WebSocket connection
    let ws = WsManager::new();
    let (audio_enabled, set_audio_enabled) = signal(false);
    let (preview_presenter, set_preview_presenter) = signal(PreviewPresenterTelemetry::default());

    // Seed audio_enabled from daemon config
    leptos::task::spawn_local(async move {
        if let Ok(cfg) = api::fetch_config().await {
            set_audio_enabled.set(cfg.audio.enabled);
        }
    });

    let ws_ctx = WsContext {
        canvas_frame: ws.canvas_frame,
        screen_canvas_frame: ws.screen_canvas_frame,
        web_viewport_canvas_frame: ws.web_viewport_canvas_frame,
        display_preview_frame: ws.display_preview_frame,
        set_display_preview_device: ws.set_display_preview_device,
        connection_state: ws.connection_state,
        preview_fps: ws.preview_fps,
        preview_target_fps: ws.preview_target_fps,
        set_preview_cap: ws.set_preview_cap,
        set_preview_consumers: ws.set_preview_consumers,
        set_screen_preview_consumers: ws.set_screen_preview_consumers,
        set_web_viewport_preview_consumers: ws.set_web_viewport_preview_consumers,
        metrics: ws.metrics,
        backpressure_notice: ws.backpressure_notice,
        active_effect: ws.active_effect,
        last_device_event: ws.last_device_event,
        audio_level: ws.audio_level,
        audio_enabled,
        set_audio_enabled,
    };
    provide_context(ws_ctx);
    provide_context(PreviewTelemetryContext {
        presenter: preview_presenter,
        set_presenter: set_preview_presenter,
    });

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
    let (is_playing, set_is_playing) = signal(false);
    let (favorite_ids, set_favorite_ids) = signal(HashSet::<String>::new());

    // Per-effect preferences store — remembers which preset was picked
    // and what the control values were for every effect the user has
    // customised, so switching effects feels stateful. Built before
    // `EffectsContext` so it can be embedded on the context itself —
    // the save/restore path runs inside spawned async tasks and can't
    // rely on `use_context` at that point.
    let preferences_store = PreferencesStore::new();
    provide_context(preferences_store);

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
        is_playing,
        set_is_playing,
        favorite_ids,
        set_favorite_ids,
        preferences: preferences_store,
        restored_effects: StoredValue::new(HashSet::new()),
    };
    provide_context(effects_ctx);

    // Effect thumbnail store — captures screenshots opportunistically while
    // effects are playing and exposes them to the browse grid for card art.
    let thumbnail_store = ThumbnailStore::new();
    provide_context(thumbnail_store);
    thumbnails::install_auto_capture(
        thumbnail_store,
        active_effect_id,
        ws.canvas_frame,
        move |effect_id| {
            effects_index.with_untracked(|effects| {
                effects
                    .iter()
                    .find(|entry| entry.effect.id == effect_id)
                    .map(|entry| entry.effect.version.clone())
            })
        },
    );

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
            "device_connected" | "device_discovered" => {
                event.device_id.as_ref().is_some_and(|device_id| {
                    !current_devices.iter().any(|device| &device.id == device_id)
                })
            }
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

    // Keep the detailed active effect state aligned with daemon WS lifecycle
    // events, including externally triggered effect switches/stops.
    Effect::new(move |previous_effect_name: Option<Option<String>>| {
        let current_effect_name = ws_ctx.active_effect.get();
        if previous_effect_name.as_ref() == Some(&current_effect_name) {
            return current_effect_name;
        }

        if current_effect_name.is_some() {
            effects_ctx.refresh_active_effect();
        } else {
            effects_ctx.set_is_playing.set(false);
        }

        current_effect_name
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
                    <Route path=path!("/displays") view=DisplaysPage />
                    <Route path=path!("/settings") view=SettingsPage />
                </Routes>
            </Shell>
        </Router>

        <leptoaster::Toaster />
    }
}
