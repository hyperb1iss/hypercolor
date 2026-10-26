//! The Studio undo notice: a short-lived bar that offers to take back a
//! destructive zone action, the alternative to asking for confirmation
//! up front.

use std::time::Duration;

use hypercolor_leptos_ext::prelude::spawn_timeout;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::icons::*;

use super::StudioContext;
use super::history::DeletedZone;

/// How long the notice waits for an Undo before it leaves.
const UNDO_WINDOW: Duration = Duration::from_secs(8);

/// One offer to undo, and what accepting it restores.
#[derive(Clone, Debug, PartialEq)]
pub struct UndoNotice {
    /// Distinguishes successive notices, so an expiry timer only clears
    /// the notice it was started for.
    pub id: u64,
    pub message: String,
    pub restore: DeletedZone,
}

impl UndoNotice {
    /// The notice offered after `record`'s zone was deleted.
    #[must_use]
    pub fn zone_deleted(id: u64, record: DeletedZone) -> Self {
        Self {
            id,
            message: format!("Deleted {}", record.name),
            restore: record,
        }
    }
}

/// Renders the current notice, bottom center of the workspace.
#[component]
pub fn UndoNoticeHost() -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let notice = studio.undo_notice;

    Effect::new(move |_| {
        let Some(id) = notice.with(|current| current.as_ref().map(|current| current.id)) else {
            return;
        };
        spawn_timeout(UNDO_WINDOW, move || {
            if notice
                .try_with_untracked(|current| current.as_ref().map(|current| current.id))
                .flatten()
                == Some(id)
            {
                notice.try_set(None);
            }
        });
    });

    // The notice and Ctrl+Z share one journal entry, so whichever restores
    // the zone first, the other no longer can; the notice leaves once its
    // deletion is no longer undoable.
    let journal = studio.history.journal;
    Effect::new(move |_| {
        let Some(token) =
            notice.with(|current| current.as_ref().map(|current| current.restore.token))
        else {
            return;
        };
        if journal.with(|journal| journal.undoable_deletion(token).is_none()) {
            notice.set(None);
        }
    });

    let undo = move |_| {
        let Some(current) = notice.get_untracked() else {
            return;
        };
        notice.set(None);
        studio.history.undo_deletion(current.restore.token);
    };

    view! {
        <div
            class="pointer-events-none absolute inset-x-0 bottom-4 z-50 flex justify-center px-4"
            role="status"
            aria-live="polite"
        >
            {move || {
                notice
                    .get()
                    .map(|current| {
                        view! {
                            <div class="pointer-events-auto flex items-center gap-3 rounded-xl border border-edge-subtle bg-surface-overlay py-2 pl-4 pr-2 text-[12px] text-fg-secondary animate-enter-fade">
                                <span class="min-w-0 truncate">{current.message}</span>
                                <button
                                    type="button"
                                    class="btn-press rounded-lg border border-accent-muted bg-accent-subtle px-3 py-1 text-[12px] font-medium text-fg-primary transition-colors hover:bg-accent-muted"
                                    disabled=move || studio.history.busy.get()
                                    on:click=undo
                                >
                                    "Undo"
                                </button>
                                <button
                                    type="button"
                                    class="flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary transition-colors hover:text-fg-primary"
                                    title="Dismiss"
                                    aria-label="Dismiss"
                                    on:click=move |_| notice.set(None)
                                >
                                    <Icon icon=LuX width="12px" height="12px" />
                                </button>
                            </div>
                        }
                    })
            }}
        </div>
    }
}
