//! Device detail sidebar — hardware spec sheet with visual zones and attachment shapes.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_use::use_throttle_fn_with_arg;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::DevicesContext;
use crate::components::attachment_panel::AttachmentPanel;
use crate::components::device_card::{backend_accent_rgb, topology_shape_svg};
use crate::icons::*;
use crate::toasts;

/// Device detail sidebar.
#[component]
pub fn DeviceDetail(#[prop(into)] device_id: Signal<String>) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    let device = Memo::new(move |_| {
        let id = device_id.get();
        ctx.devices_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|devices| devices.into_iter().find(|d| d.id == id))
    });

    let logical_devices = LocalResource::new(move || {
        let id = device_id.get();
        async move {
            if id.is_empty() {
                return Ok(Vec::new());
            }
            api::fetch_logical_devices(&id).await
        }
    });

    let (editing_name, set_editing_name) = signal(false);
    let (name_input, set_name_input) = signal(String::new());
    let (show_add_segment, set_show_add_segment) = signal(false);
    let (seg_name, set_seg_name) = signal(String::new());
    let (seg_start, set_seg_start) = signal(String::new());
    let (seg_count, set_seg_count) = signal(String::new());
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

    let delete_logical = move |logical_id: String| {
        leptos::task::spawn_local(async move {
            let _ = api::delete_logical_device(&logical_id).await;
        });
    };

    view! {
        <aside class="w-[400px] shrink-0 sticky top-0 self-start space-y-2.5 animate-slide-in-right scrollbar-none will-change-transform"
               style="max-height: calc(100vh - 10rem); overflow-y: auto">
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
                let dev_name_for_edit = dev.name.clone();
                let push_brightness = push_brightness.clone();

                view! {
                    // ── Header: Name + status + actions ──────────────────────
                    <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow"
                         style=format!("border-top: 2px solid rgba({rgb_for_border}, 0.2)")>
                        <div class="px-4 py-3">
                            // Name row
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
                                            class="text-sm font-medium text-fg-primary cursor-pointer hover:text-accent transition-colors group flex items-center gap-1.5 truncate"
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

                            // Compact metadata
                            <div class="flex items-center gap-3 text-[10px] font-mono text-fg-tertiary mb-3">
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

                            // Brightness — single compact row
                            <div class="flex items-center gap-3">
                                <Icon icon=LuSun width="12px" height="12px" style="color: rgba(139, 133, 160, 0.5)" />
                                <input
                                    type="range"
                                    min="0"
                                    max="100"
                                    step="1"
                                    class="flex-1 h-1 rounded-full appearance-none cursor-pointer"
                                    style=format!("accent-color: rgb({rgb_for_slider})")
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

                        // Actions — minimal row at bottom
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
                            <button
                                class="text-[10px] font-medium px-2 py-1 rounded-md transition-all btn-press flex items-center gap-1"
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

                    // ── Zones — visual topology shapes ───────────────────────
                    {(!dev.zones.is_empty()).then(|| {
                        let zones = dev.zones.clone();
                        let zone_rgb = rgb.clone();
                        view! {
                            <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow">
                                <div class="px-4 py-2.5 flex items-center gap-2">
                                    <Icon icon=LuGrid2x2 width="12px" height="12px" style="color: rgba(139, 133, 160, 0.6)" />
                                    <h3 class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">"Zones"</h3>
                                    <span class="text-[9px] font-mono text-fg-tertiary/40 ml-auto">{zones.len()}</span>
                                </div>
                                <div class="px-3 pb-3">
                                    <div class="grid grid-cols-2 gap-1.5">
                                        {zones.into_iter().map(|zone| {
                                            let zr = zone_rgb.clone();
                                            let svg = topology_shape_svg(&zone.topology);
                                            view! {
                                                <div class="flex items-center gap-2 px-2 py-1.5 rounded-md
                                                            bg-surface-overlay/15 hover:bg-surface-hover/30 transition-colors group/zone">
                                                    <div class="w-4 h-4 shrink-0" style=format!("color: rgba({zr}, 0.5)")
                                                         inner_html=format!(r#"<svg viewBox="0 0 16 16" width="16" height="16">{svg}</svg>"#) />
                                                    <div class="min-w-0 flex-1">
                                                        <div class="text-[11px] text-fg-primary truncate leading-tight">{zone.name}</div>
                                                        <div class="text-[9px] font-mono text-fg-tertiary/50">{zone.led_count}</div>
                                                    </div>
                                                </div>
                                            }
                                        }).collect_view()}
                                    </div>
                                </div>
                            </div>
                        }
                    })}

                    // ── Attachments ──────────────────────────────────────────
                    <AttachmentPanel device_id=device_id device=device_signal />

                    // ── Segments ─────────────────────────────────────────────
                    <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow">
                        <div class="flex items-center justify-between px-4 py-2.5">
                            <div class="flex items-center gap-2">
                                <Icon icon=LuCable width="12px" height="12px" style="color: rgba(139, 133, 160, 0.6)" />
                                <h3 class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">"Segments"</h3>
                            </div>
                            <button
                                class="px-1.5 py-0.5 rounded text-[9px] font-medium transition-all"
                                style="background: rgba(225, 53, 255, 0.06); color: rgba(225, 53, 255, 0.7)"
                                on:click=move |_| set_show_add_segment.update(|v| *v = !*v)
                            >
                                {move || if show_add_segment.get() { "Cancel" } else { "+ Add" }}
                            </button>
                        </div>

                        {move || show_add_segment.get().then(|| view! {
                            <div class="mx-3 mb-2.5 space-y-1.5 p-2.5 rounded-lg bg-surface-overlay/60 border border-edge-subtle animate-fade-in">
                                <input
                                    type="text"
                                    placeholder="Segment name"
                                    class="w-full bg-surface-base/60 border border-edge-subtle rounded px-2 py-1 text-[11px] text-fg-primary
                                           placeholder-fg-tertiary focus:outline-none focus:border-accent-muted"
                                    prop:value=move || seg_name.get()
                                    on:input=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(el) = target { set_seg_name.set(el.value()); }
                                    }
                                />
                                <div class="flex gap-1.5">
                                    <input
                                        type="number"
                                        placeholder="Start"
                                        class="flex-1 bg-surface-base/60 border border-edge-subtle rounded px-2 py-1 text-[11px] text-fg-primary font-mono
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
                                        class="flex-1 bg-surface-base/60 border border-edge-subtle rounded px-2 py-1 text-[11px] text-fg-primary font-mono
                                               placeholder-fg-tertiary focus:outline-none focus:border-accent-muted"
                                        prop:value=move || seg_count.get()
                                        on:input=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target { set_seg_count.set(el.value()); }
                                        }
                                    />
                                </div>
                                <button
                                    class="w-full px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press"
                                    style="background: rgba(80, 250, 123, 0.08); color: rgba(80, 250, 123, 0.8)"
                                    on:click=move |_| add_segment()
                                >
                                    "Create"
                                </button>
                            </div>
                        })}

                        <div class="px-3 pb-3">
                            <Suspense fallback=|| view! {
                                <div class="text-[10px] text-fg-tertiary animate-pulse py-2">"Loading..."</div>
                            }>
                                {move || {
                                    logical_devices.get().map(|result| {
                                        match result {
                                            Ok(segments) => {
                                                let dev = device.get();
                                                let total = dev.map(|d| d.total_leds).unwrap_or(0);
                                                if segments.is_empty() {
                                                    return view! {
                                                        <div class="text-[10px] text-fg-tertiary/50 py-2 text-center">"No segments"</div>
                                                    }.into_any();
                                                }
                                                view! {
                                                    <div class="space-y-1">
                                                        {segments.into_iter().map(|seg| {
                                                            let seg_id = seg.id.clone();
                                                            let is_default = seg.kind == "default";
                                                            let bar_start = if total > 0 { f64::from(seg.led_start) / total as f64 * 100.0 } else { 0.0 };
                                                            let bar_width = if total > 0 { f64::from(seg.led_count) / total as f64 * 100.0 } else { 100.0 };
                                                            view! {
                                                                <div class="px-2 py-1.5 rounded-md bg-surface-overlay/15 hover:bg-surface-hover/30 transition-colors">
                                                                    <div class="flex items-center justify-between mb-1">
                                                                        <div class="flex items-center gap-1.5">
                                                                            <span class="text-[11px] text-fg-primary font-medium">{seg.name}</span>
                                                                            {is_default.then(|| view! {
                                                                                <span class="px-1 py-0.5 rounded text-[7px] font-mono uppercase text-fg-tertiary/50 bg-surface-overlay/30">"default"</span>
                                                                            })}
                                                                        </div>
                                                                        <div class="flex items-center gap-1.5">
                                                                            <span class="text-[9px] font-mono text-fg-tertiary/60 tabular-nums">
                                                                                {seg.led_start} "\u{2013}" {seg.led_end}
                                                                            </span>
                                                                            {(!is_default).then(|| {
                                                                                let sid = seg_id.clone();
                                                                                view! {
                                                                                    <button
                                                                                        class="p-0.5 rounded text-fg-tertiary/40 hover:text-error-red transition-colors"
                                                                                        on:click=move |_| delete_logical(sid.clone())
                                                                                    >
                                                                                        <Icon icon=LuX width="10px" height="10px" />
                                                                                    </button>
                                                                                }
                                                                            })}
                                                                        </div>
                                                                    </div>
                                                                    <div class="h-1 rounded-full bg-surface-overlay/30 overflow-hidden">
                                                                        <div
                                                                            class="h-full rounded-full"
                                                                            style=format!(
                                                                                "margin-left: {bar_start:.1}%; width: {bar_width:.1}%; \
                                                                                 background: linear-gradient(90deg, rgba(225, 53, 255, 0.4), rgba(128, 255, 234, 0.3))"
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
                                                <div class="text-[10px] text-error-red py-2">{e}</div>
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
