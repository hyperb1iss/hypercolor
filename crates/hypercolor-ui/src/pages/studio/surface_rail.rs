//! The Lights & Screens rail — the left rail of the Studio workspace.
//!
//! Every surface is one selectable row. With a single LED zone the Lights
//! section holds one row ("All Lights") and no zone-management affordance
//! at all (§3.3). Once the daemon advertises the zone-lifecycle
//! capabilities (§9.6) the rail grows the `+ New zone` control, per-zone
//! rename/color/enable/delete, and the read-only Unassigned entry — the
//! rail fills in, it is not rebuilt.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::scene::UnassignedBehavior;

use crate::api;
use crate::api::zones::ZoneOutcome;
use crate::app::{CapabilitiesContext, WsContext};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;
use crate::toasts;
use crate::ws::messages::group_has_degraded_layer;

use super::StudioContext;
use super::surface::{Surface, SurfaceKind, UNASSIGNED_SURFACE_ID, surfaces_from_groups};

/// The left rail. Reads the active scene from [`StudioContext`] and drives
/// the selected-surface state.
#[component]
pub fn SurfaceRail() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let caps = expect_context::<CapabilitiesContext>();

    let surfaces = Memo::new(move |_| {
        studio
            .active_scene
            .get()
            .map(|scene| surfaces_from_groups(&scene.groups))
            .unwrap_or_default()
    });
    let lights = Memo::new(move |_| {
        surfaces
            .get()
            .into_iter()
            .filter(|surface| surface.kind == SurfaceKind::Light)
            .collect::<Vec<_>>()
    });
    let screens = Memo::new(move |_| {
        surfaces
            .get()
            .into_iter()
            .filter(|surface| surface.kind == SurfaceKind::Screen)
            .collect::<Vec<_>>()
    });
    // Multi-zone is "more than one LED zone" — the trigger for per-zone
    // controls, the Default-zone relabel, and the Unassigned entry.
    let multi_zone = Memo::new(move |_| lights.get().len() > 1);
    // `+ New zone` appears only when every zone-lifecycle capability is
    // live (§9.6): creating a zone you cannot render or fill is a trap.
    let zone_crud_ready = Memo::new(move |_| caps.zone_crud_ready());

    view! {
        <div class="flex h-full flex-col border-r border-edge-subtle/70 bg-surface-raised/40">
            <div class="border-b border-edge-subtle/60 px-4 py-3">
                <span class=label_class(LabelSize::Section, LabelTone::Strong)>
                    "Lights & Screens"
                </span>
            </div>
            <div class="scrollbar-none flex-1 space-y-4 overflow-y-auto px-3 py-3">
                <div class="space-y-1.5">
                    <div class="px-1">
                        <span class=label_class(LabelSize::Small, LabelTone::Default)>
                            "Lights"
                        </span>
                    </div>
                    {move || {
                        let items = lights.get();
                        let multi = multi_zone.get();
                        if items.is_empty() {
                            view! {
                                <div class="rounded-lg border border-dashed border-edge-subtle/45 px-3 py-4 text-center text-[11px] text-fg-tertiary/55">
                                    "No lights in this scene"
                                </div>
                            }
                                .into_any()
                        } else {
                            items
                                .into_iter()
                                .map(|surface| {
                                    view! { <SurfaceRow surface=surface multi_zone=multi /> }
                                })
                                .collect_view()
                                .into_any()
                        }
                    }}
                    <Show when=move || multi_zone.get()>
                        <UnassignedRow />
                    </Show>
                    <Show when=move || zone_crud_ready.get()>
                        <NewZoneControl />
                    </Show>
                </div>
                <SurfaceSection title="Screens" surfaces=screens kind=SurfaceKind::Screen />
            </div>
        </div>
    }
}

#[component]
fn SurfaceSection(
    title: &'static str,
    #[prop(into)] surfaces: Signal<Vec<Surface>>,
    kind: SurfaceKind,
) -> impl IntoView {
    let empty_label = match kind {
        SurfaceKind::Light => "No lights in this scene",
        SurfaceKind::Screen => "No screens connected",
    };
    view! {
        <div class="space-y-1.5">
            <div class="px-1">
                <span class=label_class(LabelSize::Small, LabelTone::Default)>{title}</span>
            </div>
            {move || {
                let items = surfaces.get();
                if items.is_empty() {
                    view! {
                        <div class="rounded-lg border border-dashed border-edge-subtle/45 px-3 py-4 text-center text-[11px] text-fg-tertiary/55">
                            {empty_label}
                        </div>
                    }
                        .into_any()
                } else {
                    items
                        .into_iter()
                        .map(|surface| {
                            view! { <SurfaceRow surface=surface multi_zone=false /> }
                        })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn SurfaceRow(surface: Surface, multi_zone: bool) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let ws = use_context::<WsContext>();
    let row_id = surface.id.clone();
    let select_id = surface.id.clone();
    let health_group = surface.id.clone();
    let health_layer_ids = surface.layer_ids.clone();
    let is_selected = Signal::derive({
        let row_id = row_id.clone();
        move || studio.selected_surface_id.get().as_deref() == Some(row_id.as_str())
    });
    let icon = match surface.kind {
        SurfaceKind::Light => LuLightbulb,
        SurfaceKind::Screen => LuMonitor,
    };
    let dimmed = !surface.enabled;
    // A surface whose layer stack carries a failed or asset-missing layer
    // flags itself on the rail, so trouble shows without opening the stack.
    let degraded = Signal::derive(move || {
        let (Some(ws), Some(scene)) = (ws, studio.active_scene.get()) else {
            return false;
        };
        ws.layer_health.with(|map| {
            group_has_degraded_layer(map, &scene.id, &health_group, &health_layer_ids)
        })
    });

    // A multi-zone Light row exposes the per-zone controls (§9.2); a
    // single-zone "All Lights" row and every Screen row stay plain. The
    // rows are rebuilt whenever the scene changes, so a plain bool tracks
    // the zone count without a reactive prop.
    let show_controls = surface.kind == SurfaceKind::Light && multi_zone;
    let row_name = surface.name.clone();
    let swatch = surface
        .color
        .clone()
        .unwrap_or_else(|| "rgba(128, 255, 234, 0.8)".to_owned());

    view! {
        <div
            class="group/row rounded-xl border transition-all"
            class=("border-accent-muted", move || is_selected.get())
            class=("bg-accent/8", move || is_selected.get())
            class=("border-edge-subtle/70", move || !is_selected.get())
            class=("bg-surface-overlay/40", move || !is_selected.get())
            class=("opacity-55", move || dimmed)
        >
            <button
                type="button"
                class="card-hover flex w-full items-center gap-2.5 rounded-xl px-3 py-2.5 text-left"
                on:click=move |_| studio.selected_surface_id.set(Some(select_id.clone()))
            >
                {if show_controls {
                    view! {
                        <span
                            class="h-3 w-3 shrink-0 rounded-full border border-edge-subtle/70"
                            style:background-color=swatch
                        />
                    }
                        .into_any()
                } else {
                    view! {
                        <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-surface-sunken/70">
                            <Icon
                                icon=icon
                                width="14px"
                                height="14px"
                                style="color: rgba(128, 255, 234, 0.8)"
                            />
                        </span>
                    }
                        .into_any()
                }}
                <span class="min-w-0 flex-1 truncate text-sm font-medium text-fg-primary">
                    {row_name}
                </span>
                <Show when=move || degraded.get()>
                    <span
                        class="shrink-0 text-[rgba(255,99,99,0.9)]"
                        title="A layer on this surface failed to render"
                    >
                        <Icon icon=LuTriangleAlert width="13px" height="13px" />
                    </span>
                </Show>
            </button>
            {show_controls
                .then(|| view! { <ZoneControls surface=surface.clone() /> })}
        </div>
    }
}

/// Per-zone controls revealed on a multi-zone Light row: inline rename, an
/// accent-color swatch, an enable toggle, and — for `Custom` zones — a
/// "make default" promotion and a delete affordance.
#[component]
fn ZoneControls(surface: Surface) -> impl IntoView {
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

/// The `+ New zone` control at the foot of the Lights section. Collapsed to
/// a button until clicked, then an inline name input — no modal.
#[component]
fn NewZoneControl() -> impl IntoView {
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

/// The §9.4 Unassigned entry — a synthetic rail row for device outputs in
/// no zone. It is not a surface: it has no layer stack and no Stage. The
/// scene's `unassigned_behavior` write control lives in the Stage; this
/// row is purely a selectable entry.
#[component]
fn UnassignedRow() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let is_selected = Signal::derive(move || {
        studio.selected_surface_id.get().as_deref() == Some(UNASSIGNED_SURFACE_ID)
    });
    let behavior_label = Signal::derive(move || {
        studio
            .active_scene
            .get()
            .map(|scene| unassigned_behavior_label(&scene.unassigned_behavior))
            .unwrap_or_default()
    });

    view! {
        <button
            type="button"
            class="card-hover flex w-full items-center gap-2.5 rounded-xl border border-dashed px-3 py-2.5 text-left transition-all"
            class=("border-accent-muted", move || is_selected.get())
            class=("bg-accent/8", move || is_selected.get())
            class=("border-edge-subtle/55", move || !is_selected.get())
            on:click=move |_| {
                studio.selected_surface_id.set(Some(UNASSIGNED_SURFACE_ID.to_owned()))
            }
        >
            <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-surface-sunken/70">
                <Icon
                    icon=LuBan
                    width="14px"
                    height="14px"
                    style="color: rgba(241, 250, 140, 0.75)"
                />
            </span>
            <span class="min-w-0 flex-1">
                <span class="block truncate text-sm font-medium text-fg-secondary">
                    "Unassigned"
                </span>
                <span class="block truncate text-[10px] text-fg-tertiary/65">
                    {move || behavior_label.get()}
                </span>
            </span>
        </button>
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
//
// Each mutation carries the active scene's `groups_revision` as the
// `If-Match` precondition; a `Stale` outcome reloads the scene so the user
// retries against the fresh revision rather than clobbering a concurrent
// edit.

fn scene_context(studio: StudioContext) -> Option<(String, u64)> {
    studio
        .active_scene
        .get_untracked()
        .map(|scene| (scene.id, scene.groups_revision))
}

fn create_zone_from(studio: StudioContext, name: &str) -> bool {
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
