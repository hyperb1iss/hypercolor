//! The zone tree — the Studio workspace's left column.
//!
//! One column that answers "what hardware do I have and how is it
//! grouped": every zone, with its devices nested beneath it, an
//! Unassigned group for hardware in no zone, and an always-present
//! "+ New zone". It replaces the old surface rail and the separate
//! device palette — devices are visible here, not behind a drawer.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::DeviceSummary;
use crate::app::{CapabilitiesContext, DevicesContext, WsContext};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;
use crate::storage;
use crate::ws::messages::group_has_degraded_layer;

use super::StudioContext;
use super::device_card::{CardMode, StudioDeviceCard};
use super::device_grouping::{
    DeviceMeta, ZoneDeviceRow, device_rows_for_zone, sort_device_rows, unassigned_device_rows,
};
use super::surface::{Surface, SurfaceKind, UNASSIGNED_SURFACE_ID, surfaces_from_groups};
use super::zone_add_device::ZoneAddDevice;
use super::zone_controls::{NewZoneControl, ZoneControls};

/// Collapsed zone ids, persisted per browser. Empty (the default) leaves
/// every zone expanded.
const COLLAPSED_KEY: &str = "hc-studio-zone-tree-collapsed";

fn load_collapsed() -> HashSet<String> {
    storage::get(COLLAPSED_KEY)
        .map(|raw| {
            raw.split(',')
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The left column. Reads the active scene and the device registry, and
/// drives the selected-surface state.
#[component]
pub fn ZoneTree() -> impl IntoView {
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
    let multi_zone = Memo::new(move |_| lights.get().len() > 1);
    let zone_crud_ready = Memo::new(move |_| caps.zone_crud_ready());

    // The device registry as `DeviceMeta` — the join target for grouping
    // a zone's outputs back to physical devices.
    let device_metas = Memo::new(move |_| {
        devices
            .devices_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|device| DeviceMeta {
                layout_device_id: device.layout_device_id,
                name: device.name,
                total_leds: u32::try_from(device.total_leds).unwrap_or(u32::MAX),
            })
            .collect::<Vec<_>>()
    });

    // The same registry keyed by layout device id — the join from a row
    // back to its full `DeviceSummary`, which the rich card needs for the
    // brand strip, vendor mark, and component breakdown.
    let device_by_id = Memo::new(move |_| {
        devices
            .devices_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|device| (device.layout_device_id.clone(), device))
            .collect::<HashMap<String, DeviceSummary>>()
    });

    // Each light zone paired with the device rows nested under it; every
    // row carries its resolved `DeviceSummary`, or `None` when the device
    // is placed in the layout but absent from the registry.
    let zone_rows = Memo::new(move |_| {
        let Some(scene) = studio.active_scene.get() else {
            return Vec::new();
        };
        let metas = device_metas.get();
        let by_id = device_by_id.get();
        lights
            .get()
            .into_iter()
            .map(|surface| {
                let outputs = scene
                    .groups
                    .iter()
                    .find(|group| group.id.to_string() == surface.id)
                    .map(|group| group.layout.zones.clone())
                    .unwrap_or_default();
                let mut base_rows = device_rows_for_zone(&outputs, &metas);
                sort_device_rows(&mut base_rows);
                let rows = base_rows
                    .into_iter()
                    .map(|row| {
                        let device = by_id.get(&row.device_id).cloned();
                        (row, device)
                    })
                    .collect::<Vec<_>>();
                (surface, rows)
            })
            .collect::<Vec<_>>()
    });
    let unassigned = Memo::new(move |_| {
        let Some(scene) = studio.active_scene.get() else {
            return Vec::new();
        };
        let by_id = device_by_id.get();
        let mut base_rows = unassigned_device_rows(&scene.groups, &device_metas.get());
        sort_device_rows(&mut base_rows);
        base_rows
            .into_iter()
            .map(|row| {
                let device = by_id.get(&row.device_id).cloned();
                (row, device)
            })
            .collect::<Vec<_>>()
    });

    let collapsed = RwSignal::new(load_collapsed());
    Effect::new(move |_| {
        let joined = collapsed.get().into_iter().collect::<Vec<_>>().join(",");
        storage::set(COLLAPSED_KEY, &joined);
    });

    view! {
        <div class="flex h-full flex-col border-r border-edge-subtle/70 bg-surface-raised/40">
            <div class="scrollbar-none flex-1 space-y-4 overflow-y-auto px-3 py-3">
                <div class="space-y-1.5">
                    <div class="px-1">
                        <span class=label_class(LabelSize::Small, LabelTone::Default)>
                            "Zones"
                        </span>
                    </div>
                    {move || {
                        let rows = zone_rows.get();
                        // Single-zone folds devices-in-no-zone under the sole
                        // LED zone as one-tap "available" rows (§3.3); only a
                        // genuinely multi-zone scene keeps the Unassigned bucket.
                        let available_rows = if multi_zone.get() {
                            Vec::new()
                        } else {
                            unassigned.get()
                        };
                        if rows.is_empty() {
                            view! {
                                <div class="rounded-lg border border-dashed border-edge-subtle/45 px-3 py-4 text-center text-[11px] text-fg-tertiary/55">
                                    "No zones"
                                </div>
                            }
                                .into_any()
                        } else {
                            rows.into_iter()
                                .enumerate()
                                .map(|(index, (surface, devices))| {
                                    let available = if index == 0 {
                                        available_rows.clone()
                                    } else {
                                        Vec::new()
                                    };
                                    view! {
                                        <ZoneNode
                                            surface=surface
                                            devices=devices
                                            available=available
                                            collapsed=collapsed
                                        />
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }
                    }}
                    <Show when=move || multi_zone.get()>
                        <UnassignedNode rows=unassigned />
                    </Show>
                    <Show when=move || zone_crud_ready.get()>
                        <NewZoneControl />
                    </Show>
                </div>
                {move || {
                    let items = screens.get();
                    (!items.is_empty()).then(|| {
                        view! {
                            <div class="space-y-1.5">
                                <div class="px-1">
                                    <span class=label_class(LabelSize::Small, LabelTone::Default)>
                                        "Screens"
                                    </span>
                                </div>
                                {items
                                    .into_iter()
                                    .map(|surface| view! { <ScreenRow surface=surface /> })
                                    .collect_view()}
                            </div>
                        }
                    })
                }}
            </div>
        </div>
    }
}

/// One light zone: a selectable header over its nested device list,
/// with the per-zone controls folded behind a kebab. The kebab is
/// always present so a single-zone scene still has a route to rename
/// or recolor its Default zone; make-default and delete remain gated
/// inside [`ZoneControls`] by [`Surface::is_deletable_zone`].
#[component]
fn ZoneNode(
    surface: Surface,
    devices: Vec<(ZoneDeviceRow, Option<DeviceSummary>)>,
    /// Connected devices in no zone, folded under the sole LED zone in a
    /// single-zone scene (§3.3); empty otherwise. Each offers a one-tap add.
    available: Vec<(ZoneDeviceRow, Option<DeviceSummary>)>,
    collapsed: RwSignal<HashSet<String>>,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let ws = use_context::<WsContext>();
    let zone_id = surface.id.clone();
    let device_count = devices.len();
    // Captured before the device lists move `available`; gates the
    // Available section and suppresses the redundant picker when the
    // one-tap rows already cover adding hardware.
    let has_available = !available.is_empty();

    let is_selected = Signal::derive({
        let zone_id = zone_id.clone();
        move || studio.selected_surface_id.get().as_deref() == Some(zone_id.as_str())
    });
    let is_open = Signal::derive({
        let zone_id = zone_id.clone();
        move || !collapsed.with(|set| set.contains(&zone_id))
    });

    let health_group = zone_id.clone();
    let health_layer_ids = surface.layer_ids.clone();
    let degraded = Signal::derive(move || {
        let (Some(ws), Some(scene)) = (ws, studio.active_scene.get()) else {
            return false;
        };
        ws.layer_health
            .with(|map| group_has_degraded_layer(map, &scene.id, &health_group, &health_layer_ids))
    });

    let controls_open = RwSignal::new(false);
    let dimmed = !surface.enabled;
    let row_name = surface.name.clone();
    let swatch = surface.color.clone();
    let controls_surface = surface.clone();

    let select_id = zone_id.clone();
    let toggle_id = zone_id.clone();

    view! {
        <div
            class="overflow-hidden rounded-xl border bg-surface-overlay/40 transition-all"
            class=("border-accent-muted", move || is_selected.get())
            class=("border-edge-subtle/70", move || !is_selected.get())
            class=("opacity-55", move || dimmed)
        >
            <div class="flex items-center">
                <button
                    type="button"
                    class="flex h-9 w-7 shrink-0 items-center justify-center text-fg-tertiary transition-colors hover:text-fg-primary"
                    title="Expand or collapse"
                    on:click=move |_| {
                        collapsed
                            .update(|set| {
                                if !set.remove(&toggle_id) {
                                    set.insert(toggle_id.clone());
                                }
                            });
                    }
                >
                    <Icon
                        icon=LuChevronRight
                        width="13px"
                        height="13px"
                        {..}
                        class=("rotate-90", move || is_open.get())
                        style="transition: transform 150ms"
                    />
                </button>
                <button
                    type="button"
                    class="card-hover flex min-w-0 flex-1 items-center gap-2.5 py-2 pr-2 text-left"
                    on:click=move |_| studio.selected_surface_id.set(Some(select_id.clone()))
                >
                    {match swatch {
                        Some(color) => view! {
                            <span
                                class="h-3 w-3 shrink-0 rounded-full border border-edge-subtle/70"
                                style:background-color=color
                            />
                        }
                            .into_any(),
                        None => view! {
                            <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-surface-sunken/70">
                                <Icon
                                    icon=LuLightbulb
                                    width="13px"
                                    height="13px"
                                    style="color: rgba(128, 255, 234, 0.8)"
                                />
                            </span>
                        }
                            .into_any(),
                    }}
                    <span class="min-w-0 flex-1">
                        <span class="block truncate text-sm font-medium text-fg-primary">
                            {row_name}
                        </span>
                        <span class="block truncate text-[10px] text-fg-tertiary/65">
                            {if device_count == 1 {
                                "1 device".to_owned()
                            } else {
                                format!("{device_count} devices")
                            }}
                        </span>
                    </span>
                    <Show when=move || degraded.get()>
                        <span
                            class="shrink-0 text-[rgba(255,99,99,0.9)]"
                            title="A layer on this zone failed to render"
                        >
                            <Icon icon=LuTriangleAlert width="13px" height="13px" />
                        </span>
                    </Show>
                </button>
                <button
                    type="button"
                    class="flex h-9 w-7 shrink-0 items-center justify-center text-fg-secondary/70 transition-colors hover:text-fg-primary"
                    title="Zone settings — rename, color, enable"
                    on:click=move |_| controls_open.update(|open| *open = !*open)
                >
                    <Icon icon=LuEllipsis width="14px" height="14px" />
                </button>
            </div>
            <div class=("hidden", move || !controls_open.get())>
                <ZoneControls surface=controls_surface />
            </div>
            <div
                class="space-y-1.5 border-t border-edge-subtle/45 bg-surface-sunken/60 px-1.5 py-2"
                class=("hidden", move || !is_open.get())
            >
                {(devices.is_empty() && !has_available)
                    .then(|| {
                        view! {
                            <div class="px-2 py-1.5 text-[10px] text-fg-tertiary/50">
                                "No devices yet"
                            </div>
                        }
                    })}
                {
                    let zone_id = zone_id.clone();
                    devices
                        .into_iter()
                        .map(move |(row, device)| {
                            view! {
                                <StudioDeviceCard row=row device=device select=zone_id.clone() />
                            }
                        })
                        .collect_view()
                }
                {has_available
                    .then({
                        let zone_id = zone_id.clone();
                        move || {
                            view! {
                                <div class="mt-1 space-y-1.5 border-t border-edge-subtle/30 pt-2">
                                    <div class="flex items-center gap-1.5 px-1">
                                        <span class=label_class(
                                            LabelSize::Micro,
                                            LabelTone::Default,
                                        )>"Available"</span>
                                        <span class="text-[9px] text-fg-tertiary/45">
                                            "tap + to add"
                                        </span>
                                    </div>
                                    {available
                                        .into_iter()
                                        .map(move |(row, device)| {
                                            view! {
                                                <StudioDeviceCard
                                                    row=row
                                                    device=device
                                                    select=zone_id.clone()
                                                    mode=CardMode::Available
                                                />
                                            }
                                        })
                                        .collect_view()}
                                </div>
                            }
                        }
                    })}
                {(!has_available)
                    .then(move || view! { <ZoneAddDevice zone_id=zone_id.clone() /> })}
            </div>
        </div>
    }
}

/// The §9.4 Unassigned group: hardware the scene places in no zone.
#[component]
fn UnassignedNode(rows: Memo<Vec<(ZoneDeviceRow, Option<DeviceSummary>)>>) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let is_selected = Signal::derive(move || {
        studio.selected_surface_id.get().as_deref() == Some(UNASSIGNED_SURFACE_ID)
    });

    view! {
        <div
            class="overflow-hidden rounded-xl border border-dashed transition-all"
            class=("border-accent-muted", move || is_selected.get())
            class=("border-edge-subtle/55", move || !is_selected.get())
        >
            <button
                type="button"
                class="card-hover flex w-full items-center gap-2.5 px-3 py-2 text-left"
                on:click=move |_| {
                    studio.selected_surface_id.set(Some(UNASSIGNED_SURFACE_ID.to_owned()))
                }
            >
                <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-surface-sunken/70">
                    <Icon
                        icon=LuBan
                        width="13px"
                        height="13px"
                        style="color: rgba(241, 250, 140, 0.75)"
                    />
                </span>
                <span class="min-w-0 flex-1">
                    <span class="block truncate text-sm font-medium text-fg-secondary">
                        "Unassigned"
                    </span>
                    <span class="block truncate text-[10px] text-fg-tertiary/65">
                        "Hardware in no zone"
                    </span>
                </span>
            </button>
            {move || {
                let rows = rows.get();
                (!rows.is_empty()).then(|| {
                    view! {
                        <div class="space-y-1.5 border-t border-edge-subtle/45 bg-surface-sunken/60 px-1.5 py-2">
                            {rows
                                .into_iter()
                                .map(|(row, device)| {
                                    view! {
                                        <StudioDeviceCard
                                            row=row
                                            device=device
                                            select=UNASSIGNED_SURFACE_ID.to_owned()
                                            mode=CardMode::Unassigned
                                        />
                                    }
                                })
                                .collect_view()}
                        </div>
                    }
                })
            }}
        </div>
    }
}

/// One display-face screen — a 1:1 surface, no nested devices.
#[component]
fn ScreenRow(surface: Surface) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let select_id = surface.id.clone();
    let is_selected = Signal::derive({
        let row_id = surface.id.clone();
        move || studio.selected_surface_id.get().as_deref() == Some(row_id.as_str())
    });
    let dimmed = !surface.enabled;
    let row_name = surface.name.clone();

    view! {
        <div
            class="rounded-xl border transition-all"
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
                <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-surface-sunken/70">
                    <Icon
                        icon=LuMonitor
                        width="14px"
                        height="14px"
                        style="color: rgba(128, 255, 234, 0.8)"
                    />
                </span>
                <span class="min-w-0 flex-1 truncate text-sm font-medium text-fg-primary">
                    {row_name}
                </span>
            </button>
        </div>
    }
}
