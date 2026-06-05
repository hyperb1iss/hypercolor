use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::config::HypercolorConfig;

use crate::components::settings_controls::*;
use crate::icons::*;
use crate::tauri_bridge::{self, WindowsDaemonServiceStatus, windows_daemon_service_conflict};
use crate::toasts;

use super::{off_output_behavior_value, read_config, sleep_behavior_value};

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
            <NativeStartupPanel />
            <WindowsDaemonServicePanel />
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

#[component]
fn NativeStartupPanel() -> impl IntoView {
    let native_available = tauri_bridge::is_tauri_available();
    let autostart = LocalResource::new(tauri_bridge::get_autostart_enabled);
    let (updating, set_updating) = signal(false);
    let toggle = move |enabled: bool| {
        if updating.get_untracked() {
            return;
        }

        let next_enabled = !enabled;
        set_updating.set(true);
        leptos::task::spawn_local(async move {
            let result = tauri_bridge::set_autostart_enabled(next_enabled).await;
            set_updating.set(false);

            match result {
                Ok(()) => {
                    if next_enabled {
                        toasts::toast_success("Hypercolor will start at sign in");
                    } else {
                        toasts::toast_success("Hypercolor startup disabled");
                    }
                    autostart.refetch();
                }
                Err(error) => {
                    toasts::toast_error(&format!("Startup setting failed: {error}"));
                    autostart.refetch();
                }
            }
        });
    };

    view! {
        <Show when=move || native_available>
            {move || match autostart.get() {
                None => view! {
                    <NativeStartupFrame>
                        <div class="flex items-center gap-2 text-xs text-fg-tertiary/60">
                            <Icon icon=LuLoader width="13px" height="13px" />
                            "Checking startup setting"
                        </div>
                    </NativeStartupFrame>
                }.into_any(),
                Some(Ok(None)) => ().into_any(),
                Some(Err(error)) => view! {
                    <NativeStartupFrame>
                        <div class="flex items-center gap-2 text-xs text-error-red/80">
                            <Icon icon=LuTriangleAlert width="13px" height="13px" />
                            {format!("Startup setting unavailable: {error}")}
                        </div>
                    </NativeStartupFrame>
                }.into_any(),
                Some(Ok(Some(enabled))) => view! {
                    <NativeStartupToggle
                        enabled=enabled
                        updating=updating
                        on_toggle=Callback::new(move |()| toggle(enabled))
                    />
                }.into_any(),
            }}
        </Show>
    }
}

#[component]
fn NativeStartupToggle(
    enabled: bool,
    #[prop(into)] updating: Signal<bool>,
    on_toggle: Callback<()>,
) -> impl IntoView {
    view! {
        <NativeStartupFrame>
            <div class="flex items-start justify-between gap-4">
                <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2">
                        <span class="text-sm text-fg-primary font-medium">"Start at Sign In"</span>
                        <span
                            class="text-[9px] font-mono px-1.5 py-0.5 rounded"
                            style="color: rgba(128, 255, 234, 0.7); background: rgba(128, 255, 234, 0.08)"
                        >
                            "app"
                        </span>
                    </div>
                    <div class="text-xs text-fg-tertiary/70 mt-0.5">
                        "Launch Hypercolor in the system tray when you sign in"
                    </div>
                </div>
                <button
                    role="switch"
                    aria-checked=enabled.to_string()
                    disabled=move || updating.get()
                    class="relative w-11 h-6 rounded-full transition-all duration-200 shrink-0 mt-0.5 cursor-pointer disabled:cursor-not-allowed disabled:opacity-60"
                    style=move || if enabled {
                        "background: rgba(225, 53, 255, 0.5); box-shadow: 0 0 10px rgba(225, 53, 255, 0.25)"
                    } else {
                        "background: rgba(139, 133, 160, 0.2)"
                    }
                    on:click=move |_| on_toggle.run(())
                >
                    <span
                        class="absolute left-0.5 top-0.5 w-5 h-5 rounded-full shadow-sm transition-transform duration-200"
                        style=move || if enabled {
                            "transform: translateX(22px); background: rgb(225, 53, 255)"
                        } else {
                            "transform: translateX(0); background: rgba(200, 200, 210, 0.6)"
                        }
                    />
                </button>
            </div>
        </NativeStartupFrame>
    }
}

#[component]
fn NativeStartupFrame(children: Children) -> impl IntoView {
    view! {
        <div
            class="mb-4 px-3 py-3 rounded-lg setting-row"
            style="background: rgba(139, 133, 160, 0.035); border: 1px solid rgba(139, 133, 160, 0.06)"
        >
            {children()}
        </div>
    }
}

#[component]
fn WindowsDaemonServicePanel() -> impl IntoView {
    let native_available = tauri_bridge::is_tauri_available();
    let status = LocalResource::new(tauri_bridge::detect_windows_daemon_service);
    let refresh = Callback::new(move |()| status.refetch());

    view! {
        <Show when=move || native_available>
            {move || match status.get() {
                Some(Ok(Some(current))) if windows_daemon_service_conflict(&current) => view! {
                    <WindowsDaemonServiceStatusPanel
                        status=current
                        on_refresh=refresh
                    />
                }.into_any(),
                Some(Err(error)) => view! {
                    <NativeStartupFrame>
                        <div class="flex items-center gap-2 text-xs text-error-red/80">
                            <Icon icon=LuTriangleAlert width="13px" height="13px" />
                            {format!("Windows service status unavailable: {error}")}
                        </div>
                    </NativeStartupFrame>
                }.into_any(),
                _ => ().into_any(),
            }}
        </Show>
    }
}

#[component]
fn WindowsDaemonServiceStatusPanel(
    status: WindowsDaemonServiceStatus,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let service_state = status
        .service
        .state
        .unwrap_or_else(|| "UNKNOWN".to_string());

    view! {
        <NativeStartupFrame>
            <div class="flex items-start justify-between gap-4">
                <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2">
                        <Icon icon=LuActivity width="15px" height="15px" style="color: rgba(241, 250, 140, 0.76)" />
                        <span class="text-sm text-fg-primary font-medium">"Windows Service Mode"</span>
                        <span
                            class="text-[9px] font-mono px-1.5 py-0.5 rounded"
                            style="color: rgba(241, 250, 140, 0.78); background: rgba(241, 250, 140, 0.08); border: 1px solid rgba(241, 250, 140, 0.12)"
                        >
                            {service_state}
                        </span>
                    </div>
                    <div class="text-xs text-fg-tertiary/70 mt-0.5">
                        {format!("Using the {} SCM daemon service", status.service_name)}
                    </div>
                </div>
                <button
                    type="button"
                    aria-label="Refresh Windows service status"
                    title="Refresh Windows service status"
                    class="inline-flex h-7 w-7 items-center justify-center rounded transition-colors shrink-0"
                    style="color: rgba(241, 250, 140, 0.76); background: rgba(241, 250, 140, 0.07); border: 1px solid rgba(241, 250, 140, 0.12)"
                    on:click=move |_| on_refresh.run(())
                >
                    <Icon icon=LuRefreshCw width="13px" height="13px" />
                </button>
            </div>
        </NativeStartupFrame>
    }
}
