//! Competing RGB software: the shared conflict state, the Devices page
//! warning banner, and the one-line hint on every device a program competes
//! for.
//!
//! Freshness rides signals, never timers. The daemon rescans on its own and
//! publishes `software_conflicts_changed` when the set changes, so the status
//! resource refetches on that hint and on every socket reconnect
//! (`connection_generation`). "Check again" is the only scan the UI asks for,
//! and only when the user clicks it.
//!
//! The state lives at app level so a dismissal expires whenever a scan shows
//! its program stopped, not only while the Devices page is open. The rules
//! (which devices a program competes for, which warnings are dismissed, when
//! a dismissal expires) live in [`crate::software_conflicts`].

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, ApiResult, SoftwareConflict, SoftwareConflictsStatus};
use crate::app::WsContext;
use crate::components::status_banner::StatusBannerTone;
use crate::icons::{LuRefreshCw, LuTriangleAlert, LuX};
use crate::software_conflicts::{
    DISMISSED_STORAGE_KEY, SOFTWARE_CONFLICTS_EVENT, banner_headline, banner_scope, check_feedback,
    encode_dismissed, parse_dismissed, prune_dismissed, visible_conflicts,
};
use crate::{storage, toasts};

/// The daemon's latest conflict scan, refetched when the daemon reports a
/// change and when the socket reconnects.
fn conflicts_resource() -> LocalResource<ApiResult<SoftwareConflictsStatus>> {
    let hint = expect_context::<WsContext>().last_device_event;
    let status = api::daemon_resource(api::fetch_software_conflicts);
    // The hint keeps its last value, so the first run would only repeat the
    // initial fetch.
    Effect::new(move |initialized: Option<()>| {
        let changed = hint
            .get()
            .is_some_and(|hint| hint.event_type == SOFTWARE_CONFLICTS_EVENT);
        if initialized.is_some() && changed {
            status.refetch();
        }
    });
    status
}

/// Conflict status plus the user's dismissals, provided once for the whole
/// app and read by the banner, the device hints, and the SMBus support card.
#[derive(Clone, Copy)]
pub struct SoftwareConflictsState {
    status: LocalResource<ApiResult<SoftwareConflictsStatus>>,
    dismissed: RwSignal<BTreeSet<String>>,
    scanning: RwSignal<bool>,
    running: Memo<Vec<SoftwareConflict>>,
    visible: Memo<Vec<SoftwareConflict>>,
}

/// Build the conflict state. Dismissals load from `localStorage`; when
/// storage is unavailable they live in memory for the session instead.
pub fn software_conflicts_state() -> SoftwareConflictsState {
    let status = conflicts_resource();
    let dismissed = RwSignal::new(parse_dismissed(
        storage::get(DISMISSED_STORAGE_KEY).as_deref(),
    ));

    Effect::new(move |_| {
        let Some(Ok(current)) = status.get() else {
            return;
        };
        let kept = prune_dismissed(&dismissed.get_untracked(), &current);
        if kept != dismissed.get_untracked() {
            store_dismissed(&kept);
            dismissed.set(kept);
        }
    });

    let running = Memo::new(move |_| match status.get() {
        Some(Ok(current)) => current.conflicts,
        _ => Vec::new(),
    });
    let visible =
        Memo::new(move |_| running.with(|all| dismissed.with(|ids| visible_conflicts(all, ids))));

    SoftwareConflictsState {
        status,
        dismissed,
        scanning: RwSignal::new(false),
        running,
        visible,
    }
}

impl SoftwareConflictsState {
    /// Every running conflict the latest scan reported, dismissed or not.
    #[must_use]
    pub fn running(self) -> Memo<Vec<SoftwareConflict>> {
        self.running
    }

    /// Running conflicts the user has not dismissed.
    #[must_use]
    pub fn visible(self) -> Memo<Vec<SoftwareConflict>> {
        self.visible
    }

    /// Hide the warning for `id` until a scan shows that program stopped.
    pub fn dismiss(self, id: &str) {
        self.dismissed.update(|ids| {
            ids.insert(id.to_owned());
        });
        self.dismissed.with_untracked(store_dismissed);
    }

    /// Ask the daemon to scan now and show its answer. Every signal access
    /// after the await is fallible, so a disposed owner ends the task quietly.
    pub fn check_again(self) {
        if self.scanning.get_untracked() {
            return;
        }
        self.scanning.set(true);
        leptos::task::spawn_local(async move {
            match api::scan_software_conflicts().await {
                Ok(current) => {
                    let Some(feedback) = self
                        .dismissed
                        .try_with_untracked(|ids| check_feedback(&current, ids))
                    else {
                        return;
                    };
                    if let Some(message) = feedback {
                        toasts::toast_info(&message);
                    }
                    let _ = self.status.try_set(Some(Ok(current)));
                }
                Err(error) => toasts::toast_error(&format!("Check failed: {error}")),
            }
            let _ = self.scanning.try_set(false);
        });
    }
}

/// Persist dismissals. A failed write keeps them in memory for this page.
fn store_dismissed(ids: &BTreeSet<String>) {
    match encode_dismissed(ids) {
        Some(encoded) => {
            storage::set(DISMISSED_STORAGE_KEY, &encoded);
        }
        None => {
            storage::remove(DISMISSED_STORAGE_KEY);
        }
    }
}

/// Warning banner listing every running, non-dismissed competing program.
/// Renders nothing when there are none.
#[component]
pub fn SoftwareConflictBanner(state: SoftwareConflictsState) -> impl IntoView {
    let tone = StatusBannerTone::Warning;
    view! {
        {move || {
            let conflicts = state.visible().get();
            (!conflicts.is_empty()).then(|| view! {
                <section
                    class=format!("{} mb-4", tone.container_class())
                    aria-label="Competing RGB software"
                >
                    <div class="flex items-start gap-3">
                        <div class=tone.icon_class()>
                            <Icon icon=LuTriangleAlert width="14px" height="14px" />
                        </div>
                        <div class="min-w-0 flex-1">
                            <div class="flex flex-wrap items-center justify-between gap-2">
                                <div class=tone.title_class()>"Competing RGB software"</div>
                                <CheckAgainButton state=state />
                            </div>
                            <ul class="mt-2 space-y-3">
                                {conflicts.into_iter().map(|conflict| view! {
                                    <ConflictRow conflict=conflict state=state />
                                }).collect_view()}
                            </ul>
                        </div>
                    </div>
                </section>
            })
        }}
    }
}

#[component]
fn ConflictRow(conflict: SoftwareConflict, state: SoftwareConflictsState) -> impl IntoView {
    let headline = banner_headline(&conflict);
    let scope = banner_scope(&conflict);
    let detected = (!conflict.matched.is_empty()).then(|| conflict.matched.join(", "));
    let dismiss_label = format!("Dismiss the {} warning", conflict.name);
    let dismiss_title = format!("Hide until {} stops and starts again", conflict.name);
    let id = conflict.id;
    view! {
        <li class="flex items-start gap-2">
            <div class="min-w-0 flex-1">
                <p class="text-sm leading-5 text-fg-secondary">
                    <span class="text-fg-primary">{headline}</span>
                    " "
                    {scope}
                </p>
                <p class="mt-1 text-sm leading-5 text-fg-secondary">{conflict.remedy}</p>
                {detected.map(|names| view! {
                    <p class="mt-1 text-[11px] leading-4 text-fg-tertiary break-words">
                        "Detected as "
                        <span class="font-mono">{names}</span>
                    </p>
                })}
            </div>
            <button
                type="button"
                class="shrink-0 rounded-md p-1 text-fg-tertiary transition-colors hover:text-fg-primary"
                aria-label=dismiss_label
                title=dismiss_title
                on:click=move |_| state.dismiss(&id)
            >
                <Icon icon=LuX width="12px" height="12px" />
            </button>
        </li>
    }
}

/// User-initiated rescan, styled as the page's standard secondary action.
#[component]
fn CheckAgainButton(state: SoftwareConflictsState) -> impl IntoView {
    let scanning = state.scanning;
    view! {
        <button
            type="button"
            class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] font-medium transition-all btn-press shrink-0 disabled:opacity-60
                   text-fg-primary bg-surface-overlay/70 border border-edge-subtle hover:border-accent-muted hover:bg-surface-overlay glow-ring"
            prop:disabled=move || scanning.get()
            on:click=move |_| state.check_again()
        >
            <span class=move || if scanning.get() { "inline-flex animate-spin" } else { "inline-flex" }>
                <Icon icon=LuRefreshCw width="12px" height="12px" />
            </span>
            {move || if scanning.get() { "Checking" } else { "Check again" }}
        </button>
    }
}

/// Compact line on a device naming the software that may hold it. It
/// explains a device that will not light; it is not an alarm.
#[component]
pub fn SoftwareConflictHint(text: String) -> impl IntoView {
    view! {
        <div class="flex items-start gap-1.5 text-[11px] leading-4 text-fg-secondary">
            <span class="mt-px inline-flex shrink-0 text-status-warning/90">
                <Icon icon=LuTriangleAlert width="11px" height="11px" />
            </span>
            <span class="min-w-0 break-words">{text}</span>
        </div>
    }
}
