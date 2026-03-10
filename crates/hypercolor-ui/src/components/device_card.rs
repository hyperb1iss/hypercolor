//! Device card — hardware showcase card with device-type identity and attachment silhouettes.

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

/// Device-type classification for visual identity.
enum DeviceClass {
    Keyboard,
    Mouse,
    FanController,
    LedController,
    WledController,
    Headset,
    Display,
    Other,
}

/// Classify a device by name/zone heuristics.
fn classify_device(device: &DeviceSummary) -> DeviceClass {
    let name = device.name.to_lowercase();
    let backend = device.backend.to_lowercase();

    if backend == "wled" {
        return DeviceClass::WledController;
    }

    if name.contains("push") || name.contains("huntsman") || name.contains("defy") {
        return DeviceClass::Keyboard;
    }
    if name.contains("basilisk") || name.contains("deathadder") || name.contains("viper") {
        return DeviceClass::Mouse;
    }
    if name.contains("prism") || name.contains("link") || name.contains("commander") {
        return DeviceClass::FanController;
    }
    if name.contains("seiren") || name.contains("kraken") || name.contains("nari") {
        return DeviceClass::Headset;
    }
    if name.contains("lcd") || name.contains("display") || name.contains("screen") {
        return DeviceClass::Display;
    }
    if name.contains("dram") || name.contains("aura") || name.contains("motherbo") {
        return DeviceClass::LedController;
    }

    DeviceClass::Other
}

/// Device class → icon.
fn device_class_icon(class: &DeviceClass) -> icondata_core::Icon {
    match class {
        DeviceClass::Keyboard => LuCode,
        DeviceClass::Mouse => LuMousePointerClick,
        DeviceClass::FanController => LuCpu,
        DeviceClass::LedController => LuLayers,
        DeviceClass::WledController => LuWifi,
        DeviceClass::Headset => LuAudioLines,
        DeviceClass::Display => LuMonitor,
        DeviceClass::Other => LuCircleDot,
    }
}

/// Device class → subtle accent tint for card identity (overlays on backend color).
fn device_class_pattern(class: &DeviceClass) -> &'static str {
    match class {
        DeviceClass::Keyboard => "repeating-linear-gradient(90deg, rgba(255,255,255,0.02) 0px, rgba(255,255,255,0.02) 2px, transparent 2px, transparent 6px)",
        DeviceClass::Mouse => "radial-gradient(ellipse at 60% 30%, rgba(255,255,255,0.03), transparent 70%)",
        DeviceClass::FanController => "conic-gradient(from 0deg, rgba(255,255,255,0.02), transparent 30%, rgba(255,255,255,0.02) 50%, transparent 80%)",
        DeviceClass::LedController | DeviceClass::WledController => "repeating-linear-gradient(135deg, rgba(255,255,255,0.015) 0px, rgba(255,255,255,0.015) 1px, transparent 1px, transparent 4px)",
        DeviceClass::Headset => "radial-gradient(circle at 30% 50%, rgba(255,255,255,0.03), transparent 60%)",
        DeviceClass::Display => "linear-gradient(180deg, rgba(255,255,255,0.03) 0%, transparent 40%)",
        DeviceClass::Other => "none",
    }
}

/// Compact connection label.
fn compact_label(device: &DeviceSummary) -> Option<String> {
    if let Some(label) = &device.connection_label {
        return Some(label.clone());
    }
    match (&device.network_hostname, &device.network_ip) {
        (Some(hostname), _) => Some(hostname.clone()),
        (None, Some(ip)) => Some(ip.clone()),
        _ => None,
    }
}

/// Zone topology → inline SVG shape hint for zone display.
pub fn topology_shape_svg(topology: &str) -> &'static str {
    match topology {
        "strip" => r#"<rect x="1" y="5" width="14" height="6" rx="2" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.6"/>"#,
        "ring" | "concentric_rings" => r#"<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.6"/>"#,
        "matrix" | "perimeter_loop" => r#"<rect x="2" y="2" width="12" height="12" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.6"/>"#,
        "point" => r#"<circle cx="8" cy="8" r="3" fill="currentColor" opacity="0.4"/>"#,
        _ => r#"<rect x="3" y="3" width="10" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1" opacity="0.4"/>"#,
    }
}

/// Hardware showcase device card.
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
    let device_class = classify_device(&device);
    let icon = device_class_icon(&device_class);
    let pattern = device_class_pattern(&device_class);
    let device_name = device.name.clone();
    let zone_count = device.zones.len();
    let total_leds = device.total_leds;
    let endpoint = compact_label(&device);
    let is_active = device.status.to_lowercase() == "active";
    let is_disabled = device.status.to_lowercase() == "disabled";

    let accent_gradient = format!(
        "background: linear-gradient(170deg, rgba({rgb}, 0.1) 0%, rgba({rgb}, 0.02) 40%, transparent 70%)"
    );
    let icon_bg = format!(
        "background: rgba({rgb}, 0.08); border: 1px solid rgba({rgb}, 0.12); color: rgba({rgb}, 0.7)"
    );
    let dot_style = format!(
        "background: rgb({status_rgb}); box-shadow: 0 0 6px rgba({status_rgb}, 0.5)"
    );

    let stagger = (index.min(12) + 1).to_string();

    view! {
        <button
            class=move || {
                let base = "relative rounded-xl border text-left w-full group overflow-hidden \
                            card-hover animate-fade-in-up cursor-pointer h-[108px]";
                let state = if is_selected.get() {
                    "border-accent-muted bg-surface-overlay ring-1 ring-accent-muted/20"
                } else if is_disabled {
                    "border-edge-subtle/50 bg-surface-overlay/40 opacity-60 hover:opacity-80"
                } else {
                    "border-edge-subtle bg-surface-overlay/80 hover:border-edge-default"
                };
                format!("{base} {state} stagger-{}", stagger)
            }
            style:--glow-rgb=rgb.clone()
            on:click=move |_| on_select.run(device_id.clone())
        >
            // Device-class texture pattern
            <div class="absolute inset-0 pointer-events-none rounded-xl opacity-60" style=format!("background-image: {pattern}") />

            // Backend accent wash
            <div class="absolute inset-0 pointer-events-none rounded-xl" style=accent_gradient.clone() />

            // Selected glow
            {
                let glow_rgb = rgb.clone();
                move || is_selected.get().then(|| {
                    let r = glow_rgb.clone();
                    view! {
                        <div
                            class="absolute inset-0 rounded-xl pointer-events-none animate-breathe"
                            style=format!(
                                "box-shadow: inset 0 0 24px rgba({r}, 0.06), 0 0 20px rgba({r}, 0.08)"
                            )
                        />
                    }
                })
            }

            <div class="relative flex flex-col justify-between h-full px-3.5 py-3">
                // Top: icon + name + status dot
                <div class="flex items-start gap-2.5">
                    <div class="w-8 h-8 rounded-lg flex items-center justify-center shrink-0 mt-0.5" style=icon_bg>
                        <Icon icon=icon width="16px" height="16px" />
                    </div>
                    <div class="flex-1 min-w-0">
                        <div class="flex items-center gap-2">
                            <h3 class="text-[13px] font-medium text-fg-primary truncate leading-tight">
                                {device_name}
                            </h3>
                            <div class={if is_active { "w-1.5 h-1.5 rounded-full shrink-0 dot-alive" } else { "w-1.5 h-1.5 rounded-full shrink-0" }}
                                 style=dot_style.clone()
                            />
                        </div>
                        // Subtle secondary info
                        {endpoint.map(|ep| view! {
                            <div class="text-[10px] font-mono text-fg-tertiary/60 truncate mt-0.5">{ep}</div>
                        })}
                    </div>
                </div>

                // Bottom: zone count + LED count on hover
                <div class="flex items-center justify-between">
                    {(zone_count > 0).then(|| {
                        let zone_rgb = rgb.clone();
                        view! {
                            <span class="text-[9px] font-mono tabular-nums" style=format!("color: rgba({zone_rgb}, 0.4)")>
                                {zone_count} " zones"
                            </span>
                        }
                    })}
                    <span class="text-[9px] font-mono text-fg-tertiary/40 tabular-nums
                                 opacity-0 group-hover:opacity-100 transition-opacity duration-200">
                        {total_leds} " LEDs"
                    </span>
                </div>
            </div>
        </button>
    }
}
