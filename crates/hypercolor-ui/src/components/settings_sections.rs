//! Settings section components — one per config domain.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::session::{OffOutputBehavior, SleepBehavior};

use crate::api;
use crate::app::WsContext;
use crate::components::settings_controls::*;
use crate::icons::*;
use crate::render_presets::{
    CANVAS_PRESETS, MAX_CUSTOM_CANVAS_HEIGHT, MAX_CUSTOM_CANVAS_WIDTH, canvas_preset_key,
};

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

// ── Audio VU Meter ────────────────────────────────────────────────────────

/// Compact level meter bar.
#[component]
fn LevelBar(
    label: &'static str,
    #[prop(into)] value: Signal<f32>,
    color: &'static str,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2 min-w-0">
            <span class="text-[10px] font-mono text-fg-tertiary/60 w-7 shrink-0 text-right uppercase">{label}</span>
            <div class="flex-1 h-1.5 rounded-full overflow-hidden" style="background: rgba(139, 133, 160, 0.1)">
                <div
                    class="h-full rounded-full transition-all duration-100"
                    style=move || format!(
                        "width: {pct}%; background: {color}; box-shadow: 0 0 6px {color}40",
                        pct = (value.get() * 100.0).clamp(0.0, 100.0),
                        color = color,
                    )
                />
            </div>
        </div>
    }
}

/// Live VU meter shown when audio capture is enabled.
#[component]
fn AudioVuMeter(#[prop(into)] enabled: Signal<bool>) -> impl IntoView {
    let ws = expect_context::<WsContext>();

    view! {
        <Show when=move || enabled.get()>
            <div class="mb-4 px-3 py-3 rounded-lg animate-fade-in" style="background: rgba(139, 133, 160, 0.04); border: 1px solid rgba(139, 133, 160, 0.06)">
                <div class="flex items-center gap-4">
                    // Beat indicator + status
                    <div class="shrink-0 flex items-center gap-2 pl-1">
                        <div
                            class="w-2.5 h-2.5 rounded-full transition-all"
                            style=move || {
                                let al = ws.audio_level.get();
                                if al.beat {
                                    "background: rgb(225, 53, 255); box-shadow: 0 0 8px rgba(225, 53, 255, 0.6); transform: scale(1.3)"
                                } else if al.level > 0.01 {
                                    "background: rgba(128, 255, 234, 0.5); box-shadow: 0 0 4px rgba(128, 255, 234, 0.3); transform: scale(1)"
                                } else {
                                    "background: rgba(139, 133, 160, 0.25); box-shadow: none; transform: scale(1)"
                                }
                            }
                        />
                    </div>

                    // Level bars
                    <div class="flex-1 space-y-1.5 min-w-0">
                        <LevelBar
                            label="vol"
                            value=Signal::derive(move || ws.audio_level.get().level)
                            color="rgba(128, 255, 234, 0.8)"
                        />
                        <div class="flex gap-3">
                            <div class="flex-1">
                                <LevelBar
                                    label="bass"
                                    value=Signal::derive(move || ws.audio_level.get().bass)
                                    color="rgba(225, 53, 255, 0.7)"
                                />
                            </div>
                            <div class="flex-1">
                                <LevelBar
                                    label="mid"
                                    value=Signal::derive(move || ws.audio_level.get().mid)
                                    color="rgba(255, 106, 193, 0.7)"
                                />
                            </div>
                            <div class="flex-1">
                                <LevelBar
                                    label="hi"
                                    value=Signal::derive(move || ws.audio_level.get().treble)
                                    color="rgba(241, 250, 140, 0.7)"
                                />
                            </div>
                        </div>
                    </div>

                    // Numeric readout
                    <div class="shrink-0 pr-1 text-right">
                        <span
                            class="text-xs font-mono tabular-nums"
                            style=move || {
                                let level = ws.audio_level.get().level;
                                if level > 0.8 {
                                    "color: rgba(255, 99, 99, 0.8)"
                                } else if level > 0.01 {
                                    "color: rgba(128, 255, 234, 0.6)"
                                } else {
                                    "color: rgba(139, 133, 160, 0.3)"
                                }
                            }
                        >
                            {move || {
                                let level = ws.audio_level.get().level;
                                if level > 0.01 {
                                    let db = (20.0 * level.log10()).max(-60.0);
                                    format!("{db:.0} dB")
                                } else {
                                    "-\u{221e} dB".to_string()
                                }
                            }}
                        </span>
                    </div>
                </div>
                // Live status hint
                <div class="flex items-center justify-between mt-2 px-1">
                    <span
                        class="text-[10px] font-mono uppercase tracking-wider"
                        style=move || {
                            let level = ws.audio_level.get().level;
                            if level > 0.01 {
                                "color: rgba(128, 255, 234, 0.5)"
                            } else {
                                "color: rgba(139, 133, 160, 0.3)"
                            }
                        }
                    >
                        {move || {
                            let al = ws.audio_level.get();
                            if al.beat {
                                "Beat detected"
                            } else if al.level > 0.01 {
                                "Listening..."
                            } else {
                                "Waiting for signal"
                            }
                        }}
                    </span>
                    <span class="text-[10px] text-fg-tertiary/30 font-mono">"Play audio to test"</span>
                </div>
            </div>
        </Show>
    }
}

// ── Audio ──────────────────────────────────────────────────────────────────

#[component]
pub fn AudioSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
    #[prop(into)] audio_devices: Signal<Vec<(String, String)>>,
    #[prop(into)] audio_device_placeholder: Signal<String>,
    #[prop(into)] audio_device_disabled: Signal<bool>,
) -> impl IntoView {
    let enabled = Signal::derive(move || read_config(config, |cfg| cfg.audio.enabled));
    let device = Signal::derive(move || read_config(config, |cfg| cfg.audio.device.clone()));
    let fft_size =
        Signal::derive(move || read_config(config, |cfg| cfg.audio.fft_size.to_string()));
    let smoothing =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.audio.smoothing)));
    let noise_gate =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.audio.noise_gate)));
    let beat_sensitivity =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.audio.beat_sensitivity)));

    let fft_options = vec![
        ("256".to_string(), "256".to_string()),
        ("512".to_string(), "512".to_string()),
        ("1024".to_string(), "1024 (default)".to_string()),
        ("2048".to_string(), "2048".to_string()),
        ("4096".to_string(), "4096".to_string()),
    ];

    view! {
        <section id="section-audio" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Audio" icon=LuAudioLines />
            <AudioVuMeter enabled=enabled />
            <SettingToggle
                label="Enabled"
                description="Enable audio capture and spectrum analysis for reactive effects"
                key="audio.enabled"
                value=enabled
                on_change=on_change
            />
            <SettingDropdown
                label="Device"
                description="Audio source for reactive effects; applies live when the daemon can switch safely"
                key="audio.device"
                value=device
                options=audio_devices
                placeholder=audio_device_placeholder
                disabled=audio_device_disabled
                on_change=on_change
            />
            <SettingDropdown
                label="FFT Size"
                description="Frequency resolution — higher values give finer detail but more latency"
                key="audio.fft_size"
                value=fft_size
                options=Signal::stored(fft_options)
                on_change=on_change
                numeric=true
            />
            <SettingSlider
                label="Smoothing"
                description="Temporal smoothing for spectrum analysis"
                key="audio.smoothing"
                value=smoothing
                on_change=on_change
                min=0.0 max=1.0 step=0.01
            />
            <SettingSlider
                label="Noise Gate"
                description="Minimum signal threshold to filter background noise"
                key="audio.noise_gate"
                value=noise_gate
                on_change=on_change
                min=0.0 max=0.5 step=0.01
            />
            <SettingSlider
                label="Beat Sensitivity"
                description="How aggressively the beat detector triggers"
                key="audio.beat_sensitivity"
                value=beat_sensitivity
                on_change=on_change
                min=0.0 max=2.0 step=0.05
            />
            <SectionReset section_label="Audio" on_reset=Callback::new(move |()| on_reset.run("audio".to_string())) />
        </section>
    }
}

// ── Screen Capture ─────────────────────────────────────────────────────────

#[component]
pub fn CaptureSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let enabled = Signal::derive(move || read_config(config, |cfg| cfg.capture.enabled));
    let source = Signal::derive(move || read_config(config, |cfg| cfg.capture.source.clone()));
    let capture_fps =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.capture_fps)));
    let monitor = Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.monitor)));

    let source_options = vec![
        ("auto".to_string(), "Auto".to_string()),
        ("pipewire".to_string(), "PipeWire".to_string()),
    ];

    view! {
        <section id="section-capture" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Screen Capture" icon=LuMonitor />
            <SettingToggle
                label="Enabled"
                description="Enable screen capture for ambient lighting effects"
                key="capture.enabled"
                value=enabled
                on_change=on_change
            />
            <SettingDropdown
                label="Source"
                description="Screen capture backend"
                key="capture.source"
                value=source
                options=Signal::stored(source_options)
                on_change=on_change
                restart_required=true
            />
            <SettingSlider
                label="Capture FPS"
                description="Screen capture frame rate"
                key="capture.capture_fps"
                value=capture_fps
                on_change=on_change
                min=1.0 max=60.0 step=1.0
                decimals=0
                integer=true
            />
            <SettingNumberInput
                label="Monitor"
                description="Monitor index for multi-display setups"
                key="capture.monitor"
                value=monitor
                on_change=on_change
                min=0.0 max=8.0 step=1.0
                restart_required=true
            />
            <SectionReset section_label="Capture" on_reset=Callback::new(move |()| on_reset.run("capture".to_string())) />
        </section>
    }
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
    let open_browser = Signal::derive(move || read_config(config, |cfg| cfg.web.open_browser));
    let mcp_enabled = Signal::derive(move || read_config(config, |cfg| cfg.mcp.enabled));

    view! {
        <section id="section-network" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Network" icon=LuGlobe />
            <SettingTextInput
                label="Listen Address"
                description="IP address the daemon binds to"
                key="daemon.listen_address"
                value=listen_addr
                on_change=on_change
                restart_required=true
                placeholder="127.0.0.1"
            />
            <SettingNumberInput
                label="Port"
                description="HTTP/WebSocket port"
                key="daemon.port"
                value=port
                on_change=on_change
                min=1024.0 max=65535.0 step=1.0
                restart_required=true
            />
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
                    "daemon.listen_address", "daemon.port",
                    "web.open_browser", "mcp.enabled",
                ] {
                    on_reset.run(key.to_string());
                }
            }) />
        </section>
    }
}

// ── Session & Power ────────────────────────────────────────────────────────

#[component]
pub fn SessionSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let enabled = Signal::derive(move || read_config(config, |cfg| cfg.session.enabled));
    let idle_enabled = Signal::derive(move || read_config(config, |cfg| cfg.session.idle_enabled));
    let dim_timeout =
        Signal::derive(move || read_config(config, |cfg| cfg.session.idle_dim_timeout_secs as f64));
    let off_timeout =
        Signal::derive(move || read_config(config, |cfg| cfg.session.idle_off_timeout_secs as f64));
    let screen_lock_behavior = Signal::derive(move || {
        read_config(config, |cfg| {
            sleep_behavior_value(cfg.session.on_screen_lock)
        })
    });
    let screen_lock_brightness = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.session.screen_lock_brightness))
    });
    let suspend_behavior = Signal::derive(move || {
        read_config(config, |cfg| sleep_behavior_value(cfg.session.on_suspend))
    });
    let off_output_behavior = Signal::derive(move || {
        read_config(config, |cfg| {
            off_output_behavior_value(cfg.session.off_output_behavior)
        })
    });
    let off_output_color =
        Signal::derive(move || read_config(config, |cfg| cfg.session.off_output_color.clone()));

    let screen_behavior_options = Signal::stored(vec![
        ("ignore".to_string(), "Ignore".to_string()),
        ("off".to_string(), "Turn Off".to_string()),
        ("dim".to_string(), "Dim".to_string()),
    ]);
    let suspend_behavior_options = Signal::stored(vec![
        ("ignore".to_string(), "Ignore".to_string()),
        ("off".to_string(), "Turn Off".to_string()),
        ("dim".to_string(), "Fade Black".to_string()),
    ]);
    let off_output_behavior_options = Signal::stored(vec![
        ("static".to_string(), "Hold Static".to_string()),
        ("release".to_string(), "Release Device".to_string()),
    ]);

    view! {
        <section id="section-session" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Session & Power" icon=LuPower />
            <SettingToggle
                label="Session Awareness"
                description="React to actual suspend/resume, screen lock, and other desktop power events"
                key="session.enabled"
                value=enabled
                on_change=on_change
            />
            <SettingDropdown
                label="Screen Lock Behavior"
                description="Choose what happens when the session locks or the display manager blanks the screen"
                key="session.on_screen_lock"
                value=screen_lock_behavior
                options=screen_behavior_options
                on_change=on_change
            />
            <Show when=move || screen_lock_behavior.get() == "dim">
                <SettingSlider
                    label="Screen Lock Brightness"
                    description="Brightness multiplier applied while the screen is locked"
                    key="session.screen_lock_brightness"
                    value=screen_lock_brightness
                    on_change=on_change
                    min=0.0 max=1.0 step=0.05
                />
            </Show>
            <SettingDropdown
                label="Suspend Behavior"
                description="What happens when the system suspends"
                key="session.on_suspend"
                value=suspend_behavior
                options=suspend_behavior_options
                on_change=on_change
            />
            <SettingDropdown
                label="Off Output Behavior"
                description="When a session event turns output off, either hold a static frame/color or release devices back to firmware"
                key="session.off_output_behavior"
                value=off_output_behavior
                options=off_output_behavior_options
                on_change=on_change
            />
            <Show when=move || off_output_behavior.get() == "static">
                <SettingTextInput
                    label="Off Hold Color"
                    description="Hex RGB color used for static hold mode, including LCD pause frames"
                    key="session.off_output_color"
                    value=off_output_color
                    on_change=on_change
                    placeholder="#000000"
                />
            </Show>
            <SettingToggle
                label="Idle Detection"
                description="Dim or turn off LEDs after a period of inactivity"
                key="session.idle_enabled"
                value=idle_enabled
                on_change=on_change
            />
            <SettingNumberInput
                label="Dim Timeout"
                description="Seconds of idle before dimming (0 = disabled)"
                key="session.idle_dim_timeout_secs"
                value=dim_timeout
                on_change=on_change
                min=0.0 max=3600.0 step=10.0
            />
            <SettingNumberInput
                label="Off Timeout"
                description="Seconds of idle before turning off LEDs (0 = disabled)"
                key="session.idle_off_timeout_secs"
                value=off_timeout
                on_change=on_change
                min=0.0 max=7200.0 step=30.0
            />
            <SectionReset section_label="Session" on_reset=Callback::new(move |()| on_reset.run("session".to_string())) />
        </section>
    }
}

// ── Device Discovery ───────────────────────────────────────────────────────

#[component]
pub fn DiscoverySection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let mdns = Signal::derive(move || read_config(config, |cfg| cfg.discovery.mdns_enabled));
    let scan_interval =
        Signal::derive(move || read_config(config, |cfg| cfg.discovery.scan_interval_secs as f64));
    let wled = Signal::derive(move || read_config(config, |cfg| driver_enabled(cfg, "wled")));
    let hue = Signal::derive(move || read_config(config, |cfg| driver_enabled(cfg, "hue")));
    let nanoleaf =
        Signal::derive(move || read_config(config, |cfg| driver_enabled(cfg, "nanoleaf")));
    view! {
        <section id="section-discovery" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Device Discovery" icon=LuRadar />
            <SettingToggle
                label="mDNS Discovery"
                description="Use multicast DNS to find devices on the local network"
                key="discovery.mdns_enabled"
                value=mdns
                on_change=on_change
                restart_required=true
            />
            <SettingNumberInput
                label="Scan Interval"
                description="Seconds between automatic discovery scans"
                key="discovery.scan_interval_secs"
                value=scan_interval
                on_change=on_change
                min=30.0 max=3600.0 step=30.0
            />
            <SettingToggle
                label="WLED Scan"
                description="Discover WLED controllers on the network"
                key="drivers.wled.enabled"
                value=wled
                on_change=on_change
            />
            <SettingToggle
                label="Hue Scan"
                description="Discover Philips Hue bridges"
                key="drivers.hue.enabled"
                value=hue
                on_change=on_change
            />
            <SettingToggle
                label="Nanoleaf Scan"
                description="Discover Nanoleaf panels and lines"
                key="drivers.nanoleaf.enabled"
                value=nanoleaf
                on_change=on_change
            />
            <SectionReset section_label="Discovery" on_reset=Callback::new(move |()| on_reset.run("discovery".to_string())) />
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

// ── Developer ──────────────────────────────────────────────────────────────

#[component]
pub fn DeveloperSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let log_level = Signal::derive(move || {
        read_config(config, |cfg| {
            format!("{:?}", cfg.daemon.log_level).to_lowercase()
        })
    });
    let extra_dirs = Signal::derive(move || {
        read_config(config, |cfg| {
            cfg.effect_engine
                .extra_effect_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
        })
    });

    let log_options = vec![
        ("trace".to_string(), "Trace".to_string()),
        ("debug".to_string(), "Debug".to_string()),
        ("info".to_string(), "Info".to_string()),
        ("warn".to_string(), "Warn".to_string()),
        ("error".to_string(), "Error".to_string()),
    ];

    view! {
        <section id="section-developer" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Developer" icon=LuCode />
            <div class="text-xs text-fg-tertiary/50 -mt-2 mb-4">"Advanced options for development and debugging"</div>
            <SettingDropdown
                label="Log Level"
                description="Daemon logging verbosity"
                key="daemon.log_level"
                value=log_level
                options=Signal::stored(log_options)
                on_change=on_change
            />
            <SettingPathList
                label="Extra Effect Directories"
                description="Additional directories to scan for custom effects"
                key="effect_engine.extra_effect_dirs"
                paths=extra_dirs
                on_change=on_change
            />
            <SectionReset section_label="Developer" on_reset=Callback::new(move |()| {
                for key in &[
                    "daemon.log_level",
                    "effect_engine.extra_effect_dirs",
                ] {
                    on_reset.run(key.to_string());
                }
            }) />
        </section>
    }
}

// ── About ──────────────────────────────────────────────────────────────────

#[component]
pub fn AboutSection() -> impl IntoView {
    let status = LocalResource::new(api::fetch_status);

    view! {
        <section id="section-about" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="About" icon=LuInfo />

            {move || {
                let stat = status.get().and_then(|r| r.ok());
                view! {
                    <div class="space-y-3">
                        <AboutRow label="Version" value=stat.as_ref().map(|s| s.version.clone()).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Uptime" value=stat.as_ref().map(|s| format_uptime(s.uptime_seconds)).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Devices" value=stat.as_ref().map(|s| s.device_count.to_string()).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Effects" value=stat.as_ref().map(|s| s.effect_count.to_string()).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Config" value=stat.as_ref().map(|s| s.config_path.clone()).unwrap_or_else(|| "—".to_string()) />
                    </div>
                }
            }}

            <div class="flex items-center gap-4 mt-4 pt-3 border-t border-edge-subtle/10">
                <a
                    href="https://github.com/hyperb1iss/hypercolor"
                    target="_blank"
                    rel="noopener"
                    class="flex items-center gap-1.5 text-xs text-fg-tertiary hover:text-accent transition-colors"
                >
                    <Icon icon=LuExternalLink width="11px" height="11px" />
                    "GitHub"
                </a>
                <span class="text-[10px] text-fg-tertiary/30">"Apache-2.0"</span>
            </div>
        </section>
    }
}

#[component]
fn AboutRow(label: &'static str, #[prop(into)] value: String) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between py-2 setting-row">
            <span class="text-sm text-fg-tertiary">{label}</span>
            <span class="text-sm text-fg-primary font-mono">{value}</span>
        </div>
    }
}

fn format_uptime(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}
