//! Device detail sidebar — cinematic device info, actions, and logical device management.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::DevicesContext;
use crate::components::device_card::backend_accent_rgb;

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
            };
            let _ = api::update_device(&id, &req).await;
            devices_resource.refetch();
        });
    };

    // Identify handler
    let identify = move || {
        let id = device_id.get();
        set_identify_active.set(true);
        leptos::task::spawn_local(async move {
            let _ = api::identify_device(&id).await;
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
                                    class="flex-1 bg-layer-2 border border-white/[0.06] rounded px-2 py-1 text-sm text-fg
                                           focus:outline-none focus:border-electric-purple/30"
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
                                    class="text-base font-medium text-fg cursor-pointer hover:text-electric-purple transition-colors group flex items-center gap-1.5"
                                    on:click=move |_| {
                                        set_name_input.set(name.clone());
                                        set_editing_name.set(true);
                                    }
                                >
                                    {dev.name.clone()}
                                    <svg class="w-3 h-3 text-fg-dim opacity-0 group-hover:opacity-100 transition-opacity" viewBox="0 0 24 24" fill="none"
                                         stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <path d="M17 3a2.85 2.83 0 114 4L7.5 20.5 2 22l1.5-5.5Z"/>
                                    </svg>
                                </span>
                            }.into_any()
                        }}
                    </div>

                    // ── Info Panel ─────────────────────────────────────────
                    <div class="rounded-xl bg-layer-1 border border-white/[0.06] overflow-hidden
                                shadow-[0_2px_12px_rgba(0,0,0,0.2)]"
                         style=accent_border>
                        <div class="p-4 space-y-3">
                            // Device metadata grid
                            <div class="grid grid-cols-2 gap-x-4 gap-y-2">
                                {detail_field("Backend", &dev.backend)}
                                {detail_field("Status", &capitalize(&dev.status))}
                                {detail_field("LEDs", &dev.total_leds.to_string())}
                                {detail_field("Firmware", &dev.firmware_version.clone().unwrap_or_else(|| "\u{2014}".to_string()))}
                            </div>

                            // Zone list
                            {(!dev.zones.is_empty()).then(|| {
                                let zones = dev.zones.clone();
                                let rgb = rgb.clone();
                                view! {
                                    <div class="pt-3 border-t border-white/[0.04]">
                                        <h4 class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-dim mb-2">"Zones"</h4>
                                        <div class="space-y-1">
                                            {zones.into_iter().map(|zone| {
                                                let zone_rgb = rgb.clone();
                                                view! {
                                                    <div class="flex items-center justify-between text-xs px-2 py-1 rounded-md
                                                                bg-white/[0.02] hover:bg-white/[0.04] transition-colors">
                                                        <div class="flex items-center gap-2">
                                                            <div class="w-1 h-3 rounded-full"
                                                                 style=format!("background: rgba({zone_rgb}, 0.4)") />
                                                            <span class="text-fg truncate">{zone.name}</span>
                                                        </div>
                                                        <span class="text-fg-dim font-mono tabular-nums">{zone.led_count}</span>
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                }
                            })}
                        </div>

                        // ── Actions ───────────────────────────────────────
                        <div class="px-4 py-3 bg-white/[0.015] border-t border-white/[0.04]">
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
                                        view! {
                                            <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                                 stroke-width="2" stroke-linecap="round">
                                                <circle cx="12" cy="12" r="10"/><line x1="4" y1="4" x2="20" y2="20"/>
                                            </svg>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                                 stroke-width="2" stroke-linecap="round">
                                                <path d="M5 12l5 5L20 7"/>
                                            </svg>
                                        }.into_any()
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
                                    <svg class=move || {
                                        if identify_active.get() { "w-3.5 h-3.5 animate-pulse" } else { "w-3.5 h-3.5" }
                                    } viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/>
                                    </svg>
                                    {move || if identify_active.get() { "Flashing..." } else { "Identify" }}
                                </button>

                            </div>
                        </div>
                    </div>

                    // ── Segments ───────────────────────────────────────────
                    <div class="rounded-xl bg-layer-1 border border-white/[0.06] overflow-hidden
                                shadow-[0_2px_12px_rgba(0,0,0,0.2)]">
                        <div class="flex items-center justify-between px-4 py-3">
                            <div class="flex items-center gap-2">
                                <svg class="w-3.5 h-3.5 text-fg-dim" viewBox="0 0 24 24" fill="none"
                                     stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <rect x="1" y="6" width="22" height="12" rx="2"/>
                                    <line x1="8" y1="6" x2="8" y2="18"/>
                                    <line x1="16" y1="6" x2="16" y2="18"/>
                                </svg>
                                <h3 class="text-xs font-mono uppercase tracking-[0.12em] text-fg-dim">"Segments"</h3>
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
                            <div class="mx-4 mb-3 space-y-2 p-3 rounded-lg bg-layer-2/60 border border-white/[0.04] animate-fade-in">
                                <input
                                    type="text"
                                    placeholder="Segment name"
                                    class="w-full bg-layer-0/60 border border-white/[0.04] rounded px-2.5 py-1.5 text-xs text-fg
                                           placeholder-fg-dim focus:outline-none focus:border-electric-purple/20"
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
                                        class="flex-1 bg-layer-0/60 border border-white/[0.04] rounded px-2.5 py-1.5 text-xs text-fg font-mono
                                               placeholder-fg-dim focus:outline-none focus:border-electric-purple/20"
                                        prop:value=move || seg_start.get()
                                        on:input=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target { set_seg_start.set(el.value()); }
                                        }
                                    />
                                    <input
                                        type="number"
                                        placeholder="Count"
                                        class="flex-1 bg-layer-0/60 border border-white/[0.04] rounded px-2.5 py-1.5 text-xs text-fg font-mono
                                               placeholder-fg-dim focus:outline-none focus:border-electric-purple/20"
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
                                <div class="text-xs text-fg-dim animate-pulse py-2">"Loading segments..."</div>
                            }>
                                {move || {
                                    logical_devices.get().map(|result| {
                                        match result {
                                            Ok(segments) => {
                                                let dev = device.get();
                                                let total = dev.map(|d| d.total_leds).unwrap_or(0);
                                                if segments.is_empty() {
                                                    return view! {
                                                        <div class="text-xs text-fg-dim py-3 text-center">"No segments configured"</div>
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
                                                                <div class="p-2.5 rounded-lg bg-white/[0.02] border border-white/[0.03]
                                                                            hover:bg-white/[0.04] transition-colors">
                                                                    <div class="flex items-center justify-between mb-2">
                                                                        <div class="flex items-center gap-1.5">
                                                                            <span class="text-xs text-fg font-medium">{seg.name}</span>
                                                                            {is_default.then(|| view! {
                                                                                <span class="px-1.5 py-0.5 rounded text-[8px] font-mono uppercase tracking-wider text-fg-dim bg-white/[0.04]">
                                                                                    "default"
                                                                                </span>
                                                                            })}
                                                                        </div>
                                                                        <div class="flex items-center gap-2">
                                                                            <span class="text-[10px] font-mono text-fg-dim tabular-nums">
                                                                                {seg.led_start} "\u{2013}" {seg.led_end} " (" {seg.led_count} ")"
                                                                            </span>
                                                                            {(!is_default).then(|| {
                                                                                let sid = seg_id.clone();
                                                                                view! {
                                                                                    <button
                                                                                        class="p-0.5 rounded text-fg-dim hover:text-error-red transition-colors"
                                                                                        title="Delete segment"
                                                                                        on:click=move |_| delete_logical(sid.clone())
                                                                                    >
                                                                                        <svg class="w-3 h-3" viewBox="0 0 24 24" fill="none"
                                                                                             stroke="currentColor" stroke-width="2">
                                                                                            <line x1="18" y1="6" x2="6" y2="18"/>
                                                                                            <line x1="6" y1="6" x2="18" y2="18"/>
                                                                                        </svg>
                                                                                    </button>
                                                                                }
                                                                            })}
                                                                        </div>
                                                                    </div>
                                                                    // LED range bar
                                                                    <div class="h-1.5 rounded-full bg-white/[0.04] overflow-hidden">
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
            <div class="text-[10px] font-mono uppercase tracking-wider text-fg-dim mb-0.5">{label}</div>
            <div class="text-xs text-fg font-mono capitalize">{value}</div>
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
