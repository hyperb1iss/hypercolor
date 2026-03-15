//! Devices page — hardware gallery grid with detail sidebar.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::app::DevicesContext;
use crate::components::device_card::DeviceCard;
use crate::components::device_detail::DeviceDetail;
use crate::icons::*;
use crate::toasts;

/// Filter chip definition: (label, accent RGB).
const STATUS_CHIPS: &[(&str, &str)] = &[
    ("all", "225, 53, 255"),
    ("active", "80, 250, 123"),
    ("connected", "130, 170, 255"),
    ("known", "139, 133, 160"),
    ("disabled", "255, 99, 99"),
];

const BACKEND_CHIPS: &[(&str, &str)] = &[
    ("all", "225, 53, 255"),
    ("razer", "225, 53, 255"),
    ("wled", "128, 255, 234"),
    ("corsair", "255, 153, 255"),
    ("hue", "255, 183, 77"),
];

/// Render a row of filter chip buttons.
fn filter_chips(
    chips: &'static [(&'static str, &'static str)],
    current: ReadSignal<String>,
    set_current: WriteSignal<String>,
) -> impl IntoView {
    chips
        .iter()
        .map(|&(label, rgb)| {
            let is_active = Memo::new(move |_| current.get() == label);
            let active_style = format!(
                "background: rgba({rgb}, 0.15); color: rgb({rgb}); border-color: rgba({rgb}, 0.3); box-shadow: 0 0 8px rgba({rgb}, 0.15)"
            );
            let inactive_style = format!(
                "color: rgba({rgb}, 0.5); border-color: rgba({rgb}, 0.08); background: transparent"
            );
            view! {
                <button
                    class="px-2 py-0.5 rounded-full text-[10px] font-medium capitalize border transition-all"
                    style=move || if is_active.get() { active_style.clone() } else { inactive_style.clone() }
                    on:click=move |_| set_current.set(label.to_string())
                >
                    {label}
                </button>
            }
        })
        .collect_view()
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

    let on_select_device = Callback::new(move |id: String| {
        let current = selected_device.get();
        if current.as_deref() == Some(&id) {
            set_selected_device.set(None);
        } else {
            set_selected_device.set(Some(id));
        }
    });

    let (filter_dropdown_open, set_filter_dropdown_open) = signal(false);

    let active_filter_count = Memo::new(move |_| {
        let mut count = 0usize;
        if status_filter.get() != "all" {
            count += 1;
        }
        if backend_filter.get() != "all" {
            count += 1;
        }
        count
    });

    view! {
        <div class="flex flex-col h-full -m-6 animate-fade-in">
            // Header — title + search + filters on one line
            <div class="shrink-0 px-6 pt-5 pb-3 bg-surface-base z-10">
                <div class="flex items-center gap-3">
                    <h1 class="text-lg font-medium text-fg-primary shrink-0">"Devices"</h1>

                    // Search bar — fills available space
                    <div class="relative flex-1 min-w-0">
                        <span class="absolute left-3 top-1/2 -translate-y-1/2 pointer-events-none text-fg-tertiary">
                            <Icon icon=LuSearch width="14px" height="14px" />
                        </span>
                        <input
                            type="text"
                            placeholder="Search devices..."
                            class="w-full bg-surface-overlay/60 border border-edge-subtle rounded-lg pl-9 pr-10 py-1.5 text-sm text-fg-primary
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

                    // Filters dropdown
                    <div class="relative shrink-0">
                        <button
                            class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs font-medium border transition-all duration-200"
                            style=move || {
                                if active_filter_count.get() > 0 {
                                    "background: rgba(225, 53, 255, 0.12); color: rgb(225, 53, 255); border-color: rgba(225, 53, 255, 0.25); box-shadow: 0 0 10px rgba(225, 53, 255, 0.15)"
                                } else {
                                    "color: rgba(139, 133, 160, 0.6); border-color: rgba(139, 133, 160, 0.12); background: rgba(139, 133, 160, 0.03)"
                                }
                            }
                            on:click=move |_| set_filter_dropdown_open.update(|v| *v = !*v)
                        >
                            <Icon icon=LuSlidersHorizontal width="13px" height="13px" />
                            "Filters"
                            {move || {
                                let count = active_filter_count.get();
                                (count > 0).then(|| view! {
                                    <span
                                        class="min-w-[16px] h-4 flex items-center justify-center rounded-full text-[9px] font-mono"
                                        style="background: rgba(225, 53, 255, 0.3); color: rgb(225, 53, 255)"
                                    >
                                        {count}
                                    </span>
                                })
                            }}
                            <span
                                class="w-3 h-3 flex items-center justify-center transition-transform duration-200"
                                class:rotate-180=move || filter_dropdown_open.get()
                            >
                                <Icon icon=LuChevronDown width="11px" height="11px" />
                            </span>
                        </button>

                        {move || filter_dropdown_open.get().then(|| view! {
                            // Backdrop
                            <div
                                class="fixed inset-0 z-20"
                                on:click=move |_| set_filter_dropdown_open.set(false)
                            />
                            <div
                                class="absolute top-full right-0 mt-1 z-30 w-[220px] max-h-[320px] overflow-y-auto
                                       rounded-xl border border-edge-subtle bg-surface-overlay dropdown-glow
                                       py-1.5 animate-fade-in animate-glow-reveal scrollbar-dropdown"
                            >
                                // ── Status section ──
                                <div class="px-3 pt-1 pb-1.5">
                                    <div class="text-[10px] font-medium uppercase tracking-wider text-fg-tertiary/50 mb-1.5">"Status"</div>
                                    <div class="flex gap-1 flex-wrap">
                                        {filter_chips(STATUS_CHIPS, status_filter, set_status_filter)}
                                    </div>
                                </div>

                                <div class="h-px bg-border-subtle/30 mx-2 my-1" />

                                // ── Backend section ──
                                <div class="px-3 pt-1 pb-1.5">
                                    <div class="text-[10px] font-medium uppercase tracking-wider text-fg-tertiary/50 mb-1.5">"Backend"</div>
                                    <div class="flex gap-1 flex-wrap">
                                        {filter_chips(BACKEND_CHIPS, backend_filter, set_backend_filter)}
                                    </div>
                                </div>

                                // ── Clear all ──
                                {move || (active_filter_count.get() > 0).then(|| view! {
                                    <div class="h-px bg-border-subtle/30 mx-2 my-1" />
                                    <button
                                        class="w-full text-left px-5 py-1.5 text-[11px] text-fg-tertiary hover:text-fg-secondary hover:bg-surface-hover/40 transition-colors"
                                        on:click=move |_| {
                                            set_status_filter.set("all".to_string());
                                            set_backend_filter.set("all".to_string());
                                        }
                                    >
                                        "Clear all filters"
                                    </button>
                                })}
                            </div>
                        })}
                    </div>

                    // Scan button
                    <button
                        class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] font-medium transition-all btn-press shrink-0"
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
