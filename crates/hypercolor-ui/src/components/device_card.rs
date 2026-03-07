//! Device card — cinematic card with backend accent, hover glow, and selected state.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::DeviceSummary;
use crate::icons::*;

/// Backend → accent RGB string for inline styles (case-insensitive).
pub fn backend_accent_rgb(backend: &str) -> &'static str {
    match backend.to_lowercase().as_str() {
        "razer" => "225, 53, 255",
        "wled" => "128, 255, 234",
        "corsair" | "corsair-bridge" => "255, 153, 255",
        "hue" => "255, 183, 77",
        _ => "139, 133, 160",
    }
}

/// Status → accent RGB for indicator dot.
fn status_dot_rgb(status: &str) -> &'static str {
    match status.to_lowercase().as_str() {
        "active" => "80, 250, 123",
        "connected" => "130, 170, 255",
        "known" => "139, 133, 160",
        "disabled" => "255, 99, 99",
        _ => "139, 133, 160",
    }
}

/// Status → human label.
fn status_label(status: &str) -> &'static str {
    match status.to_lowercase().as_str() {
        "active" => "Active",
        "connected" => "Connected",
        "known" => "Known",
        "disabled" => "Disabled",
        _ => "Unknown",
    }
}

/// Backend → Lucide icon for the device type.
fn backend_icon(backend: &str) -> icondata_core::Icon {
    match backend.to_lowercase().as_str() {
        "razer" => LuDiamond,
        "wled" => LuWifi,
        "corsair" | "corsair-bridge" => LuFlag,
        "hue" => LuSun,
        _ => LuCpu,
    }
}

fn endpoint_label(device: &DeviceSummary) -> Option<String> {
    match (&device.network_hostname, &device.network_ip) {
        (Some(hostname), Some(ip)) => Some(format!("{hostname} ({ip})")),
        (Some(hostname), None) => Some(hostname.clone()),
        (None, Some(ip)) => Some(ip.clone()),
        (None, None) => None,
    }
}

/// Cinematic device card for the devices grid.
#[component]
pub fn DeviceCard(
    device: DeviceSummary,
    #[prop(into)] is_selected: Signal<bool>,
    #[prop(into)] on_select: Callback<String>,
    #[prop(default = 0)] index: usize,
) -> impl IntoView {
    let device_id = device.id.clone();
    let rgb = backend_accent_rgb(&device.backend).to_string();
    let status_rgb = status_dot_rgb(&device.status).to_string();
    let total_leds = device.total_leds;
    let zone_count = device.zones.len();
    let backend_label = device.backend.clone();
    let device_name = device.name.clone();
    let status = status_label(&device.status);
    let icon = backend_icon(&device.backend);
    let endpoint = endpoint_label(&device);

    // Backend-colored top accent gradient
    let accent_gradient =
        format!("background: linear-gradient(180deg, rgba({rgb}, 0.07) 0%, transparent 50%)");

    let badge_style =
        format!("background: rgba({rgb}, 0.1); color: rgb({rgb}); border-color: rgba({rgb}, 0.2)");
    let dot_style =
        format!("background: rgb({status_rgb}); box-shadow: 0 0 6px rgba({status_rgb}, 0.5)");
    let icon_style = format!("color: rgba({rgb}, 0.5)");

    let stagger = (index.min(12) + 1).to_string();

    view! {
        <button
            class=move || {
                let base = "relative rounded-2xl border text-left w-full group overflow-hidden \
                            card-hover animate-fade-in-up cursor-pointer";
                let state = if is_selected.get() {
                    "border-accent-muted bg-surface-overlay animate-breathe"
                } else {
                    "border-edge-subtle bg-surface-overlay/80 hover:border-edge-default"
                };
                format!("{base} {state} stagger-{}", stagger)
            }
            style:--glow-rgb=rgb.clone()
            on:click=move |_| on_select.run(device_id.clone())
        >
            // Backend accent gradient overlay
            <div class="absolute inset-0 pointer-events-none rounded-2xl" style=accent_gradient />

            // Selected electric glow
            {move || is_selected.get().then(|| view! {
                <div
                    class="absolute inset-0 rounded-2xl pointer-events-none"
                    style="background: radial-gradient(ellipse at 50% -20%, rgba(225, 53, 255, 0.15) 0%, transparent 65%); \
                           box-shadow: inset 0 1px 0 rgba(225, 53, 255, 0.2)"
                />
                <div class="absolute top-0 left-1/2 -translate-x-1/2 w-16 h-[2px] rounded-full bg-electric-purple/60 blur-[2px]" />
            })}

            <div class="relative px-4 py-4 space-y-3">
                // Header: icon + name + backend badge
                <div class="flex items-start justify-between gap-2">
                    <div class="flex items-center gap-2.5 min-w-0 flex-1">
                        <div class="w-5 h-5 shrink-0" style=icon_style>
                            <Icon icon=icon width="20px" height="20px" />
                        </div>
                        <h3 class="text-sm font-medium text-fg-primary group-hover:text-fg-primary truncate transition-colors duration-200">
                            {device_name}
                        </h3>
                    </div>
                    <span
                        class="shrink-0 px-2 py-0.5 rounded-full text-[9px] font-mono tracking-wide border capitalize"
                        style=badge_style
                    >
                        {backend_label}
                    </span>
                </div>

                // Metrics row
                <div class="flex items-center gap-4 text-[10px] font-mono tabular-nums text-fg-tertiary">
                    <div class="flex items-center gap-1.5">
                        <Icon icon=LuCircleDot width="12px" height="12px" style="opacity: 0.4" />
                        <span>{total_leds} " LEDs"</span>
                    </div>
                    {(zone_count > 0).then(|| view! {
                        <div class="flex items-center gap-1.5">
                            <Icon icon=LuGrid2x2 width="12px" height="12px" style="opacity: 0.4" />
                            <span>{zone_count} " zones"</span>
                        </div>
                    })}
                </div>

                {endpoint.map(|endpoint| {
                    view! {
                        <div class="flex items-center gap-1.5 text-[10px] font-mono text-fg-tertiary min-w-0">
                            <Icon icon=LuGlobe width="12px" height="12px" style="opacity: 0.4" />
                            <span class="truncate">{endpoint}</span>
                        </div>
                    }
                })}

                // Footer: status with animated dot
                <div class="flex items-center gap-2 pt-2 border-t border-edge-subtle">
                    <div class="w-1.5 h-1.5 rounded-full shrink-0 dot-alive" style=dot_style />
                    <span class="text-[10px] text-fg-tertiary">{status}</span>
                </div>
            </div>
        </button>
    }
}
