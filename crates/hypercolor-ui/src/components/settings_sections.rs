//! Settings section components — one per config domain.

use std::net::IpAddr;

use hypercolor_types::config::{HypercolorConfig, NetworkAccessMode, NetworkClientScope};
use hypercolor_types::session::{OffOutputBehavior, SleepBehavior};
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::settings_controls::*;
use crate::icons::*;
use crate::input_access::{input_status_epoch, screen_status_line};
use crate::render_presets::{
    CANVAS_PRESETS, MAX_CUSTOM_CANVAS_HEIGHT, MAX_CUSTOM_CANVAS_WIDTH, canvas_preset_key,
};
use crate::{
    api::{InputSourcePlatformStatus, InputStatus, SystemStatus},
    app::WsContext,
};

mod about;
mod audio;
mod developer;
mod discovery;
mod input;
mod session;

use input::MacosSystemSettingsButton;

pub use about::AboutSection;
pub use audio::AudioSection;
pub use developer::DeveloperSection;
pub use discovery::DiscoverySection;
pub use input::InputSection;
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

// ── Screen Capture ─────────────────────────────────────────────────────────

#[component]
pub fn CaptureSection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let enabled = Signal::derive(move || read_config(config, |cfg| cfg.capture.enabled));
    let source = Signal::derive(move || read_config(config, |cfg| cfg.capture.source.clone()));
    let capture_fps =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.capture_fps)));
    let grid_cols =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.grid_cols)));
    let grid_rows =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.grid_rows)));
    let smoothing =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.smoothing)));
    let scene_cut = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.scene_cut_threshold))
    });
    let letterbox = Signal::derive(move || read_config(config, |cfg| cfg.capture.letterbox));
    let letterbox_threshold = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.letterbox_threshold))
    });
    let saturation =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.saturation)));
    let brightness =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.brightness)));
    let gamma = Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.gamma)));
    let target_led_white_x = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.target_led_white_x))
    });
    let target_led_white_y = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.target_led_white_y))
    });
    let target_led_reference_white_nits = Signal::derive(move || {
        read_config(config, |cfg| {
            f64::from(cfg.capture.target_led_reference_white_nits)
        })
    });
    let target_led_peak_nits = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.target_led_peak_nits))
    });
    let exposure_ev =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.exposure_ev)));
    let reset_calibration = Callback::new(move |()| {
        on_reset.run("capture.calibration".to_owned());
    });
    let capture_status = LocalResource::new(move || {
        let connection_generation = ws.connection_generation.get();
        let source_event = ws.last_input_source_status_event.get();
        let owner_event = ws.last_macos_daemon_ownership_event.get();
        let epoch = config.with(|current| {
            input_status_epoch(connection_generation, source_event, current.as_ref())
        });
        async move {
            let _ = (epoch, owner_event);
            crate::api::fetch_status().await
        }
    });

    // Monitor picker data. Empty means the platform's backend owns source
    // selection (the XDG portal on Linux), so the portal button renders
    // instead of a dropdown.
    let monitors_resource = crate::api::daemon_resource(crate::api::fetch_capture_monitors);
    let monitors = Signal::derive(move || {
        monitors_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
    });
    let has_monitor_picker = Signal::derive(move || !monitors.get().is_empty());
    let monitor_options = Signal::derive(move || {
        let mut options = vec![("auto".to_owned(), "Primary monitor".to_owned())];
        for monitor in monitors.get() {
            let primary = if monitor.primary { " · primary" } else { "" };
            options.push((
                monitor.value.clone(),
                format!(
                    "Monitor {} · {}\u{d7}{}{}",
                    monitor.index + 1,
                    monitor.width,
                    monitor.height,
                    primary
                ),
            ));
        }
        options
    });

    let (picking, set_picking) = signal(false);
    let (authorizing, set_authorizing) = signal(false);
    let (action_error, set_action_error) = signal(None::<String>);
    let pick_source = move |_| {
        if picking.get_untracked() {
            return;
        }
        set_picking.set(true);
        set_action_error.set(None);
        leptos::task::spawn_local(async move {
            if let Err(e) = crate::api::pick_capture_source().await {
                set_action_error.set(Some(e));
            }
            set_picking.set(false);
            capture_status.refetch();
        });
    };
    let authorize_screen = move |_| {
        if authorizing.get_untracked() {
            return;
        }
        set_authorizing.set(true);
        set_action_error.set(None);
        leptos::task::spawn_local(async move {
            if let Err(error) = crate::api::authorize_screen_recording().await {
                set_action_error.set(Some(error));
            }
            set_authorizing.set(false);
            capture_status.refetch();
        });
    };

    view! {
        <section id="section-capture" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Screen Capture" icon=LuMonitor />
            <SettingToggle
                label="Screen capture"
                description="Let effects like Ambilight and Screen Cast mirror what's on your screen"
                key="capture.enabled"
                value=enabled
                on_change=on_change
            />
            <Show when=move || {
                capture_status
                    .get()
                    .and_then(Result::ok)
                    .is_some_and(|status| macos_screen_needs_authorization(&status.input))
            }>
                <div class="flex items-center justify-between gap-4 border-b border-edge-subtle/40 py-3">
                    <div class="min-w-0">
                        <div class="text-[13px] text-fg-primary">"Screen Recording"</div>
                        <div class="text-xs text-fg-tertiary">
                            "Open Screen Recording in System Settings, enable Hypercolor, then return here."
                        </div>
                    </div>
                    <div class="flex shrink-0 items-center gap-2">
                        <MacosSystemSettingsButton
                            pane=crate::tauri_bridge::MacosSystemSettingsPane::ScreenRecording
                        />
                        <button
                            type="button"
                            class="glow-ring inline-flex shrink-0 items-center rounded-md border border-accent-muted bg-accent-subtle px-2.5 py-1.5 text-xs text-accent hover:bg-accent-muted/20 disabled:opacity-50"
                            disabled=move || !enabled.get() || authorizing.get()
                            on:click=authorize_screen
                        >
                            {move || if authorizing.get() { "Requesting…" } else { "Authorize" }}
                        </button>
                    </div>
                </div>
            </Show>
            {move || capture_status
                .get()
                .and_then(Result::ok)
                .and_then(|status| macos_screen_restart_coordinates(&status))
                .map(|(owner, epoch)| view! {
                    <MacosCaptureOwnerRestartAction
                        owner=owner
                        epoch=epoch
                        on_complete=Callback::new(move |()| capture_status.refetch())
                    />
                })}
            <Show when=move || has_monitor_picker.get()>
                <SettingDropdown
                    label="Monitor"
                    description="Which display to mirror"
                    key="capture.source"
                    value=source
                    options=monitor_options
                    on_change=on_change
                />
            </Show>
            {move || action_error.get().map(|error| view! {
                <div class="mt-2 rounded-lg border border-status-error/24 bg-status-error/8 px-3 py-2 text-xs text-status-error">
                    {error}
                </div>
            })}

            {move || {
                let status = capture_status.get().and_then(Result::ok)?;
                if !enabled.get() || macos_screen_needs_authorization(&status.input) {
                    return None;
                }
                screen_status_line(&status.input)
                    .map(|(tone, text)| input::status_line_view(tone, text))
            }}
            <Show when=move || !has_monitor_picker.get()>
                <div class="flex items-center justify-between gap-4 py-3 border-b border-edge-subtle/40">
                    <div class="min-w-0">
                        <div class="text-[13px] text-fg-primary">"Capture source"</div>
                        <div class="text-xs text-fg-tertiary">
                            "Pick a display, window, or app. Displays persist across restarts; windows and apps stay session scoped"
                        </div>
                    </div>
                    <button
                        type="button"
                        class="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-edge-subtle bg-surface-overlay px-2.5 py-1.5 text-xs text-fg-primary hover:bg-surface-overlay/80 disabled:opacity-50"
                        disabled=move || !enabled.get() || picking.get()
                        on:click=pick_source
                    >
                        {move || if picking.get() { "Opening picker\u{2026}" } else { "Choose source" }}
                    </button>
                </div>
            </Show>

            // Everything below is tuning most users never need: video-era
            // features (letterbox crop, scene cuts), sampling resolution
            // that layouts depend on, and color shaping. Collapsed so the
            // section reads as two decisions — on, and which monitor.
            <AdvancedDisclosure>
                <SettingNumberInput
                    label="Capture FPS"
                    description="How often the screen is sampled per second"
                    key="capture.capture_fps"
                    value=capture_fps
                    on_change=on_change
                    min=1.0 max={u32::MAX as f64} step=1.0
                />
                <SettingSlider
                    label="Color response"
                    description="How quickly LED colors follow the screen \u{2014} lower glides, higher snaps"
                    key="capture.smoothing"
                    value=smoothing
                    on_change=on_change
                    min=0.05 max=1.0 step=0.05
                />
                <SettingSlider
                    label="Saturation"
                    description="Color intensity of the extracted lighting"
                    key="capture.saturation"
                    value=saturation
                    on_change=on_change
                    min=0.0 max=2.5 step=0.05
                />
                <SettingSlider
                    label="Brightness"
                    description="Brightness of the extracted lighting"
                    key="capture.brightness"
                    value=brightness
                    on_change=on_change
                    min=0.0 max=2.0 step=0.05
                />
                <SettingSlider
                    label="Gamma"
                    description="Midtone shaping \u{2014} above 1.0 deepens darks"
                    key="capture.gamma"
                    value=gamma
                    on_change=on_change
                    min=0.4 max=2.5 step=0.05
                />
                <div class="mt-3 border-t border-edge-subtle/40 pt-3">
                    <div class="text-sm font-medium text-fg-primary">"LED tone mapping"</div>
                    <div class="mt-0.5 text-xs text-fg-tertiary/70">
                        "Calibrate HDR white, output headroom, and exposure"
                    </div>
                </div>
                <SettingNumberInput
                    label="White point x"
                    description="CIE xy x coordinate. D65 is 0.3127"
                    key="capture.target_led_white_x"
                    value=target_led_white_x
                    on_change=on_change
                    min={f64::from(f32::MIN_POSITIVE)} max=0.9999999403953552 step=0.0001
                    integer=false
                />
                <SettingNumberInput
                    label="White point y"
                    description="CIE xy y coordinate. D65 is 0.3290"
                    key="capture.target_led_white_y"
                    value=target_led_white_y
                    on_change=on_change
                    min={f64::from(f32::MIN_POSITIVE)} max=0.9999999403953552 step=0.0001
                    integer=false
                />
                <SettingNumberInput
                    label="Reference white"
                    description="Nominal LED reference white in nits"
                    key="capture.target_led_reference_white_nits"
                    value=target_led_reference_white_nits
                    on_change=on_change
                    min=1.0 max=5000.0 step=1.0
                />
                <SettingNumberInput
                    label="Peak luminance"
                    description="Calibrated LED peak in nits. Must exceed reference white"
                    key="capture.target_led_peak_nits"
                    value=target_led_peak_nits
                    on_change=on_change
                    min=1.0 max=10000.0 step=1.0
                />
                <SettingNumberInput
                    label="Exposure"
                    description="Tone-mapping exposure in EV stops"
                    key="capture.exposure_ev"
                    value=exposure_ev
                    on_change=on_change
                    min=-8.0 max=8.0 step=0.1
                    integer=false
                />
                <button
                    class="glow-ring mt-1 inline-flex items-center gap-1.5 rounded-md border border-edge-subtle px-2.5 py-1.5 text-xs text-fg-tertiary transition-colors hover:border-accent-muted hover:text-accent"
                    on:click=move |_| reset_calibration.run(())
                >
                    <Icon icon=LuUndo2 width="12px" height="12px" />
                    "Reset calibration"
                </button>
                <SettingSlider
                    label="Sampling columns"
                    description="Ambient color regions across the screen. Layouts reference these zones \u{2014} changing this remaps them"
                    key="capture.grid_cols"
                    value=grid_cols
                    on_change=on_change
                    min=2.0 max=32.0 step=1.0
                    decimals=0
                    integer=true
                />
                <SettingSlider
                    label="Sampling rows"
                    description="Ambient color regions down the screen. Layouts reference these zones \u{2014} changing this remaps them"
                    key="capture.grid_rows"
                    value=grid_rows
                    on_change=on_change
                    min=2.0 max=32.0 step=1.0
                    decimals=0
                    integer=true
                />
                <SettingToggle
                    label="Crop video letterbox"
                    description="Ignore the black bars around letterboxed video. Leave off for desktop mirroring \u{2014} dark scenes can be mistaken for bars"
                    key="capture.letterbox"
                    value=letterbox
                    on_change=on_change
                />
                <SettingSlider
                    label="Letterbox threshold"
                    description="How dark a row must be to count as a bar"
                    key="capture.letterbox_threshold"
                    value=letterbox_threshold
                    on_change=on_change
                    min=0.0 max=0.2 step=0.005
                    decimals=3
                />
                <SettingSlider
                    label="Scene cut threshold"
                    description="Hard video cuts above this level snap colors instantly instead of gliding"
                    key="capture.scene_cut_threshold"
                    value=scene_cut
                    on_change=on_change
                    min=0.0 max=500.0 step=10.0
                    decimals=0
                />
            </AdvancedDisclosure>
            <SectionReset section_label="Capture" on_reset=Callback::new(move |()| on_reset.run("capture".to_string())) />
        </section>
    }
}

fn macos_screen_needs_authorization(status: &InputStatus) -> bool {
    status.sources.iter().any(|source| {
        if source.retired {
            return false;
        }
        let Some(InputSourcePlatformStatus::MacosScreen { state, tcc, .. }) =
            source.platform.as_ref()
        else {
            return false;
        };
        matches!(
            state.as_deref(),
            Some("needs_user_action" | "permission_denied" | "revoked")
        ) || matches!(
            tcc.as_deref(),
            Some("not_determined" | "denied" | "revoked")
        )
    })
}

fn macos_screen_needs_restart(status: &InputStatus) -> bool {
    status.sources.iter().any(|source| {
        if source.retired {
            return false;
        }
        matches!(
            source.platform.as_ref(),
            Some(InputSourcePlatformStatus::MacosScreen { state, .. })
                if state.as_deref() == Some("needs_process_restart")
        )
    })
}

fn macos_screen_restart_coordinates(status: &SystemStatus) -> Option<(String, u64)> {
    macos_screen_needs_restart(&status.input)
        .then_some(status.macos_daemon_ownership.as_ref())
        .flatten()
        .and_then(|ownership| {
            Some((
                validate_macos_restart_owner(ownership.active_owner.as_deref()?)?,
                ownership.owner_epoch?,
            ))
        })
}

pub(super) fn validate_macos_restart_owner(owner: &str) -> Option<String> {
    match owner {
        "app_sidecar" | "launchd_service" | "homebrew_service" | "standalone" => {
            Some(owner.to_owned())
        }
        _ => None,
    }
}

#[component]
pub(super) fn MacosCaptureOwnerRestartAction(
    owner: String,
    epoch: u64,
    on_complete: Callback<()>,
) -> impl IntoView {
    let native_available = crate::tauri_bridge::is_tauri_available();
    let owner_for_action = StoredValue::new(owner.clone());
    let (restarting, set_restarting) = signal(false);
    let (result_message, set_result_message) = signal(None::<String>);
    let restart = move |_| {
        if restarting.get_untracked() || !native_available {
            return;
        }
        set_restarting.set(true);
        set_result_message.set(None);
        let owner = owner_for_action.get_value();
        leptos::task::spawn_local(async move {
            match crate::tauri_bridge::restart_macos_capture_owner(&owner, epoch).await {
                Ok(Some(crate::tauri_bridge::MacosCaptureOwnerRestartOutcome::Restarted {
                    owner,
                    ..
                })) => {
                    let _ = owner;
                    let message = "Capture service restarted.".to_owned();
                    crate::toasts::toast_success(&message);
                    set_result_message.set(Some(message));
                }
                Ok(Some(
                    crate::tauri_bridge::MacosCaptureOwnerRestartOutcome::UserActionRequired {
                        remedy,
                        ..
                    },
                )) => set_result_message.set(Some(match remedy {
                    crate::tauri_bridge::MacosOwnerRemedy::StopStandaloneOwner { pid } => {
                        format!("Stop standalone process {pid}, then retry.")
                    }
                    _ => "The active owner requires a local user action.".to_owned(),
                })),
                Ok(Some(crate::tauri_bridge::MacosCaptureOwnerRestartOutcome::Unknown)) => {
                    set_result_message.set(Some(
                        "A newer Hypercolor app returned an unknown restart result.".to_owned(),
                    ));
                }
                Ok(None) => set_result_message.set(Some(
                    "Open Hypercolor.app to finish this restart.".to_owned(),
                )),
                Err(error) => {
                    set_result_message.set(Some(format!("Capture owner restart failed: {error}")))
                }
            }
            set_restarting.set(false);
            on_complete.run(());
        });
    };

    view! {
        <div class="flex items-center justify-between gap-4 border-b border-edge-subtle/40 py-3">
            <div class="min-w-0">
                <div class="text-[13px] text-fg-primary">"Restart capture owner"</div>
                <div class="text-xs text-fg-tertiary">
                    "Permission granted. Hypercolor needs a quick restart of its capture service to start using it."
                </div>
                <Show when=move || !native_available>
                    <div class="mt-1 text-xs text-status-warning">
                        "Open Hypercolor.app to restart it."
                    </div>
                </Show>
                {move || result_message.get().map(|message| view! {
                    <div class="mt-1 text-xs text-fg-secondary">{message}</div>
                })}
            </div>
            <button
                type="button"
                class="glow-ring inline-flex shrink-0 items-center rounded-md border border-accent-muted bg-accent-subtle px-2.5 py-1.5 text-xs text-accent hover:bg-accent-muted/20 disabled:opacity-50"
                disabled=move || !native_available || restarting.get()
                on:click=restart
            >
                {move || if restarting.get() { "Restarting…" } else { "Restart" }}
            </button>
        </div>
    }
}

#[cfg(test)]
mod macos_capture_tests {
    use crate::api::{
        InputSourcePlatformStatus, InputSourceStatus, InputStatus, MacosDaemonOwnershipStatus,
        SystemStatus,
    };

    use super::{macos_screen_restart_coordinates, validate_macos_restart_owner};

    fn system_status(
        input: InputStatus,
        macos_daemon_ownership: Option<MacosDaemonOwnershipStatus>,
    ) -> SystemStatus {
        SystemStatus {
            running: true,
            version: "test".to_owned(),
            config_path: String::new(),
            uptime_seconds: 1,
            device_count: 0,
            effect_count: 0,
            active_effect: None,
            active_scene: None,
            active_scene_snapshot_locked: false,
            global_brightness: 100,
            compositor_acceleration: crate::api::RenderAccelerationStatus::default(),
            render_loop: crate::api::RenderLoopStatus::default(),
            capabilities: Vec::new(),
            input,
            macos_daemon_ownership,
        }
    }

    #[test]
    fn screen_restart_coordinates_require_exact_restart_state() {
        let mut status = system_status(
            InputStatus {
                sources: vec![InputSourceStatus {
                    kind: "screen".to_owned(),
                    platform: Some(InputSourcePlatformStatus::MacosScreen {
                        state: Some("needs_process_restart".to_owned()),
                        tcc: Some("authorized".to_owned()),
                        owner: Some("launchd_service".to_owned()),
                        selection: None,
                        tahoe: None,
                        tahoe_selection: None,
                        owner_conflict: None,
                    }),
                    ..InputSourceStatus::default()
                }],
                ..InputStatus::default()
            },
            Some(MacosDaemonOwnershipStatus {
                active_owner: Some("launchd_service".to_owned()),
                owner_epoch: Some(31),
                ..MacosDaemonOwnershipStatus::default()
            }),
        );

        assert_eq!(
            macos_screen_restart_coordinates(&status),
            Some(("launchd_service".to_owned(), 31))
        );
        status.input.sources[0].retired = true;
        assert_eq!(macos_screen_restart_coordinates(&status), None);
        status.input.sources[0].retired = false;
        status.input.sources.clear();
        assert_eq!(macos_screen_restart_coordinates(&status), None);
    }

    #[test]
    fn owner_command_names_are_closed() {
        assert_eq!(
            validate_macos_restart_owner("homebrew_service").as_deref(),
            Some("homebrew_service")
        );
        assert_eq!(validate_macos_restart_owner("future_owner"), None);
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
            <AdvancedDisclosure>
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
            </AdvancedDisclosure>
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
            <AdvancedDisclosure>
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
                    min=1.0 max=MAX_CUSTOM_CANVAS_WIDTH step=1.0
                />
                <SettingNumberInput
                    label="Canvas Height"
                    description="Pixels tall"
                    key="daemon.canvas_height"
                    value=canvas_height
                    on_change=on_change
                    min=1.0 max=MAX_CUSTOM_CANVAS_HEIGHT step=1.0
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
            </AdvancedDisclosure>
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
