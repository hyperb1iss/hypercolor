//! Offline device rollup — zones in the layout whose backing device is not currently connected.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::icons::*;
use crate::style_utils::device_accent_colors;

use super::PaletteState;

pub(super) fn render_offline_devices_section(state: PaletteState) -> Option<AnyView> {
    let devices = state.stable_devices.get();
    let connected_ids: std::collections::HashSet<String> =
        devices.iter().map(|d| d.layout_device_id.clone()).collect();

    let layout = state.layout;
    let hidden_zones = state.hidden_zones;
    let set_layout = state.set_layout;
    let set_selected_zone_ids = state.set_selected_zone_ids;
    let set_is_dirty = state.set_is_dirty;
    let set_hidden_zones = state.set_hidden_zones;

    // Collect unique offline device IDs from the layout
    let offline_devices: Vec<(String, Vec<hypercolor_types::spatial::DeviceZone>)> =
        layout.with(|current| {
            let Some(l) = current.as_ref() else {
                return Vec::new();
            };
            let mut by_device: std::collections::BTreeMap<
                String,
                Vec<hypercolor_types::spatial::DeviceZone>,
            > = std::collections::BTreeMap::new();
            for zone in &l.zones {
                if !connected_ids.contains(&zone.device_id) {
                    by_device
                        .entry(zone.device_id.clone())
                        .or_default()
                        .push(zone.clone());
                }
            }
            by_device.into_iter().collect()
        });

    if offline_devices.is_empty() {
        return None;
    }

    Some(view! {
        <>
            <div class="h-px bg-edge-subtle" />
            <div class="space-y-2">
                <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/60 flex items-center gap-1.5">
                    <Icon icon=LuWifi width="12px" height="12px" />
                    "Offline"
                </h3>
                <div class="space-y-1.5">
                    {offline_devices.into_iter().map(|(device_id, zones)| {
                        let (primary_rgb, secondary_rgb) = device_accent_colors(&device_id);
                        let zone_count = zones.len();
                        let total_leds: u32 = zones.iter().map(|z| z.topology.led_count()).sum();
                        let device_name = zones[0].name.clone();
                        let did_remove = device_id.clone();
                        let did_vis = device_id.clone();

                        // Device-level visibility for offline device
                        let offline_all_hidden = {
                            let did = device_id.clone();
                            Signal::derive(move || {
                                let hidden = hidden_zones.get();
                                layout.with(|current| {
                                    current.as_ref().map(|l| {
                                        let dzones: Vec<_> = l.zones.iter().filter(|z| z.device_id == did).collect();
                                        !dzones.is_empty() && dzones.iter().all(|z| hidden.contains(&z.id))
                                    }).unwrap_or(false)
                                })
                            })
                        };

                        view! {
                            <div
                                class="rounded-lg overflow-hidden"
                                style=format!(
                                    "border: 1px solid rgba({primary_rgb}, 0.12); \
                                     background: linear-gradient(135deg, rgba({primary_rgb}, 0.03), rgba({secondary_rgb}, 0.01)); \
                                     opacity: 0.7"
                                )
                            >
                                <div class="flex items-center gap-2 px-2.5 py-2">
                                    <div
                                        class="w-1 self-stretch rounded-full shrink-0 opacity-40"
                                        style=format!("background: linear-gradient(180deg, rgb({primary_rgb}), rgb({secondary_rgb}))")
                                    />
                                    <div class="flex-1 min-w-0">
                                        <div class="flex items-center gap-1.5">
                                            <span class="text-[11px] font-medium text-fg-secondary truncate">{device_name}</span>
                                            <span class="text-[8px] font-mono uppercase tracking-wider px-1 py-0.5 rounded border shrink-0"
                                                style="color: rgba(255, 99, 99, 0.6); border-color: rgba(255, 99, 99, 0.15); background: rgba(255, 99, 99, 0.04)"
                                            >"offline"</span>
                                        </div>
                                        <div class="text-[10px] text-fg-tertiary/60 font-mono mt-0.5">
                                            {total_leds} " LEDs"
                                            {(zone_count > 1).then(|| view! {
                                                <><span class="opacity-40">" · "</span>{zone_count}" zones"</>
                                            })}
                                        </div>
                                    </div>
                                    // Visibility toggle
                                    {move || {
                                        let did = did_vis.clone();
                                        let all_hidden = offline_all_hidden.get();
                                        Some(view! {
                                            <button
                                                class="w-6 h-6 flex items-center justify-center rounded-md transition-all shrink-0 btn-press"
                                                style=if all_hidden {
                                                    "color: var(--color-text-tertiary); opacity: 0.3"
                                                } else {
                                                    "color: var(--color-text-tertiary); opacity: 0.5"
                                                }
                                                title=if all_hidden { "Show device" } else { "Hide device" }
                                                on:click=move |ev: web_sys::MouseEvent| {
                                                    ev.stop_propagation();
                                                    let zone_ids: Vec<String> = layout.with_untracked(|current| {
                                                        current.as_ref().map(|l| {
                                                            l.zones.iter().filter(|z| z.device_id == did)
                                                                .map(|z| z.id.clone()).collect()
                                                        }).unwrap_or_default()
                                                    });
                                                    set_hidden_zones.update(|set| {
                                                        for zid in &zone_ids {
                                                            if all_hidden { set.remove(zid); } else { set.insert(zid.clone()); }
                                                        }
                                                    });
                                                }
                                            >
                                                {if all_hidden {
                                                    view! { <Icon icon=LuEyeOff width="12px" height="12px" /> }.into_any()
                                                } else {
                                                    view! { <Icon icon=LuEye width="12px" height="12px" /> }.into_any()
                                                }}
                                            </button>
                                        })
                                    }}
                                    // Remove all zones for this offline device
                                    <button
                                        class="w-6 h-6 flex items-center justify-center rounded-md transition-all shrink-0 btn-press"
                                        style="color: rgba(255, 99, 99, 0.4)"
                                        title="Remove all zones for this device"
                                        on:click=move |ev| {
                                            ev.stop_propagation();
                                            let did = did_remove.clone();
                                            set_layout.update(|l| {
                                                if let Some(layout) = l {
                                                    layout.zones.retain(|z| z.device_id != did);
                                                }
                                            });
                                            set_selected_zone_ids.set(std::collections::HashSet::new());
                                            set_is_dirty.set(true);
                                        }
                                    >
                                        <Icon icon=LuTrash2 width="12px" height="12px" />
                                    </button>
                                </div>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </div>
        </>
    }.into_any())
}
