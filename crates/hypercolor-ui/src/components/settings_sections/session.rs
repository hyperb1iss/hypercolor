use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::config::HypercolorConfig;

use crate::api::{self, MacosCapabilityOwner, MacosDaemonOwnershipStatus};
use crate::app::WsContext;
use crate::components::settings_controls::*;
use crate::icons::*;
use crate::tauri_bridge::{
    self, MacosDaemonOwnerChoice, MacosOwnerCoordinatorOutcome, MacosOwnerRemedy,
    WindowsDaemonServiceStatus, windows_daemon_service_conflict,
};
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
            <MacosDaemonOwnershipPanel />
            <WindowsDaemonServicePanel />
            <SettingToggle
                label="Session Awareness"
                description="React to actual suspend/resume, screen lock, and other desktop power events"
                key="session.enabled"
                value=enabled
                on_change=on_change
            />
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
            <AdvancedDisclosure>
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
            </AdvancedDisclosure>
            <SectionReset section_label="Session" on_reset=Callback::new(move |()| on_reset.run("session".to_string())) />
        </section>
    }
}

#[component]
fn MacosDaemonOwnershipPanel() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let native_available = tauri_bridge::is_tauri_available();
    let ownership = LocalResource::new(move || {
        let generation = ws.connection_generation.get();
        let event = ws.last_macos_daemon_ownership_event.get();
        async move {
            let _ = (generation, event);
            api::fetch_status()
                .await
                .map(|status| status.macos_daemon_ownership)
        }
    });
    let offline = LocalResource::new(tauri_bridge::macos_daemon_owner_offline_status);
    let (switching, set_switching) = signal(None::<MacosDaemonOwnerChoice>);
    let (result_message, set_result_message) = signal(None::<String>);
    let (starting_offline, set_starting_offline) = signal(false);
    let (offline_message, set_offline_message) = signal(None::<String>);
    let choose_owner = Callback::new(move |owner: MacosDaemonOwnerChoice| {
        if switching.get_untracked().is_some() {
            return;
        }
        set_switching.set(Some(owner));
        set_result_message.set(None);
        leptos::task::spawn_local(async move {
            let result = tauri_bridge::choose_macos_daemon_owner(owner).await;
            match result {
                Ok(Some(outcome)) => {
                    let message = macos_owner_outcome_message(&outcome);
                    if matches!(outcome, MacosOwnerCoordinatorOutcome::Active { .. }) {
                        toasts::toast_success(&message);
                    }
                    set_result_message.set(Some(message));
                }
                Ok(None) => set_result_message.set(Some(
                    "Open this page in Hypercolor.app to make this change.".to_owned(),
                )),
                Err(error) => set_result_message.set(Some(format!("The switch failed: {error}"))),
            }
            set_switching.set(None);
            ownership.refetch();
            offline.refetch();
        });
    });
    let start_offline_owner = Callback::new(move |remedy: MacosOwnerRemedy| {
        if starting_offline.get_untracked() {
            return;
        }
        set_starting_offline.set(true);
        set_offline_message.set(None);
        leptos::task::spawn_local(async move {
            match tauri_bridge::execute_macos_daemon_owner_offline_remedy(&remedy).await {
                Ok(Some(outcome)) => {
                    let message =
                        format!("{} started successfully.", humanize_owner(&outcome.owner),);
                    toasts::toast_success(&message);
                    set_offline_message.set(Some(message));
                }
                Ok(None) => set_offline_message.set(Some(
                    "Open this page in Hypercolor.app to start it.".to_owned(),
                )),
                Err(error) => set_offline_message.set(Some(format!("It could not start: {error}"))),
            }
            set_starting_offline.set(false);
            ownership.refetch();
            offline.refetch();
        });
    });

    view! {
        {move || match ownership.get() {
            Some(Ok(Some(status)))
                if status.conflict.is_some() || status.recovery_required.is_some() =>
            {
                view! {
                    <MacosDaemonOwnershipStatusPanel
                        status=status
                        native_available=native_available
                        switching=switching
                        result_message=result_message
                        on_choose=choose_owner
                    />
                }
                .into_any()
            }
            _ => ().into_any(),
        }}
        {move || match offline.get() {
            Some(Ok(Some(status))) => view! {
                <MacosDaemonOwnerOfflinePanel
                    status=status
                    native_available=native_available
                    starting=starting_offline
                    result_message=offline_message
                    on_start=start_offline_owner
                />
            }.into_any(),
            Some(Err(error)) if native_available => view! {
                <NativeStartupFrame>
                    <div class="text-xs text-status-error">
                        {format!("Could not read the engine status: {error}")}
                    </div>
                </NativeStartupFrame>
            }.into_any(),
            _ => ().into_any(),
        }}
    }
}

#[component]
fn MacosDaemonOwnerOfflinePanel(
    status: tauri_bridge::MacosDaemonOwnerOfflineStatus,
    native_available: bool,
    #[prop(into)] starting: Signal<bool>,
    #[prop(into)] result_message: Signal<Option<String>>,
    on_start: Callback<MacosOwnerRemedy>,
) -> impl IntoView {
    let remedy = status.remedy.clone();
    let actionable = matches!(
        remedy,
        MacosOwnerRemedy::StartLaunchdService | MacosOwnerRemedy::StartHomebrewService
    );
    let button_label = owner_remedy_button_label(&remedy);
    let remedy_for_action = StoredValue::new(remedy.clone());

    view! {
        <NativeStartupFrame>
            <div class="flex items-start justify-between gap-3 text-xs">
                <div class="flex items-start gap-2 text-status-warning">
                    <Icon icon=LuTriangleAlert width="14px" height="14px" />
                    <div>
                        <div class="font-medium">"Hypercolor's lighting engine isn't running"</div>
                        <div class="mt-0.5 text-fg-secondary">
                            {format!(
                                "{} is selected. {}",
                                humanize_owner(&status.selected_owner),
                                owner_remedy_label(&remedy),
                            )}
                        </div>
                    </div>
                </div>
                <Show when=move || actionable>
                    <button
                        type="button"
                        class="glow-ring shrink-0 rounded-md border border-accent-muted bg-accent-subtle px-2.5 py-1.5 text-xs text-accent hover:bg-accent-muted/20 disabled:opacity-50"
                        disabled=move || !native_available || starting.get()
                        on:click=move |_| on_start.run(remedy_for_action.get_value())
                    >
                        {move || if starting.get() { "Starting…" } else { button_label }}
                    </button>
                </Show>
            </div>
            {move || result_message.get().map(|message| view! {
                <div class="mt-2 text-xs text-fg-secondary">{message}</div>
            })}
        </NativeStartupFrame>
    }
}

#[component]
fn MacosDaemonOwnershipStatusPanel(
    status: MacosDaemonOwnershipStatus,
    native_available: bool,
    #[prop(into)] switching: Signal<Option<MacosDaemonOwnerChoice>>,
    #[prop(into)] result_message: Signal<Option<String>>,
    on_choose: Callback<MacosDaemonOwnerChoice>,
) -> impl IntoView {
    let conflict = status.conflict.clone();
    let choices = macos_owner_choices(&status);
    let has_choices = !choices.is_empty();
    let recovery_pending = status.recovery_required.is_some();

    view! {
        <NativeStartupFrame>
            <div class="space-y-2.5">
                {conflict.map(|conflict| view! {
                    <div class="rounded-md border border-status-warning/30 bg-status-warning/8 px-3 py-2 text-xs text-status-warning">
                        <div class="font-medium">"Two copies of Hypercolor are trying to run your lights."</div>
                        <div class="mt-0.5 text-fg-secondary">
                            {format!(
                                "{} is running now; {} also tried to start. Pick which one should own your lighting.",
                                humanize_owner(conflict.active.as_str()),
                                humanize_owner(conflict.contender.as_str()),
                            )}
                        </div>
                    </div>
                })}
                <Show when=move || recovery_pending>
                    <div class="rounded-md border border-status-warning/30 bg-status-warning/8 px-3 py-2 text-xs text-status-warning">
                        "A switch between Hypercolor installs was interrupted. Hypercolor is recovering; check back in a moment."
                    </div>
                </Show>
                <Show when=move || has_choices>
                    <div class="flex flex-wrap gap-2">
                        {choices.clone().into_iter().map(|choice| {
                            let label = owner_choice_label(choice);
                            view! {
                                <button
                                    type="button"
                                    class="glow-ring rounded-md border border-accent-muted bg-accent-subtle px-2.5 py-1.5 text-xs text-accent hover:bg-accent-muted/20 disabled:opacity-50"
                                    disabled=move || !native_available || switching.get().is_some()
                                    on:click=move |_| on_choose.run(choice)
                                >
                                    {move || if switching.get() == Some(choice) {
                                        "Switching…"
                                    } else {
                                        label
                                    }}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                    <Show when=move || !native_available>
                        <div class="text-xs text-fg-tertiary">
                            "Open Hypercolor.app to make this choice."
                        </div>
                    </Show>
                </Show>
                {move || result_message.get().map(|message| view! {
                    <div class="text-xs text-fg-secondary">{message}</div>
                })}
            </div>
        </NativeStartupFrame>
    }
}

fn macos_owner_choices(status: &MacosDaemonOwnershipStatus) -> Vec<MacosDaemonOwnerChoice> {
    let mut choices = Vec::new();
    for owner in [
        Some(status.active_owner),
        status.conflict.as_ref().map(|conflict| conflict.contender),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(choice) = macos_owner_choice(owner)
            && !choices.contains(&choice)
        {
            choices.push(choice);
        }
    }
    choices
}

const fn macos_owner_choice(owner: MacosCapabilityOwner) -> Option<MacosDaemonOwnerChoice> {
    match owner {
        MacosCapabilityOwner::AppSidecar => Some(MacosDaemonOwnerChoice::AppSidecar),
        MacosCapabilityOwner::LaunchdService => Some(MacosDaemonOwnerChoice::DirectLaunchd),
        MacosCapabilityOwner::HomebrewService => Some(MacosDaemonOwnerChoice::Homebrew),
        MacosCapabilityOwner::App
        | MacosCapabilityOwner::Broker
        | MacosCapabilityOwner::Standalone => None,
    }
}

const fn owner_choice_label(owner: MacosDaemonOwnerChoice) -> &'static str {
    match owner {
        MacosDaemonOwnerChoice::AppSidecar => "Use Hypercolor.app",
        MacosDaemonOwnerChoice::DirectLaunchd => "Use launchd service",
        MacosDaemonOwnerChoice::Homebrew => "Use Homebrew service",
        MacosDaemonOwnerChoice::Standalone => "Use the terminal daemon",
    }
}

fn macos_owner_outcome_message(outcome: &MacosOwnerCoordinatorOutcome) -> String {
    match outcome {
        MacosOwnerCoordinatorOutcome::Active { owner, .. } => {
            format!("{} now runs your lighting.", humanize_owner(owner))
        }
        MacosOwnerCoordinatorOutcome::PendingStandalone { remedy, .. } => {
            format!("Almost there. {}", owner_remedy_label(remedy))
        }
        MacosOwnerCoordinatorOutcome::RolledBack { prior_owner, .. } => format!(
            "The switch did not complete, so {} kept running your lighting.",
            humanize_owner(prior_owner),
        ),
        MacosOwnerCoordinatorOutcome::RecoveryRequired { .. } => {
            "The switch was interrupted. Hypercolor is recovering; check back in a moment."
                .to_owned()
        }
        MacosOwnerCoordinatorOutcome::Unknown => {
            "This version of Hypercolor.app could not read the result. Refresh to see the current state."
                .to_owned()
        }
    }
}

fn owner_remedy_label(remedy: &MacosOwnerRemedy) -> String {
    match remedy {
        MacosOwnerRemedy::StopStandaloneOwner { pid } => {
            format!("Quit the terminal-launched daemon (process {pid}), then try again.")
        }
        MacosOwnerRemedy::RestartStandalone { pid } => {
            format!("Restart the terminal-launched daemon (process {pid}), then try again.")
        }
        MacosOwnerRemedy::StartAppSidecar => "Start Hypercolor.app.".to_owned(),
        MacosOwnerRemedy::StartLaunchdService => "Start the launchd service.".to_owned(),
        MacosOwnerRemedy::StartHomebrewService => "Start the Homebrew service.".to_owned(),
        MacosOwnerRemedy::Unknown => "Update Hypercolor.app to finish this step.".to_owned(),
    }
}

const fn owner_remedy_button_label(remedy: &MacosOwnerRemedy) -> &'static str {
    match remedy {
        MacosOwnerRemedy::StartLaunchdService => "Start launchd service",
        MacosOwnerRemedy::StartHomebrewService => "Start Homebrew service",
        MacosOwnerRemedy::StopStandaloneOwner { .. }
        | MacosOwnerRemedy::RestartStandalone { .. }
        | MacosOwnerRemedy::StartAppSidecar
        | MacosOwnerRemedy::Unknown => "Unavailable",
    }
}

fn humanize_owner(owner: &str) -> String {
    match owner {
        "app_sidecar" => "Hypercolor.app".to_owned(),
        "launchd_service" | "direct_launchd" => "launchd service".to_owned(),
        "homebrew_service" | "homebrew" => "Homebrew service".to_owned(),
        "standalone" => "a terminal-launched daemon".to_owned(),
        value => {
            let mut value = value.replace('_', " ");
            if let Some(first) = value.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            value
        }
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
                        <div class="flex items-center gap-2 text-xs text-status-error/80">
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
                        <div class="flex items-center gap-2 text-xs text-status-error/80">
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
                        {format!("Hypercolor is running as the {} Windows service", status.service_name)}
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

#[cfg(test)]
mod tests {
    use crate::api::{
        MacosCapabilityOwner, MacosDaemonOwnerConflictStatus, MacosDaemonOwnershipStatus,
    };
    use crate::tauri_bridge::MacosDaemonOwnerChoice;

    use super::{humanize_owner, macos_owner_choices};

    #[test]
    fn owner_choices_follow_only_the_published_conflict() {
        let status = MacosDaemonOwnershipStatus {
            active_owner: MacosCapabilityOwner::AppSidecar,
            owner_epoch: 8,
            conflict: Some(MacosDaemonOwnerConflictStatus {
                active: MacosCapabilityOwner::AppSidecar,
                contender: MacosCapabilityOwner::HomebrewService,
                observed_at_ms: 42,
            }),
            recovery_required: None,
        };

        assert_eq!(
            macos_owner_choices(&status),
            vec![
                MacosDaemonOwnerChoice::AppSidecar,
                MacosDaemonOwnerChoice::Homebrew,
            ]
        );
    }

    #[test]
    fn standalone_owner_is_named_but_never_offered_as_a_managed_target() {
        let status = MacosDaemonOwnershipStatus {
            active_owner: MacosCapabilityOwner::Standalone,
            owner_epoch: 3,
            conflict: Some(MacosDaemonOwnerConflictStatus {
                active: MacosCapabilityOwner::Standalone,
                contender: MacosCapabilityOwner::LaunchdService,
                observed_at_ms: 43,
            }),
            recovery_required: None,
        };

        assert_eq!(humanize_owner("standalone"), "a terminal-launched daemon");
        assert_eq!(
            macos_owner_choices(&status),
            vec![MacosDaemonOwnerChoice::DirectLaunchd]
        );
    }
}
