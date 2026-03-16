//! Device card — hardware showcase card with device-type identity, metadata, and zone topology.

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
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Keyboard,
    Mouse,
    Hub,
    Controller,
    WledController,
    SmartLight,
    Audio,
    Display,
    Other,
}

/// All device classes for the category picker.
pub const ALL_DEVICE_CLASSES: &[DeviceClass] = &[
    DeviceClass::Keyboard,
    DeviceClass::Mouse,
    DeviceClass::Hub,
    DeviceClass::Controller,
    DeviceClass::WledController,
    DeviceClass::SmartLight,
    DeviceClass::Audio,
    DeviceClass::Display,
    DeviceClass::Other,
];

/// Parse a device class from its label string.
pub fn parse_device_class(label: &str) -> Option<DeviceClass> {
    ALL_DEVICE_CLASSES.iter().find(|c| device_class_label(c) == label).copied()
}

/// Classify a device by name/zone heuristics (auto-detection).
pub fn classify_device(device: &DeviceSummary) -> DeviceClass {
    // Check localStorage override first
    if let Some(override_label) = load_category_override(&device.id) {
        if let Some(class) = parse_device_class(&override_label) {
            return class;
        }
    }

    let name = device.name.to_lowercase();
    let backend = device.backend.to_lowercase();

    if backend == "wled" {
        return DeviceClass::WledController;
    }
    if backend == "hue" {
        return DeviceClass::SmartLight;
    }

    if name.contains("push") || name.contains("huntsman") || name.contains("defy")
        || name.contains("keyboard")
    {
        return DeviceClass::Keyboard;
    }
    if name.contains("basilisk") || name.contains("deathadder") || name.contains("viper")
        || name.contains("mouse")
    {
        return DeviceClass::Mouse;
    }
    if name.contains("prism") || name.contains("link") || name.contains("commander")
        || name.contains("hub")
    {
        return DeviceClass::Hub;
    }
    if name.contains("seiren") || name.contains("kraken") || name.contains("nari")
        || name.contains("mic") || name.contains("headset")
    {
        return DeviceClass::Audio;
    }
    if name.contains("lcd") || name.contains("display") || name.contains("screen") {
        return DeviceClass::Display;
    }
    if name.contains("dram") || name.contains("aura") || name.contains("motherbo") {
        return DeviceClass::Controller;
    }

    DeviceClass::Other
}

fn load_category_override(device_id: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(&format!("hc-device-category-{device_id}")).ok().flatten())
}

pub fn save_category_override(device_id: &str, label: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(&format!("hc-device-category-{device_id}"), label);
    }
}

/// Device class → icon.
pub fn device_class_icon(class: &DeviceClass) -> icondata_core::Icon {
    match class {
        DeviceClass::Keyboard => LuKeyboard,
        DeviceClass::Mouse => LuMousePointerClick,
        DeviceClass::Hub => LuNetwork,
        DeviceClass::Controller => LuLayers,
        DeviceClass::WledController => LuWifi,
        DeviceClass::SmartLight => LuLightbulb,
        DeviceClass::Audio => LuMic,
        DeviceClass::Display => LuMonitor,
        DeviceClass::Other => LuCircleDot,
    }
}

/// Device class → human-readable label.
pub fn device_class_label(class: &DeviceClass) -> &'static str {
    match class {
        DeviceClass::Keyboard => "Keyboard",
        DeviceClass::Mouse => "Mouse",
        DeviceClass::Hub => "Hub",
        DeviceClass::Controller => "Controller",
        DeviceClass::WledController => "WLED",
        DeviceClass::SmartLight => "Smart Light",
        DeviceClass::Audio => "Audio",
        DeviceClass::Display => "Display",
        DeviceClass::Other => "Device",
    }
}

/// Device class → type-specific accent RGB (layered over backend color).
fn device_class_tint(class: &DeviceClass) -> &'static str {
    match class {
        DeviceClass::Keyboard => "255, 200, 120",
        DeviceClass::Mouse => "130, 180, 255",
        DeviceClass::Hub => "100, 220, 200",
        DeviceClass::Controller => "180, 170, 200",
        DeviceClass::WledController => "128, 255, 234",
        DeviceClass::SmartLight => "255, 183, 77",
        DeviceClass::Audio => "200, 130, 255",
        DeviceClass::Display => "140, 200, 255",
        DeviceClass::Other => "139, 133, 160",
    }
}

/// Infer connection type from device metadata.
fn connection_type(device: &DeviceSummary) -> &'static str {
    if device.network_ip.is_some() || device.network_hostname.is_some() {
        return "Network";
    }
    match device.backend.to_lowercase().as_str() {
        "wled" | "hue" => "Network",
        _ => "USB",
    }
}

/// Connection type → icon.
fn connection_icon(conn: &str) -> icondata_core::Icon {
    match conn {
        "Network" => LuGlobe,
        _ => LuCable,
    }
}

/// Device class → subtle accent tint for card identity (overlays on backend color).
fn device_class_pattern(class: &DeviceClass) -> &'static str {
    match class {
        DeviceClass::Keyboard => {
            "repeating-linear-gradient(90deg, rgba(255,255,255,0.02) 0px, rgba(255,255,255,0.02) 2px, transparent 2px, transparent 6px)"
        }
        DeviceClass::Mouse => {
            "radial-gradient(ellipse at 60% 30%, rgba(255,255,255,0.03), transparent 70%)"
        }
        DeviceClass::Hub => {
            "conic-gradient(from 0deg, rgba(255,255,255,0.02), transparent 30%, rgba(255,255,255,0.02) 50%, transparent 80%)"
        }
        DeviceClass::Controller | DeviceClass::WledController => {
            "repeating-linear-gradient(135deg, rgba(255,255,255,0.015) 0px, rgba(255,255,255,0.015) 1px, transparent 1px, transparent 4px)"
        }
        DeviceClass::SmartLight => {
            "radial-gradient(ellipse at 50% 20%, rgba(255,183,77,0.04), transparent 60%)"
        }
        DeviceClass::Audio => {
            "radial-gradient(circle at 30% 50%, rgba(255,255,255,0.03), transparent 60%)"
        }
        DeviceClass::Display => {
            "linear-gradient(180deg, rgba(255,255,255,0.03) 0%, transparent 40%)"
        }
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
        "strip" => {
            r#"<rect x="1" y="5" width="14" height="6" rx="2" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.6"/>"#
        }
        "ring" | "concentric_rings" => {
            r#"<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.6"/>"#
        }
        "matrix" | "perimeter_loop" => {
            r#"<rect x="2" y="2" width="12" height="12" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.6"/>"#
        }
        "point" => r#"<circle cx="8" cy="8" r="3" fill="currentColor" opacity="0.4"/>"#,
        _ => {
            r#"<rect x="3" y="3" width="10" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1" opacity="0.4"/>"#
        }
    }
}

/// Status label for display.
fn status_label(status: &str) -> &'static str {
    match status.to_lowercase().as_str() {
        "active" => "Active",
        "connected" => "Connected",
        "known" => "Known",
        "disabled" => "Disabled",
        "reconnecting" => "Reconnecting",
        _ => "Unknown",
    }
}

/// Hardware showcase device card with rich metadata.
#[component]
pub fn DeviceCard(
    device: DeviceSummary,
    #[prop(into)] is_selected: Signal<bool>,
    #[prop(into)] on_select: Callback<String>,
    #[prop(into)] on_pair: Callback<String>,
    #[prop(default = 0)] index: usize,
) -> impl IntoView {
    let device_id = device.id.clone();
    let device_id_for_pair = device.id.clone();
    let rgb = backend_accent_rgb(&device.backend).to_string();
    let status_rgb = status_dot_rgb(&device.status).to_string();
    let device_class = classify_device(&device);
    let icon = device_class_icon(&device_class);
    let type_label = device_class_label(&device_class);
    let type_tint = device_class_tint(&device_class);
    let pattern = device_class_pattern(&device_class);
    let device_name = device.name.clone();
    let zone_count = device.zones.len();
    let total_leds = device.total_leds;
    let conn_type = connection_type(&device);
    let conn_icon = connection_icon(conn_type);
    let firmware = device.firmware_version.clone();
    let brightness = device.brightness;
    let endpoint = compact_label(&device);
    let status = status_label(&device.status);
    let is_active = device.status.to_lowercase() == "active";
    let is_disabled = device.status.to_lowercase() == "disabled";

    // Pairing badge info
    let auth_badge = crate::components::device_pairing_modal::auth_badge_info(&device.auth);

    // Zone topology previews — collect unique topology types with LED totals
    let zone_previews: Vec<(&'static str, usize)> = device
        .zones
        .iter()
        .take(5)
        .map(|z| (topology_shape_svg(&z.topology), z.led_count))
        .collect();
    let remaining_zones = zone_count.saturating_sub(5);

    let accent_gradient = format!(
        "background: linear-gradient(170deg, rgba({rgb}, 0.12) 0%, rgba({rgb}, 0.03) 40%, transparent 70%)"
    );
    let icon_bg = format!(
        "background: rgba({type_tint}, 0.1); border: 1px solid rgba({type_tint}, 0.15); color: rgba({type_tint}, 0.8)"
    );
    let dot_style =
        format!("background: rgb({status_rgb}); box-shadow: 0 0 6px rgba({status_rgb}, 0.5)");
    let accent_bar = format!(
        "background: linear-gradient(90deg, rgba({rgb}, 0.25), rgba({rgb}, 0.08))"
    );

    let stagger = (index.min(12) + 1).to_string();

    view! {
        <button
            class=move || {
                let base = "relative rounded-xl border text-left w-full group overflow-hidden \
                            card-hover animate-fade-in-up cursor-pointer";
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
            // Accent bar at top
            <div class="absolute top-0 left-0 right-0 h-[2px] rounded-t-xl" style=accent_bar />

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
                // ── Row 1: Icon + Name + Type + Status ────────────────────
                <div class="flex items-start gap-2.5">
                    <div class="w-9 h-9 rounded-lg flex items-center justify-center shrink-0 mt-0.5" style=icon_bg>
                        <Icon icon=icon width="18px" height="18px" />
                    </div>
                    <div class="flex-1 min-w-0">
                        <div class="flex items-center gap-2">
                            <h3 class="text-[13px] font-medium text-fg-primary truncate leading-tight">
                                {device_name}
                            </h3>
                            <div
                                class={if is_active { "w-2 h-2 rounded-full shrink-0 dot-alive" } else { "w-2 h-2 rounded-full shrink-0" }}
                                style=dot_style.clone()
                                title=status
                            />
                        </div>
                        // Type · Connection type · endpoint
                        <div class="flex items-center gap-1 mt-0.5">
                            <span class="text-[10px] font-medium" style=format!("color: rgba({}, 0.55)", rgb)>
                                {type_label}
                            </span>
                            <span class="text-[8px] text-fg-tertiary/25">{"\u{b7}"}</span>
                            <span class="flex items-center gap-0.5 text-[10px] text-fg-tertiary/45">
                                <Icon icon=conn_icon width="9px" height="9px" />
                                {conn_type}
                            </span>
                            {endpoint.map(|ep| view! {
                                <span class="text-[8px] text-fg-tertiary/25">{"\u{b7}"}</span>
                                <span class="text-[9px] font-mono text-fg-tertiary/30 truncate max-w-[90px]">{ep}</span>
                            })}
                        </div>
                    </div>
                </div>

                // ── Pairing badge ─────────────────────────────────────────
                {auth_badge.map(|(label, badge_rgb)| {
                    let pair_id = device_id_for_pair.clone();
                    view! {
                        <div class="flex items-center gap-1.5 mt-0.5">
                            <button
                                class="flex items-center gap-1 px-1.5 py-0.5 rounded-md text-[10px] font-medium transition-all btn-press"
                                style=format!(
                                    "background: rgba({badge_rgb}, 0.1); color: rgb({badge_rgb}); border: 1px solid rgba({badge_rgb}, 0.15)"
                                )
                                on:click=move |ev: web_sys::MouseEvent| {
                                    ev.stop_propagation();
                                    on_pair.run(pair_id.clone());
                                }
                            >
                                <Icon icon=LuKeyRound width="10px" height="10px" />
                                {label}
                            </button>
                        </div>
                    }
                })}

                // ── Row 2: Zone topology preview / Hue setup hint ──────────
                {if zone_count > 0 {
                    let zone_rgb = rgb.clone();
                    Some(view! {
                        <div class="flex items-center gap-1 mt-0.5">
                            {zone_previews.into_iter().map(|(svg, led_count)| {
                                let zr = zone_rgb.clone();
                                view! {
                                    <div class="flex items-center gap-0.5 px-1 py-0.5 rounded bg-surface-overlay/20"
                                         title=format!("{led_count} LEDs")>
                                        <div class="w-3 h-3 shrink-0" style=format!("color: rgba({zr}, 0.4)")
                                             inner_html=format!(r#"<svg viewBox="0 0 16 16" width="12" height="12">{svg}</svg>"#) />
                                        <span class="text-[8px] font-mono text-fg-tertiary/35 tabular-nums">{led_count}</span>
                                    </div>
                                }
                            }).collect_view()}
                            {(remaining_zones > 0).then(|| view! {
                                <span class="text-[8px] font-mono text-fg-tertiary/25">
                                    "+" {remaining_zones}
                                </span>
                            })}
                        </div>
                    }.into_any())
                } else if device.backend.to_lowercase() == "hue" {
                    Some(view! {
                        <div class="flex items-center gap-1.5 mt-1 px-2 py-1.5 rounded-md"
                             style="background: rgba(255, 183, 77, 0.05); border: 1px solid rgba(255, 183, 77, 0.08)">
                            <Icon icon=LuInfo width="10px" height="10px" style="color: rgba(255, 183, 77, 0.5); flex-shrink: 0" />
                            <span class="text-[9px] leading-tight" style="color: rgba(255, 183, 77, 0.55)">
                                "Set up an Entertainment Area in the Hue app to enable streaming"
                            </span>
                        </div>
                    }.into_any())
                } else {
                    None
                }}

                // ── Row 3: LED count + firmware + brightness ──────────────
                <div class="flex items-center justify-between">
                    <div class="flex items-center gap-1.5">
                        <span class="text-[10px] font-mono tabular-nums" style=format!("color: rgba({}, 0.4)", rgb)>
                            {total_leds} " LEDs"
                        </span>
                        {firmware.map(|fw| view! {
                            <span class="text-[8px] text-fg-tertiary/20">{"\u{b7}"}</span>
                            <span class="text-[9px] font-mono text-fg-tertiary/30">"v" {fw}</span>
                        })}
                    </div>
                    // Mini brightness bar
                    <div class="flex items-center gap-1 opacity-60 group-hover:opacity-100 transition-opacity">
                        <div class="w-10 h-[3px] rounded-full bg-surface-overlay/30 overflow-hidden">
                            <div
                                class="h-full rounded-full"
                                style=format!("width: {}%; background: rgba({}, 0.35)", brightness, rgb)
                            />
                        </div>
                        <span class="text-[8px] font-mono text-fg-tertiary/30 tabular-nums">{brightness} "%"</span>
                    </div>
                </div>
            </div>
        </button>
    }
}
