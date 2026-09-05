use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos::tachys::view::iterators::StaticVec;
use leptos_meta::*;
use leptos_router::any_nested_route::AnyNestedRoute;
use leptos_router::components::{Outlet, RouteChildren, Router, Routes, RoutesProps};
use leptos_router::path;

use hypercolor_leptos_ext::events::Input;
use hypercolor_leptos_ext::prelude::now_ms;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;
use hypercolor_types::event::LayerHealth;
use hypercolor_types::scene::{SceneKind, SceneMutationMode};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::SpatialLayout;

use crate::api;
use crate::apply_target::ApplyTarget;
use crate::color::CanvasFrameAnalysis;
use crate::components::modal::Modal;
use crate::components::shell::Shell;
use crate::components::welcome_overlay::WelcomeOverlay;
use crate::config_state::{ConfigContext, ConfigSchemaContext};
use crate::device_event_logic::should_refetch_devices_for_event;
use crate::effect_search::IndexedEffect;
use crate::extensions::{UiExtensions, parent_route, ui_route};
use crate::nav::NavExtensionItems;
use crate::pages::dashboard::DashboardPage;
use crate::pages::devices::DevicesPage;
use crate::pages::display_preview::DisplayPreviewPage;
use crate::pages::effects::EffectsPage;
use crate::pages::media::MediaPage;
use crate::pages::settings::SettingsPage;
use crate::pages::studio::StudioPage;
use crate::preferences::PreferencesStore;
use crate::preview_telemetry::{PreviewPresenterTelemetry, PreviewTelemetryContext};
use crate::thumbnails::{self, ThumbnailStore};
use crate::toasts;
use crate::ws::messages::scene_event_affects_active_effect;
use crate::ws::{
    AudioLevel, BackpressureNotice, CanvasFrame, ControlSurfaceEventHint, DeviceEventHint,
    EffectErrorHint, ExtensionEventHint, InputInjectEdge, InputSourceStatusEventHint,
    InteractivePreviewLifecycle, InteractivePreviewRequest, PerformanceMetrics, SceneEventHint,
    ScreenZonesFrame, ServiceIdentityEventHint, WsManager,
};

mod effect_state;

use effect_state::{
    apply_active_effect_snapshot, apply_active_scene_snapshot, apply_effect_to_current_led_zones,
    capture_active_effect_state, clear_active_scene_state, effect_error_toast_message,
    restore_active_effect_state,
};

/// Global WebSocket state provided via Leptos context.
#[derive(Clone, Copy)]
pub struct WsContext {
    pub canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub screen_canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub web_viewport_canvas_frame: ReadSignal<Option<CanvasFrame>>,
    /// Latest per-display JPEG frame from the `display_preview` WS
    /// channel. Cleared when the selected display changes (handled by
    /// `set_display_preview_device`).
    pub display_preview_frames: ReadSignal<HashMap<String, CanvasFrame>>,
    pub interactive_preview_frames: ReadSignal<HashMap<String, CanvasFrame>>,
    pub interactive_preview_lifecycles: ReadSignal<HashMap<String, InteractivePreviewLifecycle>>,
    pub interactive_preview_available: ReadSignal<bool>,
    /// Set to `Some(device_id)` to subscribe the live preview stream to
    /// that display, or `None` to unsubscribe. Setting the same id twice
    /// is safe — subsequent subscribes retarget without dropping the
    /// existing relay task on the server.
    pub set_display_preview_device: WriteSignal<Option<String>>,
    pub preview_fps: ReadSignal<f32>,
    pub preview_target_fps: ReadSignal<u32>,
    pub set_preview_cap: WriteSignal<u32>,
    pub set_preview_width_cap: WriteSignal<u32>,
    pub set_preview_consumers: WriteSignal<u32>,
    pub set_screen_preview_consumers: WriteSignal<u32>,
    /// Latest ambilight zone grid from the `screen_zones` WS channel.
    pub screen_zones_frame: ReadSignal<Option<ScreenZonesFrame>>,
    /// Opt-in subscription counter for the `screen_zones` WS topic.
    pub set_screen_zones_consumers: WriteSignal<u32>,
    pub set_web_viewport_preview_consumers: WriteSignal<u32>,
    pub metrics: ReadSignal<Option<PerformanceMetrics>>,
    pub sensors: ReadSignal<Option<SystemSnapshot>>,
    /// Latest per-device output telemetry snapshot. Populated only while a
    /// view has bumped `set_device_metrics_consumers`.
    pub device_metrics: ReadSignal<Option<api::DeviceMetricsSnapshot>>,
    /// Opt-in subscription counter for the `device_metrics` WS topic.
    pub set_device_metrics_consumers: WriteSignal<u32>,
    pub backpressure_notice: ReadSignal<Option<BackpressureNotice>>,
    pub active_effect: ReadSignal<Option<String>>,
    pub output_paused: ReadSignal<bool>,
    pub last_device_event: ReadSignal<Option<DeviceEventHint>>,
    pub last_scene_event: ReadSignal<Option<SceneEventHint>>,
    pub last_effect_error: ReadSignal<Option<EffectErrorHint>>,
    pub last_control_surface_event: ReadSignal<Option<ControlSurfaceEventHint>>,
    /// Latest daemon-extension state-change event. UI extensions filter on
    /// `source`/`kind` and refetch their REST state — never poll.
    pub last_extension_event: ReadSignal<Option<ExtensionEventHint>>,
    /// Latest safe source-health transition, used only to invalidate the
    /// canonical REST status snapshot.
    pub last_input_source_status_event: ReadSignal<Option<InputSourceStatusEventHint>>,
    /// Latest daemon-owner transition, used to invalidate canonical status.
    pub last_service_identity_event: ReadSignal<Option<ServiceIdentityEventHint>>,
    /// Increments each time the daemon socket (re)opens. Fold into fetcher
    /// epochs to refetch REST mirrors after a reconnect gap, since bus
    /// events are not replayed.
    pub connection_generation: ReadSignal<u64>,
    /// Per-layer runtime health, keyed by `scene/zone/layer`, fed by the
    /// daemon's `layer_health_changed` events. A layer with no entry is
    /// treated as healthy — including, until the daemon replays a snapshot
    /// on connect, layers that failed before this session connected.
    pub layer_health: ReadSignal<HashMap<String, LayerHealth>>,
    pub audio_level: ReadSignal<AudioLevel>,
    pub send_zone_layout_preview: Callback<(String, SpatialLayout)>,
    pub clear_zone_layout_preview: Callback<String>,
    pub open_interactive_preview: Callback<InteractivePreviewRequest>,
    pub close_interactive_preview: Callback<String>,
    /// Send addressed browser-preview input edges as one control-authorized
    /// `input_inject` message. No-op while disconnected.
    pub send_input_inject: Callback<(String, Vec<InputInjectEdge>)>,
}

#[derive(Clone, Copy)]
pub struct FrameAnalysisContext {
    pub live_canvas: ReadSignal<Option<CanvasFrameAnalysis>>,
}

/// Named daemon capabilities (Spec 65 §9.6). Multi-zone Studio affordances
/// gate on whether their backing capability is advertised by the daemon's
/// authenticated `/api/v1/system` status — there is no probe fallback, so an
/// absent advertisement means the affordance stays hidden.
#[derive(Clone, Copy)]
pub struct CapabilitiesContext {
    pub capabilities: Signal<HashSet<String>>,
}

impl CapabilitiesContext {
    /// Whether the daemon advertises a named capability.
    #[must_use]
    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.with(|set| set.contains(capability))
    }

    /// Whether every zone-lifecycle capability is live. `+ New zone` and
    /// the zone rows need all three: a user who can create a zone but
    /// cannot render it or move outputs into it has an unusable zone.
    #[must_use]
    pub fn zone_crud_ready(&self) -> bool {
        self.has("zone-crud")
            && self.has("multi-zone-sampling")
            && self.has("zone-device-assignment")
    }
}

/// Shared active-effect state — accessible from sidebar, effects page, etc.
#[derive(Clone, Copy)]
pub struct EffectsContext {
    pub effects_index: Memo<Vec<IndexedEffect>>,
    pub refresh_effects: Callback<()>,
    pub active_effect_id: ReadSignal<Option<String>>,
    pub set_active_effect_id: WriteSignal<Option<String>>,
    pub active_effect_target: ReadSignal<Option<api::EffectLayerTarget>>,
    pub set_active_effect_target: WriteSignal<Option<api::EffectLayerTarget>>,
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
    pub active_scene_name: ReadSignal<Option<String>>,
    pub set_active_scene_name: WriteSignal<Option<String>>,
    pub active_scene_kind: ReadSignal<Option<SceneKind>>,
    pub set_active_scene_kind: WriteSignal<Option<SceneKind>>,
    pub active_scene_mutation_mode: ReadSignal<Option<SceneMutationMode>>,
    pub set_active_scene_mutation_mode: WriteSignal<Option<SceneMutationMode>>,
    pub last_effect_error: ReadSignal<Option<EffectErrorHint>>,
    pub set_last_effect_error: WriteSignal<Option<EffectErrorHint>>,
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
    pub apply_generation: StoredValue<u64>,
    /// The zone a quick-apply targets. Studio writes it from the selected
    /// zone; every quick-apply surface reads it so applies land in the
    /// zone the user is composing (Wave B3).
    pub apply_target: RwSignal<ApplyTarget>,
    /// Refresh hook for the shared active-scene resource (crate::zones).
    /// Apply paths run it after zone-targeted writes; the snapshot
    /// application happens in the app-root effect that watches the
    /// shared scene memo, so there is exactly one fetch path.
    pub scene_refresh: Callback<()>,
    /// Per-zone active-effect state for every LED zone of the active
    /// scene, in scene order. The multi-zone now-playing source of truth;
    /// the singular `active_*` signals above mirror the primary zone only.
    pub zone_effects: Memo<Vec<crate::zones::ZoneEffectState>>,
    /// The focused zone's effect state (primary when nothing is focused).
    pub focused_zone_effect: Memo<Option<crate::zones::ZoneEffectState>>,
}

/// Shared device + layout state — accessible from devices page and layout builder.
#[derive(Clone, Copy)]
pub struct DevicesContext {
    pub devices_resource: LocalResource<api::ApiResult<Vec<api::DeviceSummary>>>,
    pub layouts_resource: LocalResource<api::ApiResult<Vec<api::LayoutSummary>>>,
}

#[derive(Clone, Copy)]
pub struct DisplaysContext {
    pub displays_resource: LocalResource<api::ApiResult<Vec<api::DisplaySummary>>>,
}

async fn apply_owned_target<Apply, ApplyFuture, CurrentGeneration>(
    request_generation: u64,
    apply: Apply,
    current_generation: CurrentGeneration,
) -> api::ApiResult<Option<api::EffectLayerTarget>>
where
    Apply: FnOnce() -> ApplyFuture,
    ApplyFuture: std::future::Future<Output = api::ApiResult<api::EffectLayerTarget>>,
    CurrentGeneration: FnOnce() -> u64,
{
    let target = apply().await?;
    Ok((current_generation() == request_generation).then_some(target))
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
            match api::fetch_primary_effect_view().await {
                Ok(Some(active)) => {
                    apply_active_effect_snapshot(&ctx, active);
                }
                Ok(None) => ctx.set_is_playing.set(false),
                Err(_) => {}
            }
        });
    }

    pub fn refresh_active_scene(&self) {
        self.scene_refresh.run(());
    }

    pub fn adopt_replacement_target(
        &self,
        observed: &api::EffectLayerTarget,
        replacement: api::EffectLayerTarget,
    ) {
        if self.active_effect_target.get_untracked().as_ref() == Some(observed) {
            self.set_active_effect_target.set(Some(replacement));
        }
    }

    /// Apply an effect by ID — sets local state + calls API.
    ///
    /// When remembered preferences exist for this effect, they're baked
    /// into the initial `/apply` request so the daemon starts rendering
    /// with the user's saved preset + controls immediately. This avoids
    /// the defaults-flash that used to occur while `restore_effect_preferences`
    /// ran its follow-up round-trip.
    pub fn apply_effect(&self, id: String) {
        let apply_target = self.apply_target.get_untracked();
        let stored_prefs = self.preferences.get(&id);
        if apply_target == ApplyTarget::AllZones {
            let ctx = *self;
            leptos::task::spawn_local(async move {
                apply_effect_to_current_led_zones(&ctx, id).await;
            });
            return;
        }

        // A body is sent when there are preferences to bake in, or when a
        // named zone has to be targeted.
        let target_zone_id = apply_target.zone_id().map(ToOwned::to_owned);
        let body =
            (stored_prefs.is_some() || target_zone_id.is_some()).then(|| api::ApplyEffectRequest {
                preset_id: stored_prefs
                    .as_ref()
                    .and_then(|prefs| prefs.preset_id.as_deref())
                    .and_then(|preset_id| preset_id.parse().ok()),
                controls: stored_prefs.as_ref().and_then(|prefs| {
                    (!prefs.control_values.is_empty())
                        .then(|| prefs.control_values.clone().into_iter().collect())
                }),
                zone: target_zone_id.as_deref().and_then(|zone_id| {
                    uuid::Uuid::parse_str(zone_id)
                        .ok()
                        .map(hypercolor_types::scene::ZoneId)
                }),
                ..api::ApplyEffectRequest::default()
            });

        // A named-zone apply renders into that zone and leaves the default
        // effect untouched, so it skips the default-state optimism below.
        if target_zone_id.is_some() {
            let ctx = *self;
            leptos::task::spawn_local(async move {
                if api::apply_effect(&id, body.as_ref()).await.is_ok() {
                    ctx.refresh_active_scene();
                } else {
                    toasts::toast_error("Couldn't apply the effect to the selected zone");
                }
            });
            return;
        }

        self.restored_effects.update_value(|set| {
            if stored_prefs.is_some() {
                set.insert(id.clone());
            } else {
                set.remove(&id);
            }
        });
        let generation = self.apply_generation.get_value().saturating_add(1);
        self.apply_generation.set_value(generation);
        self.set_last_effect_error.set(None);

        let previous = capture_active_effect_state(self);
        let selected_effect = self.effect_summary(&id);
        self.set_active_effect_target.set(None);
        self.set_active_effect_id.set(Some(id.clone()));
        self.set_active_effect_name
            .set(selected_effect.as_ref().map(|effect| effect.name.clone()));
        self.set_active_effect_category.set(
            selected_effect
                .as_ref()
                .map(|effect| effect.category.as_str().to_owned())
                .unwrap_or_default(),
        );
        // Optimistically mirror the stored controls locally so the sidebar
        // doesn't flash empty while the daemon's confirmation round-trips.
        // The snapshot callback will overwrite with the authoritative state.
        if let Some(prefs) = stored_prefs.as_ref() {
            self.set_active_control_values
                .set(prefs.control_values.clone());
            self.set_active_preset_id.set(prefs.preset_id.clone());
        } else {
            self.set_active_control_values.set(HashMap::new());
            self.set_active_preset_id.set(None);
        }
        self.set_active_controls.set(Vec::new());

        let ctx = *self;
        leptos::task::spawn_local(async move {
            match apply_owned_target(
                generation,
                || async { api::apply_effect(&id, body.as_ref()).await },
                || ctx.apply_generation.get_value(),
            )
            .await
            {
                Ok(Some(target)) => {
                    ctx.set_active_effect_target.set(Some(target));
                    ctx.refresh_active_effect();
                }
                Ok(None) => ctx.refresh_active_effect(),
                Err(_) if ctx.apply_generation.get_value() == generation => {
                    restore_active_effect_state(&ctx, previous);
                    toasts::toast_error("Couldn't apply the effect");
                }
                Err(_) => ctx.refresh_active_effect(),
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
                    toasts::toast_error("Couldn't remove the favorite");
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
                    toasts::toast_error("Couldn't save the favorite");
                }
            });
        }
    }

    /// Stop the active effect (keeps metadata visible for the sidebar).
    pub fn stop_effect(&self) {
        self.set_is_playing.set(false);
        self.set_last_effect_error.set(None);
        let ctx = *self;
        leptos::task::spawn_local(async move {
            if api::pause_output().await.is_err() {
                ctx.refresh_active_effect();
                toasts::toast_error("Couldn't pause the effect");
            }
        });
    }

    /// Resume the previously paused effect.
    pub fn resume_effect(&self) {
        self.set_is_playing.set(true);
        self.set_last_effect_error.set(None);
        let ctx = *self;
        leptos::task::spawn_local(async move {
            if api::resume_output().await.is_ok() {
                ctx.refresh_active_effect();
            } else {
                ctx.set_is_playing.set(false);
                ctx.refresh_active_effect();
            }
        });
    }
}

/// Build every app-wide context, then render the router with `ext` composed in.
///
/// Replaces the old `App` component. Route defs are taken **by value** (erased
/// `AnyNestedRoute`s are `Send` but not `Sync`, so they cannot travel through
/// context); the extension nav items are surfaced via context for the sidebar +
/// shortcut model. With an empty `UiExtensions` this renders the standalone OSS
/// app unchanged.
pub fn app_view(ext: UiExtensions) -> impl IntoView {
    let UiExtensions {
        mount,
        routes: extension_routes,
        nav_items: extension_nav,
        settings_sections: extension_settings,
        sidebar_widgets: extension_widgets,
        on_setup,
    } = ext;
    provide_meta_context();
    provide_context(mount.clone());
    leptoaster::provide_toaster();
    crate::toasts::install_root_context();
    provide_context(NavExtensionItems(extension_nav));
    provide_context(crate::extensions::SettingsExtensionSections(
        std::sync::Arc::new(extension_settings),
    ));
    provide_context(crate::extensions::SidebarExtensionWidgets(
        std::sync::Arc::new(extension_widgets),
    ));
    provide_context(crate::extensions::UiChromeFlags::default());

    // Global WebSocket connection
    let ws = WsManager::new();
    let (config, set_config) = signal(None::<hypercolor_types::config::HypercolorConfig>);
    let (api_key_required, set_api_key_required) = signal(false);
    let (preview_presenter, set_preview_presenter) = signal(PreviewPresenterTelemetry::default());
    let (live_canvas_analysis, set_live_canvas_analysis) = signal(None::<CanvasFrameAnalysis>);
    let (last_canvas_analysis_at, set_last_canvas_analysis_at) = signal(0.0_f64);
    let refresh_config = Callback::new(move |()| {
        leptos::task::spawn_local(async move {
            match api::fetch_config_typed().await {
                Ok(fresh) => {
                    set_api_key_required.set(false);
                    set_config.set(Some(fresh));
                }
                Err(api::client::ApiError::Http { status, .. })
                    if status == 401 || status == 403 =>
                {
                    set_api_key_required.set(true);
                }
                Err(error) => {
                    leptos::logging::warn!(
                        "Config fetch failed (retries on the next socket open): {error}"
                    );
                }
            }
        });
    });
    let unlock_api = Callback::new(move |api_key: String| {
        api::client::save_api_key(&api_key);
        set_api_key_required.set(false);
        reload_page();
    });
    let audio_enabled =
        Memo::new(move |_| config.get().is_some_and(|current| current.audio.enabled));
    provide_context(ConfigContext {
        config,
        set_config,
        refresh: refresh_config,
        audio_enabled,
    });
    // A one-shot fetch at wasm init loses the race when the daemon is
    // still binding (app boot) or mid-restart, and nothing would retry.
    // Keying the fetch to the socket generation refires it on every
    // WebSocket open, so config heals on the same reconnect that
    // refreshes the hint-driven resources.
    let config_connection_generation = ws.connection_generation;
    Effect::new(move |_| {
        let _generation = config_connection_generation.get();
        refresh_config.run(());
    });

    let ws_ctx = WsContext {
        canvas_frame: ws.canvas_frame,
        screen_canvas_frame: ws.screen_canvas_frame,
        web_viewport_canvas_frame: ws.web_viewport_canvas_frame,
        display_preview_frames: ws.display_preview_frames,
        interactive_preview_frames: ws.interactive_preview_frames,
        interactive_preview_lifecycles: ws.interactive_preview_lifecycles,
        interactive_preview_available: ws.interactive_preview_available,
        set_display_preview_device: ws.set_display_preview_device,
        preview_fps: ws.preview_fps,
        preview_target_fps: ws.preview_target_fps,
        set_preview_cap: ws.set_preview_cap,
        set_preview_width_cap: ws.set_preview_width_cap,
        set_preview_consumers: ws.set_preview_consumers,
        set_screen_preview_consumers: ws.set_screen_preview_consumers,
        screen_zones_frame: ws.screen_zones_frame,
        set_screen_zones_consumers: ws.set_screen_zones_consumers,
        set_web_viewport_preview_consumers: ws.set_web_viewport_preview_consumers,
        metrics: ws.metrics,
        sensors: ws.sensors,
        device_metrics: ws.device_metrics,
        set_device_metrics_consumers: ws.set_device_metrics_consumers,
        backpressure_notice: ws.backpressure_notice,
        active_effect: ws.active_effect,
        output_paused: ws.output_paused,
        last_device_event: ws.last_device_event,
        last_scene_event: ws.last_scene_event,
        last_effect_error: ws.last_effect_error,
        last_control_surface_event: ws.last_control_surface_event,
        last_extension_event: ws.last_extension_event,
        last_input_source_status_event: ws.last_input_source_status_event,
        last_service_identity_event: ws.last_service_identity_event,
        connection_generation: ws.connection_generation,
        layer_health: ws.layer_health,
        audio_level: ws.audio_level,
        send_zone_layout_preview: ws.send_zone_layout_preview,
        clear_zone_layout_preview: ws.clear_zone_layout_preview,
        open_interactive_preview: ws.open_interactive_preview,
        close_interactive_preview: ws.close_interactive_preview,
        send_input_inject: ws.send_input_inject,
    };
    provide_context(ws_ctx);
    provide_context(crate::device_metrics::install_device_metrics_store(ws_ctx));

    // Shared zone + scene state — one active-scene resource for the whole
    // app, kept fresh by WS scene events (see crate::zones).
    let (zones_ctx, _scenes_ctx) = crate::zones::provide_scene_contexts(ws.last_scene_event);
    provide_context(PreviewTelemetryContext {
        presenter: preview_presenter,
        set_presenter: set_preview_presenter,
    });
    provide_context(FrameAnalysisContext {
        live_canvas: live_canvas_analysis,
    });

    // Daemon capability advertisement (§9.6). Fetched once — the set is
    // fixed per daemon build — and exposed as a context so multi-zone
    // Studio affordances can gate on it without each re-querying status.
    let status_resource = api::daemon_resource(api::fetch_status);
    let capabilities = Memo::new(move |_| {
        status_resource
            .get()
            .and_then(Result::ok)
            .map(|status| status.capabilities.into_iter().collect::<HashSet<_>>())
            .unwrap_or_default()
    });
    provide_context(CapabilitiesContext {
        capabilities: capabilities.into(),
    });

    // The daemon's config key registry. Fetched once per connection —
    // the table is fixed per daemon build — so every settings control
    // can ask how its key applies instead of mirroring the rules.
    let config_schema_resource = api::daemon_resource(api::fetch_config_schema);
    let config_schema_entries = Memo::new(move |_| {
        config_schema_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
    });
    provide_context(ConfigSchemaContext {
        entries: config_schema_entries.into(),
    });

    Effect::new(move |_| {
        let Some(frame) = ws.canvas_frame.get() else {
            return;
        };

        let now = now_ms();
        if now - last_canvas_analysis_at.get_untracked() < 500.0 {
            return;
        }
        set_last_canvas_analysis_at.set(now);

        if let Some(analysis) = crate::color::analyze_canvas_frame(&frame) {
            set_live_canvas_analysis.set(Some(analysis));
        }
    });

    // Global effects state — shared between sidebar player + effects page
    let effects_resource = api::daemon_resource(api::fetch_effects);
    let effects_index: Memo<Vec<IndexedEffect>> = Memo::new(move |_| {
        effects_resource
            .get()
            .and_then(Result::ok)
            .map(|effects| effects.into_iter().map(IndexedEffect::new).collect())
            .unwrap_or_default()
    });
    // Per-zone effect state — what each LED zone is playing, derived
    // from the shared scene (zip preserves surfaces_from_zones' 1:1
    // scene ordering) plus the effects index for display names.
    let zone_effects = Memo::new(move |_| {
        let Some(scene) = zones_ctx.active_scene.get() else {
            return Vec::new();
        };
        let surfaces = crate::zones::surface::surfaces_from_zones(&scene.zones);
        effects_index.with(|effects| {
            scene
                .zones
                .iter()
                .zip(surfaces)
                .filter(|(_, surface)| surface.kind == crate::zones::surface::SurfaceKind::Light)
                .map(|(zone, surface)| {
                    let effect = api::zone_effect(zone);
                    let effect_id = effect.map(|effect| effect.effect_id.to_string());
                    let indexed = effect_id
                        .as_ref()
                        .and_then(|id| effects.iter().find(|entry| entry.effect.id == *id));
                    crate::zones::ZoneEffectState {
                        effect_name: indexed.map(|entry| entry.effect.name.clone()),
                        effect_category: indexed
                            .map(|entry| entry.effect.category.as_str().to_owned()),
                        control_values: effect
                            .map(|effect| effect.controls.clone())
                            .unwrap_or_default(),
                        preset_id: effect
                            .and_then(|effect| effect.preset_id)
                            .map(|preset_id| preset_id.to_string()),
                        revision: scene.revision,
                        effect_id,
                        zone: surface,
                    }
                })
                .collect::<Vec<_>>()
        })
    });
    let focused_zone_effect = Memo::new(move |_| {
        let focused = zones_ctx.focused_zone.get();
        zone_effects.with(|zones| match focused.as_deref() {
            Some(id) => zones.iter().find(|state| state.zone.id == id).cloned(),
            None => zones
                .iter()
                .find(|state| state.zone.role == hypercolor_types::scene::ZoneRole::Primary)
                .or_else(|| zones.first())
                .cloned(),
        })
    });

    let active_resource = api::daemon_resource(api::fetch_primary_effect_view);
    let favorites_resource = api::daemon_resource(api::fetch_favorites);
    let (active_effect_id, set_active_effect_id) = signal(None::<String>);
    let (active_effect_target, set_active_effect_target) = signal(None::<api::EffectLayerTarget>);
    let (active_effect_name, set_active_effect_name) = signal(None::<String>);
    let (active_effect_category, set_active_effect_category) = signal(String::new());
    let (active_controls, set_active_controls) = signal(Vec::<ControlDefinition>::new());
    let (active_control_values, set_active_control_values) =
        signal(HashMap::<String, ControlValue>::new());
    let (active_preset_id, set_active_preset_id) = signal(None::<String>);
    let (active_scene_name, set_active_scene_name) = signal(None::<String>);
    let (active_scene_kind, set_active_scene_kind) = signal(None::<SceneKind>);
    let (active_scene_mutation_mode, set_active_scene_mutation_mode) =
        signal(None::<SceneMutationMode>);
    let (last_effect_error, set_last_effect_error) = signal(None::<EffectErrorHint>);
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
        refresh_effects: Callback::new(move |()| effects_resource.refetch()),
        active_effect_id,
        set_active_effect_id,
        active_effect_target,
        set_active_effect_target,
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
        active_scene_name,
        set_active_scene_name,
        active_scene_kind,
        set_active_scene_kind,
        active_scene_mutation_mode,
        set_active_scene_mutation_mode,
        last_effect_error,
        set_last_effect_error,
        is_playing,
        set_is_playing,
        favorite_ids,
        set_favorite_ids,
        preferences: preferences_store,
        restored_effects: StoredValue::new(HashSet::new()),
        apply_generation: StoredValue::new(0),
        apply_target: RwSignal::new(ApplyTarget::Primary),
        scene_refresh: zones_ctx.refresh,
        zone_effects,
        focused_zone_effect,
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
                    .map(|entry| (entry.effect.name.clone(), entry.effect.version.clone()))
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
    let devices_resource = api::daemon_resource(api::fetch_devices);
    let layouts_resource = api::daemon_resource(api::fetch_layouts);
    let displays_resource = api::daemon_resource(api::fetch_displays);
    provide_context(DevicesContext {
        devices_resource,
        layouts_resource,
    });
    provide_context(DisplaysContext { displays_resource });

    // Refresh devices reactively from daemon lifecycle events instead of
    // rebuilding the grid on a fixed timer.
    Effect::new(move |_| {
        let Some(event) = ws_ctx.last_device_event.get() else {
            return;
        };

        let current_device_ids = devices_resource
            .get_untracked()
            .and_then(|result| result.ok())
            .map(|devices| {
                devices
                    .into_iter()
                    .map(|device| device.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let should_refetch = should_refetch_devices_for_event(
            &event.event_type,
            event.device_id.as_deref(),
            event.found_count,
            &current_device_ids,
        );

        if should_refetch {
            devices_resource.refetch();
            displays_resource.refetch();
        }
    });

    // Initialize active effect from API on load
    Effect::new(move |_| {
        if let Some(Ok(Some(active))) = active_resource.get() {
            apply_active_effect_snapshot(&effects_ctx, active);
        } else if let Some(Ok(None)) = active_resource.get() {
            effects_ctx.set_is_playing.set(false);
        }
    });

    Effect::new(move |_| {
        let has_active_effect = effects_ctx.active_effect_id.get().is_some();
        effects_ctx
            .set_is_playing
            .set(has_active_effect && !ws_ctx.output_paused.get());
    });

    Effect::new(move |_| {
        if let Some(active_scene) = zones_ctx.active_scene.get() {
            apply_active_scene_snapshot(&effects_ctx, active_scene);
        } else {
            clear_active_scene_state(&effects_ctx);
        }
    });

    // Keep the detailed active effect state aligned with daemon WS lifecycle
    // events, including externally triggered effect switches/stops.
    Effect::new(move |previous_effect_name: Option<Option<String>>| {
        let current_effect_name = ws_ctx.active_effect.get();
        if previous_effect_name.as_ref() == Some(&current_effect_name) {
            return current_effect_name;
        }

        effects_ctx.refresh_active_effect();

        current_effect_name
    });

    Effect::new(
        move |previous_effect_error: Option<Option<EffectErrorHint>>| {
            let current_effect_error = ws_ctx.last_effect_error.get();
            if previous_effect_error.as_ref() == Some(&current_effect_error) {
                return current_effect_error;
            }

            effects_ctx
                .set_last_effect_error
                .set(current_effect_error.clone());
            if let Some(effect_error) = current_effect_error.as_ref() {
                toasts::toast_error(&effect_error_toast_message(&effects_ctx, effect_error));
            }

            current_effect_error
        },
    );

    Effect::new(move |_| {
        let active_effect_id = effects_ctx.active_effect_id.get();
        let degraded = effects_ctx.last_effect_error.get();
        if active_effect_id.is_some()
            && degraded.as_ref().is_some_and(|effect_error| {
                effects_ctx
                    .effect_summary(&effect_error.effect_id)
                    .is_some_and(|effect| {
                        effect.category != hypercolor_types::effect::EffectCategory::Display
                            && Some(effect_error.effect_id.clone()) != active_effect_id
                    })
            })
        {
            effects_ctx.set_last_effect_error.set(None);
        }
    });

    Effect::new(
        move |previous_scene_event: Option<Option<SceneEventHint>>| {
            let current_scene_event = ws_ctx.last_scene_event.get();
            if previous_scene_event.as_ref() == Some(&current_scene_event) {
                return current_scene_event;
            }

            // Fast-path the scene label so the banner flips instantly;
            // the shared scene resource (crate::zones) is already
            // refetching off this same event for the structural state.
            if let Some(scene_event) = current_scene_event.as_ref()
                && scene_event.event_type == "active_scene_changed"
                && let (Some(scene_name), Some(scene_kind), Some(scene_mutation_mode)) = (
                    scene_event.scene_name.clone(),
                    scene_event.scene_kind,
                    scene_event.scene_mutation_mode,
                )
            {
                effects_ctx.set_active_scene_name.set(Some(scene_name));
                effects_ctx.set_active_scene_kind.set(Some(scene_kind));
                effects_ctx
                    .set_active_scene_mutation_mode
                    .set(Some(scene_mutation_mode));
            }
            if current_scene_event
                .as_ref()
                .is_none_or(scene_event_affects_active_effect)
            {
                effects_ctx.refresh_active_effect();
            }

            current_scene_event
        },
    );

    // Embedder startup hooks run last: every app-wide context above is
    // available to them, and nothing has rendered yet.
    for hook in on_setup {
        hook();
    }

    view! {
        <Meta charset="UTF-8" />
        <Meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <Title text="Hypercolor" />

        <Router base=mount.route_base().to_owned()>
            {app_routes(extension_routes)}
        </Router>

        <Show when=move || api_key_required.get()>
            <ApiKeyPrompt on_unlock=unlock_api />
        </Show>

        <WelcomeOverlay />

        <leptoaster::Toaster />
    }
}

#[component]
fn ApiKeyPrompt(on_unlock: Callback<String>) -> impl IntoView {
    let (api_key, set_api_key) = signal(String::new());
    let submit = Callback::new(move |()| {
        let key = api_key.get().trim().to_owned();
        if !key.is_empty() {
            on_unlock.run(key);
        }
    });
    let submit_key = submit;
    let submit_click = submit;

    let input_ref = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if let Some(input) = input_ref.get() {
            let _ = input.focus();
        }
    });

    view! {
        // Not dismissible — there is nothing behind this prompt until a key
        // is provided, so Escape/backdrop have nowhere to go.
        <Modal
            on_close=Callback::new(|()| {})
            label="Network API key"
            dismissible=false
            container_class="fixed inset-0 z-[100] flex items-center justify-center px-4"
            backdrop_class="absolute inset-0 bg-black/70 backdrop-blur-sm"
        >
            <div class="relative w-full max-w-sm rounded-lg border border-edge-subtle bg-surface-overlay p-5 modal-glow">
                <div class="text-sm font-semibold text-fg-primary">"Network API Key"</div>
                <div class="mt-1 text-xs text-fg-tertiary/75">
                    "This daemon requires a key for network access."
                </div>
                <input
                    node_ref=input_ref
                    type="password"
                    class="mt-4 w-full rounded-lg border border-edge-subtle bg-surface-overlay/60 px-3 py-2 text-sm text-fg-primary placeholder-fg-tertiary focus:border-accent-muted focus:outline-none"
                    placeholder="hc_..."
                    prop:value=move || api_key.get()
                    on:input=move |event| {
                        let input = Input::from_event(event);
                        if let Some(value) = input.value_string() {
                            set_api_key.set(value);
                        }
                    }
                    on:keydown=move |event| {
                        if event.key() == "Enter" {
                            submit_key.run(());
                        }
                    }
                />
                <button
                    type="button"
                    class="mt-4 w-full rounded-lg bg-accent px-3 py-2 text-sm font-medium text-white transition hover:bg-accent-hover"
                    on:click=move |_| submit_click.run(())
                >
                    "Connect"
                </button>
            </div>
        </Modal>
    }
}

fn reload_page() {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

/// The app shell that hosts every child route through an `<Outlet/>`.
fn shell_outlet() -> impl IntoView {
    view! {
        <Shell>
            <Outlet />
        </Shell>
    }
}

/// Compose the core routes with any extension-contributed routes into one
/// `StaticVec` of erased route defs, built **once** — leptos_router 0.8 fixes
/// its route set when `<Routes>` is constructed and cannot add routes later.
/// `/preview` stays a top-level route with no shell; every other route renders
/// inside the shell (an empty parent segment with the shell `<Outlet/>`).
fn route_defs(extra: Vec<AnyNestedRoute>) -> StaticVec<AnyNestedRoute> {
    let mut shell = vec![
        ui_route(path!("/"), DashboardPage),
        ui_route(path!("/effects"), EffectsPage),
        ui_route(path!("/effects/:id"), EffectsPage),
        ui_route(path!("/studio"), StudioPage),
        ui_route(path!("/media"), MediaPage),
        ui_route(path!("/devices"), DevicesPage),
        ui_route(path!("/settings"), SettingsPage),
    ];
    shell.extend(extra);
    StaticVec::from(vec![
        ui_route(path!("/preview"), DisplayPreviewPage),
        parent_route(path!(""), shell_outlet, StaticVec::from(shell)),
    ])
}

/// Build the `<Routes>` tree from core + extension route defs.
///
/// `<Routes>` cannot take a runtime `Vec` through the `view!` macro (the macro
/// treats a `{block}` child as a rendered view, not route defs), so we build
/// `RoutesProps` directly and wrap the composed `StaticVec` in `RouteChildren`.
fn app_routes(extra: Vec<AnyNestedRoute>) -> impl IntoView {
    Routes(
        RoutesProps::builder()
            .fallback(NotFoundPage)
            .children(RouteChildren::to_children(move || route_defs(extra)))
            .build(),
    )
}

/// The 404 surface — kept on-brand instead of a bare paragraph so a
/// mistyped URL still feels like part of the app.
#[component]
fn NotFoundPage() -> impl IntoView {
    view! {
        <div class="flex h-full flex-col items-center justify-center gap-4 p-8 animate-enter-fade">
            <div class="text-5xl font-bold tracking-tight text-accent/60">"404"</div>
            <div class="text-sm text-fg-secondary">"This page doesn't exist."</div>
            <a
                href=crate::route_ui::route_href("/")
                class="mt-2 rounded-lg border border-edge-subtle bg-surface-raised px-4 py-2 text-sm font-medium text-fg-primary transition hover:border-accent/40 hover:bg-surface-hover btn-press"
            >
                "Back to the dashboard"
            </a>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    use super::apply_owned_target;
    use crate::api::{ApiResult, EffectLayerTarget};

    struct SuspendedApply {
        generation: Rc<Cell<u64>>,
        target: Option<EffectLayerTarget>,
        suspended: bool,
    }

    impl Future for SuspendedApply {
        type Output = ApiResult<EffectLayerTarget>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if !self.suspended {
                self.suspended = true;
                self.generation.set(2);
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            Poll::Ready(Ok(self
                .target
                .take()
                .expect("apply target is returned once")))
        }
    }

    fn target(layer_id: &str) -> EffectLayerTarget {
        EffectLayerTarget {
            effect_id: "00000000-0000-0000-0000-00000000000a".to_owned(),
            zone_id: "00000000-0000-0000-0000-000000000002".to_owned(),
            layer_id: layer_id.to_owned(),
        }
    }

    #[test]
    fn suspended_direct_same_effect_apply_cannot_overwrite_newer_layer_ownership() {
        let generation = Rc::new(Cell::new(1_u64));
        let old_layer = "00000000-0000-0000-0000-000000000003";
        let new_layer = "00000000-0000-0000-0000-000000000004";
        let apply_generation = Rc::clone(&generation);
        let current_generation = Rc::clone(&generation);
        let old = apply_owned_target(
            1,
            move || SuspendedApply {
                generation: apply_generation,
                target: Some(target(old_layer)),
                suspended: false,
            },
            move || current_generation.get(),
        );
        let mut old = std::pin::pin!(old);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert!(matches!(old.as_mut().poll(&mut context), Poll::Pending));
        let current_generation = Rc::clone(&generation);
        let new = apply_owned_target(
            2,
            || std::future::ready(Ok(target(new_layer))),
            move || current_generation.get(),
        );
        let mut new = std::pin::pin!(new);
        let Poll::Ready(Ok(Some(adopted))) = new.as_mut().poll(&mut context) else {
            panic!("the newer same-effect apply should own its returned layer");
        };
        assert_eq!(adopted.layer_id, new_layer);

        let Poll::Ready(Ok(None)) = old.as_mut().poll(&mut context) else {
            panic!("the suspended older response must lose generation ownership");
        };
    }
}
