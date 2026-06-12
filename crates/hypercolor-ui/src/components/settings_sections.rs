//! Settings section components — one per config domain.

use std::net::IpAddr;

use hypercolor_types::config::{HypercolorConfig, NetworkAccessMode, NetworkClientScope};
use hypercolor_types::session::{OffOutputBehavior, SleepBehavior};
use leptos::prelude::*;

use crate::components::settings_controls::*;
use crate::icons::*;
use crate::render_presets::{
    CANVAS_PRESETS, MAX_CUSTOM_CANVAS_HEIGHT, MAX_CUSTOM_CANVAS_WIDTH, canvas_preset_key,
};

mod about;
mod audio;
mod developer;
mod discovery;
mod session;

pub use about::AboutSection;
pub use audio::AudioSection;
pub use developer::DeveloperSection;
pub use discovery::DiscoverySection;
pub use session::SessionSection;

fn read_config<T>(
    config: Signal<Option<HypercolorConfig>>,
    selector: impl FnOnce(&HypercolorConfig) -> T,
) -> T
where
    T: Default,
{
    config.with(|cfg| cfg.as_ref().map(selector).unwrap_or_default())
}

fn driver_enabled(config: &HypercolorConfig, driver_id: &str) -> bool {
    config
        .drivers
        .get(driver_id)
        .map(|driver| driver.enabled)
        .unwrap_or(true)
}

fn listen_scope_value(address: &str, remote_access: bool) -> String {
    if remote_access && is_loopback_listen_address(address) {
        return "all".to_owned();
    }

    if is_all_interfaces_listen_address(address) {
        "all".to_owned()
    } else if is_loopback_listen_address(address) {
        "local".to_owned()
    } else {
        "custom".to_owned()
    }
}

fn network_access_mode_value(mode: NetworkAccessMode) -> String {
    match mode {
        NetworkAccessMode::LocalOnly => "local_only",
        NetworkAccessMode::LanTrusted => "lan_trusted",
        NetworkAccessMode::LanProtected => "lan_protected",
        NetworkAccessMode::Custom => "custom",
    }
    .to_owned()
}

fn network_client_scope_value(scope: NetworkClientScope) -> String {
    match scope {
        NetworkClientScope::LocalSubnets => "local_subnets",
        NetworkClientScope::PrivateRanges => "private_ranges",
        NetworkClientScope::Custom => "custom",
    }
    .to_owned()
}

fn is_loopback_listen_address(address: &str) -> bool {
    let trimmed = address.trim();
    let lower = trimmed.to_ascii_lowercase();
    matches!(lower.as_str(), "localhost" | "local" | "loopback")
        || trimmed
            .parse::<IpAddr>()
            .is_ok_and(|addr| addr.is_loopback())
}

fn is_all_interfaces_listen_address(address: &str) -> bool {
    let trimmed = address.trim();
    let lower = trimmed.to_ascii_lowercase();
    matches!(lower.as_str(), "all" | "any" | "*")
        || trimmed
            .parse::<IpAddr>()
            .is_ok_and(|addr| addr.is_unspecified())
}

fn sleep_behavior_value(behavior: SleepBehavior) -> String {
    match behavior {
        SleepBehavior::Off => "off",
        SleepBehavior::Dim => "dim",
        SleepBehavior::Scene => "scene",
        SleepBehavior::Ignore => "ignore",
    }
    .to_owned()
}

fn off_output_behavior_value(behavior: OffOutputBehavior) -> String {
    match behavior {
        OffOutputBehavior::Static => "static",
        OffOutputBehavior::Release => "release",
    }
    .to_owned()
}

// ── Network ────────────────────────────────────────────────────────────────

#[component]
pub fn NetworkSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let listen_addr =
        Signal::derive(move || read_config(config, |cfg| cfg.daemon.listen_address.clone()));
    let port = Signal::derive(move || read_config(config, |cfg| f64::from(cfg.daemon.port)));
    let remote_access =
        Signal::derive(move || read_config(config, |cfg| cfg.network.remote_access));
    let access_mode = Signal::derive(move || {
        network_access_mode_value(read_config(config, |cfg| cfg.network.access_mode))
    });
    let client_scope = Signal::derive(move || {
        network_client_scope_value(read_config(config, |cfg| cfg.network.client_scope))
    });
    let allow_unauthenticated_remote_access = Signal::derive(move || {
        read_config(config, |cfg| {
            cfg.network.allow_unauthenticated_remote_access
        })
    });
    let allowed_clients =
        Signal::derive(move || read_config(config, |cfg| cfg.network.allowed_clients.join(", ")));
    let open_browser = Signal::derive(move || read_config(config, |cfg| cfg.web.open_browser));
    let mcp_enabled = Signal::derive(move || read_config(config, |cfg| cfg.mcp.enabled));
    let access_mode_options = Signal::stored(vec![
        ("local_only".to_string(), "Local".to_string()),
        ("lan_trusted".to_string(), "LAN".to_string()),
        ("lan_protected".to_string(), "Protected".to_string()),
        ("custom".to_string(), "Custom".to_string()),
    ]);
    let client_scope_options = Signal::stored(vec![
        ("local_subnets".to_string(), "Subnet".to_string()),
        ("private_ranges".to_string(), "Private".to_string()),
        ("custom".to_string(), "Custom".to_string()),
    ]);
    let scope_options = Signal::stored(vec![
        ("local".to_string(), "Local".to_string()),
        ("all".to_string(), "All".to_string()),
        ("custom".to_string(), "Custom".to_string()),
    ]);
    let (custom_scope_open, set_custom_scope_open) = signal(false);
    let listen_scope = Signal::derive(move || {
        if custom_scope_open.get() {
            "custom".to_owned()
        } else {
            listen_scope_value(&listen_addr.get(), remote_access.get())
        }
    });
    let scope_change = on_change;
    let apply_access_mode = Callback::new(move |(_, value): (String, serde_json::Value)| {
        let Some(mode) = value.as_str() else {
            return;
        };

        scope_change.run(("network.access_mode".to_string(), serde_json::json!(mode)));
        match mode {
            "local_only" => {
                scope_change.run((
                    "network.remote_access".to_string(),
                    serde_json::json!(false),
                ));
                scope_change.run((
                    "network.allow_unauthenticated_remote_access".to_string(),
                    serde_json::json!(false),
                ));
                scope_change.run((
                    "daemon.listen_address".to_string(),
                    serde_json::json!("127.0.0.1"),
                ));
            }
            "lan_trusted" => {
                scope_change.run(("network.remote_access".to_string(), serde_json::json!(true)));
                scope_change.run((
                    "network.allow_unauthenticated_remote_access".to_string(),
                    serde_json::json!(true),
                ));
                scope_change.run((
                    "daemon.listen_address".to_string(),
                    serde_json::json!("127.0.0.1"),
                ));
            }
            "lan_protected" => {
                scope_change.run(("network.remote_access".to_string(), serde_json::json!(true)));
                scope_change.run((
                    "network.allow_unauthenticated_remote_access".to_string(),
                    serde_json::json!(false),
                ));
                scope_change.run((
                    "daemon.listen_address".to_string(),
                    serde_json::json!("127.0.0.1"),
                ));
            }
            "custom" => {}
            _ => {}
        }
    });
    let apply_listen_scope = Callback::new(move |(_, value): (String, serde_json::Value)| {
        let Some(scope) = value.as_str() else {
            return;
        };

        match scope {
            "local" => {
                set_custom_scope_open.set(false);
                scope_change.run((
                    "network.remote_access".to_string(),
                    serde_json::json!(false),
                ));
                scope_change.run((
                    "daemon.listen_address".to_string(),
                    serde_json::json!("127.0.0.1"),
                ));
            }
            "all" => {
                set_custom_scope_open.set(false);
                scope_change.run((
                    "daemon.listen_address".to_string(),
                    serde_json::json!("127.0.0.1"),
                ));
                scope_change.run(("network.remote_access".to_string(), serde_json::json!(true)));
            }
            "custom" => set_custom_scope_open.set(true),
            _ => {}
        }
    });
    let custom_address_change = Callback::new(move |(key, value)| {
        on_change.run((
            "network.remote_access".to_string(),
            serde_json::json!(false),
        ));
        on_change.run((key, value));
    });
    let allowed_clients_change = Callback::new(move |(key, value): (String, serde_json::Value)| {
        let clients = value.as_str().map_or_else(Vec::new, |raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        });
        on_change.run((key, serde_json::json!(clients)));
    });

    view! {
        <section id="section-network" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Network" icon=LuGlobe />
            <SettingSegmented
                label="Access Mode"
                description="How the daemon API is exposed"
                key="network.access_mode"
                value=access_mode
                options=access_mode_options
                on_change=apply_access_mode
                restart_required=true
            />
            <Show when=move || matches!(access_mode.get().as_str(), "lan_trusted" | "lan_protected")>
                <SettingSegmented
                    label="Client Scope"
                    description="Which network clients can connect"
                    key="network.client_scope"
                    value=client_scope
                    options=client_scope_options
                    on_change=on_change
                    restart_required=true
                />
            </Show>
            <Show when=move || access_mode.get() == "custom">
            <SettingSegmented
                label="Listen Scope"
                description="Who can reach the daemon API"
                key="daemon.listen_scope"
                value=listen_scope
                options=scope_options
                on_change=apply_listen_scope
                restart_required=true
            />
            </Show>
            <Show when=move || listen_scope.get() == "custom">
                <SettingTextInput
                    label="Interface Address"
                    description="Specific host or IP to bind"
                    key="daemon.listen_address"
                    value=listen_addr
                    on_change=custom_address_change
                    restart_required=true
                    placeholder="192.168.1.42"
                />
            </Show>
            <SettingNumberInput
                label="Port"
                description="HTTP/WebSocket port"
                key="daemon.port"
                value=port
                on_change=on_change
                min=1024.0 max=65535.0 step=1.0
                restart_required=true
            />
            <Show when=move || access_mode.get() == "custom">
            <SettingToggle
                label="Allow Without API Key"
                description="Permit remote clients when no control API key is configured"
                key="network.allow_unauthenticated_remote_access"
                value=allow_unauthenticated_remote_access
                on_change=on_change
                restart_required=true
            />
            </Show>
            <Show when=move || {
                access_mode.get() == "custom" || client_scope.get() == "custom"
            }>
            <SettingTextInput
                label="Allowed Clients"
                description="Exact IPs or CIDR ranges, comma-separated"
                key="network.allowed_clients"
                value=allowed_clients
                on_change=allowed_clients_change
                restart_required=true
                placeholder="192.168.1.0/24"
            />
            </Show>
            <SettingToggle
                label="Open Browser on Start"
                description="Automatically open the web UI when the daemon starts"
                key="web.open_browser"
                value=open_browser
                on_change=on_change
            />
            <SettingToggle
                label="MCP Server"
                description="Expose Model Context Protocol server for AI agent integration"
                key="mcp.enabled"
                value=mcp_enabled
                on_change=on_change
                restart_required=true
            />
            <SectionReset section_label="Network" on_reset=Callback::new(move |()| {
                for key in &[
                    "daemon.listen_address", "daemon.port", "network.access_mode",
                    "network.client_scope", "network.remote_access",
                    "network.allow_unauthenticated_remote_access",
                    "network.allowed_clients", "web.open_browser", "mcp.enabled",
                ] {
                    on_reset.run(key.to_string());
                }
            }) />
        </section>
    }
}

// ── Rendering ──────────────────────────────────────────────────────────────

#[component]
pub fn RenderingSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let target_fps =
        Signal::derive(move || read_config(config, |cfg| cfg.daemon.target_fps.to_string()));
    let canvas_width =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.daemon.canvas_width)));
    let canvas_height =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.daemon.canvas_height)));
    let canvas_preset = Signal::derive(move || {
        read_config(config, |cfg| {
            canvas_preset_key(cfg.daemon.canvas_width, cfg.daemon.canvas_height)
        })
    });
    let render_acceleration = Signal::derive(move || {
        read_config(config, |cfg| {
            match cfg.effect_engine.compositor_acceleration_mode {
                hypercolor_types::config::RenderAccelerationMode::Cpu => "cpu".to_string(),
                hypercolor_types::config::RenderAccelerationMode::Auto => "auto".to_string(),
                hypercolor_types::config::RenderAccelerationMode::Gpu => "gpu".to_string(),
            }
        })
    });
    let effect_error_fallback = Signal::derive(move || {
        read_config(config, |cfg| {
            match cfg.effect_engine.effect_error_fallback {
                hypercolor_types::config::EffectErrorFallbackPolicy::None => "none".to_string(),
                hypercolor_types::config::EffectErrorFallbackPolicy::ClearGroups => {
                    "clear_groups".to_string()
                }
            }
        })
    });

    // FpsTier values: Minimal(10), Low(20), Medium(30), High(45), Full(60).
    let fps_tier_options = vec![
        ("10".to_string(), "10".to_string()),
        ("20".to_string(), "20".to_string()),
        ("30".to_string(), "30".to_string()),
        ("45".to_string(), "45".to_string()),
        ("60".to_string(), "60".to_string()),
    ];

    let preset_options: Vec<(String, String)> = CANVAS_PRESETS
        .iter()
        .map(|(label, _, _)| ((*label).to_string(), (*label).to_string()))
        .chain(std::iter::once((
            "custom".to_string(),
            "Custom…".to_string(),
        )))
        .collect();

    let accel_options = vec![
        ("cpu".to_string(), "CPU".to_string()),
        ("auto".to_string(), "Auto (prefer GPU)".to_string()),
        ("gpu".to_string(), "GPU (require)".to_string()),
    ];
    let fallback_options = vec![
        ("none".to_string(), "Leave as-is".to_string()),
        (
            "clear_groups".to_string(),
            "Clear failed groups".to_string(),
        ),
    ];

    // Apply a preset by issuing two config writes. "custom" is a UI-only
    // sentinel — don't echo it back to the daemon.
    let apply_preset = Callback::new(move |(_, value): (String, serde_json::Value)| {
        let Some(selected) = value.as_str() else {
            return;
        };
        if selected == "custom" {
            return;
        }
        if let Some((_, w, h)) = CANVAS_PRESETS
            .iter()
            .find(|(label, _, _)| *label == selected)
        {
            on_change.run(("daemon.canvas_width".to_string(), serde_json::json!(*w)));
            on_change.run(("daemon.canvas_height".to_string(), serde_json::json!(*h)));
        }
    });

    view! {
        <section id="section-rendering" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Rendering" icon=LuGauge />
            <div class="text-xs text-fg-tertiary/50 -mt-2 mb-4">
                "Frame rate, canvas resolution, and scene compositor acceleration"
            </div>
            <SettingSegmented
                label="Target FPS"
                description="Render loop frame rate. 30 is the balanced default; 60 uses more CPU but gives smoother motion"
                key="daemon.target_fps"
                value=target_fps
                options=Signal::stored(fps_tier_options)
                on_change=on_change
                numeric=true
            />
            <SettingDropdown
                label="Canvas Resolution"
                description="Internal render surface size. Higher values improve gradient smoothness on large layouts at the cost of CPU"
                key="daemon.canvas_preset"
                value=canvas_preset
                options=Signal::stored(preset_options)
                on_change=apply_preset
            />
            <Show when=move || canvas_preset.get() == "custom">
                <SettingNumberInput
                    label="Canvas Width"
                    description="Pixels wide"
                    key="daemon.canvas_width"
                    value=canvas_width
                    on_change=on_change
                    min=32.0 max=MAX_CUSTOM_CANVAS_WIDTH step=16.0
                />
                <SettingNumberInput
                    label="Canvas Height"
                    description="Pixels tall"
                    key="daemon.canvas_height"
                    value=canvas_height
                    on_change=on_change
                    min=32.0 max=MAX_CUSTOM_CANVAS_HEIGHT step=16.0
                />
            </Show>
            <SettingDropdown
                label="Compositor Acceleration"
                description="Accelerates scene composition only. Servo HTML rendering still uses CPU readback; GPU requires a compatible compositor path."
                key="effect_engine.compositor_acceleration_mode"
                value=render_acceleration
                options=Signal::stored(accel_options)
                on_change=on_change
                restart_required=true
            />
            <SettingDropdown
                label="Effect Error Fallback"
                description="What the daemon should do after an effect render failure. Clear failed groups swaps dark/crashed assignments back to empty scene slots."
                key="effect_engine.effect_error_fallback"
                value=effect_error_fallback
                options=Signal::stored(fallback_options)
                on_change=on_change
            />
            <SectionReset section_label="Rendering" on_reset=Callback::new(move |()| {
                for key in &[
                    "daemon.target_fps",
                    "daemon.canvas_width", "daemon.canvas_height",
                    "effect_engine.compositor_acceleration_mode",
                    "effect_engine.effect_error_fallback",
                ] {
                    on_reset.run(key.to_string());
                }
            }) />
        </section>
    }
}
