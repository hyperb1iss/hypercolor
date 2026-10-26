//! The device pane, the Studio workspace's left column.
//!
//! It answers one question about the active tab: what is in this zone,
//! and what could be? A search at the top filters every section. The
//! active zone's devices come first under a sticky header, with an
//! Offline subsection at the bottom; then one collapsed row per other LED
//! zone; then the Unassigned section with the scene's policy for
//! hardware no zone claims. While the Screens tab is active the pane
//! lists the scene's Screens instead.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use hypercolor_types::scene::ZoneRole;

use crate::api::zones::ZoneOutcome;
use crate::api::{self, DeviceSummary};
use crate::app::{CapabilitiesContext, DevicesContext};
use crate::channel_names;
use crate::components::page_search_bar::PAGE_SEARCH_INPUT_ID;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::pointer_gesture::listener_element;
use crate::toasts;

use super::StudioContext;
use super::device_assignment::{DeviceMeta, ZoneDeviceRow, saved_only_device_rows};
use super::device_card::{CardMode, Elsewhere, StudioDeviceCard, fetch_attachments_if_needed};
use super::drag::{DragController, FocusReturn};
use super::drag_model::DropTarget;
use super::move_menu::{MoveMenuAnchor, is_menu_key};
use super::moves::zone_device_payload;
use super::offline_device_card::OfflineDeviceCard;
use super::pane_model::{
    PaneInputs, PaneModel, ZoneSection, build_pane, normalize_search, parse_unassigned_policy,
    section_count, shows_unassigned, unassigned_policy_options, unassigned_policy_value,
};
use super::screen_list::ScreenList;
use super::surface::{Surface, SurfaceKind, surfaces_from_zones};

/// Element id of an Other zones row, the target a split device's
/// "more outputs" hint scrolls to.
#[must_use]
pub fn pane_zone_dom_id(zone_id: &str) -> String {
    format!("pane-zone-{zone_id}")
}

/// Element id of an Other zones row's expand toggle.
#[must_use]
pub fn pane_zone_toggle_dom_id(zone_id: &str) -> String {
    format!("pane-zone-toggle-{zone_id}")
}

/// A row the user opened or closed is theirs: a new zone's automatic
/// expansion no longer closes it.
fn claim_pane_row(studio: StudioContext, zone_id: &str) {
    studio.pane_auto_expanded.update(|auto| {
        if let Some((_, rows)) = auto.as_mut() {
            rows.remove(zone_id);
        }
    });
}

/// Element id of a compact device row in an Other zones row.
#[must_use]
pub fn compact_row_dom_id(zone_id: &str, device_id: &str) -> String {
    format!("compact-row-{zone_id}-{device_id}")
}

/// Expand an Other zones row and bring it into view.
pub fn reveal_pane_zone(studio: StudioContext, zone_id: &str) {
    studio.pane_expanded.update(|expanded| {
        expanded.insert(zone_id.to_owned());
    });
    claim_pane_row(studio, zone_id);
    let dom_id = pane_zone_dom_id(zone_id);
    request_animation_frame(move || {
        if let Some(element) = document().get_element_by_id(&dom_id) {
            element.scroll_into_view();
        }
    });
}

#[component]
pub fn DevicePane() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let drag = expect_context::<DragController>();
    let devices = expect_context::<DevicesContext>();
    let search = RwSignal::new(String::new());
    let query = Memo::new(move |_| normalize_search(&search.get()));
    let on_screens = Memo::new(move |_| {
        let selected = studio.selected_surface_id.get();
        studio.active_scene.with(|scene| {
            scene.as_ref().is_some_and(|scene| {
                scene.zones.iter().any(|zone| {
                    zone.role == ZoneRole::Display
                        && selected.as_deref() == Some(zone.id.to_string().as_str())
                })
            })
        })
    });

    provide_context(super::offline_device_card::SavedControllersRefresh(
        studio.refresh_rig_layout,
    ));

    let registry = Memo::new(move |_| {
        devices
            .devices_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
    });
    let registry_ready = Memo::new(move |_| matches!(devices.devices_resource.get(), Some(Ok(_))));
    let lights = Memo::new(move |_| {
        studio.active_scene.with(|scene| {
            scene
                .as_ref()
                .map(|scene| {
                    surfaces_from_zones(&scene.zones)
                        .into_iter()
                        .filter(|surface| surface.kind == SurfaceKind::Light)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });

    // Every name a device answers to in search beyond its own: its
    // channels (as the hardware and the user name them) and the
    // components bound to them.
    let search_terms = Memo::new(move |_| {
        let attachments = studio.attachment_cache.get();
        registry
            .get()
            .into_iter()
            .map(|device| {
                let mut terms = Vec::new();
                for channel in &device.segments {
                    terms.push(channel.name.clone());
                    terms.push(channel_names::effective_channel_name(
                        &device.id,
                        &channel.id,
                        &channel.name,
                    ));
                }
                for binding in attachments.get(&device.id).into_iter().flatten() {
                    terms.push(binding.template_name.clone());
                    terms.extend(binding.name.clone());
                }
                (device.layout_device_id, terms)
            })
            .collect::<HashMap<_, _>>()
    });
    // Component names live in the attachment cache, which cards fill as
    // they mount. A search must also reach devices in collapsed rows, so
    // starting a search fills the rest of the cache once.
    Effect::new(move |was_searching: Option<bool>| {
        let searching = !query.with(String::is_empty);
        if searching && was_searching != Some(true) {
            for device in registry.get_untracked() {
                fetch_attachments_if_needed(studio, &device.id);
            }
        }
        searching
    });

    let pane = Memo::new(move |_| {
        let Some(scene) = studio.active_scene.get() else {
            return PaneModel::default();
        };
        let metas: Vec<DeviceMeta> = registry
            .get()
            .into_iter()
            .map(|device| DeviceMeta {
                layout_device_id: device.layout_device_id,
                name: device.name,
                total_leds: device.total_leds,
            })
            .collect();
        let saved_only = studio.rig_layout.with(|layout| {
            layout.as_ref().map_or_else(Vec::new, |layout| {
                saved_only_device_rows(
                    &scene.zones,
                    registry_ready.get().then_some(metas.as_slice()),
                    &layout.zones,
                )
            })
        });
        let active = studio.selected_surface_id.get().unwrap_or_default();
        search_terms.with(|terms| {
            build_pane(PaneInputs {
                zones: &scene.zones,
                active_zone_id: &active,
                devices: &metas,
                saved_only,
                search: &query.get(),
                terms,
            })
        })
    });

    view! {
        // Touch drags start here, so the pane listens for touchmove from
        // a touch's first move; see `DragController::on_touch_move`.
        <div
            class="flex h-full flex-col border-r border-edge-subtle/70 bg-surface-raised/40"
            on:touchmove=move |ev| drag.on_touch_move(&ev)
        >
            <PaneSearch search=search on_screens=on_screens />
            <div class="scrollbar-none flex-1 overflow-y-auto px-3 pb-3" data-pane-scroll="">
                {move || {
                    if on_screens.get() {
                        return view! { <ScreenList query=query /> }.into_any();
                    }
                    view! {
                        <ActiveZoneSection pane=pane lights=lights registry=registry />
                        <OtherZones pane=pane lights=lights registry=registry />
                        <UnassignedSection pane=pane lights=lights registry=registry />
                    }
                        .into_any()
                }}
            </div>
        </div>
    }
}

fn surface_for(lights: &[Surface], zone_id: &str) -> Option<Surface> {
    lights.iter().find(|surface| surface.id == zone_id).cloned()
}

fn summary_for(registry: &[DeviceSummary], row: &ZoneDeviceRow) -> Option<DeviceSummary> {
    registry
        .iter()
        .find(|device| device.layout_device_id == row.device_id)
        .cloned()
}

/// Section 1: the active zone. Its header names the zone and counts its
/// devices, sticks while the pane scrolls, and never collapses.
#[component]
fn ActiveZoneSection(
    pane: Memo<PaneModel>,
    lights: Memo<Vec<Surface>>,
    registry: Memo<Vec<DeviceSummary>>,
) -> impl IntoView {
    let drag = expect_context::<DragController>();
    let section = Memo::new(move |_| pane.with(|pane| pane.active.clone()));
    let searching = Memo::new(move |_| pane.with(|pane| pane.searching));
    let surface = Memo::new(move |_| {
        let section = section.get()?;
        lights.with(|lights| surface_for(lights, &section.zone_id))
    });
    let zone_id =
        Memo::new(move |_| section.with(|section| section.as_ref().map(|s| s.zone_id.clone())));
    let look = move || {
        zone_id
            .get()
            .map(|id| drag.look(&DropTarget::PaneZone(id)).section_class())
            .unwrap_or_default()
    };
    view! {
        <section
            class=move || format!("rounded-lg pb-3 transition-colors {}", look())
            aria-label="Devices in this zone"
            data-pane-zone=move || zone_id.get()
        >
            // The pane is translucent over the page base, so the sticky
            // header layers the same two surfaces to stay opaque.
            <div class="sticky top-0 z-10 -mx-3 bg-surface-base">
                <div class="flex items-center gap-2 bg-surface-raised/40 px-4 py-2">
                    {move || {
                        surface
                            .get()
                            .map(|surface| {
                                view! {
                                    <span
                                        class="h-2 w-2 shrink-0 rounded-full bg-fg-tertiary/45"
                                        style:background-color=surface.color.clone()
                                        aria-hidden="true"
                                    />
                                    <span class=format!(
                                        "{} min-w-0 truncate",
                                        label_class(LabelSize::Small, LabelTone::Default),
                                    )>{surface.name}</span>
                                }
                            })
                    }}
                    <span class="font-mono text-[10px] tabular-nums text-fg-tertiary/70">
                        {move || {
                            section
                                .get()
                                .map(|section| {
                                    section_count(section.hits(), section.total, searching.get())
                                })
                        }}
                    </span>
                </div>
            </div>
            <div class="space-y-1.5">
                {move || {
                    let Some(section) = section.get() else {
                        return ().into_any();
                    };
                    let zone_id = section.zone_id.clone();
                    let names = lights.get();
                    let devices = registry.get();
                    let empty = section.devices.is_empty() && section.offline.is_empty();
                    let empty_note = empty
                        .then(|| {
                            let note = if searching.get() {
                                "No devices here match"
                            } else {
                                "No devices yet. Add some from Unassigned below."
                            };
                            view! {
                                <div class="rounded-lg border border-dashed border-edge-subtle/45 px-3 py-4 text-center text-[11px] text-fg-tertiary/60">
                                    {note}
                                </div>
                            }
                        });
                    let cards = section
                        .devices
                        .iter()
                        .map(|entry| {
                            let elsewhere = entry
                                .elsewhere
                                .iter()
                                .map(|hint| Elsewhere {
                                    zone_name: surface_for(&names, &hint.zone_id)
                                        .map(|surface| surface.name)
                                        .unwrap_or_default(),
                                    zone_id: hint.zone_id.clone(),
                                    outputs: hint.outputs,
                                })
                                .collect::<Vec<_>>();
                            view! {
                                <StudioDeviceCard
                                    row=entry.row.clone()
                                    device=summary_for(&devices, &entry.row)
                                    select=zone_id.clone()
                                    elsewhere=elsewhere
                                />
                            }
                        })
                        .collect_view();
                    let offline = (!section.offline.is_empty())
                        .then(|| {
                            let count = section.offline.len();
                            let rows = section
                                .offline
                                .iter()
                                .map(|entry| {
                                    view! {
                                        <OfflineDeviceCard
                                            row=entry.row.clone()
                                            select=zone_id.clone()
                                            placed=true
                                        />
                                    }
                                })
                                .collect_view();
                            view! {
                                <div class="space-y-1.5 pt-2">
                                    <div class="flex items-center gap-1.5 px-1">
                                        <span class=label_class(
                                            LabelSize::Micro,
                                            LabelTone::Default,
                                        )>"Offline"</span>
                                        <span class="font-mono text-[9px] tabular-nums text-fg-tertiary/60">
                                            {count}
                                        </span>
                                    </div>
                                    {rows}
                                </div>
                            }
                        });
                    view! { {empty_note} {cards} {offline} }.into_any()
                }}
            </div>
        </section>
    }
}

/// Section 2: one collapsed row per other LED zone. A row click expands
/// it and never switches tabs; switching is the tab strip's job.
#[component]
fn OtherZones(
    pane: Memo<PaneModel>,
    lights: Memo<Vec<Surface>>,
    registry: Memo<Vec<DeviceSummary>>,
) -> impl IntoView {
    let others = Memo::new(move |_| pane.with(|pane| pane.others.clone()));
    let searching = Memo::new(move |_| pane.with(|pane| pane.searching));
    view! {
        <Show when=move || !others.with(Vec::is_empty)>
            <section class="space-y-1.5 border-t border-edge-subtle/45 py-3" aria-label="Other zones">
                <div class="px-1">
                    <span class=label_class(LabelSize::Small, LabelTone::Default)>"Other zones"</span>
                </div>
                {move || {
                    others
                        .get()
                        .into_iter()
                        .map(|section| {
                            let surface = lights.with(|lights| surface_for(lights, &section.zone_id));
                            view! {
                                <OtherZoneRow
                                    section=section
                                    surface=surface
                                    searching=searching.get()
                                    registry=registry
                                />
                            }
                        })
                        .collect_view()
                }}
            </section>
        </Show>
    }
}

/// One other zone: identity dot, name, and device count, expanding to
/// compact device rows. The row carries its zone id so a drag can drop
/// on it.
#[component]
fn OtherZoneRow(
    section: ZoneSection,
    surface: Option<Surface>,
    searching: bool,
    registry: Memo<Vec<DeviceSummary>>,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let drag = expect_context::<DragController>();
    let zone_id = StoredValue::new(section.zone_id.clone());
    let target = DropTarget::PaneZone(section.zone_id.clone());
    let look = move || drag.look(&target).section_class();
    let expanded = Memo::new(move |_| {
        searching
            || studio
                .pane_expanded
                .with(|expanded| zone_id.with_value(|id| expanded.contains(id)))
    });
    let name = surface
        .as_ref()
        .map(|surface| surface.name.clone())
        .unwrap_or_default();
    let color = surface.as_ref().and_then(|surface| surface.color.clone());
    let dimmed = surface.as_ref().is_some_and(|surface| !surface.enabled);
    let count = section_count(section.hits(), section.total, searching);
    let list_id = format!("pane-zone-list-{}", section.zone_id);
    let rows: Vec<ZoneDeviceRow> = section
        .devices
        .into_iter()
        .chain(section.offline)
        .map(|entry| entry.row)
        .collect();

    view! {
        <div
            id=pane_zone_dom_id(&section.zone_id)
            data-pane-zone=section.zone_id.clone()
            class=move || format!(
                "overflow-hidden rounded-lg border border-edge-subtle/60 bg-surface-overlay/30 transition-colors {}",
                look(),
            )
            class=("opacity-60", move || dimmed)
        >
            <button
                type="button"
                id=pane_zone_toggle_dom_id(&section.zone_id)
                class="flex w-full items-center gap-2 px-2.5 py-2 text-left transition-colors hover:bg-surface-hover/30"
                aria-expanded=move || expanded.get().to_string()
                aria-controls=list_id.clone()
                on:click=move |_| {
                    if searching {
                        return;
                    }
                    studio
                        .pane_expanded
                        .update(|expanded| {
                            let id = zone_id.get_value();
                            if !expanded.remove(&id) {
                                expanded.insert(id);
                            }
                        });
                    claim_pane_row(studio, &zone_id.get_value());
                }
            >
                <Icon
                    icon=LuChevronRight
                    width="12px"
                    height="12px"
                    {..}
                    class="shrink-0 text-fg-tertiary transition-transform duration-150"
                    class=("rotate-90", move || expanded.get())
                />
                <span
                    class="h-2 w-2 shrink-0 rounded-full bg-fg-tertiary/45"
                    style:background-color=color
                    aria-hidden="true"
                />
                <span class="min-w-0 flex-1 truncate text-[12px] font-medium text-fg-secondary">
                    {name}
                </span>
                <span class="shrink-0 font-mono text-[10px] tabular-nums text-fg-tertiary/70">
                    {count}
                </span>
            </button>
            <Show when=move || expanded.get()>
                <div
                    id=list_id.clone()
                    class="space-y-0.5 border-t border-edge-subtle/40 bg-surface-sunken/50 px-1.5 py-1.5"
                >
                    {if rows.is_empty() {
                        view! {
                            <div class="px-2 py-1 text-[10px] text-fg-tertiary/55">"No devices"</div>
                        }
                            .into_any()
                    } else {
                        rows.iter()
                            .map(|row| {
                                view! {
                                    <CompactDeviceRow
                                        row=row.clone()
                                        zone_id=zone_id.get_value()
                                        registry=registry
                                    />
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </div>
            </Show>
        </div>
    }
}

/// A one-line device row inside an Other zones row: identity strip, name,
/// and LED count. It drags out to move the device, and a click (or Enter)
/// opens "Move to" for it.
#[component]
fn CompactDeviceRow(
    row: ZoneDeviceRow,
    zone_id: String,
    registry: Memo<Vec<DeviceSummary>>,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let drag = expect_context::<DragController>();
    let device_id = row.device_id.clone();
    let accent = Memo::new(move |_| {
        registry.with(|registry| {
            registry
                .iter()
                .find(|device| device.layout_device_id == device_id)
                .map(|device| {
                    let brand = crate::components::device_card::classify_brand(device);
                    let (primary, secondary) = crate::components::device_card::brand_colors(&brand);
                    format!("background: linear-gradient(180deg, rgb({primary}), rgb({secondary}))")
                })
        })
    });
    let offline = !row.resolved;
    let label = if offline {
        super::device_card::friendly_offline_label(&row.device_id).to_owned()
    } else {
        row.name.clone()
    };
    let label_title = label.clone();
    let dom_id = compact_row_dom_id(&zone_id, &row.device_id);
    let payload = {
        let (zone_id, device_id, label) = (zone_id.clone(), row.device_id.clone(), label.clone());
        move || zone_device_payload(studio, &zone_id, &device_id, &label)
    };
    let menu_payload = payload.clone();
    let focus = FocusReturn {
        target: dom_id.clone(),
        fallback: Some(pane_zone_toggle_dom_id(&zone_id)),
    };
    let open_menu = move |element: &web_sys::Element| {
        let payload = menu_payload();
        drag.menu.set(Some(MoveMenuAnchor::below(
            element,
            vec![(payload.label.clone(), payload)],
            0,
            focus.clone(),
        )));
    };
    let key_menu = open_menu.clone();
    view! {
        <div
            id=dom_id
            role="button"
            tabindex="0"
            aria-haspopup="menu"
            title="Drag to move, or choose where to move it"
            class="flex cursor-grab touch-pan-y select-none items-center gap-2 rounded px-1.5 py-1 transition-colors hover:bg-surface-hover/30 [-webkit-touch-callout:none]"
            class=("opacity-60", move || offline)
            data-device-id=row.device_id.clone()
            data-zone-id=zone_id
            on:pointerdown=move |ev: web_sys::PointerEvent| drag.press(&ev, payload())
            on:click=move |ev: web_sys::MouseEvent| {
                if let Some(element) = listener_element(&ev) {
                    open_menu(&element);
                }
            }
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if matches!(ev.key().as_str(), "Enter" | " ") || is_menu_key(&ev) {
                    ev.prevent_default();
                    if let Some(element) = listener_element(&ev) {
                        key_menu(&element);
                    }
                }
            }
        >
            <span
                class="h-3.5 w-0.5 shrink-0 rounded-full bg-fg-tertiary/40"
                style=move || accent.get().unwrap_or_default()
            />
            <span class="min-w-0 flex-1 truncate text-[11px] text-fg-secondary" title=label_title>
                {label}
            </span>
            <span class="shrink-0 font-mono text-[9px] tabular-nums text-fg-tertiary/60">
                {row.led_count}
            </span>
        </div>
    }
}

/// Section 3: hardware no zone claims. Expanded whenever it has rows; the
/// header carries the scene's policy for those outputs.
#[component]
fn UnassignedSection(
    pane: Memo<PaneModel>,
    lights: Memo<Vec<Surface>>,
    registry: Memo<Vec<DeviceSummary>>,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let drag = expect_context::<DragController>();
    let section = Memo::new(move |_| pane.with(|pane| pane.unassigned.clone()));
    let searching = Memo::new(move |_| pane.with(|pane| pane.searching));
    let look = move || drag.look(&DropTarget::Unassigned).section_class();
    let visible = Memo::new(move |_| {
        let total = section.with(|section| section.total);
        let hits = section.with(|section| section.rows.len());
        shows_unassigned(lights.with(Vec::len), total) && (!searching.get() || hits > 0)
    });
    view! {
        <Show when=move || visible.get()>
            <section
                class=move || format!(
                    "space-y-1.5 rounded-lg border-t border-edge-subtle/45 py-3 transition-colors {}",
                    look(),
                )
                aria-label="Unassigned"
                data-pane-unassigned="true"
            >
                <div class="flex items-center gap-1.5 px-1">
                    <span class=label_class(LabelSize::Small, LabelTone::Default)>"Unassigned"</span>
                    <span class="font-mono text-[10px] tabular-nums text-fg-tertiary/70">
                        {move || {
                            section
                                .with(|section| {
                                    section_count(section.rows.len(), section.total, searching.get())
                                })
                        }}
                    </span>
                    <div class="flex-1" />
                    <UnassignedPolicy lights=lights />
                </div>
                {move || {
                    let rows = section.with(|section| section.rows.clone());
                    if rows.is_empty() {
                        return view! {
                            <div class="px-1 text-[10px] text-fg-tertiary/55">"Nothing unassigned"</div>
                        }
                            .into_any();
                    }
                    let target = studio.selected_surface_id.get().unwrap_or_default();
                    let devices = registry.get();
                    rows.into_iter()
                        .map(|row| {
                            match summary_for(&devices, &row) {
                                Some(device) => view! {
                                    <StudioDeviceCard
                                        row=row
                                        device=Some(device)
                                        select=target.clone()
                                        mode=CardMode::Unassigned
                                    />
                                }
                                    .into_any(),
                                None => view! {
                                    <OfflineDeviceCard row=row select=target.clone() placed=false />
                                }
                                    .into_any(),
                            }
                        })
                        .collect_view()
                        .into_any()
                }}
            </section>
        </Show>
    }
}

/// The scene's Unassigned policy as a compact select: Off, Hold last
/// colors, or Follow a zone. Read-only when the daemon does not advertise
/// `scene-unassigned-behavior-write`.
#[component]
fn UnassignedPolicy(lights: Memo<Vec<Surface>>) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let caps = expect_context::<CapabilitiesContext>();
    let writable = Memo::new(move |_| caps.has("scene-unassigned-behavior-write"));
    let value = Memo::new(move |_| {
        studio.active_scene.with(|scene| {
            scene
                .as_ref()
                .map(|scene| unassigned_policy_value(&scene.unassigned_behavior))
                .unwrap_or_default()
        })
    });
    let options = Memo::new(move |_| {
        let names: Vec<(String, String)> = lights.with(|lights| {
            lights
                .iter()
                .map(|surface| (surface.id.clone(), surface.name.clone()))
                .collect()
        });
        unassigned_policy_options(&names)
    });
    let label = Memo::new(move |_| {
        let current = value.get();
        options.with(|options| {
            options
                .iter()
                .find(|(option, _)| *option == current)
                .map(|(_, label)| label.clone())
                .unwrap_or_default()
        })
    });
    let on_change = Callback::new(move |picked: String| {
        let Some(behavior) = parse_unassigned_policy(&picked) else {
            toasts::toast_error("Unrecognized Unassigned option");
            return;
        };
        let Some(revision) = studio
            .active_scene
            .with_untracked(|scene| scene.as_ref().map(|scene| scene.revision))
        else {
            return;
        };
        spawn_local(async move {
            match api::zones::update_unassigned_behavior(&behavior, revision).await {
                Ok(ZoneOutcome::Applied(_)) => studio.refresh_scene.run(()),
                Ok(ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                    studio.refresh_scene.run(());
                }
                Err(error) => toasts::toast_error(&format!("Policy update failed: {error}")),
            }
        });
    });
    view! {
        <div class="w-32 min-w-0 shrink" title="What outputs in no zone show">
            {move || {
                if writable.get() {
                    view! {
                        <SilkSelect
                            value=Signal::derive(move || value.get())
                            options=Signal::derive(move || options.get())
                            on_change=on_change
                            class="rounded-md border border-edge-subtle/60 bg-surface-overlay/40 px-2 py-1 text-[11px] text-fg-secondary"
                        />
                    }
                        .into_any()
                } else {
                    view! {
                        <span class="block truncate text-right text-[11px] text-fg-tertiary">
                            {move || label.get()}
                        </span>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// The pane's search field. It owns the page search id, so the shell's
/// `/` shortcut lands here; focusing it on a narrow viewport opens the
/// drawer it lives in. Escape clears a term, then leaves the field.
#[component]
fn PaneSearch(search: RwSignal<String>, on_screens: Memo<bool>) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let placeholder = move || {
        if on_screens.get() {
            "Search screens"
        } else {
            "Search devices"
        }
    };
    view! {
        <div class="relative shrink-0 px-3 pb-2 pt-3">
            <span class="pointer-events-none absolute left-5.5 top-1/2 mt-0.5 -translate-y-1/2 text-fg-tertiary">
                <Icon icon=LuSearch width="13px" height="13px" />
            </span>
            <input
                id=PAGE_SEARCH_INPUT_ID
                type="text"
                placeholder=placeholder
                aria-label=placeholder
                class="w-full rounded-lg border border-edge-subtle bg-surface-overlay/60 py-1.5 pl-8 pr-8 text-[12px] text-fg-primary placeholder-fg-tertiary transition-[border-color,box-shadow] duration-300 focus:border-accent-muted focus:outline-none search-glow glow-ring"
                prop:value=move || search.get()
                on:input=move |ev| search.set(event_target_value(&ev))
                on:focus=move |_| {
                    // Below Tailwind's `lg` breakpoint the pane is a drawer.
                    let narrow = window()
                        .inner_width()
                        .ok()
                        .and_then(|width| width.as_f64())
                        .is_some_and(|width| width < 1024.0);
                    if narrow {
                        studio.pane_drawer.set(true);
                    }
                }
                on:keydown=move |ev: web_sys::KeyboardEvent| {
                    if ev.key() != "Escape" {
                        return;
                    }
                    ev.prevent_default();
                    if search.with_untracked(String::is_empty) {
                        if let Some(input) = ev
                            .target()
                            .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
                        {
                            let _ = input.blur();
                        }
                    } else {
                        search.set(String::new());
                    }
                }
            />
            <kbd class="pointer-events-none absolute right-5 top-1/2 mt-0.5 -translate-y-1/2 rounded border border-edge-subtle bg-surface-overlay/30 px-1.5 py-0.5 font-mono text-[9px] text-fg-tertiary">
                "/"
            </kbd>
        </div>
    }
}

/// Every zone id the "+" opens in the pane: all LED zones but the new one.
#[must_use]
pub fn other_led_zone_ids(zones: &[api::ZoneResource]) -> HashSet<String> {
    zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .map(|zone| zone.id.to_string())
        .collect()
}
