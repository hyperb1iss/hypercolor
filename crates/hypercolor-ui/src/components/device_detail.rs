//! Device detail sidebar — hardware spec sheet with visual zones and components.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_throttle_fn_with_arg;
use wasm_bindgen::JsCast;

use crate::api::{self, DeviceAuthState};
use crate::app::DevicesContext;
use crate::components::attachment_panel::WiringPanel;
use crate::components::device_card::{
    ALL_DEVICE_CLASSES, backend_accent_rgb, classify_device, device_class_icon, device_class_label,
    save_category_override, topology_shape_svg,
};
use crate::components::device_pairing_modal::needs_pairing;
use crate::icons::*;
use crate::toasts;

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
        let new_name = name_input.get();
        if new_name.trim().is_empty() {
            set_editing_name.set(false);
            return;
        }
        set_editing_name.set(false);
        let devices_resource = ctx.devices_resource;
        leptos::task::spawn_local(async move {
            let req = api::UpdateDeviceRequest {
                name: Some(new_name),
                enabled: None,
                brightness: None,
            };
            let _ = api::update_device(&id, &req).await;
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
            let promise = js_sys::Promise::new(&mut |resolve, _| {
                let _ = web_sys::window()
                    .expect("window")
                    .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 3_000);
            });
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            set_identify_active.set(false);
        });
    };

    view! {
        <div class="space-y-2.5">
            {move || device.get().map(|dev| {
                let rgb = backend_accent_rgb(&dev.backend).to_string();
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
                let dev_for_category = dev.clone();

                view! {
                    // ── Header: Name + status + actions ──────────────────────
                    <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow"
                         style=format!("border-top: 2px solid rgba({rgb_for_border}, 0.2)")>
                        <div class="px-4 py-3">
                            <div class="flex items-center gap-2.5 mb-2">
                                <div class="w-2 h-2 rounded-full shrink-0 dot-alive"
                                     style=format!("background: rgb({dot_rgb}); box-shadow: 0 0 8px rgba({dot_rgb}, 0.5)") />
                                {move || if editing_name.get() {
                                    view! {
                                        <input
                                            type="text"
                                            class="flex-1 bg-surface-overlay border border-edge-subtle rounded px-2 py-0.5 text-sm text-fg-primary
                                                   focus:outline-none focus:border-accent-muted"
                                            prop:value=move || name_input.get()
                                            on:input=move |ev| {
                                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                if let Some(el) = target { set_name_input.set(el.value()); }
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

                            <div class="flex items-center gap-3 text-[10px] font-mono text-fg-tertiary mb-2">
                                <span class="capitalize">{dev.backend.clone()}</span>
                                <span class="w-px h-3 bg-border-subtle" />
                                <span>{dev.total_leds} " LEDs"</span>
                                <span class="w-px h-3 bg-border-subtle" />
                                <span class="capitalize">{dev.status.clone()}</span>
                                {dev.firmware_version.clone().map(|fw| view! {
                                    <span class="w-px h-3 bg-border-subtle" />
                                    <span>"v" {fw}</span>
                                })}
                            </div>

                            // Device category (editable)
                            {
                                let dev_id_for_cat = dev_for_category.id.clone();
                                let current_class = classify_device(&dev_for_category);
                                let current_icon = device_class_icon(&current_class);
                                let (cat_label, set_cat_label) = signal(device_class_label(&current_class).to_string());
                                view! {
                                    <div class="flex items-center gap-2 mb-3">
                                        <Icon icon=current_icon width="11px" height="11px" style="color: rgba(139, 133, 160, 0.5)" />
                                        <select
                                            class="bg-transparent border-none text-[10px] font-mono text-fg-tertiary cursor-pointer
                                                   focus:outline-none hover:text-fg-secondary transition-colors appearance-none"
                                            prop:value=move || cat_label.get()
                                            on:change={
                                                let did = dev_id_for_cat.clone();
                                                move |ev| {
                                                    let value = event_target_value(&ev);
                                                    save_category_override(&did, &value);
                                                    set_cat_label.set(value);
                                                }
                                            }
                                        >
                                            {ALL_DEVICE_CLASSES.iter().map(|c| {
                                                let label = device_class_label(c);
                                                view! { <option value=label>{label}</option> }
                                            }).collect_view()}
                                        </select>
                                        <span class="text-[8px] text-fg-tertiary/25">"(category)"</span>
                                    </div>
                                }
                            }

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
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(el) = target
                                            && let Ok(brightness) = el.value().parse::<u8>()
                                        {
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

                        <div class="px-4 py-2 bg-surface-overlay/10 border-t border-edge-subtle flex items-center gap-2">
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

                    // ── Hue entertainment area hint (when no zones) ─────────
                    {(dev.zones.is_empty() && dev.backend.to_lowercase() == "hue").then(|| {
                        view! {
                            <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow">
                                <div class="px-4 py-3">
                                    <div class="flex items-center gap-2 mb-2">
                                        <Icon icon=LuLightbulb width="13px" height="13px" style="color: rgba(255, 183, 77, 0.6)" />
                                        <h3 class="text-[11px] font-medium" style="color: rgba(255, 183, 77, 0.8)">
                                            "Entertainment Area Required"
                                        </h3>
                                    </div>
                                    <p class="text-[10px] text-fg-tertiary/60 leading-relaxed mb-2">
                                        "Hypercolor streams to Hue lights through the Entertainment API. "
                                        "To get started:"
                                    </p>
                                    <ol class="text-[10px] text-fg-tertiary/50 leading-relaxed space-y-1 pl-4 list-decimal mb-2">
                                        <li>"Open the Hue app on your phone"</li>
                                        <li>"Go to Settings \u{2192} Entertainment areas"</li>
                                        <li>"Create an area and add the lights you want to control"</li>
                                        <li>"Come back here and re-scan for devices"</li>
                                    </ol>
                                    <p class="text-[9px] text-fg-tertiary/35 leading-relaxed">
                                        "Each light in the entertainment area becomes an addressable channel "
                                        "that Hypercolor can stream color data to in real time."
                                    </p>
                                </div>
                            </div>
                        }
                    })}

                    // ── Zones (expanded by default, animated collapse) ──────
                    {(!dev.zones.is_empty()).then(|| {
                        let zones = dev.zones.clone();
                        let zone_rgb = rgb.clone();
                        let zone_count = zones.len();
                        let (zones_open, set_zones_open) = signal(true);
                        let dev_id_for_zone = dev.id.clone();
                        view! {
                            <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow">
                                <button
                                    class="w-full px-4 py-2.5 flex items-center gap-2 hover:bg-surface-hover/20 transition-colors"
                                    on:click=move |_| set_zones_open.update(|v| *v = !*v)
                                >
                                    <Icon icon=LuGrid2x2 width="12px" height="12px" style="color: rgba(139, 133, 160, 0.6)" />
                                    <h3 class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">"Zones"</h3>
                                    <span class="text-[9px] font-mono text-fg-tertiary/40 ml-auto">{zone_count}</span>
                                    <span class="w-3 h-3 flex items-center justify-center transition-transform duration-200"
                                          class:rotate-180=move || !zones_open.get()>
                                        <Icon icon=LuChevronDown width="11px" height="11px" style="color: rgba(139, 133, 160, 0.4)" />
                                    </span>
                                </button>
                                // Animated expand/collapse via grid-template-rows
                                <div
                                    class="transition-[grid-template-rows] duration-200 ease-out"
                                    style=move || if zones_open.get() {
                                        "display: grid; grid-template-rows: 1fr"
                                    } else {
                                        "display: grid; grid-template-rows: 0fr"
                                    }
                                >
                                    <div style="overflow: hidden">
                                        <div class="px-3 pb-3">
                                            <div class="grid grid-cols-2 gap-1.5">
                                                {zones.into_iter().map(|zone| {
                                                    let zr = zone_rgb.clone();
                                                    let svg = topology_shape_svg(&zone.topology);
                                                    let zone_name = zone.name.clone();
                                                    let dev_id = dev_id_for_zone.clone();

                                                    // Display metadata from topology hint
                                                    let display_info = zone.topology_hint.as_ref().and_then(|h| {
                                                        if let api::ZoneTopologySummary::Display { width, height, circular } = h {
                                                            Some((*width, *height, *circular))
                                                        } else { None }
                                                    });

                                                    view! {
                                                        <div class="flex items-center gap-2 px-2 py-1.5 rounded-md
                                                                    bg-surface-overlay/15 hover:bg-surface-hover/30 transition-colors group/zone">
                                                            <div class="w-4 h-4 shrink-0" style=format!("color: rgba({zr}, 0.5)")
                                                                 inner_html=format!(r#"<svg viewBox="0 0 16 16" width="16" height="16">{svg}</svg>"#) />
                                                            <div class="min-w-0 flex-1">
                                                                <div class="text-[11px] text-fg-primary leading-tight">{zone_name}</div>
                                                                <div class="flex items-center gap-1.5">
                                                                    <span class="text-[9px] font-mono text-fg-tertiary/50">{zone.led_count} " LEDs"</span>
                                                                    {display_info.map(|(w, h, circular)| {
                                                                        let label = if circular { format!("{w}\u{00d7}{h} \u{25cb}") } else { format!("{w}\u{00d7}{h}") };
                                                                        view! {
                                                                            <span class="text-[8px] font-mono text-fg-tertiary/30">{label}</span>
                                                                        }
                                                                    })}
                                                                </div>
                                                            </div>
                                                            // Channel identify button — flashes only this channel's LEDs
                                                            <button
                                                                class="w-4 h-4 flex items-center justify-center rounded shrink-0
                                                                       opacity-0 group-hover/zone:opacity-100 transition-opacity
                                                                       text-fg-tertiary/40 hover:text-accent btn-press"
                                                                title="Identify channel"
                                                                on:click={
                                                                    let dev_id = dev_id.clone();
                                                                    let zone_id = zone.id.clone();
                                                                    move |ev: web_sys::MouseEvent| {
                                                                        ev.stop_propagation();
                                                                        let did = dev_id.clone();
                                                                        let zid = zone_id.clone();
                                                                        leptos::task::spawn_local(async move {
                                                                            if let Err(e) = api::identify_zone(&did, &zid).await {
                                                                                toasts::toast_error(&format!("Identify failed: {e}"));
                                                                            } else {
                                                                                toasts::toast_success("Flashing channel");
                                                                            }
                                                                        });
                                                                    }
                                                                }
                                                            >
                                                                <Icon icon=LuZap width="9px" height="9px" />
                                                            </button>
                                                        </div>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        </div>
                                    </div>
                                </div>
                            </div>
                        }
                    })}

                    // ── Components ────────────────────────────────────────────
                    <WiringPanel device_id=device_id device=device_signal />
                }
            })}
        </div>
    }
}
