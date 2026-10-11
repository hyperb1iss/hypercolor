//! Shared release state, refreshed through extension events and socket epochs.

use leptos::prelude::*;

use crate::api::{self, updates::UpdatePreferencesPatch};
use crate::app::{CapabilitiesContext, WsContext};

pub const CAPABILITY: &str = "oss-release-notifications";

#[derive(Clone, Copy)]
pub(crate) struct ReleaseNotificationsContext {
    pub enabled: Memo<bool>,
    pub status: LocalResource<Option<api::ApiResult<api::updates::UpdateStatus>>>,
    pub pending: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub check: Callback<()>,
    pub preferences: Callback<UpdatePreferencesPatch>,
}

pub(crate) fn provide_release_notifications() {
    let capabilities = expect_context::<CapabilitiesContext>();
    let ws = expect_context::<WsContext>();
    let enabled = Memo::new(move |_| capabilities.has(CAPABILITY));
    let status = api::daemon_resource(move || {
        let enabled = enabled.get();
        async move {
            if enabled {
                Some(api::updates::fetch_release_notifications().await)
            } else {
                None
            }
        }
    });
    Effect::new(move |_| {
        let event = ws.last_extension_event.get();
        if enabled.get_untracked()
            && event
                .as_ref()
                .is_some_and(|event| is_release_notification_event(&event.source, &event.kind))
        {
            status.refetch();
        }
    });

    let pending = RwSignal::new(false);
    let error = RwSignal::new(None);
    let check = Callback::new(move |()| {
        if !enabled.get_untracked() || pending.get_untracked() {
            return;
        }
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::updates::check_release_notifications().await {
                Ok(result) => {
                    if result.release.is_none() {
                        crate::toasts::toast_info("No newer stable release is available.");
                    }
                    status.refetch();
                }
                Err(err) => error.set(Some(format!("Could not check GitHub: {err}"))),
            }
            pending.set(false);
        });
    });
    let preferences = Callback::new(move |patch| {
        if !enabled.get_untracked() || pending.get_untracked() {
            return;
        }
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::updates::patch_release_notifications(&patch).await {
                Ok(_) => status.refetch(),
                Err(err) => {
                    let message = format!("Could not save update preferences: {err}");
                    crate::toasts::toast_error(&message);
                    error.set(Some(message));
                }
            }
            pending.set(false);
        });
    });
    provide_context(ReleaseNotificationsContext {
        enabled,
        status,
        pending,
        error,
        check,
        preferences,
    });
}

/// Ignore unrelated extension traffic instead of fetching on every event.
#[must_use]
pub fn is_release_notification_event(source: &str, kind: &str) -> bool {
    source == "oss-updates" && kind == "status"
}

/// A skipped release remains available in Settings without resurfacing a notice.
#[must_use]
pub fn should_notify(status: &api::updates::UpdateStatus) -> bool {
    status
        .release
        .as_ref()
        .is_some_and(|release| status.skipped_version.as_deref() != Some(release.version.as_str()))
}
