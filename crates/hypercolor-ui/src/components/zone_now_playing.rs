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

use hypercolor_types::scene::ZoneRole;

use crate::api;
use crate::api::zones::ZoneOutcome;
use crate::app::EffectsContext;
use crate::apply_target::ApplyTarget;
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
/// PATCH. A stale scene revision means the document changed under us:
/// refresh the shared scene and ask the user to retry — never clobber.
pub fn set_zone_enabled(zones_ctx: ZonesContext, zone_id: String, enabled: bool) {
    let Some(scene) = zones_ctx.active_scene.get_untracked() else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let revision = scene.revision;
    spawn_local(async move {
        let request = api::zones::PatchZoneRequest {
            enabled: Some(enabled),
            ..Default::default()
        };
        match api::zones::update_zone(&zone_id, &request, revision).await {
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
                            href=crate::route_ui::route_href("/studio")
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

/// Point the preview, quick-applies, and the controls at one LED zone,
/// the same choice Studio's selection and the effects page's apply-to
/// picker make. The Default zone is the unfocused state.
pub fn focus_zone(zones_ctx: ZonesContext, fx: EffectsContext, zone_id: &str, role: ZoneRole) {
    let (focused, target) = if role == ZoneRole::Primary {
        (None, ApplyTarget::Primary)
    } else {
        (
            Some(zone_id.to_owned()),
            ApplyTarget::Zone(zone_id.to_owned()),
        )
    };
    if zones_ctx.focused_zone.get_untracked() != focused {
        zones_ctx.focused_zone.set(focused);
    }
    if fx.apply_target.get_untracked() != target {
        fx.apply_target.set(target);
    }
}

/// Per-zone chips for the preview cabinet overlay, one per LED zone
/// (color dot, zone name, what it shows). The preview shows one zone at a
/// time; each chip picks that zone, which is also the zone the cabinet's
/// presets, quick-applies, and controls target.
#[component]
pub fn ZoneEffectChips() -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let zones_ctx = expect_context::<ZonesContext>();
    let shown_zone = Memo::new(move |_| zones_ctx.target_zone().map(|zone| zone.id));

    view! {
        <div
            class="pointer-events-auto flex flex-wrap items-center gap-1.5"
            role="group"
            aria-label="Zone shown in the preview"
        >
            {move || {
                fx.zone_effects
                    .get()
                    .into_iter()
                    .map(|state| {
                        let dot = state
                            .zone
                            .color
                            .clone()
                            .unwrap_or_else(|| "var(--color-accent)".to_owned());
                        let dot_glow = format!("0 0 6px {dot}");
                        let label = state.stack_label();
                        let enabled = state.zone.enabled;
                        let zone_name = state.zone.name.clone();
                        let title = match (enabled, state.stack_description()) {
                            (false, _) => format!("Show {zone_name} (paused)"),
                            (true, Some(stack)) => format!("Show {zone_name}: {stack}"),
                            (true, None) => format!("Show {zone_name}"),
                        };
                        let zone_id = state.zone.id.clone();
                        let role = state.zone.role;
                        let selected = {
                            let zone_id = zone_id.clone();
                            Signal::derive(move || {
                                shown_zone.get().as_deref() == Some(zone_id.as_str())
                            })
                        };
                        view! {
                            <button
                                type="button"
                                aria-pressed=move || selected.get().to_string()
                                title=title
                                class=move || {
                                    let base = "inline-flex items-center gap-1.5 max-w-full rounded-full \
                                                border backdrop-blur-sm px-2 py-1 \
                                                transition-all duration-150 btn-press \
                                                focus-visible:outline-none focus-visible:ring-1 \
                                                focus-visible:ring-accent/60";
                                    let state = if selected.get() {
                                        "border-accent/60 bg-accent/20 \
                                         shadow-[0_0_10px_var(--glow-accent-50)]"
                                    } else {
                                        "border-edge-subtle/60 bg-black/45 \
                                         hover:border-edge-strong hover:bg-black/60"
                                    };
                                    format!("{base} {state}")
                                }
                                class:opacity-60=!enabled
                                on:click=move |_| focus_zone(zones_ctx, fx, &zone_id, role)
                            >
                                <span
                                    class="w-1.5 h-1.5 rounded-full shrink-0"
                                    style:background=dot
                                    style:box-shadow=dot_glow
                                />
                                <span class="text-[10px] font-medium text-fg-primary truncate max-w-[110px] \
                                             drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)]">
                                    {zone_name}
                                </span>
                                {label.map(|text| view! {
                                    <span class="text-[10px] text-fg-tertiary truncate max-w-[120px] \
                                                 drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)]">
                                        {text}
                                    </span>
                                })}
                            </button>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
