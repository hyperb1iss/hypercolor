//! Zone lifecycle controls and mutations for the Studio zone UI.
//!
//! The per-zone control cluster (rename / color / enable / make-default /
//! delete), the `+ New zone` affordance, and the zone-mutation helpers
//! they drive. Extracted so the zone tree can reuse them without dragging
//! in the retired surface rail.
//!
//! Every mutation carries the active scene's `groups_revision` as the
//! `If-Match` precondition; a `Stale` outcome reloads the scene so the
//! user retries against the fresh revision rather than clobbering a
//! concurrent edit.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::scene::UnassignedBehavior;

use crate::api;
use crate::api::zones::ZoneOutcome;
use crate::icons::*;
use crate::toasts;

use super::StudioContext;
use super::surface::Surface;

/// Per-zone controls: inline rename, an accent-color swatch, an enable
/// toggle, and — for `Custom` zones — a "make default" promotion and a
/// delete affordance.
#[component]
pub fn ZoneControls(surface: Surface) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let zone_id = surface.id.clone();
    let renaming = RwSignal::new(false);
    let confirm_delete = RwSignal::new(false);
    let deletable = surface.is_deletable_zone();

    let name_for_input = surface.name.clone();
    let color_value = surface
        .color
        .clone()
        .unwrap_or_else(|| "#80ffea".to_owned());
    let enabled = surface.enabled;

    view! {
        <div class="flex items-center gap-1 border-t border-edge-subtle/45 px-2.5 py-1.5">
            {
                let zone_id = zone_id.clone();
                move || {
                    if renaming.get() {
                        let zone_id = zone_id.clone();
                        view! {
                            <input
                                class="min-w-0 flex-1 rounded-md border border-edge-subtle/70 bg-surface-sunken/60 px-2 py-1 text-[12px] text-fg-primary outline-none focus:border-accent-muted"
                                prop:value=name_for_input.clone()
                                autofocus
                                on:keydown={
                                    let zone_id = zone_id.clone();
                                    move |ev| {
                                        if ev.key() == "Enter" {
                                            let value = event_target_value(&ev);
                                            commit_zone_rename(studio, &zone_id, &value);
                                            renaming.set(false);
                                        } else if ev.key() == "Escape" {
                                            renaming.set(false);
                                        }
                                    }
                                }
                                on:blur=move |ev| {
                                    let value = event_target_value(&ev);
                                    commit_zone_rename(studio, &zone_id, &value);
                                    renaming.set(false);
                                }
                            />
                        }
                            .into_any()
                    } else {
                        view! {
                            <button
                                type="button"
                                class="chip-interactive inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] text-fg-tertiary hover:text-fg-secondary"
                                title="Rename zone"
                                on:click=move |_| renaming.set(true)
                            >
                                <Icon icon=LuPencil width="11px" height="11px" />
                                "Rename"
                            </button>
                        }
                            .into_any()
                    }
                }
            }
            <label
                class="relative inline-flex h-6 w-6 shrink-0 cursor-pointer items-center justify-center rounded-md border border-edge-subtle/70 bg-surface-sunken/60"
                title="Zone accent color"
            >
                <span
                    class="h-3 w-3 rounded-full"
                    style:background-color=color_value.clone()
                />
                <input
                    type="color"
                    class="absolute inset-0 cursor-pointer opacity-0"
                    prop:value=color_value.clone()
                    on:change={
                        let zone_id = zone_id.clone();
                        move |ev| {
                            let value = event_target_value(&ev);
                            commit_zone_color(studio, &zone_id, &value);
                        }
                    }
                />
            </label>
            <button
                type="button"
                class="chip-interactive inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-edge-subtle/70"
                class=("text-[rgba(80,250,123,0.85)]", move || enabled)
                class=("text-fg-tertiary/55", move || !enabled)
                title=if enabled { "Disable zone" } else { "Enable zone" }
                on:click={
                    let zone_id = zone_id.clone();
                    move |_| commit_zone_enabled(studio, &zone_id, !enabled)
                }
            >
                <Icon icon=LuPower width="11px" height="11px" />
            </button>
            <div class="flex-1" />
            <Show when=move || deletable>
                {
                    let zone_id = zone_id.clone();
                    move || {
                        let promote_id = zone_id.clone();
                        let delete_id = zone_id.clone();
                        if confirm_delete.get() {
                            view! {
                                <button
                                    type="button"
                                    class="rounded-md bg-[rgba(255,99,99,0.16)] px-1.5 py-1 text-[10px] font-semibold uppercase tracking-wide text-[rgba(255,99,99,0.95)]"
                                    on:click={
                                        let delete_id = delete_id.clone();
                                        move |_| {
                                            commit_zone_delete(studio, &delete_id);
                                            confirm_delete.set(false);
                                        }
                                    }
                                >
                                    "Delete"
                                </button>
                                <button
                                    type="button"
                                    class="chip-interactive inline-flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary"
                                    on:click=move |_| confirm_delete.set(false)
                                >
                                    <Icon icon=LuX width="11px" height="11px" />
                                </button>
                            }
                                .into_any()
                        } else {
                            view! {
                                <button
                                    type="button"
                                    class="chip-interactive inline-flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary hover:text-fg-secondary"
                                    title="Make this the default zone"
                                    on:click=move |_| commit_make_default(studio, &promote_id)
                                >
                                    <Icon icon=LuCircleCheck width="11px" height="11px" />
                                </button>
                                <button
                                    type="button"
                                    class="chip-interactive inline-flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary hover:text-[rgba(255,99,99,0.9)]"
                                    title="Delete zone"
                                    on:click=move |_| confirm_delete.set(true)
                                >
                                    <Icon icon=LuTrash2 width="11px" height="11px" />
                                </button>
                            }
                                .into_any()
                        }
                    }
                }
            </Show>
        </div>
    }
}

/// The `+ New zone` control. Collapsed to a button until clicked, then an
/// inline name input — no modal.
#[component]
pub fn NewZoneControl() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let creating = RwSignal::new(false);

    view! {
        {move || {
            if creating.get() {
                view! {
                    <input
                        class="w-full rounded-lg border border-accent-muted bg-surface-sunken/60 px-3 py-2 text-[12px] text-fg-primary outline-none"
                        placeholder="Zone name…"
                        autofocus
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                let value = event_target_value(&ev);
                                if create_zone_from(studio, &value) {
                                    creating.set(false);
                                }
                            } else if ev.key() == "Escape" {
                                creating.set(false);
                            }
                        }
                        on:blur=move |_| creating.set(false)
                    />
                }
                    .into_any()
            } else {
                view! {
                    <button
                        type="button"
                        class="chip-interactive flex w-full items-center justify-center gap-1.5 rounded-lg border border-dashed border-edge-subtle/55 px-3 py-2 text-[11px] font-medium text-fg-tertiary hover:border-accent-muted hover:text-fg-secondary"
                        on:click=move |_| creating.set(true)
                    >
                        <Icon icon=LuPlus width="12px" height="12px" />
                        "New zone"
                    </button>
                }
                    .into_any()
            }
        }}
    }
}

/// Plain-words rendering of a scene's `unassigned_behavior` (§9.4).
#[must_use]
pub fn unassigned_behavior_label(behavior: &UnassignedBehavior) -> String {
    match behavior {
        UnassignedBehavior::Off => "Turn off".to_owned(),
        UnassignedBehavior::Hold => "Hold last colors".to_owned(),
        UnassignedBehavior::Fallback(_) => "Follow a zone".to_owned(),
    }
}

// ── Zone mutations ───────────────────────────────────────────────────────

fn scene_context(studio: StudioContext) -> Option<(String, u64)> {
    studio
        .active_scene
        .get_untracked()
        .map(|scene| (scene.id, scene.groups_revision))
}

/// Create a zone from a typed name. Returns whether the request was sent
/// — `false` keeps the inline input open so the user can fix the name.
pub fn create_zone_from(studio: StudioContext, name: &str) -> bool {
    let name = name.trim().to_owned();
    if name.is_empty() {
        toasts::toast_error("Zone name must not be empty");
        return false;
    }
    let Some((scene_id, revision)) = scene_context(studio) else {
        toasts::toast_error("No active scene is available");
        return false;
    };
    spawn_local(async move {
        match api::zones::create_zone(&scene_id, &name, None, Some(revision)).await {
            Ok(ZoneOutcome::Applied(zone)) => {
                studio.selected_surface_id.set(Some(zone.id.to_string()));
                toasts::toast_success(&format!("Zone \"{}\" created", zone.name));
                studio.refresh_scene.run(());
            }
            Ok(ZoneOutcome::Stale { .. }) => {
                toasts::toast_error("Scene changed elsewhere — reloaded, try again");
                studio.refresh_scene.run(());
            }
            Err(error) => toasts::toast_error(&format!("Zone create failed: {error}")),
        }
    });
    true
}

fn commit_zone_rename(studio: StudioContext, zone_id: &str, name: &str) {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return;
    }
    let request = api::zones::UpdateZoneRequest {
        name: Some(name),
        ..Default::default()
    };
    apply_zone_update(studio, zone_id, request, "Zone renamed");
}

fn commit_zone_color(studio: StudioContext, zone_id: &str, color: &str) {
    let request = api::zones::UpdateZoneRequest {
        color: Some(Some(color.to_owned())),
        ..Default::default()
    };
    apply_zone_update(studio, zone_id, request, "Zone color updated");
}

fn commit_zone_enabled(studio: StudioContext, zone_id: &str, enabled: bool) {
    let request = api::zones::UpdateZoneRequest {
        enabled: Some(enabled),
        ..Default::default()
    };
    apply_zone_update(
        studio,
        zone_id,
        request,
        if enabled { "Zone enabled" } else { "Zone disabled" },
    );
}

fn commit_make_default(studio: StudioContext, zone_id: &str) {
    let request = api::zones::UpdateZoneRequest {
        make_primary: Some(true),
        ..Default::default()
    };
    apply_zone_update(studio, zone_id, request, "Default zone changed");
}

fn apply_zone_update(
    studio: StudioContext,
    zone_id: &str,
    request: api::zones::UpdateZoneRequest,
    success: &'static str,
) {
    let Some((scene_id, revision)) = scene_context(studio) else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let zone_id = zone_id.to_owned();
    spawn_local(async move {
        match api::zones::update_zone(&scene_id, &zone_id, &request, Some(revision)).await {
            Ok(ZoneOutcome::Applied(_)) => {
                toasts::toast_success(success);
                studio.refresh_scene.run(());
            }
            Ok(ZoneOutcome::Stale { .. }) => {
                toasts::toast_error("Scene changed elsewhere — reloaded, try again");
                studio.refresh_scene.run(());
            }
            Err(error) => toasts::toast_error(&format!("Zone update failed: {error}")),
        }
    });
}

fn commit_zone_delete(studio: StudioContext, zone_id: &str) {
    let Some((scene_id, revision)) = scene_context(studio) else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let zone_id = zone_id.to_owned();
    spawn_local(async move {
        match api::zones::delete_zone(&scene_id, &zone_id, Some(revision)).await {
            Ok(ZoneOutcome::Applied(())) => {
                toasts::toast_success("Zone deleted");
                studio.refresh_scene.run(());
            }
            Ok(ZoneOutcome::Stale { .. }) => {
                toasts::toast_error("Scene changed elsewhere — reloaded, try again");
                studio.refresh_scene.run(());
            }
            Err(error) => toasts::toast_error(&format!("Zone delete failed: {error}")),
        }
    });
}
