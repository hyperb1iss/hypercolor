//! Why the native macOS app is not running Hypercolor yet.
//!
//! While an update is open, or when macOS runs a copy from a temporary
//! location, the app leaves its daemon alone. The window says so here, for as
//! long as that lasts; the page behind it has nothing to connect to.

use leptos::prelude::*;

use crate::components::modal::Modal;
use crate::tauri_bridge::{self, MacosAppStartupHold};

const fn hold_copy(hold: MacosAppStartupHold) -> (&'static str, &'static str) {
    match hold {
        MacosAppStartupHold::UpdateInProgress => (
            "Updating Hypercolor",
            "This takes a minute or two. Your lights come back on their own when it's done.",
        ),
        MacosAppStartupHold::MoveToApplications => (
            "Move Hypercolor to Applications",
            "macOS is running this copy from a temporary location, so Hypercolor can't start \
             here. Quit it, drag Hypercolor into your Applications folder, and open it from \
             there.",
        ),
    }
}

#[component]
pub fn StartupHoldNotice() -> impl IntoView {
    let (hold, set_hold) = signal(None::<MacosAppStartupHold>);
    leptos::task::spawn_local(tauri_bridge::follow_macos_app_startup_hold(move |next| {
        set_hold.set(next);
    }));

    view! {
        {move || {
            hold.get()
                .map(|hold| {
                    let (title, body) = hold_copy(hold);
                    view! {
                        <Modal
                            on_close=Callback::new(|()| {})
                            label=title
                            dismissible=Signal::derive(|| false)
                            close_on_backdrop=false
                            container_class="fixed inset-0 z-[300] flex items-center justify-center px-4"
                            backdrop_class="absolute inset-0 bg-black/75 backdrop-blur-sm"
                        >
                            <div class="relative w-full max-w-md rounded-xl border border-edge-subtle bg-surface-overlay p-6 modal-glow animate-enter-fade">
                                <div class="text-base font-semibold text-fg-primary">{title}</div>
                                <div class="mt-2 text-sm text-fg-tertiary/80 leading-relaxed">
                                    {body}
                                </div>
                            </div>
                        </Modal>
                    }
                })
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::{MacosAppStartupHold, hold_copy};

    #[test]
    fn hold_copy_speaks_to_players() {
        for hold in [
            MacosAppStartupHold::UpdateInProgress,
            MacosAppStartupHold::MoveToApplications,
        ] {
            let (title, body) = hold_copy(hold);
            let text = format!("{title} {body}").to_lowercase();
            for plumbing in ["daemon", "launchd", "translocat", "bundle", "machine"] {
                assert!(!text.contains(plumbing), "{plumbing} in {text}");
            }
        }
    }
}
