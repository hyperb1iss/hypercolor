//! Devices page — device management grid with detail sidebar + layout builder tab.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::app::DevicesContext;
use crate::components::device_card::DeviceCard;
use crate::components::device_detail::DeviceDetail;
use crate::components::layout_builder::LayoutBuilder;
use crate::icons::*;
use crate::toasts;

/// Status filter options.
const STATUSES: &[&str] = &["all", "active", "connected", "known", "disabled"];

/// Backend filter options.
const BACKENDS: &[&str] = &["all", "razer", "wled", "openrgb", "corsair", "hue"];

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
        "openrgb" => "80, 250, 123",
        "corsair" => "255, 153, 255",
        "hue" => "255, 183, 77",
        _ => "225, 53, 255",
    }
}

/// Devices page with tabbed layout (Devices + Layout).
#[component]
pub fn DevicesPage() -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    let (active_tab, set_active_tab) = signal("devices".to_string());
    let (search, set_search) = signal(String::new());
    let (status_filter, set_status_filter) = signal("all".to_string());
    let (backend_filter, set_backend_filter) = signal("all".to_string());
    let (selected_device, set_selected_device) = signal(None::<String>);

    // Filter devices (case-insensitive matching)
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
            // Tab bar
            <div class="shrink-0 px-6 pt-6 pb-0 bg-surface-base z-10">
                <div class="flex items-center gap-1 border-b border-edge-subtle">
                    {["devices", "layout"].into_iter().map(|tab| {
                        let tab_str = tab.to_string();
                        let tab_click = tab.to_string();
                        let is_active = {
                            let tab = tab.to_string();
                            Memo::new(move |_| active_tab.get() == tab)
                        };
                        let icon = if tab == "devices" { view! { <Icon icon=LuGrid2x2 width="16px" height="16px" /> }.into_any() } else { view! { <Icon icon=LuLayoutTemplate width="16px" height="16px" /> }.into_any() };
                        view! {
                            <button
                                class="px-4 py-2.5 text-sm font-medium transition-colors relative capitalize flex items-center gap-2"
                                class:text-fg-primary=move || is_active.get()
                                class:text-fg-tertiary=move || !is_active.get()
                                on:click=move |_| set_active_tab.set(tab_click.clone())
                            >
                                {icon}
                                {tab_str}
                                <div
                                    class="absolute bottom-0 left-0 right-0 h-[2px] bg-accent rounded-t-full transition-opacity"
                                    class:opacity-100=move || is_active.get()
                                    class:opacity-0=move || !is_active.get()
                                />
                            </button>
                        }
                    }).collect_view()}
                </div>
            </div>

            // Tab content
            {move || {
                if active_tab.get() == "layout" {
                    view! { <LayoutBuilder /> }.into_any()
                } else {
                    view! {
                        // Devices tab header
                        <div class="shrink-0 px-6 pt-4 pb-4 space-y-3 bg-surface-base z-10">
                            // Title row with scan button
                            <div class="flex items-center justify-between">
                                <div class="flex items-baseline gap-3">
                                    <h1 class="text-lg font-medium text-fg-primary">"Devices"</h1>
                                    <span class="text-[11px] font-mono text-fg-tertiary tabular-nums">
                                        {move || device_count.get()} " devices"
                                    </span>
                                </div>
                                <button
                                    class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all btn-press"
                                    style="background: rgba(128, 255, 234, 0.08); border: 1px solid rgba(128, 255, 234, 0.15); color: rgb(128, 255, 234)"
                                    on:click=move |_| {
                                        let devices_resource = ctx.devices_resource;
                                        leptos::task::spawn_local(async move {
                                            let _ = crate::api::discover_devices().await;
                                            toasts::toast_info("Scanning for devices…");
                                            devices_resource.refetch();
                                        });
                                    }
                                >
                                    <Icon icon=LuRefreshCw width="14px" height="14px" />
                                    "Scan"
                                </button>
                            </div>

                            // Search bar
                            <div class="relative">
                                <span class="absolute left-3.5 top-1/2 -translate-y-1/2 pointer-events-none text-fg-tertiary">
                                    <Icon icon=LuSearch width="14px" height="14px" />
                                </span>
                                <input
                                    type="text"
                                    placeholder="Search devices..."
                                    class="w-full bg-surface-overlay/60 border border-edge-subtle rounded-lg pl-9 pr-10 py-2 text-sm text-fg-primary
                                           placeholder-fg-tertiary focus:outline-none focus:border-accent-muted
                                           search-glow glow-ring transition-all duration-300"
                                    prop:value=move || search.get()
                                    on:input=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(el) = target { set_search.set(el.value()); }
                                    }
                                />
                                <kbd class="absolute right-3 top-1/2 -translate-y-1/2 text-[9px] font-mono text-fg-tertiary bg-surface-overlay/30 px-1.5 py-0.5 rounded border border-edge-subtle">"/"</kbd>
                            </div>

                            // Combined filter row — status + separator + backends
                            <div class="flex items-center gap-1.5 flex-wrap">
                                {STATUSES.iter().map(|s| {
                                    let s = s.to_string();
                                    let s_clone = s.clone();
                                    let rgb = if s == "all" { "225, 53, 255" } else { status_accent_rgb(&s) }.to_string();
                                    let is_active = {
                                        let s = s.clone();
                                        Memo::new(move |_| status_filter.get() == s)
                                    };
                                    let active_style = format!(
                                        "background: rgba({rgb}, 0.15); color: rgb({rgb}); border-color: rgba({rgb}, 0.3); box-shadow: 0 0 12px rgba({rgb}, 0.15)"
                                    );
                                    let inactive_style = format!(
                                        "color: rgba({rgb}, 0.5); border-color: rgba({rgb}, 0.08); background: rgba({rgb}, 0.02)"
                                    );
                                    view! {
                                        <button
                                            class="px-2.5 py-1 rounded-full text-[11px] font-medium capitalize border transition-all"
                                            style=move || if is_active.get() { active_style.clone() } else { inactive_style.clone() }
                                            on:click=move |_| set_status_filter.set(s_clone.clone())
                                        >
                                            {s.clone()}
                                        </button>
                                    }
                                }).collect_view()}

                                // Subtle separator
                                <div class="w-px h-4 bg-border-subtle mx-1" />

                                {BACKENDS.iter().skip(1).map(|b| {
                                    let b = b.to_string();
                                    let b_clone = b.clone();
                                    let rgb = backend_chip_rgb(&b).to_string();
                                    let is_active = {
                                        let b = b.clone();
                                        Memo::new(move |_| backend_filter.get() == b)
                                    };
                                    let active_style = format!(
                                        "background: rgba({rgb}, 0.15); color: rgb({rgb}); border-color: rgba({rgb}, 0.3); box-shadow: 0 0 12px rgba({rgb}, 0.15)"
                                    );
                                    let inactive_style = format!(
                                        "color: rgba({rgb}, 0.4); border-color: rgba({rgb}, 0.06); background: transparent"
                                    );
                                    view! {
                                        <button
                                            class="px-2.5 py-1 rounded-full text-[11px] font-medium capitalize border transition-all"
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

                        // Scrollable grid + detail sidebar
                        <div class="flex-1 overflow-y-auto px-6 pb-6">
                            <div class="flex gap-5 items-start">
                                // Device grid
                                <div class="flex-1 min-w-0">
                                    <Suspense fallback=move || view! { <DevicesLoadingSkeleton /> }>
                                        {move || {
                                            let devices = filtered_devices.get();
                                            if devices.is_empty() {
                                                view! {
                                                    <div class="flex flex-col items-center justify-center py-24 space-y-3">
                                                        <Icon icon=LuCpu width="48px" height="48px" style="color: rgba(139, 133, 160, 0.2)" />
                                                        <div class="text-fg-tertiary text-sm">"No devices found"</div>
                                                        <div class="text-fg-tertiary/40 text-xs">"Try a different search or filter"</div>
                                                    </div>
                                                }.into_any()
                                            } else {
                                                let has_selected = selected_device.get().is_some();
                                                let grid_class = if has_selected {
                                                    "grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3"
                                                } else {
                                                    "grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4"
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
                    }.into_any()
                }
            }}
        </div>
    }
}

/// Loading skeleton for the devices grid.
#[component]
fn DevicesLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4">
            {(0..6).map(|i| {
                let stagger = format!("animation-delay: {}ms", i * 80);
                view! {
                    <div class="rounded-2xl border border-edge-subtle bg-surface-overlay/40 px-4 py-4 animate-pulse space-y-3" style=stagger>
                        <div class="flex justify-between items-start">
                            <div class="flex items-center gap-2.5">
                                <div class="w-5 h-5 bg-surface-overlay/40 rounded" />
                                <div class="h-4 w-32 bg-surface-overlay/40 rounded" />
                            </div>
                            <div class="h-4 w-14 bg-surface-overlay/40 rounded-full" />
                        </div>
                        <div class="flex gap-4">
                            <div class="h-3 w-16 bg-surface-overlay/20 rounded" />
                            <div class="h-3 w-16 bg-surface-overlay/20 rounded" />
                        </div>
                        <div class="flex items-center gap-2 pt-2 border-t border-edge-subtle">
                            <div class="w-1.5 h-1.5 bg-surface-overlay/40 rounded-full" />
                            <div class="h-2.5 w-16 bg-surface-overlay/20 rounded" />
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
