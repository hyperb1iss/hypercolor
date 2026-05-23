//! First-run welcome overlay.
//!
//! Renders a single centered card on first launch with a short
//! orientation message, an inline "Start at sign in" preference, and a
//! "Let's go" CTA. Dismissing applies the autostart choice (when native
//! bridge is available) and persists a marker so the overlay doesn't
//! reappear.
//!
//! Future passes can layer additional steps onto the same skeleton —
//! e.g. motherboard-aware hardware-support offer and post-wizard device
//! discovery kickoff.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use crate::icons::*;
use crate::tauri_bridge::{self, PawnIoSupportStatus, smbus_support_ready};

#[component]
pub fn WelcomeOverlay() -> impl IntoView {
    let pending = LocalResource::new(tauri_bridge::is_first_run_pending);
    let pawnio = LocalResource::new(tauri_bridge::detect_pawnio_support);
    let (dismissed, set_dismissed) = signal(false);
    let (dismissing, set_dismissing) = signal(false);
    // Default to enabled — most users who installed an RGB orchestration
    // app want it running with the session. Users who don't can flip the
    // toggle in two clicks or change it later in Settings → Session.
    let (autostart_enabled, set_autostart_enabled) = signal(true);
    let navigate = use_navigate();

    let show = Signal::derive(move || {
        if dismissed.get() {
            return false;
        }
        matches!(pending.get(), Some(Ok(Some(true))))
    });

    // Hardware-support offer triggers only when the user is on Windows,
    // their motherboard is from a vendor we know how to drive, and the
    // SMBus broker isn't already running. Avoids steering Mac/Linux
    // users (or Windows users on non-RGB hardware) toward a flow that
    // won't help them.
    let should_offer_hardware = Signal::derive(move || {
        match pawnio.get() {
            Some(Ok(Some(status))) => hardware_offer_visible(&status),
            _ => false,
        }
    });

    // Callbacks (rather than raw closures) so the outer view closure
    // can be Fn — Callback is internally Arc-clonable, so moving it
    // into `on:click` doesn't consume the surrounding closure.
    let dismiss = Callback::new(move |()| {
        spawn_dismiss_task(
            dismissing,
            set_dismissing,
            set_dismissed,
            autostart_enabled.get_untracked(),
            None,
        );
    });
    let navigate_for_settings = navigate;
    let dismiss_to_settings = Callback::new(move |()| {
        let navigate_clone = navigate_for_settings.clone();
        spawn_dismiss_task(
            dismissing,
            set_dismissing,
            set_dismissed,
            autostart_enabled.get_untracked(),
            Some(Box::new(move || {
                navigate_clone("/settings", NavigateOptions::default());
            })),
        );
    });

    view! {
        <Show when=move || show.get()>
            <div class="fixed inset-0 z-[200] flex items-center justify-center bg-black/75 backdrop-blur-sm px-4">
                <div
                    class="w-full max-w-md rounded-xl border border-edge-subtle bg-surface-overlay p-6 modal-glow animate-enter-fade"
                    style="box-shadow: 0 0 60px rgba(225, 53, 255, 0.18), 0 0 24px rgba(128, 255, 234, 0.08)"
                >
                    <div class="flex items-center gap-2.5">
                        <Icon
                            icon=LuZap
                            width="18px"
                            height="18px"
                            style="color: rgba(225, 53, 255, 0.9)"
                        />
                        <span class="text-base font-semibold text-fg-primary">
                            "Welcome to Hypercolor"
                        </span>
                    </div>
                    <div class="mt-2 text-sm text-fg-tertiary/80 leading-relaxed">
                        "Orchestrate every RGB device in your setup from one place. \
                         Network lights, motherboard zones, and ambient capture \
                         live in a single canvas."
                    </div>

                    <div class="mt-5 space-y-2.5">
                        <WelcomePoint
                            label="Devices"
                            description="Add network lights from the Devices page. mDNS picks most up automatically."
                        />
                        <WelcomePoint
                            label="Effects"
                            description="Pick a built-in look from Effects or wire one to an audio or capture source."
                        />
                        <WelcomePoint
                            label="Hardware support"
                            description="On Windows, Settings → Device Discovery can install motherboard SMBus access."
                        />
                    </div>

                    <AutostartRow
                        enabled=Signal::derive(move || autostart_enabled.get())
                        on_toggle=Callback::new(move |()| {
                            set_autostart_enabled.update(|on| *on = !*on);
                        })
                    />

                    <Show when=move || should_offer_hardware.get()>
                        <button
                            type="button"
                            class="mt-5 w-full rounded-lg px-3 py-2.5 text-sm font-medium transition-all disabled:cursor-not-allowed flex items-center justify-center gap-2"
                            style=move || if dismissing.get() {
                                "color: rgba(139, 133, 160, 0.55); background: rgba(139, 133, 160, 0.08); border: 1px solid rgba(139, 133, 160, 0.12)"
                            } else {
                                "color: rgba(128, 255, 234, 0.95); background: rgba(128, 255, 234, 0.06); border: 1px solid rgba(128, 255, 234, 0.25)"
                            }
                            disabled=move || dismissing.get()
                            on:click=move |_| dismiss_to_settings.run(())
                        >
                            <Icon icon=LuShieldCheck width="13px" height="13px" />
                            "Set up RGB hardware support"
                        </button>
                    </Show>

                    <button
                        type="button"
                        class="mt-3 w-full rounded-lg px-3 py-2.5 text-sm font-medium transition-all disabled:cursor-not-allowed"
                        style=move || if dismissing.get() {
                            "color: rgba(139, 133, 160, 0.55); background: rgba(139, 133, 160, 0.08); border: 1px solid rgba(139, 133, 160, 0.12)"
                        } else {
                            "color: rgb(10, 12, 18); background: rgb(225, 53, 255); border: 1px solid rgba(225, 53, 255, 0.5); box-shadow: 0 0 12px rgba(225, 53, 255, 0.25)"
                        }
                        disabled=move || dismissing.get()
                        on:click=move |_| dismiss.run(())
                    >
                        {move || if dismissing.get() { "Saving..." } else { "Let's go" }}
                    </button>
                </div>
            </div>
        </Show>
    }
}

fn spawn_dismiss_task(
    dismissing: ReadSignal<bool>,
    set_dismissing: WriteSignal<bool>,
    set_dismissed: WriteSignal<bool>,
    want_autostart: bool,
    after: Option<Box<dyn FnOnce() + 'static>>,
) {
    if dismissing.get_untracked() {
        return;
    }
    set_dismissing.set(true);
    leptos::task::spawn_local(async move {
        // Apply the autostart preference first so the user's choice is
        // honored even if marking the wizard complete somehow fails.
        // Best-effort: we log but don't fail the dismissal on a plugin
        // hiccup — the user can still toggle it from Settings.
        if let Err(error) = tauri_bridge::set_autostart_enabled(want_autostart).await {
            leptos::logging::warn!("welcome autostart write failed: {error}");
        }

        let result = tauri_bridge::mark_first_run_complete().await;
        set_dismissing.set(false);
        if let Err(error) = result {
            leptos::logging::warn!("mark_first_run_complete failed: {error}");
        }
        // Hide regardless. A transient bridge failure shouldn't trap
        // the user behind the overlay; next launch reappears, which is
        // acceptable.
        set_dismissed.set(true);
        if let Some(after) = after {
            after();
        }
    });
}

/// Whether the wizard should surface the "set up RGB hardware" offer
/// for the given status. Public for unit testing.
#[must_use]
pub fn hardware_offer_visible(status: &PawnIoSupportStatus) -> bool {
    if !status.platform_supported || smbus_support_ready(status) {
        return false;
    }
    status
        .motherboard
        .as_ref()
        .is_some_and(hypercolor_types::motherboard::MotherboardInfo::is_likely_rgb_capable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tauri_bridge::{PawnIoModuleStatus, ServiceSupportStatus};

    fn windows_status() -> PawnIoSupportStatus {
        PawnIoSupportStatus {
            platform_supported: true,
            pawnio_home: None,
            pawnio_runtime_installed: false,
            pawnio_service: ServiceSupportStatus {
                installed: false,
                state: None,
            },
            smbus_service: ServiceSupportStatus {
                installed: false,
                state: None,
            },
            bundled_asset_root: None,
            helper_script: None,
            broker_executable: None,
            bundled_installer_available: true,
            bundled_modules: vec![PawnIoModuleStatus {
                name: "SmbusI801.bin".to_string(),
                bundled: true,
            }],
            install_available: true,
            motherboard: Some(hypercolor_types::motherboard::MotherboardInfo {
                manufacturer: "ASUSTeK COMPUTER INC.".to_string(),
                product: "ROG STRIX X670E-E".to_string(),
                version: None,
            }),
            conflicting_rgb_tools: Vec::new(),
        }
    }

    #[test]
    fn offer_visible_on_windows_with_rgb_board_and_no_smbus() {
        assert!(hardware_offer_visible(&windows_status()));
    }

    #[test]
    fn offer_hidden_when_smbus_already_running() {
        let mut status = windows_status();
        status.pawnio_runtime_installed = true;
        status.smbus_service.installed = true;
        status.smbus_service.state = Some("RUNNING".to_string());
        assert!(!hardware_offer_visible(&status));
    }

    #[test]
    fn offer_hidden_when_motherboard_is_not_rgb_capable() {
        let mut status = windows_status();
        status.motherboard = Some(hypercolor_types::motherboard::MotherboardInfo {
            manufacturer: "Dell Inc.".to_string(),
            product: "OptiPlex 7090".to_string(),
            version: None,
        });
        assert!(!hardware_offer_visible(&status));
    }

    #[test]
    fn offer_hidden_when_motherboard_unknown() {
        let mut status = windows_status();
        status.motherboard = None;
        assert!(!hardware_offer_visible(&status));
    }

    #[test]
    fn offer_hidden_on_unsupported_platform() {
        let mut status = windows_status();
        status.platform_supported = false;
        assert!(!hardware_offer_visible(&status));
    }
}

#[component]
fn AutostartRow(
    #[prop(into)] enabled: Signal<bool>,
    on_toggle: Callback<()>,
) -> impl IntoView {
    view! {
        <div
            class="mt-5 flex items-center justify-between gap-3 rounded-lg px-3 py-2.5"
            style="background: rgba(139, 133, 160, 0.05); border: 1px solid rgba(139, 133, 160, 0.08)"
        >
            <div class="min-w-0">
                <div class="text-sm text-fg-primary font-medium">"Start at sign in"</div>
                <div class="text-xs text-fg-tertiary/70 mt-0.5">
                    "Launch Hypercolor in the tray when you log in. Toggle later in Settings."
                </div>
            </div>
            <button
                role="switch"
                aria-checked=move || enabled.get().to_string()
                class="relative w-11 h-6 rounded-full transition-all duration-200 shrink-0 cursor-pointer"
                style=move || if enabled.get() {
                    "background: rgba(225, 53, 255, 0.5); box-shadow: 0 0 10px rgba(225, 53, 255, 0.25)"
                } else {
                    "background: rgba(139, 133, 160, 0.2)"
                }
                on:click=move |_| on_toggle.run(())
            >
                <span
                    class="absolute left-0.5 top-0.5 w-5 h-5 rounded-full shadow-sm transition-transform duration-200"
                    style=move || if enabled.get() {
                        "transform: translateX(22px); background: rgb(225, 53, 255)"
                    } else {
                        "transform: translateX(0); background: rgba(200, 200, 210, 0.6)"
                    }
                />
            </button>
        </div>
    }
}

#[component]
fn WelcomePoint(label: &'static str, description: &'static str) -> impl IntoView {
    view! {
        <div class="flex items-start gap-2.5">
            <span
                class="mt-1.5 inline-block w-1.5 h-1.5 rounded-full shrink-0"
                style="background: rgba(128, 255, 234, 0.75); box-shadow: 0 0 6px rgba(128, 255, 234, 0.4)"
            />
            <div class="min-w-0">
                <div class="text-sm text-fg-primary font-medium">{label}</div>
                <div class="text-xs text-fg-tertiary/70 leading-snug">{description}</div>
            </div>
        </div>
    }
}
