//! Device card — cinematic card with backend accent, hover glow, and selected state.

use leptos::prelude::*;

use crate::api::DeviceSummary;

/// Backend → accent RGB string for inline styles (case-insensitive).
pub fn backend_accent_rgb(backend: &str) -> &'static str {
    match backend.to_lowercase().as_str() {
        "razer" => "225, 53, 255",
        "wled" => "128, 255, 234",
        "openrgb" => "80, 250, 123",
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

/// Backend SVG icon path — returns an inline icon per backend type.
fn backend_icon_svg(backend: &str) -> &'static str {
    match backend.to_lowercase().as_str() {
        // Razer: diamond/gaming shape
        "razer" => r#"<path d="M12 2L2 12l10 10 10-10L12 2z" stroke="currentColor" stroke-width="1.5" fill="none"/><path d="M12 8v8M8 12h8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
        // WLED: wifi/signal shape
        "wled" => r#"<path d="M12 20h.01" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><path d="M8.5 16.5a5 5 0 017 0" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round"/><path d="M5 13a10 10 0 0114 0" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round"/>"#,
        // OpenRGB: lightbulb
        "openrgb" => r#"<path d="M9 18h6M10 22h4M12 2a7 7 0 00-4 12.7V17h8v-2.3A7 7 0 0012 2z" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
        // Corsair: sail/flag
        "corsair" | "corsair-bridge" => r#"<path d="M4 21V4a1 1 0 011-1h14l-4 6 4 6H5" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
        // Hue: bridge/circle
        "hue" => r#"<circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="1.5" fill="none"/><circle cx="12" cy="12" r="3" stroke="currentColor" stroke-width="1.5" fill="none"/><line x1="12" y1="3" x2="12" y2="9" stroke="currentColor" stroke-width="1.5"/><line x1="12" y1="15" x2="12" y2="21" stroke="currentColor" stroke-width="1.5"/>"#,
        // Generic: CPU/chip
        _ => r#"<rect x="4" y="4" width="16" height="16" rx="2" stroke="currentColor" stroke-width="1.5" fill="none"/><rect x="9" y="9" width="6" height="6" rx="1" stroke="currentColor" stroke-width="1.5" fill="none"/><line x1="9" y1="2" x2="9" y2="4" stroke="currentColor" stroke-width="1.5"/><line x1="15" y1="2" x2="15" y2="4" stroke="currentColor" stroke-width="1.5"/><line x1="9" y1="20" x2="9" y2="22" stroke="currentColor" stroke-width="1.5"/><line x1="15" y1="20" x2="15" y2="22" stroke="currentColor" stroke-width="1.5"/>"#,
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
    let icon_svg = backend_icon_svg(&device.backend);

    // Backend-colored top accent gradient
    let accent_gradient = format!(
        "background: linear-gradient(180deg, rgba({rgb}, 0.07) 0%, transparent 50%)"
    );

    // Hover glow shadow
    let hover_glow = format!(
        "0 8px 32px rgba({rgb}, 0.08), 0 0 1px rgba({rgb}, 0.2)"
    );

    let badge_style = format!(
        "background: rgba({rgb}, 0.1); color: rgb({rgb}); border-color: rgba({rgb}, 0.2)"
    );
    let dot_style = format!(
        "background: rgb({status_rgb}); box-shadow: 0 0 6px rgba({status_rgb}, 0.5)"
    );
    let icon_style = format!(
        "color: rgba({rgb}, 0.5)"
    );

    let stagger = (index.min(12) + 1).to_string();

    view! {
        <button
            class=move || {
                let base = "relative rounded-2xl border text-left w-full group overflow-hidden \
                            card-hover animate-fade-in-up cursor-pointer";
                let state = if is_selected.get() {
                    "border-electric-purple/30 bg-layer-2 animate-breathe"
                } else {
                    "border-white/[0.05] bg-layer-2/80 hover:border-white/10"
                };
                format!("{base} {state} stagger-{}", stagger)
            }
            style:--hover-glow=hover_glow.clone()
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
                            <svg viewBox="0 0 24 24" class="w-full h-full" inner_html=icon_svg />
                        </div>
                        <h3 class="text-sm font-medium text-zinc-200 group-hover:text-fg truncate transition-colors duration-200">
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
                <div class="flex items-center gap-4 text-[11px] font-mono tabular-nums text-fg-dim">
                    <div class="flex items-center gap-1.5">
                        <svg class="w-3 h-3 opacity-40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                            <circle cx="12" cy="12" r="3"/>
                        </svg>
                        <span>{total_leds} " LEDs"</span>
                    </div>
                    {(zone_count > 0).then(|| view! {
                        <div class="flex items-center gap-1.5">
                            <svg class="w-3 h-3 opacity-40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                <rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/>
                                <rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/>
                            </svg>
                            <span>{zone_count} " zones"</span>
                        </div>
                    })}
                </div>

                // Footer: status with animated dot
                <div class="flex items-center gap-2 pt-2 border-t border-white/[0.04]">
                    <div class="w-1.5 h-1.5 rounded-full shrink-0 dot-alive" style=dot_style />
                    <span class="text-[10px] text-fg-dim">{status}</span>
                </div>
            </div>
        </button>
    }
}
