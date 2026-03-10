//! Settings section components — one per config domain.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::session::{IdleBackend, SleepBehavior};

use crate::api;
use crate::app::WsContext;
use crate::components::settings_controls::*;
use crate::icons::*;

fn read_config<T>(
    config: Signal<Option<HypercolorConfig>>,
    selector: impl FnOnce(&HypercolorConfig) -> T,
) -> T
where
    T: Default,
{
    config.with(|cfg| cfg.as_ref().map(selector).unwrap_or_default())
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

fn idle_backend_value(backend: IdleBackend) -> String {
    match backend {
        IdleBackend::Auto => "auto",
        IdleBackend::Wayland => "wayland",
        IdleBackend::X11 => "x11",
        IdleBackend::Dbus => "dbus",
        IdleBackend::Disabled => "disabled",
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
            <SectionHeader
                title="Audio"
                description="Reactive input, analysis, and beat detection for effects that listen to the room."
                icon=LuAudioLines
            />
            <AudioVuMeter enabled=enabled />
            <SettingGroupHeading
                title="Input"
                description="Choose the capture source and whether the daemon should listen at all."
            />
            <SettingToggle
                label="Enabled"
                description="Enable audio capture and spectrum analysis for reactive effects"
                key="audio.enabled"
                value=enabled
                on_change=on_change
            />
            <SettingDropdown
                label="Device"
                description="Audio source for reactive effects"
                key="audio.device"
                value=device
                options=audio_devices
                on_change=on_change
            />
            <SettingGroupHeading
                title="Analysis"
                description="Tune frequency resolution, smoothing, and sensitivity for the reactive pipeline."
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
        ("x11".to_string(), "X11".to_string()),
    ];

    view! {
        <section id="section-capture" class="pt-5 pb-3 space-y-0">
            <SectionHeader
                title="Screen Capture"
                description="Ambient capture settings for effects that mirror the display."
                icon=LuMonitor
            />
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

// ── Effect Engine ──────────────────────────────────────────────────────────

#[component]
pub fn EngineSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let renderer = Signal::derive(move || {
        read_config(config, |cfg| cfg.effect_engine.preferred_renderer.clone())
    });
    let servo = Signal::derive(move || read_config(config, |cfg| cfg.effect_engine.servo_enabled));
    let wgpu =
        Signal::derive(move || read_config(config, |cfg| cfg.effect_engine.wgpu_backend.clone()));
    let extra_dirs = Signal::derive(move || {
        read_config(config, |cfg| {
            cfg.effect_engine
                .extra_effect_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
        })
    });
    let watch_effects =
        Signal::derive(move || read_config(config, |cfg| cfg.effect_engine.watch_effects));
    let watch_config =
        Signal::derive(move || read_config(config, |cfg| cfg.effect_engine.watch_config));

    let renderer_options = vec![
        ("auto".to_string(), "Auto".to_string()),
        ("servo".to_string(), "Servo".to_string()),
        ("wgpu".to_string(), "wgpu".to_string()),
    ];
    let wgpu_options = vec![
        ("auto".to_string(), "Auto".to_string()),
        ("vulkan".to_string(), "Vulkan".to_string()),
        ("gl".to_string(), "OpenGL".to_string()),
    ];

    view! {
        <section id="section-engine" class="pt-5 pb-3 space-y-0">
            <SectionHeader
                title="Effect Engine"
                description="Choose the renderer, browser runtime, and file watching behavior that power your effect library."
                icon=LuZap
            />
            <SettingGroupHeading
                title="Renderer"
                description="These choices shape how HTML and shader effects are executed."
            />
            <SettingDropdown
                label="Preferred Renderer"
                description="Rendering backend for effects"
                key="effect_engine.preferred_renderer"
                value=renderer
                options=Signal::stored(renderer_options)
                on_change=on_change
                restart_required=true
            />
            <SettingToggle
                label="Servo Enabled"
                description="Enable the Servo browser engine for HTML effects"
                key="effect_engine.servo_enabled"
                value=servo
                on_change=on_change
                restart_required=true
            />
            <SettingDropdown
                label="wgpu Backend"
                description="GPU backend for shader-based effects"
                key="effect_engine.wgpu_backend"
                value=wgpu
                options=Signal::stored(wgpu_options)
                on_change=on_change
                restart_required=true
            />
            <SettingGroupHeading
                title="Library"
                description="Watch your local effect sources and extend the search path with custom directories."
            />
            <SettingPathList
                label="Extra Effect Directories"
                description="Additional directories to scan for custom effects"
                key="effect_engine.extra_effect_dirs"
                paths=extra_dirs
                on_change=on_change
            />
            <SettingToggle
                label="Watch Effects"
                description="Auto-reload effects when source files change"
                key="effect_engine.watch_effects"
                value=watch_effects
                on_change=on_change
                restart_required=true
            />
            <SettingToggle
                label="Watch Config"
                description="Auto-reload when hypercolor.toml changes on disk"
                key="effect_engine.watch_config"
                value=watch_config
                on_change=on_change
                restart_required=true
            />
            <SectionReset section_label="Effect Engine" on_reset=Callback::new(move |()| on_reset.run("effect_engine".to_string())) />
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
    let target_fps =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.daemon.target_fps)));
    let ws_fps =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.web.websocket_fps)));
    let open_browser = Signal::derive(move || read_config(config, |cfg| cfg.web.open_browser));

    view! {
        <section id="section-network" class="pt-5 pb-3 space-y-0">
            <SectionHeader
                title="Runtime & Web"
                description="Listener settings, render cadence, and preview streaming for the daemon and web UI."
                icon=LuGlobe
            />
            <SettingGroupHeading
                title="Daemon"
                description="Where Hypercolor listens and how fast it pushes frames."
            />
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
            <SettingSlider
                label="Target FPS"
                description="Render loop frame rate for the lighting engine"
                key="daemon.target_fps"
                value=target_fps
                on_change=on_change
                min=1.0 max=120.0 step=1.0
                decimals=0
                integer=true
            />
            <SettingGroupHeading
                title="Web Preview"
                description="Control the browser preview stream and startup convenience behavior."
            />
            <SettingSlider
                label="WebSocket FPS"
                description="Frame rate for the live preview stream"
                key="web.websocket_fps"
                value=ws_fps
                on_change=on_change
                min=1.0 max=60.0 step=1.0
                decimals=0
                integer=true
            />
            <SettingToggle
                label="Open Browser on Start"
                description="Automatically open the web UI when the daemon starts"
                key="web.open_browser"
                value=open_browser
                on_change=on_change
            />
            <SectionReset section_label="Runtime & Web" on_reset=Callback::new(move |()| {
                // Reset only the keys owned by this section — avoid nuking the
                // entire "daemon" section which would wipe developer settings too.
                for key in &[
                    "daemon.listen_address", "daemon.port", "daemon.target_fps",
                    "web.websocket_fps", "web.open_browser",
                ] {
                    on_reset.run(key.to_string());
                }
            }) />
        </section>
    }
}

// ── MCP ────────────────────────────────────────────────────────────────────

#[component]
pub fn McpSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let enabled = Signal::derive(move || read_config(config, |cfg| cfg.mcp.enabled));
    let base_path = Signal::derive(move || read_config(config, |cfg| cfg.mcp.base_path.clone()));
    let stateful_mode = Signal::derive(move || read_config(config, |cfg| cfg.mcp.stateful_mode));
    let json_response = Signal::derive(move || read_config(config, |cfg| cfg.mcp.json_response));
    let sse_keep_alive_secs =
        Signal::derive(move || read_config(config, |cfg| cfg.mcp.sse_keep_alive_secs as f64));

    view! {
        <section id="section-mcp" class="pt-5 pb-3 space-y-0">
            <SectionHeader
                title="MCP"
                description="Configure the Model Context Protocol surface exposed by the daemon."
                icon=LuCable
            />
            <SettingToggle
                label="Enabled"
                description="Expose Hypercolor's Model Context Protocol server on the main HTTP listener"
                key="mcp.enabled"
                value=enabled
                on_change=on_change
                restart_required=true
            />
            <SettingTextInput
                label="Base Path"
                description="HTTP mount path for the MCP endpoint on the existing server"
                key="mcp.base_path"
                value=base_path
                on_change=on_change
                restart_required=true
                placeholder="/mcp"
            />
            <SettingToggle
                label="Stateful Sessions"
                description="Use MCP session headers and SSE streams for multi-request conversations"
                key="mcp.stateful_mode"
                value=stateful_mode
                on_change=on_change
                restart_required=true
            />
            <SettingToggle
                label="JSON Responses"
                description="Return plain JSON instead of SSE for stateless requests; ignored when stateful sessions are enabled"
                key="mcp.json_response"
                value=json_response
                on_change=on_change
                restart_required=true
            />
            <SettingNumberInput
                label="SSE Keep Alive"
                description="Seconds between SSE keep-alive pings in stateful mode (0 disables keep-alives)"
                key="mcp.sse_keep_alive_secs"
                value=sse_keep_alive_secs
                on_change=on_change
                min=0.0 max=300.0 step=1.0
                restart_required=true
            />
            <SectionReset section_label="MCP" on_reset=Callback::new(move |()| on_reset.run("mcp".to_string())) />
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
    let idle_backend = Signal::derive(move || {
        read_config(config, |cfg| idle_backend_value(cfg.session.idle_backend))
    });
    let dim_timeout =
        Signal::derive(move || read_config(config, |cfg| cfg.session.idle_dim_timeout_secs as f64));
    let off_timeout =
        Signal::derive(move || read_config(config, |cfg| cfg.session.idle_off_timeout_secs as f64));
    let screen_lock_behavior = Signal::derive(move || {
        read_config(config, |cfg| {
            sleep_behavior_value(cfg.session.on_screen_lock)
        })
    });
    let screen_lock_scene =
        Signal::derive(move || read_config(config, |cfg| cfg.session.screen_lock_scene.clone()));
    let screen_lock_brightness = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.session.screen_lock_brightness))
    });
    let screen_lock_fade =
        Signal::derive(move || read_config(config, |cfg| cfg.session.screen_lock_fade_ms as f64));
    let screen_unlock_fade =
        Signal::derive(move || read_config(config, |cfg| cfg.session.screen_unlock_fade_ms as f64));
    let suspend_behavior = Signal::derive(move || {
        read_config(config, |cfg| sleep_behavior_value(cfg.session.on_suspend))
    });
    let suspend_fade =
        Signal::derive(move || read_config(config, |cfg| cfg.session.suspend_fade_ms as f64));
    let resume_fade =
        Signal::derive(move || read_config(config, |cfg| cfg.session.resume_fade_ms as f64));
    let lid_close_behavior = Signal::derive(move || {
        read_config(config, |cfg| sleep_behavior_value(cfg.session.on_lid_close))
    });
    let lid_close_brightness = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.session.lid_close_brightness))
    });
    let lid_close_scene =
        Signal::derive(move || read_config(config, |cfg| cfg.session.lid_close_scene.clone()));
    let lid_close_fade =
        Signal::derive(move || read_config(config, |cfg| cfg.session.lid_close_fade_ms as f64));
    let lid_open_fade =
        Signal::derive(move || read_config(config, |cfg| cfg.session.lid_open_fade_ms as f64));

    let screen_behavior_options = Signal::stored(vec![
        ("ignore".to_string(), "Ignore".to_string()),
        ("off".to_string(), "Turn Off".to_string()),
        ("dim".to_string(), "Dim output".to_string()),
        ("scene".to_string(), "Activate scene".to_string()),
    ]);
    let suspend_behavior_options = Signal::stored(vec![
        ("ignore".to_string(), "Ignore".to_string()),
        ("off".to_string(), "Turn Off".to_string()),
        ("dim".to_string(), "Dim output".to_string()),
    ]);
    let idle_backend_options = Signal::stored(vec![
        ("auto".to_string(), "Auto".to_string()),
        ("wayland".to_string(), "Wayland".to_string()),
        ("x11".to_string(), "X11".to_string()),
        ("dbus".to_string(), "D-Bus".to_string()),
        ("disabled".to_string(), "Disabled".to_string()),
    ]);

    view! {
        <section id="section-session" class="pt-5 pb-3 space-y-0">
            <SectionHeader
                title="Session & Power"
                description="Actual suspend, lock, idle, and lid policies so the lighting engine behaves predictably around the desktop."
                icon=LuPower
            />
            <SettingToggle
                label="Session Awareness"
                description="React to actual suspend/resume, screen lock, and other desktop power events"
                key="session.enabled"
                value=enabled
                on_change=on_change
            />
            <SettingGroupHeading
                title="Lock"
                description="Control what happens when the session locks and how quickly the output recovers."
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
            <Show when=move || screen_lock_behavior.get() == "scene">
                <SettingTextInput
                    label="Screen Lock Scene"
                    description="Named scene to activate while the screen is locked"
                    key="session.screen_lock_scene"
                    value=screen_lock_scene
                    on_change=on_change
                    placeholder="night-mode"
                />
            </Show>
            <SettingNumberInput
                label="Screen Lock Fade"
                description="Milliseconds to fade into the screen-lock state"
                key="session.screen_lock_fade_ms"
                value=screen_lock_fade
                on_change=on_change
                min=0.0 max=10000.0 step=50.0
            />
            <SettingNumberInput
                label="Unlock Fade"
                description="Milliseconds to restore output after the session unlocks"
                key="session.screen_unlock_fade_ms"
                value=screen_unlock_fade
                on_change=on_change
                min=0.0 max=10000.0 step=50.0
            />
            <SettingGroupHeading
                title="Suspend"
                description="Tune how Hypercolor fades before suspend and restores after resume."
            />
            <SettingDropdown
                label="Suspend Behavior"
                description="Choose what happens when the OS is actually preparing to suspend"
                key="session.on_suspend"
                value=suspend_behavior
                options=suspend_behavior_options
                on_change=on_change
            />
            <SettingNumberInput
                label="Suspend Fade"
                description="Milliseconds to fade before the kernel suspends"
                key="session.suspend_fade_ms"
                value=suspend_fade
                on_change=on_change
                min=0.0 max=5000.0 step=25.0
            />
            <SettingNumberInput
                label="Resume Fade"
                description="Milliseconds to restore output after resume"
                key="session.resume_fade_ms"
                value=resume_fade
                on_change=on_change
                min=0.0 max=5000.0 step=25.0
            />
            <SettingGroupHeading
                title="Idle"
                description="Use activity detection to dim or switch off LEDs after inactivity."
            />
            <SettingToggle
                label="Idle Detection"
                description="Dim or turn off LEDs after a period of inactivity"
                key="session.idle_enabled"
                value=idle_enabled
                on_change=on_change
            />
            <SettingDropdown
                label="Idle Backend"
                description="Preferred source for idle state detection on this machine"
                key="session.idle_backend"
                value=idle_backend
                options=idle_backend_options
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
            <SettingGroupHeading
                title="Lid"
                description="Laptop-only behavior for lid close and reopen events."
            />
            <SettingDropdown
                label="Lid Close Behavior"
                description="Policy to apply when the laptop lid closes"
                key="session.on_lid_close"
                value=lid_close_behavior
                options=screen_behavior_options
                on_change=on_change
            />
            <Show when=move || lid_close_behavior.get() == "dim">
                <SettingSlider
                    label="Lid Close Brightness"
                    description="Brightness multiplier applied while the lid is closed"
                    key="session.lid_close_brightness"
                    value=lid_close_brightness
                    on_change=on_change
                    min=0.0 max=1.0 step=0.05
                />
            </Show>
            <Show when=move || lid_close_behavior.get() == "scene">
                <SettingTextInput
                    label="Lid Close Scene"
                    description="Named scene to activate while the lid is closed"
                    key="session.lid_close_scene"
                    value=lid_close_scene
                    on_change=on_change
                    placeholder="sleep-scene"
                />
            </Show>
            <SettingNumberInput
                label="Lid Close Fade"
                description="Milliseconds to fade into the lid-close state"
                key="session.lid_close_fade_ms"
                value=lid_close_fade
                on_change=on_change
                min=0.0 max=5000.0 step=25.0
            />
            <SettingNumberInput
                label="Lid Open Fade"
                description="Milliseconds to restore output after the lid opens"
                key="session.lid_open_fade_ms"
                value=lid_open_fade
                on_change=on_change
                min=0.0 max=5000.0 step=25.0
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
    let wled = Signal::derive(move || read_config(config, |cfg| cfg.discovery.wled_scan));
    let hue = Signal::derive(move || read_config(config, |cfg| cfg.discovery.hue_scan));
    view! {
        <section id="section-discovery" class="pt-5 pb-3 space-y-0">
            <SectionHeader
                title="Device Discovery"
                description="Background scanning for local controllers, bridges, and lighting endpoints."
                icon=LuRadar
            />
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
                key="discovery.wled_scan"
                value=wled
                on_change=on_change
            />
            <SettingToggle
                label="Hue Scan"
                description="Discover Philips Hue bridges"
                key="discovery.hue_scan"
                value=hue
                on_change=on_change
            />
            <SectionReset section_label="Discovery" on_reset=Callback::new(move |()| on_reset.run("discovery".to_string())) />
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
    let canvas_width =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.daemon.canvas_width)));
    let canvas_height =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.daemon.canvas_height)));
    let max_devices =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.daemon.max_devices)));
    let wasm_plugins = Signal::derive(move || read_config(config, |cfg| cfg.features.wasm_plugins));
    let hue_entertainment =
        Signal::derive(move || read_config(config, |cfg| cfg.features.hue_entertainment));
    let midi_input = Signal::derive(move || read_config(config, |cfg| cfg.features.midi_input));

    let log_options = vec![
        ("trace".to_string(), "Trace".to_string()),
        ("debug".to_string(), "Debug".to_string()),
        ("info".to_string(), "Info".to_string()),
        ("warn".to_string(), "Warn".to_string()),
        ("error".to_string(), "Error".to_string()),
    ];

    view! {
        <section id="section-developer" class="pt-5 pb-3 space-y-0">
            <SectionHeader
                title="Developer"
                description="Diagnostics, canvas overrides, and explicitly experimental runtime features."
                icon=LuCode
            />
            <SettingGroupHeading
                title="Diagnostics"
                description="Low-level controls used for development, profiling, and debugging."
            />
            <SettingDropdown
                label="Log Level"
                description="Daemon logging verbosity"
                key="daemon.log_level"
                value=log_level
                options=Signal::stored(log_options)
                on_change=on_change
            />
            <SettingNumberInput
                label="Canvas Width"
                description="Internal render canvas width in pixels"
                key="daemon.canvas_width"
                value=canvas_width
                on_change=on_change
                min=32.0 max=1920.0 step=16.0
                restart_required=true
            />
            <SettingNumberInput
                label="Canvas Height"
                description="Internal render canvas height in pixels"
                key="daemon.canvas_height"
                value=canvas_height
                on_change=on_change
                min=32.0 max=1080.0 step=16.0
                restart_required=true
            />
            <SettingNumberInput
                label="Max Devices"
                description="Maximum number of connected devices"
                key="daemon.max_devices"
                value=max_devices
                on_change=on_change
                min=1.0 max=128.0 step=1.0
            />

            // Feature flags
            <div class="flex items-center gap-2 mt-4 mb-3 pt-3 border-t border-edge-subtle/10">
                <Icon icon=LuFlaskConical width="14px" height="14px" style="color: rgba(241, 250, 140, 0.5)" />
                <span class="text-xs font-mono uppercase tracking-[0.08em] text-fg-tertiary/60">"Experimental Features"</span>
            </div>
            <SettingToggle
                label="WASM Plugins"
                description="Enable WebAssembly plugin runtime"
                key="features.wasm_plugins"
                value=wasm_plugins
                on_change=on_change
                restart_required=true
            />
            <SettingToggle
                label="Hue Entertainment"
                description="Enable Philips Hue Entertainment API for low-latency streaming"
                key="features.hue_entertainment"
                value=hue_entertainment
                on_change=on_change
                restart_required=true
            />
            <SettingToggle
                label="MIDI Input"
                description="Enable MIDI device input for effect control"
                key="features.midi_input"
                value=midi_input
                on_change=on_change
                restart_required=true
            />
            <SectionReset section_label="Developer" on_reset=Callback::new(move |()| {
                // Developer section spans multiple config keys — reset individually
                for key in &[
                    "daemon.log_level",
                    "daemon.canvas_width", "daemon.canvas_height",
                    "daemon.max_devices", "features",
                ] {
                    on_reset.run(key.to_string());
                }
            }) />
        </section>
    }
}

// ── About ──────────────────────────────────────────────────────────────────

#[component]
pub fn AboutSection(#[prop(into)] config_path: Signal<String>) -> impl IntoView {
    let status = LocalResource::new(api::fetch_status);

    view! {
        <section id="section-about" class="pt-5 pb-3 space-y-0">
            <SectionHeader
                title="About"
                description="Runtime status and project metadata for the daemon currently serving this UI."
                icon=LuInfo
            />

            {move || {
                let stat = status.get().and_then(|r| r.ok());
                let path = config_path.get();
                view! {
                    <div class="space-y-3">
                        <AboutRow label="Version" value=stat.as_ref().map(|s| s.version.clone()).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Uptime" value=stat.as_ref().map(|s| format_uptime(s.uptime_seconds)).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Devices" value=stat.as_ref().map(|s| s.device_count.to_string()).unwrap_or_else(|| "—".to_string()) />
                        <AboutRow label="Effects" value=stat.as_ref().map(|s| s.effect_count.to_string()).unwrap_or_else(|| "—".to_string()) />
                        {(!path.is_empty()).then(|| view! {
                            <AboutRow label="Config" value=path.clone() />
                        })}
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
