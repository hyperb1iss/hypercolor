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

use crate::icons::*;
use crate::tauri_bridge;

#[component]
pub fn WelcomeOverlay() -> impl IntoView {
    let pending = LocalResource::new(tauri_bridge::is_first_run_pending);
    let (dismissed, set_dismissed) = signal(false);
    let (dismissing, set_dismissing) = signal(false);
    // Default to enabled — most users who installed an RGB orchestration
    // app want it running with the session. Users who don't can flip the
    // toggle in two clicks or change it later in Settings → Session.
    let (autostart_enabled, set_autostart_enabled) = signal(true);

    let show = Signal::derive(move || {
        if dismissed.get() {
            return false;
        }
        matches!(pending.get(), Some(Ok(Some(true))))
    });

    let dismiss = move |_| {
        if dismissing.get_untracked() {
            return;
        }
        set_dismissing.set(true);
        let want_autostart = autostart_enabled.get_untracked();
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
            // the user behind the overlay; next launch reappears, which
            // is acceptable.
            set_dismissed.set(true);
        });
    };

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

                    <button
                        type="button"
                        class="mt-5 w-full rounded-lg px-3 py-2.5 text-sm font-medium transition-all disabled:cursor-not-allowed"
                        style=move || if dismissing.get() {
                            "color: rgba(139, 133, 160, 0.55); background: rgba(139, 133, 160, 0.08); border: 1px solid rgba(139, 133, 160, 0.12)"
                        } else {
                            "color: rgb(10, 12, 18); background: rgb(225, 53, 255); border: 1px solid rgba(225, 53, 255, 0.5); box-shadow: 0 0 12px rgba(225, 53, 255, 0.25)"
                        }
                        disabled=move || dismissing.get()
                        on:click=dismiss
                    >
                        {move || if dismissing.get() { "Saving..." } else { "Let's go" }}
                    </button>
                </div>
            </div>
        </Show>
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
