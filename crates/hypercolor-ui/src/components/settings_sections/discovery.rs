use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::config::HypercolorConfig;

use crate::api;
use crate::components::settings_controls::*;
use crate::driver_settings::{DiscoveryDriverSetting, discovery_driver_settings};
use crate::icons::*;
use crate::tauri_bridge::{
    self, PawnIoHelperOptions, PawnIoSupportStatus, bundled_payload_ready, smbus_support_ready,
};
use crate::toasts;

use super::{driver_enabled, read_config};

// ── Device Discovery ───────────────────────────────────────────────────────

#[component]
pub fn DiscoverySection(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    #[prop(into)] driver_modules: Signal<Vec<api::DriverSummary>>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<String>,
) -> impl IntoView {
    let mdns = Signal::derive(move || read_config(config, |cfg| cfg.discovery.mdns_enabled));
    let scan_interval =
        Signal::derive(move || read_config(config, |cfg| cfg.discovery.scan_interval_secs as f64));
    let discovery_drivers =
        Signal::derive(move || discovery_driver_settings(&driver_modules.get()));
    view! {
        <section id="section-discovery" class="pt-5 pb-3 space-y-0">
            <SectionHeader title="Device Discovery" icon=LuRadar />
            <HardwareSupportPanel />
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
            <div class="pt-2 space-y-2">
                <For
                    each=move || discovery_drivers.get()
                    key=|setting| setting.id.clone()
                    children=move |setting| {
                        let driver_id = setting.id.clone();
                        let enabled = Signal::derive(move || {
                            read_config(config, |cfg| driver_enabled(cfg, &driver_id))
                        });
                        view! {
                            <DiscoveryDriverRow
                                setting=setting
                                value=enabled
                                on_change=on_change
                            />
                        }
                    }
                />
            </div>
            <SectionReset section_label="Discovery" on_reset=Callback::new(move |()| on_reset.run("discovery".to_string())) />
        </section>
    }
}

#[component]
fn HardwareSupportPanel() -> impl IntoView {
    let native_available = tauri_bridge::is_tauri_available();
    let status = LocalResource::new(tauri_bridge::detect_pawnio_support);
    let (installing, set_installing) = signal(false);
    let (reboot_required, set_reboot_required) = signal(false);
    let install = move |_| {
        if installing.get_untracked() {
            return;
        }

        set_installing.set(true);
        leptos::task::spawn_local(async move {
            let result = tauri_bridge::launch_pawnio_helper(PawnIoHelperOptions::default()).await;
            set_installing.set(false);

            match result {
                Ok(result) => {
                    if result.exit_code == Some(3010) {
                        // Windows Installer reboot-required convention —
                        // PawnIO's signed kernel driver finishes binding
                        // its SCM service entry on the next boot. Surface
                        // a persistent banner instead of pretending the
                        // install is fully live.
                        set_reboot_required.set(true);
                        toasts::toast_success(
                            "Hardware support installed. Restart Windows to finish driver activation.",
                        );
                    } else {
                        set_reboot_required.set(false);
                        toasts::toast_success("Windows hardware support installed");
                    }
                    status.refetch();
                    // Kick a device-discovery rescan so SMBus motherboard /
                    // DRAM devices that were unreachable before PawnIO is
                    // installed surface without requiring a daemon restart.
                    if let Err(error) = api::devices::discover_devices().await {
                        leptos::logging::warn!(
                            "post-install device rescan request failed: {error}"
                        );
                    }
                }
                Err(error) => {
                    toasts::toast_error(&format!("Hardware support install failed: {error}"));
                    status.refetch();
                }
            }
        });
    };

    view! {
        <Show when=move || native_available>
            <Show when=move || reboot_required.get()>
                <div
                    class="mb-3 flex items-center gap-2 rounded-lg px-3 py-2 text-xs"
                    style="color: rgba(241, 250, 140, 0.95); background: rgba(241, 250, 140, 0.08); border: 1px solid rgba(241, 250, 140, 0.18)"
                >
                    <Icon icon=LuTriangleAlert width="13px" height="13px" />
                    <span>
                        "Restart Windows to finish PawnIO driver activation. Lights will be unresponsive until you reboot."
                    </span>
                </div>
            </Show>
            {move || match status.get() {
                None => view! {
                    <HardwareSupportFrame>
                        <div class="flex items-center gap-2 text-xs text-fg-tertiary/60">
                            <Icon icon=LuLoader width="13px" height="13px" />
                            "Checking Windows hardware support"
                        </div>
                    </HardwareSupportFrame>
                }.into_any(),
                Some(Ok(None)) => ().into_any(),
                Some(Err(error)) => view! {
                    <HardwareSupportFrame>
                        <div class="flex items-center gap-2 text-xs text-error-red/80">
                            <Icon icon=LuTriangleAlert width="13px" height="13px" />
                            {format!("Hardware support status unavailable: {error}")}
                        </div>
                    </HardwareSupportFrame>
                }.into_any(),
                Some(Ok(Some(current))) if !current.platform_supported => ().into_any(),
                Some(Ok(Some(current)))
                    if current
                        .motherboard
                        .as_ref()
                        .is_some_and(|board| !board.is_likely_rgb_capable()) =>
                {
                    // Detected motherboard from a vendor without known
                    // Hypercolor RGB support — don't surface the install panel
                    // at all. Network devices (Hue/WLED/etc.) still work
                    // without PawnIO.
                    ().into_any()
                }
                Some(Ok(Some(current))) => view! {
                    <HardwareSupportStatusPanel
                        status=current
                        installing=installing
                        on_install=Callback::new(install)
                    />
                }.into_any(),
            }}
        </Show>
    }
}

#[component]
fn HardwareSupportStatusPanel(
    status: PawnIoSupportStatus,
    #[prop(into)] installing: Signal<bool>,
    on_install: Callback<()>,
) -> impl IntoView {
    let ready = smbus_support_ready(&status);
    let payload_ready = bundled_payload_ready(&status);
    let can_install = status.install_available && !ready;
    let service_running = status.smbus_service.state.as_deref() == Some("RUNNING");
    let install_label = if ready {
        "Ready"
    } else if installing.get_untracked() {
        "Installing"
    } else {
        "Install support"
    };

    let board_summary = status
        .motherboard
        .as_ref()
        .filter(|board| board.is_likely_rgb_capable())
        .map(|board| format!("Detected: {} {}", board.manufacturer, board.product));

    let running_conflicts: Vec<String> = status
        .conflicting_rgb_tools
        .iter()
        .filter(|tool| tool.running)
        .map(|tool| tool.name.clone())
        .collect();
    let conflict_warning = (!running_conflicts.is_empty()).then(|| {
        format!(
            "Other RGB software is running: {}. Quit it first to avoid SMBus conflicts.",
            running_conflicts.join(", ")
        )
    });

    view! {
        <HardwareSupportFrame>
            <div class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
                <div class="min-w-0">
                    <div class="flex items-center gap-2">
                        <Icon icon=LuShieldCheck width="15px" height="15px" style="color: rgba(128, 255, 234, 0.72)" />
                        <span class="text-sm text-fg-primary font-medium">"Windows SMBus Support"</span>
                    </div>
                    <div class="text-xs text-fg-tertiary/70 mt-1">
                        "PawnIO runtime and HypercolorSmBus broker for motherboard and memory RGB"
                    </div>
                    {board_summary.map(|summary| view! {
                        <div class="text-[11px] text-accent-cyan/80 mt-1 font-mono truncate">
                            {summary}
                        </div>
                    })}
                    {conflict_warning.map(|warning| view! {
                        <div
                            class="flex items-center gap-1.5 text-[11px] mt-1.5 px-2 py-1 rounded"
                            style="color: rgba(241, 250, 140, 0.95); background: rgba(241, 250, 140, 0.08); border: 1px solid rgba(241, 250, 140, 0.18)"
                        >
                            <Icon icon=LuTriangleAlert width="11px" height="11px" />
                            <span>{warning}</span>
                        </div>
                    })}
                </div>
                <button
                    class="px-3 py-1.5 rounded-lg text-xs font-medium transition-all shrink-0 disabled:cursor-not-allowed"
                    style=move || if ready {
                        "color: rgba(80, 250, 123, 0.75); background: rgba(80, 250, 123, 0.08); border: 1px solid rgba(80, 250, 123, 0.12)"
                    } else if can_install && !installing.get() {
                        "color: rgb(10, 12, 18); background: rgb(128, 255, 234); border: 1px solid rgba(128, 255, 234, 0.5); box-shadow: 0 0 12px rgba(128, 255, 234, 0.2)"
                    } else {
                        "color: rgba(139, 133, 160, 0.55); background: rgba(139, 133, 160, 0.08); border: 1px solid rgba(139, 133, 160, 0.12)"
                    }
                    disabled=move || ready || !can_install || installing.get()
                    on:click=move |_| on_install.run(())
                >
                    {move || if installing.get() { "Installing" } else { install_label }}
                </button>
            </div>
            <div class="flex flex-wrap gap-2 pt-3">
                <SupportPill label="Runtime" ready=status.pawnio_runtime_installed />
                <SupportPill label="Broker" ready=status.smbus_service.installed />
                <SupportPill label="Running" ready=service_running />
                <SupportPill label="Payload" ready=payload_ready />
            </div>
        </HardwareSupportFrame>
    }
}

#[component]
fn HardwareSupportFrame(children: Children) -> impl IntoView {
    view! {
        <div
            class="mb-4 rounded-lg px-3 py-3"
            style="background: rgba(128, 255, 234, 0.035); border: 1px solid rgba(128, 255, 234, 0.08)"
        >
            {children()}
        </div>
    }
}

#[component]
fn SupportPill(label: &'static str, ready: bool) -> impl IntoView {
    view! {
        <span
            class="inline-flex items-center gap-1.5 rounded px-2 py-1 text-[10px] font-mono uppercase tracking-wide"
            style=if ready {
                "color: rgba(80, 250, 123, 0.78); background: rgba(80, 250, 123, 0.08); border: 1px solid rgba(80, 250, 123, 0.12)"
            } else {
                "color: rgba(241, 250, 140, 0.72); background: rgba(241, 250, 140, 0.07); border: 1px solid rgba(241, 250, 140, 0.12)"
            }
        >
            <Icon icon=if ready { LuCheck } else { LuTriangleAlert } width="11px" height="11px" />
            {label}
        </span>
    }
}

#[component]
fn DiscoveryDriverRow(
    setting: DiscoveryDriverSetting,
    #[prop(into)] value: Signal<bool>,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let click_key = setting.key.clone();
    let keydown_key = setting.key.clone();
    let click_change = on_change;
    let keydown_change = on_change;
    view! {
        <div class="setting-row py-3">
            <div class="flex items-center justify-between gap-4">
                <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-1.5">
                        <span class="text-sm text-fg-primary font-medium mr-1">{setting.label}</span>
                        {setting.transport_labels.into_iter().map(|label| view! {
                            <span class="text-[10px] font-mono uppercase tracking-wide px-1.5 py-0.5 rounded text-fg-tertiary/75 bg-surface-overlay/60 border border-edge-subtle/50">
                                {label}
                            </span>
                        }).collect_view()}
                        {setting.supports_pairing.then(|| view! {
                            <span class="text-[10px] font-mono uppercase tracking-wide px-1.5 py-0.5 rounded"
                                style="color: rgba(128, 255, 234, 0.78); background: rgba(128, 255, 234, 0.07); border: 1px solid rgba(128, 255, 234, 0.12)">
                                "Pairing"
                            </span>
                        })}
                    </div>
                </div>
                <button
                    role="switch"
                    aria-checked=move || value.get().to_string()
                    class="relative w-11 h-6 rounded-full transition-all duration-200 shrink-0 cursor-pointer"
                    style=move || if value.get() {
                        "background: rgba(225, 53, 255, 0.5); box-shadow: 0 0 10px rgba(225, 53, 255, 0.25)"
                    } else {
                        "background: rgba(139, 133, 160, 0.2)"
                    }
                    on:click=move |_| {
                        click_change.run((click_key.clone(), serde_json::json!(!value.get_untracked())));
                    }
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        if matches!(ev.key().as_str(), " " | "Enter") {
                            ev.prevent_default();
                            keydown_change.run((
                                keydown_key.clone(),
                                serde_json::json!(!value.get_untracked()),
                            ));
                        }
                    }
                >
                    <span
                        class="absolute left-0.5 top-0.5 w-5 h-5 rounded-full shadow-sm transition-transform duration-200"
                        style=move || if value.get() {
                            "transform: translateX(22px); background: rgb(225, 53, 255)"
                        } else {
                            "transform: translateX(0); background: rgba(200, 200, 210, 0.6)"
                        }
                    />
                </button>
            </div>
        </div>
    }
}
