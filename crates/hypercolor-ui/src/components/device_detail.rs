//! Device detail sidebar — cinematic device info, actions, and logical device management.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::DevicesContext;
use crate::components::attachment_panel::AttachmentPanel;
use crate::components::device_card::backend_accent_rgb;
use crate::icons::*;
use crate::toasts;

/// Device detail sidebar.
#[component]
pub fn DeviceDetail(#[prop(into)] device_id: Signal<String>) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    // Derive selected device from context
    let device = Memo::new(move |_| {
        let id = device_id.get();
        ctx.devices_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|devices| devices.into_iter().find(|d| d.id == id))
    });

    // Logical devices resource — re-fetches when device changes
    let logical_devices = LocalResource::new(move || {
        let id = device_id.get();
        async move {
            if id.is_empty() {
                return Ok(Vec::new());
            }
            api::fetch_logical_devices(&id).await
        }
    });

    // Editing state
    let (editing_name, set_editing_name) = signal(false);
    let (name_input, set_name_input) = signal(String::new());
    let (show_add_segment, set_show_add_segment) = signal(false);
    let (seg_name, set_seg_name) = signal(String::new());
    let (seg_start, set_seg_start) = signal(String::new());
    let (seg_count, set_seg_count) = signal(String::new());
    let (identify_active, set_identify_active) = signal(false);
    let device_signal = Signal::derive(move || device.get());

    // Save name handler
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

    // Toggle enabled
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

    // Device brightness
    let set_brightness = move |brightness: u8| {
        let Some(dev) = device.get() else { return };
        let id = dev.id.clone();
        let devices_resource = ctx.devices_resource;
        leptos::task::spawn_local(async move {
            let req = api::UpdateDeviceRequest {
                name: None,
                enabled: None,
                brightness: Some(brightness),
            };
            match api::update_device(&id, &req).await {
                Ok(_) => devices_resource.refetch(),
                Err(error) => toasts::toast_error(&format!("Brightness update failed: {error}")),
            }
        });
    };

    // Identify handler
    let identify = move || {
        let id = device_id.get();
        set_identify_active.set(true);
        toasts::toast_info("Identifying device...");
        leptos::task::spawn_local(async move {
            if let Err(error) = api::identify_device(&id).await {
                set_identify_active.set(false);
                toasts::toast_error(&format!("Identify failed: {error}"));
                return;
            }

            toasts::toast_success("Device identify flash started");

            // Flash indicator for 3s then reset
            let promise = js_sys::Promise::new(&mut |resolve, _| {
                let _ = web_sys::window()
                    .expect("window")
                    .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 3_000);
            });
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            set_identify_active.set(false);
        });
    };

    // Add segment handler
    let add_segment = move || {
        let id = device_id.get();
        let name = seg_name.get();
        let start: u32 = seg_start.get().parse().unwrap_or(0);
        let count: u32 = seg_count.get().parse().unwrap_or(1);

        if name.trim().is_empty() {
            return;
        }

        set_show_add_segment.set(false);
        set_seg_name.set(String::new());
        set_seg_start.set(String::new());
        set_seg_count.set(String::new());

        leptos::task::spawn_local(async move {
            let req = api::CreateLogicalDeviceRequest {
                name,
                led_start: start,
                led_count: count,
                enabled: Some(true),
            };
            let _ = api::create_logical_device(&id, &req).await;
        });
    };

    // Delete logical device
    let delete_logical = move |logical_id: String| {
        leptos::task::spawn_local(async move {
            let _ = api::delete_logical_device(&logical_id).await;
        });
    };

    view! {
        <aside class="w-[420px] shrink-0 sticky top-0 self-start space-y-3 animate-slide-in-right scrollbar-none will-change-transform"
               style="max-height: calc(100vh - 10rem); overflow-y: auto">
            {move || device.get().map(|dev| {
                let rgb = backend_accent_rgb(&dev.backend).to_string();
                let status_rgb = crate::components::device_card::backend_accent_rgb(&dev.backend);
                let dot_style = format!("background: rgb({}); box-shadow: 0 0 8px rgba({}, 0.6)", status_rgb, status_rgb);
                let accent_border = format!("border-top: 2px solid rgba({rgb}, 0.15)");
                let enabled = dev.status != "disabled";
                let dev_name_for_edit = dev.name.clone();

                view! {
                    // ── Device Header ──────────────────────────────────────
                    <div class="flex items-center gap-2.5 px-1">
                        <div class="w-2.5 h-2.5 rounded-full dot-alive shrink-0" style=dot_style />
                        {move || if editing_name.get() {
                            view! {
                                <input
                                    type="text"
                                    class="flex-1 bg-surface-overlay border border-edge-subtle rounded px-2 py-1 text-sm text-fg-primary
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
                                    class="text-base font-medium text-fg-primary cursor-pointer hover:text-accent transition-colors group flex items-center gap-1.5"
                                    on:click=move |_| {
                                        set_name_input.set(name.clone());
                                        set_editing_name.set(true);
                                    }
                                >
                                    {dev.name.clone()}
                                    <span class="w-3 h-3 text-fg-tertiary opacity-0 group-hover:opacity-100 transition-opacity">
                                        <Icon icon=LuPencil width="12px" height="12px" />
                                    </span>
                                </span>
                            }.into_any()
                        }}
                    </div>

                    // ── Info Panel ─────────────────────────────────────────
                    <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow"
                         style=accent_border>
                        <div class="p-4 space-y-3">
                            // Device metadata grid
                            <div class="grid grid-cols-2 gap-x-4 gap-y-2">
                                {detail_field("Backend", &dev.backend)}
                                {detail_field("Status", &capitalize(&dev.status))}
                                {detail_field("LEDs", &dev.total_leds.to_string())}
                                {detail_field("Firmware", &dev.firmware_version.clone().unwrap_or_else(|| "\u{2014}".to_string()))}
                                {detail_field(
                                    "Hostname",
                                    &dev.network_hostname.clone().unwrap_or_else(|| "\u{2014}".to_string()),
                                )}
                                {detail_field(
                                    "IP",
                                    &dev.network_ip.clone().unwrap_or_else(|| "\u{2014}".to_string()),
                                )}
                            </div>

                            <div class="pt-3 border-t border-edge-subtle space-y-2">
                                <div class="flex items-center justify-between gap-3">
                                    <div>
                                        <h4 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">
                                            "Device Brightness"
                                        </h4>
                                        <p class="text-[11px] text-fg-tertiary mt-1">
                                            "Scales this device after global output brightness."
                                        </p>
                                    </div>
                                    <span class="text-sm font-mono tabular-nums text-fg-primary">
                                        {format!("{}%", dev.brightness)}
                                    </span>
                                </div>
                                <input
                                    type="range"
                                    min="0"
                                    max="100"
                                    step="1"
                                    class="w-full h-1 rounded-full appearance-none cursor-pointer"
                                    style="accent-color: rgb(225, 53, 255); background: rgba(139, 133, 160, 0.15)"
                                    prop:value=dev.brightness.to_string()
                                    on:change=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(el) = target
                                            && let Ok(brightness) = el.value().parse::<u8>()
                                        {
                                            set_brightness(brightness);
                                        }
                                    }
                                />
                            </div>

                            // Zone list
                            {(!dev.zones.is_empty()).then(|| {
                                let zones = dev.zones.clone();
                                let rgb = rgb.clone();
                                view! {
                                    <div class="pt-3 border-t border-edge-subtle">
                                        <h4 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary mb-2">"Zones"</h4>
                                        <div class="space-y-1">
                                            {zones.into_iter().map(|zone| {
                                                let zone_rgb = rgb.clone();
                                                view! {
                                                    <div class="flex items-center justify-between text-xs px-2 py-1 rounded-md
                                                                bg-surface-overlay/20 hover:bg-surface-hover/40 transition-colors">
                                                        <div class="flex items-center gap-2">
                                                            <div class="w-1 h-3 rounded-full"
                                                                 style=format!("background: rgba({zone_rgb}, 0.4)") />
                                                            <span class="text-fg-primary truncate">{zone.name}</span>
                                                        </div>
                                                        <span class="text-fg-tertiary font-mono tabular-nums">{zone.led_count}</span>
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                }
                            })}
                        </div>

                        // ── Actions ───────────────────────────────────────
                        <div class="px-4 py-3 bg-surface-overlay/15 border-t border-edge-subtle">
                            <div class="flex items-center gap-2">
                                // Enable/Disable toggle
                                <button
                                    class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all btn-press"
                                    style=move || {
                                        if enabled {
                                            "background: rgba(255, 99, 99, 0.08); border: 1px solid rgba(255, 99, 99, 0.15); color: rgb(255, 99, 99)".to_string()
                                        } else {
                                            "background: rgba(80, 250, 123, 0.08); border: 1px solid rgba(80, 250, 123, 0.15); color: rgb(80, 250, 123)".to_string()
                                        }
                                    }
                                    on:click=move |_| toggle_enabled()
                                >
                                    {if enabled {
                                        view! { <Icon icon=LuBan width="14px" height="14px" /> }.into_any()
                                    } else {
                                        view! { <Icon icon=LuPower width="14px" height="14px" /> }.into_any()
                                    }}
                                    {if enabled { "Disable" } else { "Enable" }}
                                </button>

                                // Identify button
                                <button
                                    class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all btn-press"
                                    style=move || {
                                        let rgb_val = rgb.clone();
                                        if identify_active.get() {
                                            format!("background: rgba({rgb_val}, 0.2); border: 1px solid rgba({rgb_val}, 0.4); color: rgb({rgb_val}); box-shadow: 0 0 16px rgba({rgb_val}, 0.3)")
                                        } else {
                                            format!("background: rgba({rgb_val}, 0.08); border: 1px solid rgba({rgb_val}, 0.15); color: rgb({rgb_val})")
                                        }
                                    }
                                    on:click=move |_| identify()
                                >
                                    <span class=move || if identify_active.get() { "animate-pulse" } else { "" }>
                                        <Icon icon=LuZap width="14px" height="14px" />
                                    </span>
                                    {move || if identify_active.get() { "Flashing..." } else { "Identify" }}
                                </button>

                            </div>
                        </div>
                    </div>

                    <AttachmentPanel device_id=device_id device=device_signal />

                    // ── Segments ───────────────────────────────────────────
                    <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow">
                        <div class="flex items-center justify-between px-4 py-3">
                            <div class="flex items-center gap-2">
                                <Icon icon=LuCable width="14px" height="14px" style="color: rgba(139, 133, 160, 1)" />
                                <h3 class="text-xs font-mono uppercase tracking-[0.12em] text-fg-tertiary">"Segments"</h3>
                            </div>
                            <button
                                class="px-2 py-0.5 rounded text-[10px] font-medium transition-all"
                                style="background: rgba(225, 53, 255, 0.08); border: 1px solid rgba(225, 53, 255, 0.15); color: rgb(225, 53, 255)"
                                on:click=move |_| set_show_add_segment.update(|v| *v = !*v)
                            >
                                {move || if show_add_segment.get() { "Cancel" } else { "+ Add" }}
                            </button>
                        </div>

                        // Add segment form
                        {move || show_add_segment.get().then(|| view! {
                            <div class="mx-4 mb-3 space-y-2 p-3 rounded-lg bg-surface-overlay/60 border border-edge-subtle animate-fade-in">
                                <input
                                    type="text"
                                    placeholder="Segment name"
                                    class="w-full bg-surface-base/60 border border-edge-subtle rounded px-2.5 py-1.5 text-xs text-fg-primary
                                           placeholder-fg-tertiary focus:outline-none focus:border-accent-muted"
                                    prop:value=move || seg_name.get()
                                    on:input=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(el) = target { set_seg_name.set(el.value()); }
                                    }
                                />
                                <div class="flex gap-2">
                                    <input
                                        type="number"
                                        placeholder="Start"
                                        class="flex-1 bg-surface-base/60 border border-edge-subtle rounded px-2.5 py-1.5 text-xs text-fg-primary font-mono
                                               placeholder-fg-tertiary focus:outline-none focus:border-accent-muted"
                                        prop:value=move || seg_start.get()
                                        on:input=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target { set_seg_start.set(el.value()); }
                                        }
                                    />
                                    <input
                                        type="number"
                                        placeholder="Count"
                                        class="flex-1 bg-surface-base/60 border border-edge-subtle rounded px-2.5 py-1.5 text-xs text-fg-primary font-mono
                                               placeholder-fg-tertiary focus:outline-none focus:border-accent-muted"
                                        prop:value=move || seg_count.get()
                                        on:input=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target { set_seg_count.set(el.value()); }
                                        }
                                    />
                                </div>
                                <button
                                    class="w-full px-3 py-1.5 rounded-lg text-xs font-medium transition-all btn-press"
                                    style="background: rgba(80, 250, 123, 0.1); border: 1px solid rgba(80, 250, 123, 0.15); color: rgb(80, 250, 123)"
                                    on:click=move |_| add_segment()
                                >
                                    "Create Segment"
                                </button>
                            </div>
                        })}

                        // Logical device list
                        <div class="px-4 pb-4">
                            <Suspense fallback=|| view! {
                                <div class="text-xs text-fg-tertiary animate-pulse py-2">"Loading segments..."</div>
                            }>
                                {move || {
                                    logical_devices.get().map(|result| {
                                        match result {
                                            Ok(segments) => {
                                                let dev = device.get();
                                                let total = dev.map(|d| d.total_leds).unwrap_or(0);
                                                if segments.is_empty() {
                                                    return view! {
                                                        <div class="text-xs text-fg-tertiary py-3 text-center">"No segments configured"</div>
                                                    }.into_any();
                                                }
                                                view! {
                                                    <div class="space-y-1.5">
                                                        {segments.into_iter().map(|seg| {
                                                            let seg_id = seg.id.clone();
                                                            let is_default = seg.kind == "default";
                                                            let bar_pct_start = if total > 0 { f64::from(seg.led_start) / total as f64 * 100.0 } else { 0.0 };
                                                            let bar_pct_width = if total > 0 { f64::from(seg.led_count) / total as f64 * 100.0 } else { 100.0 };
                                                            view! {
                                                                <div class="p-2.5 rounded-lg bg-surface-overlay/20 border border-edge-subtle
                                                                            hover:bg-surface-hover/40 transition-colors">
                                                                    <div class="flex items-center justify-between mb-2">
                                                                        <div class="flex items-center gap-1.5">
                                                                            <span class="text-xs text-fg-primary font-medium">{seg.name}</span>
                                                                            {is_default.then(|| view! {
                                                                                <span class="px-1.5 py-0.5 rounded text-[8px] font-mono uppercase tracking-wider text-fg-tertiary bg-surface-overlay/40">
                                                                                    "default"
                                                                                </span>
                                                                            })}
                                                                        </div>
                                                                        <div class="flex items-center gap-2">
                                                                            <span class="text-[11px] font-mono text-fg-tertiary tabular-nums">
                                                                                {seg.led_start} "\u{2013}" {seg.led_end} " (" {seg.led_count} ")"
                                                                            </span>
                                                                            {(!is_default).then(|| {
                                                                                let sid = seg_id.clone();
                                                                                view! {
                                                                                    <button
                                                                                        class="p-0.5 rounded text-fg-tertiary hover:text-error-red transition-colors"
                                                                                        title="Delete segment"
                                                                                        on:click=move |_| delete_logical(sid.clone())
                                                                                    >
                                                                                        <Icon icon=LuX width="12px" height="12px" />
                                                                                    </button>
                                                                                }
                                                                            })}
                                                                        </div>
                                                                    </div>
                                                                    // LED range bar
                                                                    <div class="h-1.5 rounded-full bg-surface-overlay/40 overflow-hidden">
                                                                        <div
                                                                            class="h-full rounded-full transition-all duration-300"
                                                                            style=format!(
                                                                                "margin-left: {bar_pct_start:.1}%; width: {bar_pct_width:.1}%; \
                                                                                 background: linear-gradient(90deg, rgba(225, 53, 255, 0.5), rgba(128, 255, 234, 0.4))"
                                                                            )
                                                                        />
                                                                    </div>
                                                                </div>
                                                            }
                                                        }).collect_view()}
                                                    </div>
                                                }.into_any()
                                            }
                                            Err(e) => view! {
                                                <div class="text-xs text-error-red py-2">{e}</div>
                                            }.into_any(),
                                        }
                                    })
                                }}
                            </Suspense>
                        </div>
                    </div>
                }
            })}
        </aside>
    }
}

/// Inline metadata field for the info grid.
fn detail_field(label: &'static str, value: &str) -> impl IntoView + use<> {
    let value = value.to_string();
    view! {
        <div>
            <div class="text-[9px] font-mono uppercase tracking-wider text-fg-tertiary mb-0.5">{label}</div>
            <div class="text-xs text-fg-primary font-mono capitalize">{value}</div>
        </div>
    }
}

/// Capitalize first letter.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
