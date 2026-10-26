//! Zone-scoped effect controls for the effects page.
//!
//! The controls edit the target zone, which the preview's zone tabs pick.
//! The primary zone keeps the page's existing plumbing untouched: the
//! `active_controls` / `active_control_values` signals and the
//! current-controls PATCH session. Any other zone edits its own controls
//! through the real effect layer read from `/scene`, so tuning zone 2
//! never silently rewrites the primary zone.
//!
//! Single-zone scenes render exactly the primary panel.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;

use crate::api;
use crate::app::EffectsContext;
use crate::components::control_panel::ControlPanel;
use crate::components::zone_tabs::ZoneTabLabel;
use crate::control_session::{
    ControlPatchConfig, ControlPatchFn, ControlPatchFuture, use_control_patch_session,
};
use crate::style_utils::category_accent_rgb;
use crate::toasts;
use crate::zones::{ZoneEffectState, ZonesContext};

/// Flush debounce for zone-scoped control edits — the effects page's
/// product-contract cadence (the layer inspector's 120 ms is Studio's).
const ZONE_CONTROLS_DEBOUNCE_MS: f64 = 75.0;

/// Control schemas cached per effect id. Zone values live in the scene,
/// but the control *definitions* come from `fetch_effect_detail`; the
/// cache keeps tab switches from refetching a schema already seen. Entries
/// carry the connection epoch so reconnecting refreshes their definitions.
pub type ZoneControlSchemaCache = StoredValue<HashMap<String, (u64, Vec<ControlDefinition>)>>;

/// The controls-card body: the panel for the target zone. The primary
/// zone renders the caller-supplied signals and change handler verbatim;
/// any other zone mounts a [`ZoneControlsPanel`] scoped to that zone's
/// real effect layer.
#[component]
pub fn ZoneScopedControls(
    /// Primary-zone control schema (today's plumbing).
    #[prop(into)]
    controls: Signal<Vec<ControlDefinition>>,
    /// Primary-zone live values (today's plumbing).
    #[prop(into)]
    control_values: Signal<HashMap<String, ControlValue>>,
    /// Page accent for the primary panel.
    #[prop(into)]
    accent_rgb: Signal<String>,
    /// Primary-zone change handler — the current-controls PATCH session.
    #[prop(into)]
    on_control_change: Callback<(String, serde_json::Value)>,
    /// Shared per-effect control-schema cache, owned by the page so it
    /// survives docking/undocking the controls column.
    schema_cache: ZoneControlSchemaCache,
) -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let fx = expect_context::<EffectsContext>();

    // The non-primary LED zone the tabs have selected; `None` renders
    // the primary panel. A focused Screen, the primary zone itself, or
    // a zone that has left the scene all fall back to primary.
    let selected_zone_id = Memo::new(move |_| {
        let focused = zones_ctx.focused_zone.get()?;
        zones_ctx.led_zones.with(|zones| {
            zones
                .iter()
                .any(|zone| zone.id == focused && zone.role != ZoneRole::Primary)
                .then_some(focused)
        })
    });
    let selected_state = Memo::new(move |_| {
        let zone_id = selected_zone_id.get()?;
        fx.zone_effects
            .with(|zones| zones.iter().find(|state| state.zone.id == zone_id).cloned())
    });
    // Only immutable write authority owns the panel lifetime. Values and
    // revisions reconcile inside the existing session instead of remounting it.
    let selected_target = Memo::new(move |_| {
        let state = selected_state.get()?;
        let effect_id = state.effect_id?;
        zones_ctx.active_scene.with(|scene| {
            let scene = scene.as_ref()?;
            let zone = scene
                .zones
                .iter()
                .find(|zone| zone.id.to_string() == state.zone.id)?;
            let layer_id = zone
                .layers
                .iter()
                .rev()
                .find_map(|layer| match &layer.source {
                    LayerSource::Effect {
                        effect_id: current, ..
                    } if current.to_string() == effect_id => Some(layer.id.to_string()),
                    _ => None,
                })?;
            Some((scene.id.to_string(), state.zone.id, layer_id, effect_id))
        })
    });

    view! {
        <div class="space-y-2.5">
            {move || {
                let Some(_) = selected_zone_id.get() else {
                    // Primary tab — today's exact panel.
                    return view! {
                        <ControlPanel
                            controls=controls
                            control_values=control_values
                            accent_rgb=accent_rgb
                            on_change=on_control_change
                        />
                    }
                        .into_any();
                };
                let Some((_scene_id, zone_id, layer_id, effect_id)) = selected_target.get() else {
                    return view! {
                        <ZoneQuietNotice message="Nothing playing in this zone" />
                    }
                        .into_any();
                };
                view! {
                    <ZoneControlsPanel
                        effect_id=effect_id
                        zone_id=zone_id
                        layer_id=layer_id
                        state=selected_state
                        schema_cache=schema_cache
                    />
                }
                    .into_any()
            }}
        </div>
    }
}

/// The zone the controls card edits, named in its header while the
/// scene has more than one LED zone.
#[component]
pub fn ControlsZoneName() -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    move || {
        if !zones_ctx.multi_zone.get() {
            return None;
        }
        let zone = zones_ctx.target_zone()?;
        let title = format!("Editing {}", zone.name);
        Some(view! {
            <span
                class="inline-flex min-w-0 max-w-[160px] items-center gap-1.5 text-[11px] font-medium text-fg-secondary"
                title=title
            >
                <ZoneTabLabel zone=zone />
            </span>
        })
    }
}

/// Controls for one non-primary zone's directly-assigned effect. The
/// schema comes from the (cached) effect detail; values seed from the
/// zone's scene-stored controls; edits run through the shared patch
/// session against the real layer returned by the scene. Only a change to
/// scene, zone, layer, or effect identity retires the panel and its session.
#[component]
fn ZoneControlsPanel(
    effect_id: String,
    zone_id: String,
    layer_id: String,
    state: Memo<Option<ZoneEffectState>>,
    schema_cache: ZoneControlSchemaCache,
) -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let ws = expect_context::<crate::app::WsContext>();
    let accent_rgb = Signal::derive(move || {
        let category = state
            .get()
            .and_then(|state| state.effect_category)
            .unwrap_or_default();
        category_accent_rgb(&category).to_string()
    });

    let schema = api::daemon_resource({
        let effect_id = effect_id.clone();
        move || {
            let effect_id = effect_id.clone();
            let generation = ws.connection_generation.get();
            async move {
                if let Some((epoch, defs)) =
                    schema_cache.with_value(|cache| cache.get(&effect_id).cloned())
                    && epoch == generation
                {
                    return Ok(defs);
                }
                let detail = api::fetch_effect_detail(&effect_id).await?;
                schema_cache.try_update_value(|cache| {
                    cache.insert(effect_id, (generation, detail.controls.clone()));
                });
                Ok::<_, api::ApiError>(detail.controls)
            }
        }
    });
    let schema_value = Signal::derive(move || {
        schema.get().and_then(Result::ok).or_else(|| {
            schema_cache.with_value(|cache| cache.get(&effect_id).map(|(_, defs)| defs.clone()))
        })
    });
    let defs = Signal::derive(move || schema_value.get().unwrap_or_default());

    // Optimistic local values, seeded from the zone's scene state.
    let (values, set_values) = signal(
        state
            .get_untracked()
            .map(|state| state.control_values)
            .unwrap_or_default(),
    );

    // The layer id came from the live document. Replacement retires it, so
    // a stale control patch cannot land on a newer effect.
    let session_target = Signal::stored(Some(format!("{zone_id}:{layer_id}")));
    let patch: ControlPatchFn = Arc::new({
        let zone_id = zone_id.clone();
        move |_target: String,
              payload: crate::optimistic_controls::ControlValueMap,
              _version: Option<u64>|
              -> ControlPatchFuture {
            let zone_id = zone_id.clone();
            let layer_id = layer_id.clone();
            Box::pin(async move {
                api::patch_layer_controls(&zone_id, &layer_id, &payload).await?;
                Ok(api::MutationOutcome::Applied(None))
            })
        }
    });
    let session = use_control_patch_session(ControlPatchConfig {
        target: session_target,
        defs,
        set_values,
        initial_version: None,
        debounce_ms: ZONE_CONTROLS_DEBOUNCE_MS,
        patch,
        on_error: Callback::new(|error: String| {
            toasts::toast_error(&format!("Zone controls failed: {error}"));
        }),
        recover: Callback::new(move |()| {
            if let Some(state) = state.get_untracked() {
                set_values.set(state.control_values);
            }
            zones_ctx.refresh.run(());
        }),
        on_committed: None,
        flush_guard: None,
    });

    Effect::new(move |_| {
        if let Some(state) = state.get() {
            session.reconcile_values.run(state.control_values);
        }
    });
    let on_change = session.on_change;

    view! {
        <Show
            when=move || schema_value.get().is_some()
            fallback=move || view! { <ZoneQuietNotice message="Loading controls…" /> }
        >
            <ControlPanel
                controls=defs
                control_values=values
                accent_rgb=accent_rgb
                on_change=on_change
            />
        </Show>
    }
    .into_any()
}

/// Quiet inline notice for a zone tab with nothing to edit — matches
/// the layer inspector's loading box.
#[component]
fn ZoneQuietNotice(message: &'static str) -> impl IntoView {
    view! {
        <div class="rounded-lg border border-edge-subtle/50 bg-surface-sunken/40 px-3 py-4 text-center text-[11px] text-fg-tertiary/55">
            {message}
        </div>
    }
}
