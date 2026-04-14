//! Devices page — hardware gallery grid with resizable detail sidebar.
//! All layout state (selected device, sidebar width, filters) persists via localStorage.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::api::DeviceSummary;
use crate::app::DevicesContext;
use crate::components::device_card::DeviceCard;
use crate::components::device_detail::DeviceDetail;
use crate::components::device_pairing_modal::{DevicePairingModal, ForgetCredentialsModal};
use crate::components::page_header::PageHeader;
use crate::components::resize_handle::ResizeHandle;
use crate::icons::*;
use crate::style_utils::filter_chips;
use crate::toasts;

// ── LocalStorage keys + helpers ─────────────────────────────────────────────

const SIDEBAR_DEFAULT: f64 = 400.0;
const SIDEBAR_MIN: f64 = 300.0;
const SIDEBAR_MAX: f64 = 600.0;

const LS_SIDEBAR: &str = "hc-devices-sidebar-width";
const LS_SELECTED: &str = "hc-devices-selected";
const LS_STATUS: &str = "hc-devices-status-filter";
const LS_BACKEND: &str = "hc-devices-backend-filter";

fn ls_get(key: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(key).ok().flatten())
}

fn ls_set(key: &str, value: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(key, value);
    }
}

fn ls_remove(key: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.remove_item(key);
    }
}

fn load_sidebar_width() -> f64 {
    ls_get(LS_SIDEBAR)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(SIDEBAR_DEFAULT)
        .clamp(SIDEBAR_MIN, SIDEBAR_MAX)
}

// ── Filter definitions ──────────────────────────────────────────────────────

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
    ("nanoleaf", "100, 220, 160"),
];

// ── Page component ──────────────────────────────────────────────────────────

#[component]
pub fn DevicesPage() -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    // Restore persisted state
    let (search, set_search) = signal(String::new());
    let (status_filter, set_status_filter) =
        signal(ls_get(LS_STATUS).unwrap_or_else(|| "all".to_string()));
    let (backend_filter, set_backend_filter) =
        signal(ls_get(LS_BACKEND).unwrap_or_else(|| "all".to_string()));
    let (selected_device, set_selected_device) = signal(ls_get(LS_SELECTED));

    // Persist filter changes
    Effect::new(move |_| {
        let s = status_filter.get();
        if s == "all" {
            ls_remove(LS_STATUS);
        } else {
            ls_set(LS_STATUS, &s);
        }
    });
    Effect::new(move |_| {
        let b = backend_filter.get();
        if b == "all" {
            ls_remove(LS_BACKEND);
        } else {
            ls_set(LS_BACKEND, &b);
        }
    });
    Effect::new(move |_| {
        if let Some(id) = selected_device.get() {
            ls_set(LS_SELECTED, &id);
        } else {
            ls_remove(LS_SELECTED);
        }
    });

    // Resizable sidebar
    let (sidebar_width, set_sidebar_width) = signal(load_sidebar_width());
    let sidebar_start = StoredValue::new(0.0_f64);

    let on_drag_start = Callback::new(move |()| {
        sidebar_start.set_value(sidebar_width.get_untracked());
    });
    let on_drag = Callback::new(move |delta: f64| {
        if let Some(start) = sidebar_start.try_get_value() {
            let clamped = (start - delta).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
            set_sidebar_width.set(clamped);
        }
    });
    let on_drag_end = Callback::new(move |()| {
        if let Some(width) = sidebar_width.try_get_untracked() {
            ls_set(LS_SIDEBAR, &format!("{width:.0}"));
        }
    });

    let filtered_devices = Memo::new(move |_| {
        let Some(Ok(devices)) = ctx.devices_resource.get() else {
            return Vec::new();
        };

        let search_term = search.get().to_lowercase();
        let status = status_filter.get();
        let backend = backend_filter.get();

        let mut filtered: Vec<_> = devices
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
            .collect();

        // Sort: active > connected > reconnecting > known > disabled,
        // alphabetical inside each tier. Disconnected gear drops to the bottom
        // so the grid leads with what's live right now.
        filtered.sort_by(|a, b| {
            let tier = |status: &str| -> u8 {
                match status.to_lowercase().as_str() {
                    "active" => 0,
                    "connected" => 1,
                    "reconnecting" => 2,
                    "known" => 3,
                    "disabled" => 4,
                    _ => 5,
                }
            };
            tier(&a.status)
                .cmp(&tier(&b.status))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        filtered
    });

    let device_count = Memo::new(move |_| {
        ctx.devices_resource
            .get()
            .and_then(|r| r.ok())
            .map(|d| d.len())
            .unwrap_or(0)
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

    // Pairing modal state — holds the device to show the pairing/forget modal for
    let (pairing_device, set_pairing_device) = signal(Option::<DeviceSummary>::None);
    let (forget_device, set_forget_device) = signal(Option::<DeviceSummary>::None);

    let on_pair_device = Callback::new(move |device_id: String| {
        if let Some(Ok(devices)) = ctx.devices_resource.get()
            && let Some(dev) = devices.into_iter().find(|d| d.id == device_id)
        {
            set_pairing_device.set(Some(dev));
        }
    });

    let on_forget_device = Callback::new(move |device_id: String| {
        if let Some(Ok(devices)) = ctx.devices_resource.get()
            && let Some(dev) = devices.into_iter().find(|d| d.id == device_id)
        {
            set_forget_device.set(Some(dev));
        }
    });

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

    let _close_listener = window_event_listener(ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            set_filter_dropdown_open.set(false);
        }
    });

    view! {
        <div class="flex flex-col h-full animate-fade-in">
            <div class="shrink-0 glass-subtle border-b border-edge-default relative">
                <div class="px-6 pt-5 pb-4">
                    <div class="flex items-end justify-between gap-4">
                        <PageHeader
                            icon=LuCpu
                            title="Devices"
                            subtitle="Your hardware — pair, identify, and tune."
                            accent_rgb="128, 255, 234"
                            gradient="linear-gradient(105deg,#80ffea 0%,#e8f4ff 55%,#80ffea 100%)"
                        />

                        <span class="shrink-0 text-[11px] font-mono text-fg-tertiary/55 tabular-nums">
                            {move || {
                                let total = device_count.get();
                                let filtered = filtered_devices.get().len();
                                if filtered == total {
                                    format!("{total} devices")
                                } else {
                                    format!("{filtered}/{total} devices")
                                }
                            }}
                        </span>
                    </div>
                </div>

                <div class="px-6 pb-3 flex items-center gap-3">
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
                                    <span class="min-w-[16px] h-4 flex items-center justify-center rounded-full text-[9px] font-mono"
                                          style="background: rgba(225, 53, 255, 0.3); color: rgb(225, 53, 255)">
                                        {count}
                                    </span>
                                })
                            }}
                            <span class="w-3 h-3 flex items-center justify-center transition-transform duration-200"
                                  class:rotate-180=move || filter_dropdown_open.get()>
                                <Icon icon=LuChevronDown width="11px" height="11px" />
                            </span>
                        </button>

                        {move || filter_dropdown_open.get().then(|| view! {
                            <div class="fixed inset-0 z-20" on:click=move |_| set_filter_dropdown_open.set(false) />
                            <div class="absolute top-full right-0 mt-1 z-30 w-[240px] max-h-[360px] overflow-y-auto
                                       rounded-xl border border-edge-subtle bg-surface-overlay dropdown-glow
                                       py-1.5 animate-fade-in animate-glow-reveal scrollbar-dropdown">
                                <div class="px-3 pt-1 pb-1.5">
                                    <div class="text-[10px] font-medium uppercase tracking-wider text-fg-tertiary/50 mb-1.5">"Status"</div>
                                    <div class="flex gap-1 flex-wrap">
                                        {filter_chips(STATUS_CHIPS, status_filter, set_status_filter)}
                                    </div>
                                </div>
                                <div class="h-px bg-border-subtle/30 mx-2 my-1" />
                                <div class="px-3 pt-1 pb-1.5">
                                    <div class="text-[10px] font-medium uppercase tracking-wider text-fg-tertiary/50 mb-1.5">"Backend"</div>
                                    <div class="flex gap-1 flex-wrap">
                                        {filter_chips(BACKEND_CHIPS, backend_filter, set_backend_filter)}
                                    </div>
                                </div>
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

                // Accent strip
                <div class="absolute bottom-0 left-0 right-0 h-[2px]"
                     style="background: linear-gradient(90deg, rgba(128, 255, 234, 0.4), rgba(128, 255, 234, 0.05))" />
            </div>

            // Grid + resizable sidebar
            <div class="flex-1 overflow-hidden">
                <div class="flex h-full">
                    <div class="flex-1 min-w-0 overflow-y-auto px-6 pb-6 pt-4">
                        <Suspense fallback=move || view! { <DevicesLoadingSkeleton /> }>
                            {move || {
                                let devices = filtered_devices.get();
                                if devices.is_empty() {
                                    view! {
                                        <div class="flex flex-col items-center justify-center py-20 space-y-3">
                                            <Icon icon=LuCpu width="36px" height="36px"
                                                  style="color: rgba(128, 255, 234, 0.25); filter: drop-shadow(0 0 12px rgba(128, 255, 234, 0.15))" />
                                            <div class="text-fg-tertiary/50 text-xs font-mono tracking-wide">"No devices found"</div>
                                        </div>
                                    }.into_any()
                                } else {
                                    let has_selected = selected_device.get().is_some();
                                    let grid_class = if has_selected {
                                        "grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-2.5"
                                    } else {
                                        "grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-3"
                                    };
                                    view! {
                                        <div class=grid_class>
                                            {devices.into_iter().enumerate().map(|(i, dev)| {
                                                let dev_id = dev.id.clone();
                                                let is_selected = Signal::derive(move || {
                                                    selected_device.get().as_deref() == Some(&dev_id)
                                                });
                                                view! {
                                                    <DeviceCard device=dev is_selected=is_selected on_select=on_select_device on_pair=on_pair_device index=i />
                                                }
                                            }).collect_view()}
                                        </div>
                                    }.into_any()
                                }
                            }}
                        </Suspense>
                    </div>

                    {move || selected_device.get().map(|id| {
                        let device_id = Signal::derive(move || id.clone());
                        view! {
                            <ResizeHandle on_drag_start=on_drag_start on_drag=on_drag on_drag_end=on_drag_end />
                            <aside
                                class="shrink-0 overflow-y-auto pb-6 pr-6 pt-4 scrollbar-none animate-slide-in-right"
                                style=move || format!("width: {:.0}px", sidebar_width.get())
                            >
                                <DeviceDetail device_id=device_id on_pair=on_pair_device on_forget=on_forget_device />
                            </aside>
                        }
                    })}
                </div>
            </div>

            // ── Pairing modal overlay ────────────────────────────────────
            {move || pairing_device.get().map(|dev| {
                view! {
                    <DevicePairingModal
                        device=dev
                        on_close=Callback::new(move |()| set_pairing_device.set(None))
                        on_paired=Callback::new(move |paired_id: String| {
                            // Guard: only dismiss if the modal still belongs to this device.
                            // A stale async response from a previously-closed modal must not
                            // dismiss a modal the user opened for a different device.
                            if pairing_device.get().as_ref().is_some_and(|d| d.id == paired_id) {
                                set_pairing_device.set(None);
                            }
                        })
                    />
                }
            })}

            // ── Forget credentials modal overlay ─────────────────────────
            {move || forget_device.get().map(|dev| {
                view! {
                    <ForgetCredentialsModal
                        device=dev
                        on_close=Callback::new(move |()| set_forget_device.set(None))
                        on_forgot=Callback::new(move |forgot_id: String| {
                            if forget_device.get().as_ref().is_some_and(|d| d.id == forgot_id) {
                                set_forget_device.set(None);
                            }
                        })
                    />
                }
            })}
        </div>
    }
}

#[component]
fn DevicesLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-3">
            {(0..6).map(|i| {
                let stagger = format!("animation-delay: {}ms", i * 60);
                view! {
                    <div class="rounded-xl border border-edge-subtle/50 bg-surface-overlay/30 h-[140px] animate-pulse" style=stagger>
                        <div class="px-4 py-3 space-y-3">
                            <div class="flex items-center gap-2.5">
                                <div class="w-9 h-9 bg-surface-overlay/30 rounded-lg" />
                                <div class="space-y-1.5 flex-1">
                                    <div class="h-3.5 w-28 bg-surface-overlay/30 rounded" />
                                    <div class="h-2.5 w-20 bg-surface-overlay/20 rounded" />
                                </div>
                            </div>
                            <div class="flex gap-1.5">
                                <div class="w-10 h-5 bg-surface-overlay/15 rounded-full" />
                                <div class="w-10 h-5 bg-surface-overlay/15 rounded-full" />
                            </div>
                            <div class="flex justify-between items-center">
                                <div class="h-2.5 w-16 bg-surface-overlay/15 rounded" />
                                <div class="h-[3px] w-12 bg-surface-overlay/15 rounded-full" />
                            </div>
                        </div>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
