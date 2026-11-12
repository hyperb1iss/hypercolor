//! The Studio undo notice: a short-lived bar that offers to take back an
//! action that did more than its gesture showed. A deleted zone gets one
//! in place of a confirmation up front, and so does a move that took
//! layers with it between zones.

use std::time::Duration;

use hypercolor_leptos_ext::prelude::spawn_timeout;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::icons::*;

use super::StudioContext;
use super::history::{Carried, DeletedZone, EntryId};

/// How long the notice waits for an Undo before it leaves.
const UNDO_WINDOW: Duration = Duration::from_secs(8);

/// What accepting a notice takes back.
#[derive(Clone, Debug, PartialEq)]
pub enum Restore {
    /// A deleted zone, rebuilt from its record.
    Zone(DeletedZone),
    /// The step at this journal place, while it is still the newest.
    Step(EntryId),
}

/// One offer to undo, and what accepting it restores.
#[derive(Clone, Debug, PartialEq)]
pub struct UndoNotice {
    /// Distinguishes successive notices, so an expiry timer only clears
    /// the notice it was started for.
    pub id: u64,
    pub message: String,
    pub restore: Restore,
}

impl UndoNotice {
    /// The notice offered after `record`'s zone was deleted.
    #[must_use]
    pub fn zone_deleted(id: u64, record: DeletedZone) -> Self {
        Self {
            id,
            message: format!("Deleted {}", record.name),
            restore: Restore::Zone(record),
        }
    }

    /// The notice offered after a move took layers with it. Journal
    /// places are unique to the page, so the place is the notice's id.
    #[must_use]
    pub fn carried(carried: Carried) -> Self {
        Self {
            id: carried.entry,
            message: carried.message,
            restore: Restore::Step(carried.entry),
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

    // A move that carried layers raises its own notice.
    let carried = studio.history.carried;
    Effect::new(move |_| {
        if let Some(carried) = carried.get() {
            notice.set(Some(UndoNotice::carried(carried)));
        }
    });

    // The notice and Ctrl+Z share one journal entry, so whichever takes
    // the step back first, the other no longer can; the notice leaves once
    // its step is no longer the one an undo would take.
    let journal = studio.history.journal;
    Effect::new(move |_| {
        let Some(restore) =
            notice.with(|current| current.as_ref().map(|current| current.restore.clone()))
        else {
            return;
        };
        let undoable = journal.with(|journal| match &restore {
            Restore::Zone(record) => journal.undoable_deletion(record.token).is_some(),
            Restore::Step(entry) => journal.is_latest(*entry),
        });
        if !undoable {
            notice.set(None);
        }
    });

    let undo = move |_| {
        let Some(current) = notice.get_untracked() else {
            return;
        };
        notice.set(None);
        match current.restore {
            Restore::Zone(record) => studio.history.undo_deletion(record.token),
            Restore::Step(entry) => {
                if journal.with_untracked(|journal| journal.is_latest(entry)) {
                    studio.history.replay(false);
                }
            }
        }
    };

    view! {
        <div
            // Centered on the Stage: a docked composition panel takes its
            // width from the Stage, so the notice steps aside with it.
            class="pointer-events-none absolute inset-x-0 bottom-4 z-50 flex justify-center px-4 transition-[padding] duration-200 motion-reduce:transition-none"
            class=("xl:pr-[436px]", move || studio.composition_open.get())
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
