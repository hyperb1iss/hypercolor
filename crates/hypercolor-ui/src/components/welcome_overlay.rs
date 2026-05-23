//! First-run welcome overlay.
//!
//! Single-screen MVP: dashboard renders behind a centered card with a
//! short orientation message and a "Let's go" CTA. Dismissing persists
//! a marker via the Tauri bridge so the overlay doesn't reappear.
//!
//! Content steps (hardware support offer, autostart prompt, device
//! discovery kickoff) will land in a follow-up commit on top of this
//! scaffolding.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::icons::*;
use crate::tauri_bridge;

#[component]
pub fn WelcomeOverlay() -> impl IntoView {
    let pending = LocalResource::new(tauri_bridge::is_first_run_pending);
    let (dismissed, set_dismissed) = signal(false);
    let (dismissing, set_dismissing) = signal(false);

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
        leptos::task::spawn_local(async move {
            let result = tauri_bridge::mark_first_run_complete().await;
            set_dismissing.set(false);
            match result {
                Ok(()) => {
                    set_dismissed.set(true);
                }
                Err(error) => {
                    leptos::logging::warn!("mark_first_run_complete failed: {error}");
                    // Hide the overlay anyway so a transient bridge
                    // failure doesn't trap the user behind it. Next
                    // launch will re-show; that's acceptable.
                    set_dismissed.set(true);
                }
            }
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

                    <button
                        type="button"
                        class="mt-6 w-full rounded-lg px-3 py-2.5 text-sm font-medium transition-all disabled:cursor-not-allowed"
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
