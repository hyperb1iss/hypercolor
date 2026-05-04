//! Device detail sidebar — hardware spec sheet with channels and components.

use std::time::Duration;

use hypercolor_leptos_ext::events::Input;
use hypercolor_leptos_ext::prelude::sleep;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_throttle_fn_with_arg;

use crate::api::{self, DeviceAuthState};
use crate::app::DevicesContext;
use crate::components::attachment_panel::WiringPanel;
use crate::components::device_card::{
    brand_colors, brand_label, brand_vendor, classify_brand, driver_identifier_label,
};
use crate::components::device_driver_controls::DeviceDriverControls;
use crate::components::device_pairing_modal::needs_pairing;
use crate::icons::*;
use crate::toasts;
use crate::vendors::{VendorMark, VendorMarkSize};

/// Device detail sidebar.
#[component]
pub fn DeviceDetail(
    #[prop(into)] device_id: Signal<String>,
    #[prop(into)] on_pair: Callback<String>,
    #[prop(into)] on_forget: Callback<String>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    let device = Memo::new(move |_| {
        let id = device_id.get();
        ctx.devices_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|devices| devices.into_iter().find(|d| d.id == id))
    });

    let (editing_name, set_editing_name) = signal(false);
    let (name_input, set_name_input) = signal(String::new());
    let (identify_active, set_identify_active) = signal(false);
    let (device_brightness, set_device_brightness) = signal(100_u8);
    let device_signal = Signal::derive(move || device.get());

    Effect::new(move |_| {
        if let Some(dev) = device.get() {
            set_device_brightness.set(dev.brightness);
        }
    });

    let save_name = move || {
        let id = device_id.get();
        let Some(current_device) = device.get() else {
            set_editing_name.set(false);
            return;
        };
        let new_name = name_input.get();
        let next_name = new_name.trim().to_string();
        if next_name.is_empty() {
            set_editing_name.set(false);
            return;
        }
        let previous_name = current_device.name.clone();
        let layout_device_id = current_device.layout_device_id.clone();
        set_editing_name.set(false);
        let devices_resource = ctx.devices_resource;
        leptos::task::spawn_local(async move {
            let req = api::UpdateDeviceRequest {
                name: Some(next_name.clone()),
                enabled: None,
                brightness: None,
            };
            if api::update_device(&id, &req).await.is_ok()
                && let Ok(mut layout) = api::fetch_active_layout().await
            {
                let layout_id = layout.id.clone();
                if crate::layout_utils::sync_device_display_name_in_layout(
                    &mut layout,
                    &layout_device_id,
                    &previous_name,
                    &next_name,
                ) {
                    let _ = api::update_layout(
                        &layout_id,
                        &api::UpdateLayoutApiRequest {
                            name: None,
                            description: None,
                            canvas_width: None,
                            canvas_height: None,
                            zones: Some(layout.zones),
                        },
                    )
                    .await;
                    let _ = api::apply_layout(&layout_id).await;
                }
            }
            devices_resource.refetch();
        });
    };

    let toggle_enabled = move || {
        let Some(dev) = device.get() else { return };
        let id = dev.id.clone();
        let currently_active = dev.status != "disabled";
        let devices_resource = ctx.devices_resource;
        leptos::task::spawn_local(async move {
            let req = api::UpdateDeviceRequest {
                name: None,
                enabled: Some(!currently_active),
                brightness: None,
            };
            let _ = api::update_device(&id, &req).await;
            devices_resource.refetch();
            if currently_active {
                toasts::toast_info("Device disabled");
            } else {
                toasts::toast_success("Device enabled");
            }
        });
    };

    let push_brightness = use_throttle_fn_with_arg(
        move |brightness: u8| {
            let Some(dev) = device.get_untracked() else {
                return;
            };
            let id = dev.id.clone();
            let devices_resource = ctx.devices_resource;
            leptos::task::spawn_local(async move {
                let req = api::UpdateDeviceRequest {
                    name: None,
                    enabled: None,
                    brightness: Some(brightness),
                };
                if let Err(error) = api::update_device(&id, &req).await {
                    toasts::toast_error(&format!("Brightness failed: {error}"));
                    devices_resource.refetch();
                }
            });
        },
        50.0,
    );

    let identify = move || {
        let id = device_id.get();
        set_identify_active.set(true);
        leptos::task::spawn_local(async move {
            if let Err(error) = api::identify_device(&id).await {
                set_identify_active.set(false);
                toasts::toast_error(&format!("Identify failed: {error}"));
                return;
            }
            toasts::toast_success("Flashing device");
            sleep(Duration::from_secs(3)).await;
            set_identify_active.set(false);
        });
    };

    view! {
        <div class="space-y-2.5">
            {move || device.get().map(|dev| {
                let brand = classify_brand(&dev);
                let (rgb, secondary_rgb) = brand_colors(&brand);
                let vendor_label = brand_label(&brand);
                let vendor = brand_vendor(&brand);
                let driver_label = vendor_label.clone().unwrap_or_else(|| {
                    let identifier = if dev.origin.driver_id.trim().is_empty() {
                        &dev.origin.backend_id
                    } else {
                        &dev.origin.driver_id
                    };
                    driver_identifier_label(identifier).unwrap_or_else(|| identifier.to_string())
                });
                let route_backend = dev.origin.backend_id.clone();
                let route_label = (!dev.origin.driver_id.trim().is_empty()
                    && route_backend != dev.origin.driver_id
                    && !route_backend.trim().is_empty())
                .then(|| {
                    driver_identifier_label(&route_backend).unwrap_or_else(|| route_backend.clone())
                });
                let rgb_for_border = rgb.clone();
                let rgb_for_slider = rgb.clone();
                let rgb_for_identify = rgb.clone();
                let dot_rgb = match dev.status.as_str() {
                    "active" => "80, 250, 123",
                    "connected" => "130, 170, 255",
                    "disabled" => "255, 99, 99",
                    _ => "139, 133, 160",
                };
                let enabled = dev.status != "disabled";
                let pairing_required = needs_pairing(&dev.auth);
                let auth_state = dev.auth.as_ref().map(|a| a.state.clone());
                let can_pair = dev.auth.as_ref().map(|a| a.can_pair).unwrap_or(false);
                let dev_id_for_pair = dev.id.clone();
                let dev_id_for_forget = dev.id.clone();
                let descriptor = dev.auth.as_ref().and_then(|a| a.descriptor.clone());
                let last_error = dev.auth.as_ref().and_then(|a| a.last_error.clone());
                let dev_name_for_edit = dev.name.clone();
                let push_brightness = push_brightness.clone();
                let hero_bg = format!(
                    "background: \
                     radial-gradient(ellipse at 15% 0%, rgba({rgb_for_border}, 0.32) 0%, transparent 55%), \
                     radial-gradient(ellipse at 95% 15%, rgba({secondary_rgb}, 0.22) 0%, transparent 60%), \
                     linear-gradient(180deg, rgba({rgb_for_border}, 0.10) 0%, transparent 58%)"
                );

                let zone_count = dev.zones.len();
                let connection_endpoint = dev.connection.endpoint.clone();
                let is_network = dev.connection.transport == "network";

                view! {
                    // ── Header: Driver chip + Name + status + actions ─────────
                    // No border-top accent strip — the hero gradient does the branding.
                    <div class="relative rounded-xl bg-surface-raised border border-edge-subtle/60 overflow-hidden"
                         style:--glow-rgb=rgb.clone()
                         style=format!("box-shadow: 0 0 18px rgba({rgb_for_border}, 0.08), inset 0 1px 0 rgba(255,255,255,0.03)")>
                        // Hero duotone wash
                        <div class="absolute inset-0 pointer-events-none rounded-xl" style=hero_bg />
                        // Cross-hatch for depth
                        <div class="absolute inset-0 pointer-events-none rounded-xl opacity-40"
                             style="background-image: repeating-linear-gradient(135deg, rgba(255,255,255,0.015) 0px, rgba(255,255,255,0.015) 1px, transparent 1px, transparent 6px)" />

                        <div class="relative px-4 py-3">
                            // Brand mark + driver label — status lives in the dot next to the name
                            {(vendor.is_some() || vendor_label.is_some()).then(|| {
                                let chip_rgb = rgb.clone();
                                let label = vendor_label.clone();
                                view! {
                                    <div class="mb-2 flex items-center gap-2">
                                        {vendor.map(|v| view! {
                                            <VendorMark vendor=v size=VendorMarkSize::Sm />
                                        })}
                                        {label.map(|label| view! {
                                            <span class="text-[10px] font-mono font-bold tracking-[0.16em]"
                                                  style=format!("color: rgba({chip_rgb}, 0.9)")>
                                                {label}
                                            </span>
                                        })}
                                    </div>
                                }
                            })}

                            <div class="flex items-center gap-2.5 mb-2">
                                <div class="w-2 h-2 rounded-full shrink-0 dot-alive"
                                     style=format!("background: rgb({dot_rgb}); box-shadow: 0 0 10px rgba({dot_rgb}, 0.7)") />
                                {move || if editing_name.get() {
                                    view! {
                                        <input
                                            type="text"
                                            class="flex-1 bg-surface-overlay border border-edge-subtle rounded px-2 py-0.5 text-sm text-fg-primary
                                                   focus:outline-none focus:border-accent-muted"
                                            prop:value=move || name_input.get()
                                            on:input=move |ev| {
                                                let event = Input::from_event(ev);
                                                if let Some(value) = event.value_string() {
                                                    set_name_input.set(value);
                                                }
                                            }
                                            on:keydown=move |ev| {
                                                if ev.key() == "Enter" { save_name(); }
                                                if ev.key() == "Escape" { set_editing_name.set(false); }
                                            }
                                            on:blur=move |_| save_name()
                                        />
                                    }.into_any()
                                } else {
                                    let name = dev_name_for_edit.clone();
                                    view! {
                                        <span
                                            class="text-sm font-medium text-fg-primary cursor-pointer hover:text-accent transition-colors group flex items-center gap-1.5"
                                            on:click=move |_| {
                                                set_name_input.set(name.clone());
                                                set_editing_name.set(true);
                                            }
                                        >
                                            {dev.name.clone()}
                                            <span class="w-3 h-3 text-fg-tertiary opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
                                                <Icon icon=LuPencil width="12px" height="12px" />
                                            </span>
                                        </span>
                                    }.into_any()
                                }}
                            </div>

                            <div class="flex items-center gap-2 text-[10px] font-mono text-fg-tertiary/65 mb-3">
                                <span>{driver_label}</span>
                                {route_label.clone().map(|backend| view! {
                                    <span class="text-fg-tertiary/30">{"\u{b7}"}</span>
                                    <span>"via " {backend}</span>
                                })}
                                {dev.firmware_version.clone().map(|fw| view! {
                                    <span class="text-fg-tertiary/30">{"\u{b7}"}</span>
                                    <span>"v" {fw}</span>
                                })}
                            </div>

                            // ── Stats chips: LEDs · channels · connection ──
                            <div class="flex items-center gap-1.5 flex-wrap mb-3">
                                <span class="inline-flex items-center gap-1 text-[10px] font-mono px-1.5 py-0.5 rounded-md"
                                      style=format!(
                                          "color: rgba({rgb_for_border}, 0.9); \
                                           background: rgba({rgb_for_border}, 0.10); \
                                           border: 1px solid rgba({rgb_for_border}, 0.22)"
                                      )>
                                    <span class="font-bold tabular-nums">{dev.total_leds}</span>
                                    <span class="opacity-70">"LEDs"</span>
                                </span>
                                <span class="inline-flex items-center gap-1 text-[10px] font-mono px-1.5 py-0.5 rounded-md
                                             text-fg-tertiary bg-surface-overlay/25 border border-edge-subtle/35">
                                    <span class="font-bold tabular-nums text-fg-secondary">{zone_count}</span>
                                    <span>{if zone_count == 1 { "channel" } else { "channels" }}</span>
                                </span>
                                {connection_endpoint.clone().map(|ep| {
                                    let ep_title = ep.clone();
                                    view! {
                                        <span class="inline-flex items-center gap-1 text-[10px] font-mono px-1.5 py-0.5 rounded-md
                                                     text-fg-tertiary bg-surface-overlay/25 border border-edge-subtle/35 max-w-[180px]"
                                              title=ep_title>
                                            <Icon icon={if is_network { LuGlobe } else { LuCable }}
                                                  width="10px" height="10px"
                                                  style=format!("color: rgba({rgb_for_border}, 0.7)") />
                                            <span class="truncate">{ep}</span>
                                        </span>
                                    }
                                })}
                            </div>

                            // ── Pairing panel ────────────────────────────────
                            {match auth_state.as_ref() {
                                Some(DeviceAuthState::Required) if can_pair => {
                                    let pair_id = dev_id_for_pair.clone();
                                    let rgb_pair = rgb.clone();
                                    let desc_title = descriptor.as_ref().map(|d| d.title.clone());
                                    Some(view! {
                                        <div class="mb-3 px-3 py-2.5 rounded-lg border"
                                             style="background: rgba(255, 183, 77, 0.05); border-color: rgba(255, 183, 77, 0.12)">
                                            <div class="flex items-center gap-2 mb-1.5">
                                                <Icon icon=LuKeyRound width="12px" height="12px" style="color: rgba(255, 183, 77, 0.7)" />
                                                <span class="text-[11px] font-medium" style="color: rgba(255, 183, 77, 0.8)">
                                                    {desc_title.unwrap_or_else(|| "Pairing required".to_string())}
                                                </span>
                                            </div>
                                            <p class="text-[10px] text-fg-tertiary/60 mb-2">
                                                "This device requires pairing before it can be controlled."
                                            </p>
                                            <button
                                                class="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-[10px] font-medium transition-all btn-press"
                                                style=format!(
                                                    "background: rgba({rgb_pair}, 0.1); color: rgb({rgb_pair}); border: 1px solid rgba({rgb_pair}, 0.15)"
                                                )
                                                on:click=move |_| on_pair.run(pair_id.clone())
                                            >
                                                <Icon icon=LuZap width="10px" height="10px" />
                                                "Start pairing"
                                            </button>
                                        </div>
                                    }.into_any())
                                }
                                Some(DeviceAuthState::Error) => {
                                    let pair_id = dev_id_for_pair.clone();
                                    let forget_id = dev_id_for_forget.clone();
                                    let rgb_pair = rgb.clone();
                                    let err_msg = last_error.clone();
                                    Some(view! {
                                        <div class="mb-3 px-3 py-2.5 rounded-lg border"
                                             style="background: rgba(255, 99, 99, 0.05); border-color: rgba(255, 99, 99, 0.12)">
                                            <div class="flex items-center gap-2 mb-1">
                                                <Icon icon=LuTriangleAlert width="12px" height="12px" style="color: rgba(255, 99, 99, 0.7)" />
                                                <span class="text-[11px] font-medium" style="color: rgba(255, 99, 99, 0.8)">
                                                    "Authentication error"
                                                </span>
                                            </div>
                                            {err_msg.map(|msg| view! {
                                                <p class="text-[10px] text-fg-tertiary/50 mb-2">{msg}</p>
                                            })}
                                            <div class="flex items-center gap-2">
                                                {can_pair.then(|| {
                                                    let pair_id = pair_id.clone();
                                                    let rgb_pair = rgb_pair.clone();
                                                    view! {
                                                        <button
                                                            class="flex items-center gap-1 px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press"
                                                            style=format!(
                                                                "background: rgba({rgb_pair}, 0.1); color: rgb({rgb_pair}); border: 1px solid rgba({rgb_pair}, 0.15)"
                                                            )
                                                            on:click=move |_| on_pair.run(pair_id.clone())
                                                        >
                                                            <Icon icon=LuRefreshCw width="9px" height="9px" />
                                                            "Repair"
                                                        </button>
                                                    }
                                                })}
                                                <button
                                                    class="flex items-center gap-1 px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press
                                                           text-fg-tertiary/60 bg-surface-overlay/20 border border-edge-subtle hover:text-fg-tertiary"
                                                    on:click=move |_| on_forget.run(forget_id.clone())
                                                >
                                                    <Icon icon=LuTrash2 width="9px" height="9px" />
                                                    "Forget"
                                                </button>
                                            </div>
                                        </div>
                                    }.into_any())
                                }
                                Some(DeviceAuthState::Configured) => {
                                    let forget_id = dev_id_for_forget.clone();
                                    Some(view! {
                                        <div class="mb-3 px-3 py-2 rounded-lg border flex items-center justify-between"
                                             style="background: rgba(80, 250, 123, 0.03); border-color: rgba(80, 250, 123, 0.08)">
                                            <div class="flex items-center gap-2">
                                                <Icon icon=LuShieldCheck width="11px" height="11px" style="color: rgba(80, 250, 123, 0.5)" />
                                                <span class="text-[10px] text-fg-tertiary/50">"Credentials configured"</span>
                                            </div>
                                            <button
                                                class="text-[9px] font-medium text-fg-tertiary/30 hover:text-fg-tertiary/60 transition-colors"
                                                on:click=move |_| on_forget.run(forget_id.clone())
                                            >
                                                "Forget"
                                            </button>
                                        </div>
                                    }.into_any())
                                }
                                _ => None,
                            }}

                            // ── Brightness (gated when pairing required) ────────
                            <div class=if pairing_required { "flex items-center gap-3 opacity-40 pointer-events-none" } else { "flex items-center gap-3" }>
                                <Icon icon=LuSun width="12px" height="12px" style="color: rgba(139, 133, 160, 0.5)" />
                                <input
                                    type="range" min="0" max="100" step="1"
                                    class="flex-1 h-1 rounded-full appearance-none cursor-pointer"
                                    style=format!("accent-color: rgb({rgb_for_slider})")
                                    disabled=pairing_required
                                    prop:value=move || device_brightness.get().to_string()
                                    on:input=move |ev| {
                                        let event = Input::from_event(ev);
                                        if let Some(brightness) = event.value::<u8>() {
                                            set_device_brightness.set(brightness);
                                            push_brightness(brightness);
                                        }
                                    }
                                />
                                <span class="text-[10px] font-mono tabular-nums text-fg-tertiary w-8 text-right">
                                    {move || format!("{}%", device_brightness.get())}
                                </span>
                            </div>
                        </div>

                        <div class="relative px-4 py-2 bg-surface-overlay/10 border-t border-edge-subtle flex items-center gap-2">
                            <button
                                class="text-[10px] font-medium px-2 py-1 rounded-md transition-all btn-press flex items-center gap-1"
                                style=move || {
                                    if enabled {
                                        "color: rgba(255, 99, 99, 0.7); background: rgba(255, 99, 99, 0.06)".to_string()
                                    } else {
                                        "color: rgba(80, 250, 123, 0.7); background: rgba(80, 250, 123, 0.06)".to_string()
                                    }
                                }
                                on:click=move |_| toggle_enabled()
                            >
                                {if enabled {
                                    view! { <Icon icon=LuBan width="10px" height="10px" /> }.into_any()
                                } else {
                                    view! { <Icon icon=LuPower width="10px" height="10px" /> }.into_any()
                                }}
                                {if enabled { "Disable" } else { "Enable" }}
                            </button>
                            // Identify (gated when pairing required)
                            <button
                                class="text-[10px] font-medium px-2 py-1 rounded-md transition-all btn-press flex items-center gap-1"
                                class:opacity-40=pairing_required
                                class:pointer-events-none=pairing_required
                                disabled=pairing_required
                                style=move || {
                                    let r = rgb_for_identify.clone();
                                    if identify_active.get() {
                                        format!("color: rgb({r}); background: rgba({r}, 0.15)")
                                    } else {
                                        format!("color: rgba({r}, 0.6); background: rgba({r}, 0.06)")
                                    }
                                }
                                on:click=move |_| identify()
                            >
                                <span class=move || if identify_active.get() { "animate-pulse" } else { "" }>
                                    <Icon icon=LuZap width="10px" height="10px" />
                                </span>
                                {move || if identify_active.get() { "Flashing..." } else { "Identify" }}
                            </button>
                        </div>
                    </div>

                    <DeviceDriverControls device_id=device_id />

                    // ── Empty topology hint (when no zones) ─────────────────
                    {dev.zones.is_empty().then(|| {
                        let hint_rgb = rgb.clone();
                        view! {
                            <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow">
                                <div class="px-4 py-3">
                                    <div class="flex items-center gap-2 mb-2">
                                        <Icon icon=LuInfo width="13px" height="13px" style=format!("color: rgba({hint_rgb}, 0.6)") />
                                        <h3 class="text-[11px] font-medium" style=format!("color: rgba({hint_rgb}, 0.8)")>
                                            "No Addressable Zones"
                                        </h3>
                                    </div>
                                    <p class="text-[10px] text-fg-tertiary/60 leading-relaxed">
                                        "This driver has not reported any controllable LED zones for this device yet."
                                    </p>
                                </div>
                            </div>
                        }
                    })}

                    // ── Channels (unified: zone info + component editor) ─────
                    <WiringPanel device_id=device_id device=device_signal />
                }
            })}
        </div>
    }
}
