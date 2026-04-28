//! Device card — hardware showcase card with driver identity, metadata, and zone topology.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::DeviceSummary;
use crate::components::device_metrics_strip::DeviceMetricsStrip;
use crate::icons::*;
use crate::storage;
use crate::style_utils::device_accent_colors;

// ── Driver presentation ─────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Eq)]
pub struct DeviceBrand {
    label: Option<String>,
    primary_rgb: String,
    secondary_rgb: String,
}

pub fn classify_brand(device: &DeviceSummary) -> DeviceBrand {
    let color_key = if device.backend.trim().is_empty() {
        device.id.as_str()
    } else {
        device.backend.as_str()
    };
    let (primary_rgb, secondary_rgb) = device_accent_colors(color_key);

    DeviceBrand {
        label: backend_label(&device.backend),
        primary_rgb,
        secondary_rgb,
    }
}

pub fn brand_colors(brand: &DeviceBrand) -> (String, String) {
    (brand.primary_rgb.clone(), brand.secondary_rgb.clone())
}

pub fn brand_label(brand: &DeviceBrand) -> Option<String> {
    brand.label.clone()
}

pub fn backend_accent_rgb(backend: &str) -> String {
    device_accent_colors(backend).0
}

fn backend_label(backend: &str) -> Option<String> {
    let label = backend
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ");

    (!label.is_empty()).then_some(label)
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
    NetworkController,
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
    DeviceClass::NetworkController,
    DeviceClass::SmartLight,
    DeviceClass::Audio,
    DeviceClass::Display,
    DeviceClass::Other,
];

/// Parse a device class from its label string.
pub fn parse_device_class(label: &str) -> Option<DeviceClass> {
    ALL_DEVICE_CLASSES
        .iter()
        .find(|c| device_class_label(c) == label)
        .copied()
}

/// Classify a device by name/zone heuristics (auto-detection).
pub fn classify_device(device: &DeviceSummary) -> DeviceClass {
    // Check localStorage override first
    if let Some(override_label) = load_category_override(&device.id)
        && let Some(class) = parse_device_class(&override_label)
    {
        return class;
    }

    let name = device.name.to_lowercase();

    if device.network_ip.is_some() || device.network_hostname.is_some() {
        return DeviceClass::NetworkController;
    }

    if name.contains("keyboard") {
        return DeviceClass::Keyboard;
    }
    if name.contains("mouse") {
        return DeviceClass::Mouse;
    }
    if name.contains("hub") || name.contains("bridge") {
        return DeviceClass::Hub;
    }
    if name.contains("mic") || name.contains("headset") || name.contains("audio") {
        return DeviceClass::Audio;
    }
    if name.contains("lcd") || name.contains("display") || name.contains("screen") {
        return DeviceClass::Display;
    }
    if name.contains("light") || name.contains("lamp") || name.contains("panel") {
        return DeviceClass::SmartLight;
    }
    if name.contains("controller") || name.contains("node") || name.contains("strip") {
        return DeviceClass::Controller;
    }

    DeviceClass::Other
}

fn load_category_override(device_id: &str) -> Option<String> {
    storage::get(&format!("hc-device-category-{device_id}"))
}

pub fn save_category_override(device_id: &str, label: &str) {
    storage::set(&format!("hc-device-category-{device_id}"), label);
}

/// Device class → icon.
pub fn device_class_icon(class: &DeviceClass) -> icondata_core::Icon {
    match class {
        DeviceClass::Keyboard => LuKeyboard,
        DeviceClass::Mouse => LuMousePointerClick,
        DeviceClass::Hub => LuNetwork,
        DeviceClass::Controller => LuLayers,
        DeviceClass::NetworkController => LuWifi,
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
        DeviceClass::NetworkController => "Network Controller",
        DeviceClass::SmartLight => "Smart Light",
        DeviceClass::Audio => "Audio",
        DeviceClass::Display => "Display",
        DeviceClass::Other => "Device",
    }
}

/// Infer connection type from device metadata.
fn connection_type(device: &DeviceSummary) -> &'static str {
    if device.network_ip.is_some() || device.network_hostname.is_some() {
        return "Network";
    }
    "USB"
}

/// Connection type → icon.
fn connection_icon(conn: &str) -> icondata_core::Icon {
    match conn {
        "Network" => LuGlobe,
        _ => LuCable,
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

/// Hardware showcase device card with driver identity and metric-forward layout.
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

    // Driver identity → duotone accents + compact backend chip
    let brand = classify_brand(&device);
    let (primary, secondary) = brand_colors(&brand);
    let vendor_label = brand_label(&brand);
    let vendor_label_for_chip = vendor_label.clone();
    let vendor_label_for_separator = vendor_label.clone();

    let primary_sel = primary.clone();
    let status_rgb = status_dot_rgb(&device.status).to_string();
    let device_class = classify_device(&device);
    let icon = device_class_icon(&device_class);
    let type_label = device_class_label(&device_class);
    let device_name = device.name.clone();
    let zone_count = device.zones.len();
    let total_leds = device.total_leds;
    let conn_type = connection_type(&device);
    let conn_icon = connection_icon(conn_type);
    let firmware = device.firmware_version.clone();
    let brightness = device.brightness;
    let status = status_label(&device.status);
    let is_active = device.status.to_lowercase() == "active";
    let is_disabled = device.status.to_lowercase() == "disabled";
    let shows_metrics = matches!(
        device.status.to_lowercase().as_str(),
        "active" | "connected" | "reconnecting"
    );
    let metrics_device_id = device.id.clone();

    // Pairing badge info
    let auth_badge = crate::components::device_pairing_modal::auth_badge_info(&device.auth);

    // Zone topology previews — cycle the SilkCircuit palette so each zone
    // reads as its own thing instead of a wall of grey chips.
    let zone_palette = [
        "128, 255, 234",
        "255, 106, 193",
        "80, 250, 123",
        "241, 250, 140",
        "225, 53, 255",
        "110, 180, 255",
    ];
    let zone_previews: Vec<(&'static str, usize, &'static str)> = device
        .zones
        .iter()
        .take(5)
        .enumerate()
        .map(|(i, z)| {
            (
                topology_shape_svg(&z.topology),
                z.led_count,
                zone_palette[i % zone_palette.len()],
            )
        })
        .collect();
    let remaining_zones = zone_count.saturating_sub(5);

    // Hero background — duotone gradient tied to driver identity, richer than a wash
    let hero_bg = format!(
        "background: \
         radial-gradient(ellipse at 18% 0%, rgba({primary}, 0.28) 0%, transparent 55%), \
         radial-gradient(ellipse at 95% 10%, rgba({secondary}, 0.20) 0%, transparent 60%), \
         linear-gradient(180deg, rgba({primary}, 0.08) 0%, transparent 62%)"
    );
    // Inner glow surface that breathes on active, stays calm otherwise
    let ambient_glow =
        format!("box-shadow: inset 0 0 32px rgba({primary}, 0.06), 0 0 18px rgba({primary}, 0.05)");
    let dot_style =
        format!("background: rgb({status_rgb}); box-shadow: 0 0 10px rgba({status_rgb}, 0.7)");
    // Icon gets the driver primary so the glyph feels owned by the module
    let icon_bg = format!(
        "background: linear-gradient(140deg, rgba({primary}, 0.18), rgba({secondary}, 0.10)); \
         border: 1px solid rgba({primary}, 0.28); color: rgba({primary}, 0.95); \
         box-shadow: 0 0 10px rgba({primary}, 0.15), inset 0 0 8px rgba({primary}, 0.08)"
    );
    // Glowing LED count — this is the hero stat on each card
    let led_count_style = format!(
        "color: rgb({primary}); \
         text-shadow: 0 0 14px rgba({primary}, 0.45), 0 0 2px rgba({primary}, 0.9)"
    );
    let brightness_bar_style = format!(
        "width: {brightness}%; \
         background: linear-gradient(90deg, rgba({primary}, 0.9), rgba({secondary}, 0.7)); \
         box-shadow: 0 0 8px rgba({primary}, 0.45)"
    );

    let stagger = (index.min(12) + 1).to_string();

    view! {
        <button
            class=move || {
                let base = "relative rounded-xl border text-left w-full group overflow-hidden \
                            card-hover animate-fade-in-up cursor-pointer";
                let state = if is_selected.get() {
                    "bg-surface-overlay"
                } else if is_disabled {
                    "border-edge-subtle/50 bg-surface-overlay/40 opacity-60 hover:opacity-80"
                } else {
                    "border-edge-subtle bg-surface-overlay/70 hover:border-edge-default"
                };
                format!("{base} {state} stagger-{}", stagger)
            }
            style:--glow-rgb=primary.clone()
            // Selected ring uses the driver accent, not a universal purple — keeps
            // the highlight from fighting the card's hero gradient.
            style=move || {
                if is_selected.get() {
                    format!("border-color: rgba({primary}, 0.55)", primary = primary_sel.clone())
                } else {
                    String::new()
                }
            }
            on:click=move |_| on_select.run(device_id.clone())
        >
            // Hero duotone gradient (driver-coded)
            <div class="absolute inset-0 pointer-events-none rounded-xl" style=hero_bg />

            // Subtle grid-texture cross-hatch for depth
            <div class="absolute inset-0 pointer-events-none rounded-xl opacity-40"
                 style="background-image: repeating-linear-gradient(135deg, rgba(255,255,255,0.015) 0px, rgba(255,255,255,0.015) 1px, transparent 1px, transparent 6px)" />

            // Ambient inner glow — always on, breathes when active
            <div
                class=move || {
                    if is_active {
                        "absolute inset-0 rounded-xl pointer-events-none animate-breathe"
                    } else {
                        "absolute inset-0 rounded-xl pointer-events-none"
                    }
                }
                style=ambient_glow
            />

            // Selected ring (breathing halo)
            {
                let glow_rgb = primary.clone();
                move || is_selected.get().then(|| {
                    let r = glow_rgb.clone();
                    view! {
                        <div
                            class="absolute inset-0 rounded-xl pointer-events-none animate-breathe"
                            style=format!(
                                "box-shadow: inset 0 0 28px rgba({r}, 0.14), 0 0 26px rgba({r}, 0.18)"
                            )
                        />
                    }
                })
            }

            <div class="relative flex flex-col h-full p-3.5 gap-2.5">
                // ── Row 1: Icon + device name + driver/type + status dot ──
                <div class="flex items-start gap-2.5">
                    <div class="w-10 h-10 rounded-lg flex items-center justify-center shrink-0" style=icon_bg>
                        <Icon icon=icon width="20px" height="20px" />
                    </div>
                    <div class="flex-1 min-w-0">
                        <div class="flex items-center gap-2">
                            <h3 class="text-[14px] font-semibold text-fg-primary truncate leading-tight flex-1">
                                {device_name}
                            </h3>
                            <div
                                class={if is_active {
                                    "w-2 h-2 rounded-full shrink-0 dot-alive"
                                } else {
                                    "w-2 h-2 rounded-full shrink-0"
                                }}
                                style=dot_style
                                title=status
                            />
                        </div>
                        // Driver · type · connection — single meta line
                        <div class="flex items-center gap-1.5 mt-1">
                            {vendor_label_for_chip.map(|label| view! {
                                <span class="text-[9px] font-mono font-bold tracking-[0.14em]"
                                      style=format!("color: rgba({primary}, 0.9)", primary = primary.clone())>
                                    {label}
                                </span>
                            })}
                            {vendor_label_for_separator.map(|_| view! {
                                <span class="text-[8px] text-fg-tertiary/25">{"\u{b7}"}</span>
                            })}
                            <span class="text-[10px]" style=format!("color: rgba({primary}, 0.60)", primary = primary.clone())>
                                {type_label}
                            </span>
                            <span class="text-[8px] text-fg-tertiary/25">{"\u{b7}"}</span>
                            <span class="flex items-center gap-0.5 text-[10px] text-fg-tertiary/45">
                                <Icon icon=conn_icon width="9px" height="9px" />
                                {conn_type}
                            </span>
                        </div>
                    </div>
                </div>

                // ── Pairing badge ─────────────────────────────────────────
                {auth_badge.map(|(label, badge_rgb)| {
                    let pair_id = device_id_for_pair.clone();
                    view! {
                        <button
                            class="self-start flex items-center gap-1 px-1.5 py-0.5 rounded-full text-[10px] font-medium transition-all btn-press"
                            style=format!(
                                "background: rgba({badge_rgb}, 0.10); color: rgb({badge_rgb}); border: 1px solid rgba({badge_rgb}, 0.22)"
                            )
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                on_pair.run(pair_id.clone());
                            }
                        >
                            <Icon icon=LuKeyRound width="10px" height="10px" />
                            {label}
                        </button>
                    }
                })}

                // ── Row 2: Zone topology pills (colored, subtle) ──────────
                {if zone_count > 0 {
                    Some(view! {
                        <div class="flex items-center gap-1 flex-wrap">
                            {zone_previews.into_iter().map(|(svg, led_count, zrgb)| {
                                view! {
                                    <div class="flex items-center gap-1 px-1.5 py-[2px] rounded"
                                         style=format!(
                                             "background: rgba({zrgb}, 0.06); \
                                              border: 1px solid rgba({zrgb}, 0.15)"
                                         )
                                         title=format!("{led_count} LEDs")>
                                        <div class="w-3 h-3 shrink-0" style=format!("color: rgba({zrgb}, 0.85)")
                                             inner_html=format!(r#"<svg viewBox="0 0 16 16" width="12" height="12">{svg}</svg>"#) />
                                        <span class="text-[9px] font-mono tabular-nums"
                                              style=format!("color: rgba({zrgb}, 0.75)")>{led_count}</span>
                                    </div>
                                }
                            }).collect_view()}
                            {(remaining_zones > 0).then(|| view! {
                                <span class="text-[9px] font-mono text-fg-tertiary/50 px-1">
                                    "+" {remaining_zones}
                                </span>
                            })}
                        </div>
                    }.into_any())
                } else {
                    Some(view! {
                        <div class="flex items-center gap-1.5 px-2 py-1.5 rounded-lg"
                             style=format!("background: rgba({primary}, 0.05); border: 1px solid rgba({primary}, 0.15)", primary = primary.clone())>
                            <Icon icon=LuInfo width="10px" height="10px" style=format!("color: rgba({primary}, 0.6); flex-shrink: 0", primary = primary.clone()) />
                            <span class="text-[9px] leading-tight" style=format!("color: rgba({primary}, 0.7)", primary = primary.clone())>
                                "No addressable zones reported"
                            </span>
                        </div>
                    }.into_any())
                }}

                // ── Row 3: Hero LED count + brightness meter ──────────────
                <div class="flex items-end justify-between gap-2 mt-auto pt-1">
                    <div class="flex items-baseline gap-1">
                        <span class="text-[22px] font-bold tabular-nums leading-none tracking-tight"
                              style=led_count_style>
                            {total_leds}
                        </span>
                        <span class="text-[10px] font-mono uppercase tracking-widest"
                              style=format!("color: rgba({primary}, 0.45)", primary = primary.clone())>
                            "LEDs"
                        </span>
                        {firmware.map(|fw| view! {
                            <span class="text-[8px] font-mono text-fg-tertiary/30 ml-1">"v" {fw}</span>
                        })}
                    </div>
                    <div class="flex items-center gap-1.5">
                        <Icon icon=LuSun width="10px" height="10px" style=format!("color: rgba({primary}, 0.4)", primary = primary.clone()) />
                        <div class="w-14 h-[3px] rounded-full bg-surface-overlay/40 overflow-hidden">
                            <div
                                class="h-full rounded-full transition-all duration-200"
                                style=brightness_bar_style
                            />
                        </div>
                        <span class="text-[9px] font-mono tabular-nums"
                              style=format!("color: rgba({primary}, 0.60)", primary = primary.clone())>
                            {brightness} "%"
                        </span>
                    </div>
                </div>

                // ── Row 4: Live per-device metrics strip (connected only) ─
                {shows_metrics.then(|| view! {
                    <DeviceMetricsStrip device_id=metrics_device_id.clone() />
                })}
            </div>
        </button>
    }
}
