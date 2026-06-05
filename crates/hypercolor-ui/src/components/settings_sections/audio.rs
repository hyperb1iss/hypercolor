use leptos::prelude::*;

use hypercolor_types::config::HypercolorConfig;

use crate::app::WsContext;
use crate::components::settings_controls::*;
use crate::icons::*;

use super::read_config;

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
            <div class="mb-4 px-3 py-3 rounded-lg animate-enter-fade" style="background: rgba(139, 133, 160, 0.04); border: 1px solid rgba(139, 133, 160, 0.06)">
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
