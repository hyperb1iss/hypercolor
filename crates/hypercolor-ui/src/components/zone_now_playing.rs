//! The sidebar's compact zone selector and selected-zone controls.
//!
//! Selection shares the app's focused zone, keeping the preview and player
//! on the same target. The dropdown holds every zone without growing the rail.
//! Zone pause/resume goes through the guarded zone PATCH
//! (`api::zones::update_zone` with `enabled`), never the global stop.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use crate::api;
use crate::api::zones::ZoneOutcome;
use crate::app::EffectsContext;
use crate::components::silk_select::SilkSelect;
use crate::components::zone_tabs::ZoneDot;
use crate::icons::*;
use crate::toasts;
use crate::zones::ZonesContext;

/// Flip one zone's `enabled` flag through the revision-guarded zone
/// PATCH. While Studio is open the patch goes through its history, so
/// it is an undo step like the tab menu's pause. A stale scene revision
/// means the document changed under us: refresh the shared scene and ask
/// the user to retry, never clobber.
pub fn set_zone_enabled(zones_ctx: ZonesContext, zone_id: String, enabled: bool) {
    let request = api::zones::PatchZoneRequest {
        enabled: Some(enabled),
        ..Default::default()
    };
    // A Studio already gone leaves a callback that no longer runs; the
    // patch then writes directly.
    if let Some(patch) = zones_ctx.studio_zone_patch.try_get_value().flatten()
        && patch.try_run((zone_id.clone(), request.clone())).is_some()
    {
        return;
    }
    let Some(scene) = zones_ctx.active_scene.get_untracked() else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let fence = api::Fence::of(&scene);
    spawn_local(async move {
        match api::zones::update_zone(&zone_id, &request, &fence).await {
            Ok(ZoneOutcome::Applied(_)) => zones_ctx.refresh.run(()),
            Ok(ZoneOutcome::Stale { .. }) => {
                zones_ctx.refresh.run(());
                toasts::toast_info("Scene changed, try again");
            }
            Err(error) => toasts::toast_error(&format!("Couldn't update the zone: {error}")),
        }
    });
}

/// A fixed-height player card with every LED zone in a bounded dropdown.
#[component]
pub fn SidebarZoneSelector() -> impl IntoView {
    let fx = expect_context::<EffectsContext>();
    let zones = expect_context::<ZonesContext>();
    let selected = Memo::new(move |_| zones.target_zone());
    let value = Signal::derive(move || selected.get().map(|zone| zone.id).unwrap_or_default());
    let options = Signal::derive(move || {
        zones
            .led_zones
            .get()
            .into_iter()
            .map(|zone| {
                let label = if zone.enabled {
                    zone.name
                } else {
                    format!("{} (paused)", zone.name)
                };
                (zone.id, label)
            })
            .collect::<Vec<_>>()
    });
    let effect_label = Memo::new(move |_| {
        let id = value.get();
        fx.zone_effects.with(|states| {
            states
                .iter()
                .find(|state| state.zone.id == id)
                .and_then(crate::zones::ZoneEffectState::display_label)
                .unwrap_or_else(|| "No effect".to_owned())
        })
    });
    let enabled = move || selected.with(|zone| zone.as_ref().is_some_and(|zone| zone.enabled));
    let toggle_label = move || {
        selected.with(|zone| {
            zone.as_ref().map_or_else(String::new, |zone| {
                format!(
                    "{} {}",
                    if zone.enabled { "Pause" } else { "Resume" },
                    zone.name
                )
            })
        })
    };
    let selection_label = Signal::derive(move || {
        selected.with(|zone| {
            zone.as_ref().map_or_else(
                || "Preview zone".to_owned(),
                |zone| {
                    format!(
                        "Preview zone: {}, {}{}",
                        zone.name,
                        effect_label.get(),
                        if zone.enabled { "" } else { ", paused" },
                    )
                },
            )
        })
    });

    view! {
        <div class="px-3" data-testid="sidebar-zone-selector">
            <div class="flex items-center gap-1 rounded-lg border border-edge-subtle bg-surface-overlay">
                <div class="min-w-0 flex-1">
                    <SilkSelect
                        value=value
                        options=options
                        on_change=Callback::new(move |id: String| zones.focused_zone.set(Some(id)))
                        aria_label=selection_label
                        class="px-2 py-2 text-fg-primary hover:bg-surface-hover"
                        fit_options=true
                    >
                        <span class="flex items-center gap-2 min-w-0">
                            <span class="flex shrink-0 text-[11px]">
                                <ZoneDot color=Signal::derive(move || selected.get().and_then(|zone| zone.color)) />
                            </span>
                            <span class="flex min-w-0 flex-1 flex-col gap-0.5">
                                <span class="truncate text-[11px] font-medium leading-tight">
                                    {move || selected.get().map(|zone| zone.name).unwrap_or_default()}
                                </span>
                                <span class="truncate text-[10px] text-fg-secondary" title=move || effect_label.get()>
                                    {move || effect_label.get()}
                                </span>
                            </span>
                        </span>
                    </SilkSelect>
                </div>
                <button
                    type="button"
                    class="shrink-0 mr-1 p-1.5 rounded-md text-fg-tertiary hover:text-accent \
                           hover:bg-surface-hover player-btn"
                    title=toggle_label
                    aria-label=toggle_label
                    disabled=move || selected.get().is_none()
                    on:click=move |_| {
                        if let Some(zone) = selected.get_untracked() {
                            set_zone_enabled(zones, zone.id, !zone.enabled);
                        }
                    }
                >
                    <span class="flex" aria-hidden="true">
                        {move || if enabled() {
                            view! { <Icon icon=LuPause width="12px" height="12px" /> }.into_any()
                        } else {
                            view! { <Icon icon=LuPlay width="12px" height="12px" /> }.into_any()
                        }}
                    </span>
                </button>
            </div>
        </div>
    }
}
