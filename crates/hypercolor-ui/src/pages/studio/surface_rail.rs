//! The Lights & Screens rail — the left rail of the Studio workspace.
//!
//! Every surface is one selectable row. With a single LED zone the Lights
//! section holds one row ("All Lights") and no zone-management affordance
//! at all (§3.3). Once the daemon advertises the zone-lifecycle
//! capabilities (§9.6) the rail grows the `+ New zone` control, per-zone
//! rename/color/enable/delete, and the read-only Unassigned entry — the
//! rail fills in, it is not rebuilt.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::app::{CapabilitiesContext, DevicesContext, WsContext};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;
use crate::ws::messages::group_has_degraded_layer;

use super::StudioContext;
use super::surface::{Surface, SurfaceKind, UNASSIGNED_SURFACE_ID, surfaces_from_groups};
use super::zone_controls::{NewZoneControl, ZoneControls, unassigned_behavior_label};

/// The left rail. Reads the active scene from [`StudioContext`] and drives
/// the selected-surface state.
#[component]
pub fn SurfaceRail() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let caps = expect_context::<CapabilitiesContext>();
    let devices = expect_context::<DevicesContext>();

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

    // LED-lit devices the lone "All Lights" zone drives. A display-face
    // device reports zero LEDs (an LCD is not a light), and OS monitors
    // never appear in the device registry at all, so counting devices
    // with LEDs lands on exactly the hardware "All Lights" feeds. `None`
    // until the list resolves, so the rail never flashes a stale subtitle.
    let led_device_count = Memo::new(move |_| {
        devices
            .devices_resource
            .get()
            .and_then(Result::ok)
            .map(|list| list.iter().filter(|device| device.total_leds > 0).count())
    });

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
                        // The lone "All Lights" zone drives every light, so it
                        // carries the device count; per-zone rows in a
                        // multi-zone scene cannot be attributed from here.
                        let led_devices = if multi { None } else { led_device_count.get() };
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
                                    view! {
                                        <SurfaceRow
                                            surface=surface
                                            multi_zone=multi
                                            device_count=led_devices
                                        />
                                    }
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
fn SurfaceRow(
    surface: Surface,
    multi_zone: bool,
    /// Device count shown as a row subtitle. `Some` only for the lone
    /// "All Lights" row; `None` leaves the row a single line.
    #[prop(optional_no_strip)]
    device_count: Option<usize>,
) -> impl IntoView {
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
                <span class="min-w-0 flex-1">
                    <span class="block truncate text-sm font-medium text-fg-primary">
                        {row_name}
                    </span>
                    {device_count.map(|count| {
                        let label = if count == 1 {
                            "1 device".to_owned()
                        } else {
                            format!("{count} devices")
                        };
                        view! {
                            <span class="block truncate text-[10px] text-fg-tertiary/65">
                                {label}
                            </span>
                        }
                    })}
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
