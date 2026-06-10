//! Zone-scoped effect controls for the effects page.
//!
//! In a multi-zone scene the controls card grows a zone tab strip. The
//! primary tab keeps the page's existing plumbing untouched — the
//! `active_controls` / `active_control_values` signals and the
//! current-controls PATCH session. A non-primary tab edits that zone's
//! own controls through the layers route (a zone's synthetic legacy
//! layer id is the zone id itself, per `Zone::legacy_layer_id`), so
//! tuning zone 2 no longer silently rewrites the primary zone — design
//! doc 57 §1.3's "most misleading interaction in the app".
//!
//! Single-zone scenes render no strip and exactly today's panel.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_types::effect::{ControlDefinition, ControlValue};
use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;

use crate::api;
use crate::app::EffectsContext;
use crate::components::control_panel::ControlPanel;
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
/// cache keeps tab switches from refetching a schema already seen.
pub type ZoneControlSchemaCache = StoredValue<HashMap<String, Vec<ControlDefinition>>>;

/// The controls-card body: an optional zone tab strip plus the panel
/// for the selected zone. The primary tab renders the caller-supplied
/// signals and change handler verbatim; non-primary tabs mount a
/// [`ZoneControlsPanel`] scoped to that zone's legacy layer.
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
    let scene_id = Memo::new(move |_| {
        zones_ctx
            .active_scene
            .with(|scene| scene.as_ref().map(|scene| scene.id.clone()))
    });

    view! {
        <div class="space-y-2.5">
            {move || {
                zones_ctx
                    .multi_zone
                    .get()
                    .then(|| view! { <ZoneTabStrip selected_zone_id=selected_zone_id /> })
            }}
            {move || {
                let Some(state) = selected_state.get() else {
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
                let Some(scene_id) = scene_id.get() else {
                    return ().into_any();
                };
                let Some(effect_id) = state.effect_id.clone() else {
                    return view! {
                        <ZoneQuietNotice message="Nothing playing in this zone" />
                    }
                        .into_any();
                };
                view! {
                    <ZoneControlsPanel
                        scene_id=scene_id
                        effect_id=effect_id
                        state=state
                        schema_cache=schema_cache
                    />
                }
                    .into_any()
            }}
        </div>
    }
}

/// One tab per LED zone of the active scene: zone color dot + name.
/// Selection mirrors `ZonesContext::focused_zone` (`None` = primary),
/// the same visible choice the apply-target picker and sidebar follow.
#[component]
fn ZoneTabStrip(selected_zone_id: Memo<Option<String>>) -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();

    view! {
        <div
            class="flex flex-wrap items-center gap-1"
            role="tablist"
            aria-label="Zone controls target"
        >
            {move || {
                zones_ctx
                    .led_zones
                    .get()
                    .into_iter()
                    .map(|zone| {
                        let is_primary = zone.role == ZoneRole::Primary;
                        let selected = {
                            let zone_id = zone.id.clone();
                            Signal::derive(move || match selected_zone_id.get() {
                                None => is_primary,
                                Some(id) => !is_primary && id == zone_id,
                            })
                        };
                        let focus_id = zone.id.clone();
                        let dot = zone
                            .color
                            .clone()
                            .unwrap_or_else(|| "var(--color-electric-purple)".to_owned());
                        let dot_glow = format!("0 0 6px {dot}");
                        view! {
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || selected.get().to_string()
                                title=format!("Tune controls for {}", zone.name)
                                class=move || {
                                    let base = "inline-flex min-w-0 items-center gap-1.5 rounded-lg \
                                                border px-2.5 py-1 text-[11px] font-medium \
                                                transition-all duration-150 focus-visible:outline-none \
                                                focus-visible:ring-1 focus-visible:ring-accent/50";
                                    let state = if selected.get() {
                                        "border-accent/50 bg-accent/12 text-fg-primary"
                                    } else {
                                        "border-edge-subtle bg-surface-sunken/50 text-fg-secondary \
                                         hover:border-edge-strong hover:text-fg-primary"
                                    };
                                    format!("{base} {state}")
                                }
                                on:click=move |_| {
                                    zones_ctx
                                        .focused_zone
                                        .set((!is_primary).then(|| focus_id.clone()));
                                }
                            >
                                <span
                                    class="h-1.5 w-1.5 shrink-0 rounded-full"
                                    style:background=dot
                                    style:box-shadow=dot_glow
                                />
                                <span class="max-w-[110px] truncate">{zone.name.clone()}</span>
                            </button>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}

/// Controls for one non-primary zone's directly-assigned effect. The
/// schema comes from the (cached) effect detail; values seed from the
/// zone's scene-stored controls; edits run through the shared patch
/// session against the zone's synthetic legacy layer. The host body
/// re-mounts this panel whenever the zone or its effect changes, so the
/// seeds are always fresh.
#[component]
fn ZoneControlsPanel(
    scene_id: String,
    effect_id: String,
    state: ZoneEffectState,
    schema_cache: ZoneControlSchemaCache,
) -> impl IntoView {
    let zone_id = state.zone.id.clone();
    let accent_rgb = {
        let category = state.effect_category.clone().unwrap_or_default();
        Signal::derive(move || category_accent_rgb(&category).to_string())
    };

    let schema = LocalResource::new({
        let effect_id = effect_id.clone();
        move || {
            let effect_id = effect_id.clone();
            async move {
                if let Some(defs) = schema_cache.with_value(|cache| cache.get(&effect_id).cloned())
                {
                    return Ok(defs);
                }
                let detail = api::fetch_effect_detail(&effect_id).await?;
                schema_cache.update_value(|cache| {
                    cache.insert(effect_id, detail.controls.clone());
                });
                Ok::<_, String>(detail.controls)
            }
        }
    });
    let defs = Signal::derive(move || schema.get().and_then(Result::ok).unwrap_or_default());

    // Optimistic local values, seeded from the zone's scene state.
    let (values, set_values) = signal(state.control_values.clone());

    // Mirror Studio's layer-inspector request shape: the zone's controls
    // live on its synthetic legacy layer, whose id is the zone id itself.
    let patch: ControlPatchFn = Arc::new({
        let zone_id = zone_id.clone();
        move |payload: serde_json::Value, version: Option<u64>| -> ControlPatchFuture {
            let scene_id = scene_id.clone();
            let zone_id = zone_id.clone();
            Box::pin(async move {
                let outcome =
                    api::patch_layer_controls(&scene_id, &zone_id, &zone_id, &payload, version)
                        .await?;
                Ok(outcome.map(|stack| Some(stack.layers_version)))
            })
        }
    });
    let session = use_control_patch_session(ControlPatchConfig {
        defs,
        set_values,
        initial_version: Some(state.layers_version),
        debounce_ms: ZONE_CONTROLS_DEBOUNCE_MS,
        patch,
        on_error: Callback::new(|error: String| {
            toasts::toast_error(&format!("Zone controls failed: {error}"));
        }),
        flush_guard: None,
    });

    view! {
        {move || {
            if schema.get().is_none() {
                view! { <ZoneQuietNotice message="Loading controls…" /> }.into_any()
            } else {
                view! {
                    <ControlPanel
                        controls=defs
                        control_values=values
                        accent_rgb=accent_rgb
                        on_change=session.on_change
                    />
                }
                    .into_any()
            }
        }}
    }
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
