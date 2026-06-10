//! Per-zone now-playing surfaces — the sidebar's compact zone rows and
//! the preview cabinet's zone chips.
//!
//! Both read [`EffectsContext::zone_effects`], the per-zone source of
//! truth derived from the shared active scene, so a two-zone scene shows
//! two honest rows instead of mirroring the primary zone everywhere.
//! Zone pause/resume goes through the guarded zone PATCH
//! (`api::zones::update_zone` with `enabled`), never the global stop.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use leptos_router::components::A;

use crate::api;
use crate::api::zones::ZoneOutcome;
use crate::app::EffectsContext;
use crate::icons::*;
use crate::toasts;
use crate::zones::{ZoneEffectState, ZonesContext};

/// How many zone rows the sidebar shows before folding the rest into a
/// "+N more zones" link to Studio.
pub const SIDEBAR_ZONE_ROW_CAP: usize = 3;

/// Split the per-zone states into the rows a capped list shows and the
/// count of zones folded into the overflow indicator.
#[must_use]
pub fn split_zone_rows(
    mut states: Vec<ZoneEffectState>,
    cap: usize,
) -> (Vec<ZoneEffectState>, usize) {
    let overflow = states.len().saturating_sub(cap);
    states.truncate(cap);
    (states, overflow)
}

/// Flip one zone's `enabled` flag through the revision-guarded zone
/// PATCH. A stale `groups_revision` means the scene changed under us:
/// refresh the shared scene and ask the user to retry — never clobber.
pub fn set_zone_enabled(zones_ctx: ZonesContext, zone_id: String, enabled: bool) {
    let Some(scene) = zones_ctx.active_scene.get_untracked() else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let scene_id = scene.id;
    let revision = scene.groups_revision;
    spawn_local(async move {
        let request = api::zones::UpdateZoneRequest {
            enabled: Some(enabled),
            ..Default::default()
        };
        match api::zones::update_zone(&scene_id, &zone_id, &request, Some(revision)).await {
            Ok(ZoneOutcome::Applied(_)) => zones_ctx.refresh.run(()),
            Ok(ZoneOutcome::Stale { .. }) => {
                zones_ctx.refresh.run(());
                toasts::toast_info("Scene changed, try again");
            }
            Err(error) => toasts::toast_error(&format!("Couldn't update the zone: {error}")),
        }
    });
}

/// The sidebar's per-zone now-playing list — one compact row per LED
/// zone (capped, with an overflow link to Studio). Rendered inside the
/// Now Playing panel, so the `--np-*` palette variables are in scope
/// for the swatch fallback.
#[component]
pub fn SidebarZoneRows() -> impl IntoView {
    let fx = expect_context::<EffectsContext>();

    view! {
        <div class="px-3 space-y-0.5">
            {move || {
                let (rows, overflow) = split_zone_rows(fx.zone_effects.get(), SIDEBAR_ZONE_ROW_CAP);
                view! {
                    {rows
                        .into_iter()
                        .map(|state| view! { <SidebarZoneRow state=state /> })
                        .collect_view()}
                    {(overflow > 0).then(|| view! {
                        <A
                            href="/studio"
                            attr:class="flex items-center gap-1.5 px-1.5 py-1 rounded-md \
                                        text-[10px] text-fg-tertiary hover:text-fg-primary \
                                        hover:bg-surface-hover/30 transition-colors duration-200"
                            attr:title="Open Studio to see every zone"
                        >
                            <Icon icon=LuChevronRight width="10px" height="10px" />
                            {format!(
                                "+{overflow} more zone{}",
                                if overflow == 1 { "" } else { "s" }
                            )}
                        </A>
                    })}
                }
            }}
        </div>
    }
}

/// One zone row: color swatch, zone name, what it is showing, and a
/// per-zone pause/resume toggle.
#[component]
fn SidebarZoneRow(state: ZoneEffectState) -> impl IntoView {
    let zones_ctx = expect_context::<ZonesContext>();
    let zone_id = state.zone.id.clone();
    let zone_name = state.zone.name.clone();
    let enabled = state.zone.enabled;
    let swatch = state
        .zone
        .color
        .clone()
        .unwrap_or_else(|| "rgb(var(--np-primary))".to_owned());
    let swatch_glow = format!("0 0 6px {swatch}");
    let label = state
        .display_label()
        .unwrap_or_else(|| "No effect".to_owned());
    let toggle_title = if enabled {
        format!("Pause {zone_name}")
    } else {
        format!("Resume {zone_name}")
    };

    view! {
        <div
            class="flex items-center gap-2 px-1.5 py-1 rounded-md min-w-0"
            class:opacity-60=!enabled
        >
            <div
                class="w-2 h-2 rounded-full shrink-0"
                style:background=swatch
                style:box-shadow=swatch_glow
            />
            <div class="min-w-0 flex-1">
                <div class="text-[11px] font-medium text-fg-primary truncate leading-tight">
                    {zone_name}
                </div>
                <div
                    class="text-[10px] truncate mt-0.5"
                    style:color="rgba(var(--np-secondary), 0.85)"
                >
                    {label}
                </div>
            </div>
            <button
                class="shrink-0 p-1 rounded text-fg-tertiary hover:text-fg-primary \
                       hover:bg-surface-hover/40 focus-visible:outline-none \
                       focus-visible:ring-1 focus-visible:ring-accent/50 player-btn"
                title=toggle_title.clone()
                aria-label=toggle_title
                on:click=move |_| set_zone_enabled(zones_ctx, zone_id.clone(), !enabled)
            >
                {if enabled {
                    view! { <Icon icon=LuPause width="12px" height="12px" /> }.into_any()
                } else {
                    view! { <Icon icon=LuPlay width="12px" height="12px" /> }.into_any()
                }}
            </button>
        </div>
    }
}

/// Per-zone chips for the preview cabinet overlay — one chip per LED
/// zone (color dot + zone name + what it is showing). Display-only;
/// rendered inside the cabinet's `pointer-events-none` info overlay.
#[component]
pub fn ZoneEffectChips() -> impl IntoView {
    let fx = expect_context::<EffectsContext>();

    view! {
        <div class="flex flex-wrap items-center gap-1.5">
            {move || {
                fx.zone_effects
                    .get()
                    .into_iter()
                    .map(|state| {
                        let dot = state
                            .zone
                            .color
                            .clone()
                            .unwrap_or_else(|| "var(--color-electric-purple)".to_owned());
                        let dot_glow = format!("0 0 6px {dot}");
                        let label = state.display_label();
                        let enabled = state.zone.enabled;
                        view! {
                            <span
                                class="inline-flex items-center gap-1.5 max-w-full rounded-full \
                                       border border-edge-subtle/60 bg-black/45 backdrop-blur-sm \
                                       px-2 py-1"
                                class:opacity-60=!enabled
                                title=if enabled { "" } else { "Zone paused" }
                            >
                                <span
                                    class="w-1.5 h-1.5 rounded-full shrink-0"
                                    style:background=dot
                                    style:box-shadow=dot_glow
                                />
                                <span class="text-[10px] font-medium text-fg-primary truncate max-w-[110px] \
                                             drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)]">
                                    {state.zone.name.clone()}
                                </span>
                                {label.map(|text| view! {
                                    <span class="text-[10px] text-fg-tertiary truncate max-w-[120px] \
                                                 drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)]">
                                        {text}
                                    </span>
                                })}
                            </span>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
