//! The device card for the Studio zone tree.
//!
//! Each physical device under a zone renders as a card carrying its
//! brand identity: a duotone accent strip, the vendor mark, the LED
//! count, and a per-channel breakdown with live component data. The
//! card body selects the parent zone; trailing actions hide every
//! output, identify the hardware, and remove it from the zone.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::spatial::SpatialLayout;

use crate::api::zones::ZoneOutcome;
use crate::api::{self, DeviceSummary, ZoneTopologySummary};
use crate::channel_names;
use crate::components::device_card::{
    brand_colors, brand_label, brand_vendor, classify_brand, classify_device, device_class_icon,
    driver_identifier_label, topology_shape_svg,
};
use crate::icons::*;
use crate::layout_utils;
use crate::toasts;
use crate::vendors::{VendorMark, VendorMarkSize};

use super::device_grouping::ZoneDeviceRow;
use super::surface::UNASSIGNED_SURFACE_ID;
use super::{StudioContext, hidden_outputs_storage_key};

/// Components a card lists before the rest collapse into a "+N" tail.
const MAX_COMPONENTS: usize = 5;

/// One row in the per-channel breakdown beneath the card body.
///
/// Captures the channel's identity — slot id, zone name, and display
/// name — so the component-binding match helper can surface live
/// attachment data against the channel's aliases, plus the output id
/// when the channel has an output in this zone so the row can be
/// hidden individually.
struct ComponentRow {
    /// User-facing channel label, identical to the row's display name.
    name: String,
    shape_svg: &'static str,
    led_count: usize,
    /// `Output.id` (`DeviceZone.id`) when this channel has an output
    /// in the current zone — `None` for the Unassigned bucket or a
    /// channel with no placed output, in which case the row offers no
    /// hide toggle.
    output_id: Option<String>,
    slot_id: String,
    zone_name: Option<String>,
    display_name: String,
}

/// One physical device under a zone. The card body selects the parent zone
/// (or the Unassigned entry) named by `select`; trailing actions identify
/// the hardware and, inside a real zone, remove it.
#[component]
pub fn StudioDeviceCard(
    row: ZoneDeviceRow,
    device: Option<DeviceSummary>,
    select: String,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let row_device_id = row.device_id.clone();

    let Some(device) = device else {
        // Offline or removed: still placed in the layout, but the device
        // registry has no entry — a muted row, no brand identity. It can
        // still be removed from the zone; it cannot be identified.
        let name = row.name;
        let leds = led_label(row.led_count);
        let select_body = select.clone();
        return view! {
            <div
                class="card-hover flex w-full items-center rounded-lg border border-dashed border-edge-subtle/45"
                title="Offline: placed in the layout but not connected"
            >
                <button
                    type="button"
                    class="flex min-w-0 flex-1 items-center gap-2 px-2.5 py-2 text-left"
                    on:click=move |_| studio.selected_surface_id.set(Some(select_body.clone()))
                >
                    <Icon
                        icon=LuCpu
                        width="12px"
                        height="12px"
                        style="color: rgba(139, 133, 160, 0.5)"
                    />
                    <span class="min-w-0 flex-1 truncate text-[11px] text-fg-tertiary/60">
                        {name}
                    </span>
                    <span class="shrink-0 font-mono text-[9px] tabular-nums text-fg-tertiary/45">
                        {leds}
                    </span>
                </button>
                {card_actions(studio, select, row_device_id, None, Vec::new(), None)}
            </div>
        }
        .into_any();
    };

    let brand = classify_brand(&device);
    let (primary, secondary) = brand_colors(&brand);
    let vendor = brand_vendor(&brand);
    // A daemon-reported display topology hint is the authoritative
    // "this is a screen" signal — and it carries the real resolution.
    let resolution = display_resolution(&device);
    let class_icon = if resolution.is_some() {
        LuMonitor
    } else {
        device_class_icon(&classify_device(&device))
    };
    // The driver label is the card's identity only when there is no
    // vendor mark to carry it.
    let driver_label = vendor.is_none().then(|| {
        brand_label(&brand).unwrap_or_else(|| {
            let id = if device.origin.driver_id.trim().is_empty() {
                device.origin.backend_id.as_str()
            } else {
                device.origin.driver_id.as_str()
            };
            driver_identifier_label(id).unwrap_or_else(|| id.to_owned())
        })
    });
    let transport = transport_label(&device.connection.transport);
    let device_name = device.name.clone();
    // A screen has no addressable LED tally — its layout topology is a
    // pixel grid. Show the real resolution; LED devices show the count.
    let leds = match resolution {
        Some((width, height)) => format!("{width} \u{d7} {height}"),
        None => led_label(row.led_count),
    };

    // Snapshot the current zone's layout once — used both to resolve
    // each channel's output id and to gather the bulk-hide id set.
    // The Unassigned bucket has no layout (its devices are by
    // definition placed in no zone), so the snapshot is `None` there.
    let in_zone = select != UNASSIGNED_SURFACE_ID;
    let layout_snapshot: Option<SpatialLayout> = if in_zone {
        studio.active_scene.with_untracked(|scene| {
            scene.as_ref().and_then(|scene| {
                scene
                    .groups
                    .iter()
                    .find(|group| group.id.to_string() == select)
                    .map(|group| group.layout.clone())
            })
        })
    } else {
        None
    };
    let scene_key: Option<String> = in_zone
        .then(|| {
            studio.active_scene.with_untracked(|scene| {
                scene
                    .as_ref()
                    .map(|scene| hidden_outputs_storage_key(&scene.id, &select))
            })
        })
        .flatten();
    let device_output_ids: Vec<String> = layout_snapshot
        .as_ref()
        .map(|layout| {
            layout
                .zones
                .iter()
                .filter(|output| output.device_id == device.layout_device_id)
                .map(|output| output.id.clone())
                .collect()
        })
        .unwrap_or_default();

    let total_components = device.zones.len();
    let component_rows: Vec<ComponentRow> = device
        .zones
        .iter()
        .take(MAX_COMPONENTS)
        .map(|channel| {
            let display_name =
                channel_names::effective_channel_name(&device.id, &channel.id, &channel.name);
            let output_id = layout_snapshot.as_ref().and_then(|layout| {
                layout_utils::representative_zone_id_for_device_slot(
                    layout,
                    &device.layout_device_id,
                    Some(channel.name.as_str()),
                )
            });
            ComponentRow {
                name: display_name.clone(),
                shape_svg: topology_shape_svg(&channel.topology),
                led_count: channel.led_count,
                output_id,
                slot_id: channel.id.clone(),
                zone_name: Some(channel.name.clone()),
                display_name,
            }
        })
        .collect();
    let remaining = total_components.saturating_sub(MAX_COMPONENTS);
    let show_components = total_components > 1;

    // Live component bindings for this device — fetched once per card
    // mount, shared via the Studio context cache so each row reads it
    // reactively without re-fetching.
    let physical_id_for_fetch = device.id.clone();
    Effect::new(move |_| {
        fetch_attachments_if_needed(studio, &physical_id_for_fetch);
    });
    let physical_id_for_rows = device.id.clone();

    // The duotone radial hero glow and ambient inner/outer glow that
    // give the card its brand identity — a flat linear wash reads as
    // generic chrome, not hardware.
    let card_style = format!(
        "border: 1px solid rgba({primary}, 0.24); \
         background: \
         radial-gradient(ellipse at 14% 0%, rgba({primary}, 0.24), transparent 60%), \
         radial-gradient(ellipse at 100% 6%, rgba({secondary}, 0.15), transparent 64%), \
         linear-gradient(160deg, rgba({primary}, 0.07), transparent 72%); \
         box-shadow: inset 0 0 20px rgba({primary}, 0.06), \
         0 0 14px rgba({primary}, 0.07), 0 1px 3px rgba(0, 0, 0, 0.28)"
    );
    let strip_style =
        format!("background: linear-gradient(180deg, rgb({primary}), rgb({secondary}))");
    let icon_style = format!("color: rgba({primary}, 0.9)");
    let list_style = format!("border-color: rgba({primary}, 0.12)");
    let shape_style = format!("color: rgba({primary}, 0.7)");

    let select_body = select.clone();
    let physical_id = device.id.clone();

    view! {
        <div class="card-hover w-full overflow-hidden rounded-lg" style=card_style>
            <div class="flex items-stretch">
                <button
                    type="button"
                    class="flex min-w-0 flex-1 items-stretch gap-2.5 px-2.5 py-2 text-left"
                    on:click=move |_| studio.selected_surface_id.set(Some(select_body.clone()))
                >
                    <div class="w-1 shrink-0 self-stretch rounded-full" style=strip_style />
                    <div class="min-w-0 flex-1 space-y-1">
                        <div class="flex items-center gap-1.5">
                            {match vendor {
                                Some(v) => {
                                    view! { <VendorMark vendor=v size=VendorMarkSize::Xs /> }
                                        .into_any()
                                }
                                None => {
                                    view! {
                                        <Icon
                                            icon=class_icon
                                            width="13px"
                                            height="13px"
                                            style=icon_style
                                        />
                                    }
                                        .into_any()
                                }
                            }}
                            <span class="min-w-0 flex-1 truncate text-[12px] font-medium text-fg-primary">
                                {device_name}
                            </span>
                        </div>
                        <div class="flex items-center gap-1.5 font-mono text-[10px] text-fg-tertiary/70">
                            <span class="tabular-nums">{leds}</span>
                            {transport
                                .map(|t| {
                                    view! {
                                        <>
                                            <span class="text-fg-tertiary/30">"\u{b7}"</span>
                                            <span>{t}</span>
                                        </>
                                    }
                                })}
                            {driver_label
                                .map(|label| {
                                    view! {
                                        <>
                                            <span class="text-fg-tertiary/30">"\u{b7}"</span>
                                            <span class="truncate uppercase tracking-wide">
                                                {label}
                                            </span>
                                        </>
                                    }
                                })}
                        </div>
                    </div>
                </button>
                {card_actions(
                    studio,
                    select,
                    row_device_id,
                    Some(physical_id),
                    device_output_ids,
                    scene_key.clone(),
                )}
            </div>
            {show_components
                .then(move || {
                    let scene_key = scene_key.clone();
                    view! {
                        <div class="space-y-0.5 border-t px-1.5 py-1.5" style=list_style>
                            {component_rows
                                .into_iter()
                                .map(|row| {
                                    component_row_view(
                                        studio,
                                        scene_key.clone(),
                                        physical_id_for_rows.clone(),
                                        row,
                                        shape_style.clone(),
                                    )
                                })
                                .collect_view()}
                            {(remaining > 0)
                                .then(|| {
                                    view! {
                                        <div class="pt-0.5 pl-6 text-[9px] text-fg-tertiary/45">
                                            {format!("+{remaining} more")}
                                        </div>
                                    }
                                })}
                        </div>
                    }
                })}
        </div>
    }
    .into_any()
}

/// The hide-all, identify, and remove cluster on the trailing edge of
/// a card. Hide-all toggles every output of this device in the current
/// zone in unison; identify flashes the hardware and is offered
/// whenever the device is online (`physical_id` is `Some`); remove
/// appears only inside a real zone, since the Unassigned entry has
/// nothing to remove from.
fn card_actions(
    studio: StudioContext,
    select: String,
    device_id: String,
    physical_id: Option<String>,
    output_ids: Vec<String>,
    scene_key: Option<String>,
) -> impl IntoView {
    let in_zone = select != UNASSIGNED_SURFACE_ID;
    // Hide-all is only meaningful when the card sits in a real zone
    // (so it has a scene_key) and the device actually owns outputs
    // there (otherwise there is nothing to toggle).
    let hide_all_pair: Option<(String, Vec<String>)> = scene_key
        .filter(|_| !output_ids.is_empty())
        .map(|key| (key, output_ids));
    view! {
        <div class="flex shrink-0 items-center gap-0.5 self-center pr-1.5">
            {hide_all_pair
                .map(|(key, ids)| {
                    let probe_key = key.clone();
                    let probe_ids = ids.clone();
                    let all_hidden = Signal::derive(move || {
                        studio.hidden_outputs.with(|map| {
                            map.get(&probe_key).is_some_and(|hidden| {
                                probe_ids.iter().all(|id| hidden.contains(id))
                            })
                        })
                    });
                    let toggle_key = key;
                    let toggle_ids = ids;
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary/55 transition-colors hover:text-fg-secondary"
                            title=move || {
                                if all_hidden.get() {
                                    "Show every output for this device"
                                } else {
                                    "Hide every output for this device"
                                }
                            }
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                let currently_all = all_hidden.get();
                                let key = toggle_key.clone();
                                let ids = toggle_ids.clone();
                                studio
                                    .hidden_outputs
                                    .update(|map| {
                                        let entry = map.entry(key).or_default();
                                        for id in ids {
                                            if currently_all {
                                                entry.remove(&id);
                                            } else {
                                                entry.insert(id);
                                            }
                                        }
                                    });
                            }
                        >
                            {move || {
                                if all_hidden.get() {
                                    view! { <Icon icon=LuEyeOff width="12px" height="12px" /> }
                                        .into_any()
                                } else {
                                    view! { <Icon icon=LuEye width="12px" height="12px" /> }
                                        .into_any()
                                }
                            }}
                        </button>
                    }
                })}
            {physical_id
                .map(|id| {
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary/55 transition-colors hover:text-fg-secondary"
                            title="Identify (flash the hardware)"
                            on:click=move |_| identify_device_now(&id)
                        >
                            <Icon icon=LuZap width="12px" height="12px" />
                        </button>
                    }
                })}
            {in_zone
                .then(|| {
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-6 w-6 items-center justify-center rounded-md transition-colors"
                            style="color: rgba(255, 99, 99, 0.5)"
                            title="Remove from this zone"
                            on:click=move |_| {
                                remove_device_from_zone(studio, select.clone(), device_id.clone());
                            }
                        >
                            <Icon icon=LuTrash2 width="12px" height="12px" />
                        </button>
                    }
                })}
        </div>
    }
}

/// Render one per-channel row beneath the card body.
///
/// Each row shows the channel's topology shape and name, a live
/// component badge listing what is bound to this channel, the channel's
/// LED count, and a per-output hide toggle when the channel has an
/// output in the current zone. Hidden state is per-(scene, zone) client
/// UI state, not the daemon's `layout_auto_exclusions`.
fn component_row_view(
    studio: StudioContext,
    scene_key: Option<String>,
    physical_device_id: String,
    row: ComponentRow,
    shape_style: String,
) -> impl IntoView {
    let ComponentRow {
        name,
        shape_svg,
        led_count,
        output_id,
        slot_id,
        zone_name,
        display_name,
    } = row;

    // Bindings that target THIS channel, filtered through the alias
    // helper the palette card uses so slot id, zone name, and display
    // name are all accepted as matches.
    let binding_device = physical_device_id;
    let binding_slot_id = slot_id;
    let binding_zone_name = zone_name;
    let binding_display = display_name;
    let bindings = Signal::derive(move || {
        studio.attachment_cache.with(|cache| {
            cache
                .get(&binding_device)
                .map(|all| {
                    all.iter()
                        .filter(|binding| {
                            layout_utils::attachment_binding_matches_slot_alias(
                                &binding.slot_id,
                                Some(&binding_slot_id),
                                binding_zone_name.as_deref(),
                                &binding_display,
                            )
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });

    // The eye toggle only renders when both the scene_key and the
    // output id are known — i.e. the row sits in a real zone and the
    // channel has a placed output to hide.
    let hide_pair: Option<(String, String)> = match (&scene_key, &output_id) {
        (Some(key), Some(id)) => Some((key.clone(), id.clone())),
        _ => None,
    };
    let row_probe_pair = hide_pair.clone();
    let is_hidden = Signal::derive(move || {
        let Some((key, id)) = row_probe_pair.as_ref() else {
            return false;
        };
        studio
            .hidden_outputs
            .with(|map| map.get(key).is_some_and(|hidden| hidden.contains(id)))
    });

    view! {
        <div class="flex items-center gap-2 px-1 py-1">
            <div
                class="h-3 w-3 shrink-0"
                style=shape_style
                inner_html=format!(
                    r#"<svg viewBox="0 0 16 16" width="12" height="12">{shape_svg}</svg>"#,
                )
            />
            <div class="min-w-0 flex-1">
                <div class="truncate text-[10px] text-fg-tertiary">{name}</div>
                {move || {
                    let bindings = bindings.get();
                    if bindings.is_empty() {
                        return None;
                    }
                    let total: u32 = bindings.iter().map(|b| b.instances.max(1)).sum();
                    let label = if total == 1 {
                        bindings[0]
                            .name
                            .clone()
                            .unwrap_or_else(|| bindings[0].template_name.clone())
                    } else {
                        format!("{total} components")
                    };
                    let title = bindings
                        .iter()
                        .map(|b| {
                            let name = b
                                .name
                                .clone()
                                .unwrap_or_else(|| b.template_name.clone());
                            if b.instances > 1 {
                                format!(
                                    "{name} \u{d7}{} ({} LEDs)",
                                    b.instances, b.effective_led_count,
                                )
                            } else {
                                format!("{name} ({} LEDs)", b.effective_led_count)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    Some(
                        view! {
                            <span
                                class="mt-0.5 inline-flex max-w-[140px] items-center gap-1 truncate rounded px-1 py-0.5 font-mono text-[8px]"
                                style="color: rgb(128, 255, 234); background: rgba(128, 255, 234, 0.08); border: 1px solid rgba(128, 255, 234, 0.12)"
                                title=title
                            >
                                <Icon icon=LuCable width="8px" height="8px" />
                                <span class="truncate">{label}</span>
                            </span>
                        },
                    )
                }}
            </div>
            <span class="shrink-0 font-mono text-[9px] tabular-nums text-fg-tertiary/55">
                {led_count}
            </span>
            {hide_pair
                .map(|(key, id)| {
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-5 w-5 shrink-0 items-center justify-center rounded text-fg-tertiary/50 transition-colors hover:text-fg-secondary"
                            title=move || {
                                if is_hidden.get() { "Show output" } else { "Hide output" }
                            }
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                let key = key.clone();
                                let id = id.clone();
                                studio
                                    .hidden_outputs
                                    .update(|map| {
                                        let entry = map.entry(key).or_default();
                                        if !entry.remove(&id) {
                                            entry.insert(id);
                                        }
                                    });
                            }
                        >
                            {move || {
                                if is_hidden.get() {
                                    view! { <Icon icon=LuEyeOff width="10px" height="10px" /> }
                                        .into_any()
                                } else {
                                    view! { <Icon icon=LuEye width="10px" height="10px" /> }
                                        .into_any()
                                }
                            }}
                        </button>
                    }
                })}
        </div>
    }
}

/// Lazily populate the Studio-wide attachment cache for one device.
///
/// Cards trigger this once on mount. The check on the cache lets the
/// same device's card unmount and remount (zone collapsed and reopened)
/// without re-fetching, and lets every card across the tree share one
/// cache entry per physical device.
fn fetch_attachments_if_needed(studio: StudioContext, physical_device_id: &str) {
    if physical_device_id.is_empty() {
        return;
    }
    if studio
        .attachment_cache
        .with_untracked(|cache| cache.contains_key(physical_device_id))
    {
        return;
    }
    let id = physical_device_id.to_owned();
    spawn_local(async move {
        if let Ok(profile) = api::fetch_device_attachments(&id).await {
            studio.attachment_cache.update(|cache| {
                cache.insert(id, profile.bindings);
            });
        }
    });
}

/// Flash a device's LEDs so the user can locate it physically.
fn identify_device_now(device_id: &str) {
    let device_id = device_id.to_owned();
    spawn_local(async move {
        match api::identify_device(&device_id).await {
            Ok(()) => toasts::toast_success("Flashing device"),
            Err(error) => toasts::toast_error(&format!("Identify failed: {error}")),
        }
    });
}

/// Unassign every output this device has in `zone_id`. The removals run in
/// sequence because each one bumps `groups_revision`; threading the new
/// revision into the next call lets a multi-output controller leave the
/// zone in a single user action.
fn remove_device_from_zone(studio: StudioContext, zone_id: String, device_id: String) {
    let Some(scene) = studio.active_scene.get_untracked() else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let output_ids: Vec<String> = scene
        .groups
        .iter()
        .find(|group| group.id.to_string() == zone_id)
        .map(|group| {
            group
                .layout
                .zones
                .iter()
                .filter(|output| output.device_id == device_id)
                .map(|output| output.id.clone())
                .collect()
        })
        .unwrap_or_default();
    if output_ids.is_empty() {
        return;
    }
    let scene_id = scene.id.clone();
    let mut revision = scene.groups_revision;
    spawn_local(async move {
        for output_id in output_ids {
            match api::zones::unassign_device(&scene_id, &zone_id, &output_id, Some(revision)).await
            {
                Ok(ZoneOutcome::Applied(next)) => revision = next,
                Ok(ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere — reloaded, try again");
                    studio.refresh_scene.run(());
                    return;
                }
                Err(error) => {
                    toasts::toast_error(&format!("Remove failed: {error}"));
                    studio.refresh_scene.run(());
                    return;
                }
            }
        }
        toasts::toast_success("Device removed from zone");
        studio.refresh_scene.run(());
    });
}

/// Group a number's digits in threes: `230400` → `"230,400"`.
fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char::from(*byte));
    }
    out
}

/// A device's display resolution in pixels, from the first zone the
/// daemon tags with a `Display` topology hint — `None` for an ordinary
/// LED device, whose zones are strips, rings, and matrices.
fn display_resolution(device: &DeviceSummary) -> Option<(u32, u32)> {
    device
        .zones
        .iter()
        .find_map(|zone| match zone.topology_hint {
            Some(ZoneTopologySummary::Display { width, height, .. }) => Some((width, height)),
            _ => None,
        })
}

/// "1 LED" / "1,406 LEDs".
fn led_label(count: u32) -> String {
    if count == 1 {
        "1 LED".to_owned()
    } else {
        format!("{} LEDs", group_digits(u64::from(count)))
    }
}

/// Short transport name for the card's meta line, or `None` to omit it.
fn transport_label(transport: &str) -> Option<&'static str> {
    match transport.trim() {
        "network" => Some("Network"),
        "usb" => Some("USB"),
        "smbus" => Some("SMBus"),
        "bridge" => Some("Bridge"),
        "midi" => Some("MIDI"),
        "serial" => Some("Serial"),
        _ => None,
    }
}
