//! Devices page — hardware gallery grid with detail sidebar.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::app::DevicesContext;
use crate::components::device_card::DeviceCard;
use crate::components::device_detail::DeviceDetail;
use crate::icons::*;
use crate::toasts;

/// Status filter options.
const STATUSES: &[&str] = &["all", "active", "connected", "known", "disabled"];

/// Backend filter options.
const BACKENDS: &[&str] = &["all", "razer", "wled", "corsair", "hue"];

/// Status → accent RGB for filter chips.
fn status_accent_rgb(status: &str) -> &'static str {
    match status {
        "active" => "80, 250, 123",
        "connected" => "130, 170, 255",
        "known" => "139, 133, 160",
        "disabled" => "255, 99, 99",
        _ => "225, 53, 255",
    }
}

/// Backend → accent RGB for filter chips.
fn backend_chip_rgb(backend: &str) -> &'static str {
    match backend {
        "razer" => "225, 53, 255",
        "wled" => "128, 255, 234",
        "corsair" => "255, 153, 255",
        "hue" => "255, 183, 77",
        _ => "225, 53, 255",
    }
}

/// Devices page with filter chips, search, and device grid.
#[component]
pub fn DevicesPage() -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    let (search, set_search) = signal(String::new());
    let (status_filter, set_status_filter) = signal("all".to_string());
    let (backend_filter, set_backend_filter) = signal("all".to_string());
    let (selected_device, set_selected_device) = signal(None::<String>);

    let filtered_devices = Memo::new(move |_| {
        let Some(Ok(devices)) = ctx.devices_resource.get() else {
            return Vec::new();
        };

        let search_term = search.get().to_lowercase();
        let status = status_filter.get();
        let backend = backend_filter.get();

        devices
            .into_iter()
            .filter(|d| {
                if status != "all" && d.status.to_lowercase() != status {
                    return false;
                }
                if backend != "all" && d.backend.to_lowercase() != backend {
                    return false;
                }
                if !search_term.is_empty() {
                    let matches_name = d.name.to_lowercase().contains(&search_term);
                    let matches_backend = d.backend.to_lowercase().contains(&search_term);
                    return matches_name || matches_backend;
                }
                true
            })
            .collect::<Vec<_>>()
    });

    let device_count = Memo::new(move |_| filtered_devices.get().len());

    let on_select_device = Callback::new(move |id: String| {
        let current = selected_device.get();
        if current.as_deref() == Some(&id) {
            set_selected_device.set(None);
        } else {
            set_selected_device.set(Some(id));
        }
    });

    view! {
        <div class="flex flex-col h-full -m-6 animate-fade-in">
            // Header
            <div class="shrink-0 px-6 pt-6 pb-3 space-y-2.5 bg-surface-base z-10">
                // Title + scan
                <div class="flex items-center justify-between">
                    <div class="flex items-baseline gap-2.5">
                        <h1 class="text-lg font-medium text-fg-primary">"Devices"</h1>
                        <span class="text-[10px] font-mono text-fg-tertiary/50 tabular-nums">
                            {move || device_count.get()}
                        </span>
                    </div>
                    <button
                        class="flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-[11px] font-medium transition-all btn-press"
                        style="background: rgba(128, 255, 234, 0.06); border: 1px solid rgba(128, 255, 234, 0.1); color: rgba(128, 255, 234, 0.8)"
                        on:click=move |_| {
                            let devices_resource = ctx.devices_resource;
                            leptos::task::spawn_local(async move {
                                let _ = crate::api::discover_devices().await;
                                toasts::toast_info("Scanning...");
                                devices_resource.refetch();
                            });
                        }
                    >
                        <Icon icon=LuRefreshCw width="12px" height="12px" />
                        "Scan"
                    </button>
                </div>

                // Search
                <div class="relative">
                    <span class="absolute left-3 top-1/2 -translate-y-1/2 pointer-events-none text-fg-tertiary/40">
                        <Icon icon=LuSearch width="13px" height="13px" />
                    </span>
                    <input
                        type="text"
                        placeholder="Search devices..."
                        class="w-full bg-surface-overlay/50 border border-edge-subtle rounded-lg pl-8 pr-8 py-1.5 text-[12px] text-fg-primary
                               placeholder-fg-tertiary/40 focus:outline-none focus:border-accent-muted
                               search-glow glow-ring transition-all duration-300"
                        prop:value=move || search.get()
                        on:input=move |ev| {
                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                            if let Some(el) = target { set_search.set(el.value()); }
                        }
                    />
                    <kbd class="absolute right-2.5 top-1/2 -translate-y-1/2 text-[8px] font-mono text-fg-tertiary/30 bg-surface-overlay/20 px-1 py-0.5 rounded border border-edge-subtle/50">"/"</kbd>
                </div>

                // Filters: status + backends
                <div class="flex items-center gap-1 flex-wrap">
                    {STATUSES.iter().map(|s| {
                        let s = s.to_string();
                        let s_clone = s.clone();
                        let rgb = if s == "all" { "225, 53, 255" } else { status_accent_rgb(&s) }.to_string();
                        let is_active = {
                            let s = s.clone();
                            Memo::new(move |_| status_filter.get() == s)
                        };
                        let active_style = format!(
                            "background: rgba({rgb}, 0.12); color: rgb({rgb}); border-color: rgba({rgb}, 0.25)"
                        );
                        let inactive_style = format!(
                            "color: rgba({rgb}, 0.35); border-color: transparent; background: transparent"
                        );
                        view! {
                            <button
                                class="px-2 py-0.5 rounded-full text-[10px] font-medium capitalize border transition-all"
                                style=move || if is_active.get() { active_style.clone() } else { inactive_style.clone() }
                                on:click=move |_| set_status_filter.set(s_clone.clone())
                            >
                                {s.clone()}
                            </button>
                        }
                    }).collect_view()}

                    <div class="w-px h-3 bg-border-subtle/30 mx-0.5" />

                    {BACKENDS.iter().skip(1).map(|b| {
                        let b = b.to_string();
                        let b_clone = b.clone();
                        let rgb = backend_chip_rgb(&b).to_string();
                        let is_active = {
                            let b = b.clone();
                            Memo::new(move |_| backend_filter.get() == b)
                        };
                        let active_style = format!(
                            "background: rgba({rgb}, 0.12); color: rgb({rgb}); border-color: rgba({rgb}, 0.25)"
                        );
                        let inactive_style = format!(
                            "color: rgba({rgb}, 0.3); border-color: transparent; background: transparent"
                        );
                        view! {
                            <button
                                class="px-2 py-0.5 rounded-full text-[10px] font-medium capitalize border transition-all"
                                style=move || {
                                    if is_active.get() {
                                        active_style.clone()
                                    } else {
                                        inactive_style.clone()
                                    }
                                }
                                on:click=move |_| {
                                    let current = backend_filter.get();
                                    if current == b_clone {
                                        set_backend_filter.set("all".to_string());
                                    } else {
                                        set_backend_filter.set(b_clone.clone());
                                    }
                                }
                            >
                                {b.clone()}
                            </button>
                        }
                    }).collect_view()}
                </div>
            </div>

            // Grid + sidebar
            <div class="flex-1 overflow-y-auto px-6 pb-6">
                <div class="flex gap-4 items-start">
                    // Device grid
                    <div class="flex-1 min-w-0">
                        <Suspense fallback=move || view! { <DevicesLoadingSkeleton /> }>
                            {move || {
                                let devices = filtered_devices.get();
                                if devices.is_empty() {
                                    view! {
                                        <div class="flex flex-col items-center justify-center py-20 space-y-2">
                                            <Icon icon=LuCpu width="36px" height="36px" style="color: rgba(139, 133, 160, 0.15)" />
                                            <div class="text-fg-tertiary/40 text-xs">"No devices found"</div>
                                        </div>
                                    }.into_any()
                                } else {
                                    let has_selected = selected_device.get().is_some();
                                    let grid_class = if has_selected {
                                        "grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-2.5"
                                    } else {
                                        "grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3"
                                    };
                                    view! {
                                        <div class=grid_class>
                                            {devices.into_iter().enumerate().map(|(i, dev)| {
                                                let dev_id = dev.id.clone();
                                                let is_selected = Signal::derive(move || {
                                                    selected_device.get().as_deref() == Some(&dev_id)
                                                });
                                                view! {
                                                    <DeviceCard
                                                        device=dev
                                                        is_selected=is_selected
                                                        on_select=on_select_device
                                                        index=i
                                                    />
                                                }
                                            }).collect_view()}
                                        </div>
                                    }.into_any()
                                }
                            }}
                        </Suspense>
                    </div>

                    // Detail sidebar
                    {move || selected_device.get().map(|id| {
                        let device_id = Signal::derive(move || id.clone());
                        view! {
                            <DeviceDetail device_id=device_id />
                        }
                    })}
                </div>
            </div>
        </div>
    }
}

/// Loading skeleton.
#[component]
fn DevicesLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3">
            {(0..6).map(|i| {
                let stagger = format!("animation-delay: {}ms", i * 60);
                view! {
                    <div class="rounded-xl border border-edge-subtle/50 bg-surface-overlay/30 h-[108px] animate-pulse" style=stagger>
                        <div class="px-3.5 py-3 space-y-3">
                            <div class="flex items-center gap-2.5">
                                <div class="w-8 h-8 bg-surface-overlay/30 rounded-lg" />
                                <div class="space-y-1.5 flex-1">
                                    <div class="h-3.5 w-28 bg-surface-overlay/30 rounded" />
                                    <div class="h-2.5 w-16 bg-surface-overlay/20 rounded" />
                                </div>
                            </div>
                            <div class="flex gap-1.5">
                                <div class="w-4 h-4 bg-surface-overlay/15 rounded" />
                                <div class="w-4 h-4 bg-surface-overlay/15 rounded" />
                            </div>
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
