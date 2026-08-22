//! Shared debounced control-patch session.
//!
//! Every live control surface (effect controls, display face controls,
//! Studio layer controls) follows the same shape: an input edit ticks an
//! optimistic local `ControlValue` map immediately, the admitted value is
//! queued into a pending batch keyed by control id (last write per key
//! wins, so a slider drag sends only its final position), and a debounced
//! flush PATCHes the coalesced batch to the daemon. Versioned routes echo
//! an `If-Match` token and rebase on a `412 Stale` reply.
//!
//! [`use_control_patch_session`] owns those mechanics once — debounce,
//! pending-batch coalescing, optimistic application, and version
//! reconciliation — while each surface supplies its patch request,
//! authoritative recovery, error toast, and (optionally) a flush guard.
//! Cadences are product contracts: do not slow them down for convenience.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use hypercolor_types::effect::ControlDefinition;
use leptos::prelude::*;
use leptos_use::use_debounce_fn;

use crate::api::{ApiError, ApiResult, MutationOutcome};
use crate::async_helpers::spawn_mutation;
use crate::optimistic_controls::{ControlValueMap, OptimisticControlSession};

/// Future returned by a surface's patch function. Not `Send` — it runs on
/// the single-threaded WASM executor via `spawn_local`.
pub type ControlPatchFuture =
    Pin<Box<dyn Future<Output = ApiResult<MutationOutcome<Option<u64>>>>>>;

/// One targeted control-surface PATCH. A versioned route returns
/// `Applied(Some(new_version))` so the session can chain the next
/// `If-Match` without a refetch; an unversioned route returns
/// `Applied(None)`. `Stale { current }` triggers one rebase-and-retry.
pub type ControlPatchFn =
    Arc<dyn Fn(String, ControlValueMap, Option<u64>) -> ControlPatchFuture + Send + Sync>;

/// Configuration for [`use_control_patch_session`].
pub struct ControlPatchConfig {
    /// Immutable authority key attached to every admitted batch. A target
    /// change invalidates older queued work before it can reach `patch`.
    pub target: Signal<Option<String>>,
    /// Control schema used to normalize raw JSON edits into typed
    /// [`ControlValue`](hypercolor_types::control::ControlValue)s for the
    /// optimistic local map.
    pub defs: Signal<Vec<ControlDefinition>>,
    /// Optimistic local control values; ticked synchronously on every
    /// edit so the UI never waits on the PATCH round-trip.
    pub set_values: WriteSignal<ControlValueMap>,
    /// Version token to echo as `If-Match`, when the route is versioned.
    pub initial_version: Option<u64>,
    /// Flush debounce in milliseconds. Per-surface cadences are product
    /// contracts (120 ms layer controls, 75 ms face controls).
    pub debounce_ms: f64,
    /// The surface's PATCH request. Site-specific success side effects
    /// (adopting a response payload, bumping refresh ticks) belong inside
    /// this closure.
    pub patch: ControlPatchFn,
    /// Error arm for a failed flush — typically a prefixed toast.
    pub on_error: Callback<String>,
    /// Restore authoritative values after any terminal patch failure.
    /// Every queued optimistic batch is cleared before this callback runs.
    pub recover: Callback<()>,
    /// Runs after the complete ordered queue commits for one target.
    pub on_committed: Option<Callback<String>>,
    /// Optional pre-flush gate. Returning `false` cancels the flush and
    /// drops the pending batch; the guard performs its own user feedback
    /// (toast, value revert) before returning.
    pub flush_guard: Option<Callback<(), bool>>,
}

/// Handle returned by [`use_control_patch_session`].
#[derive(Clone, Copy)]
pub struct ControlPatchSession {
    /// Wire to the control panel's change callback: applies the edit
    /// optimistically, queues it, and schedules a debounced flush.
    pub on_change: Callback<(String, serde_json::Value)>,
    /// Admit a compound edit atomically under the same target identity.
    pub on_changes: Callback<Vec<(String, serde_json::Value)>>,
    /// Flush the pending batch immediately, bypassing the debounce.
    pub flush_now: Callback<()>,
    /// Drop any queued-but-unsent edits.
    pub clear_pending: Callback<()>,
    /// Live version token echoed as `If-Match`; `None` for unversioned
    /// surfaces. Adopted from `Applied`/`Stale` outcomes automatically.
    pub version: RwSignal<Option<u64>>,
}

/// What the session does after a patch attempt resolves. Pure decision
/// logic, split out from the signal wiring so it is directly testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileAction {
    /// The patch applied; adopt the returned version when one came back.
    Adopt { new_version: Option<u64> },
    /// The precondition failed; adopt `current` and retry the batch once.
    RetryOnce { current: u64 },
    /// The precondition failed again after the single retry; adopt
    /// `current` and recover so two racing writers cannot ping-pong forever.
    GiveUp { current: u64 },
}

/// Decide how to reconcile one patch outcome. A `Stale` reply earns
/// exactly one rebase-and-retry; a second `Stale` (a genuine concurrent
/// writer) adopts the fresh token and recovers from authoritative state.
#[must_use]
pub fn reconcile_outcome(
    outcome: &MutationOutcome<Option<u64>>,
    already_retried: bool,
) -> ReconcileAction {
    match *outcome {
        MutationOutcome::Applied(new_version) => ReconcileAction::Adopt { new_version },
        MutationOutcome::Stale { current } if !already_retried => {
            ReconcileAction::RetryOnce { current }
        }
        MutationOutcome::Stale { current } => ReconcileAction::GiveUp { current },
    }
}

/// Merge a failed flush batch with edits queued while it was in flight.
/// Newer edits win per key, so a retry never resurrects a value the user
/// has already moved past.
#[must_use]
pub fn merge_retry_batch(failed: ControlValueMap, newer: ControlValueMap) -> ControlValueMap {
    let mut merged = failed;
    merged.extend(newer);
    merged
}

/// Create a debounced, optimistic control-patch session.
///
/// Each edit handed to `on_change` is applied to the local value map
/// immediately, queued into the pending batch, and flushed after
/// `debounce_ms` of quiet. A flush takes the whole batch, sends it through
/// `patch` with the current version token, and reconciles the outcome:
/// `Applied` adopts the returned version, `Stale` rebases onto the
/// daemon's token and retries the batch once (merged under any newer
/// pending edits). Exactly one request is in flight per session; edits queued
/// during a request drain in the next ordered batch, so responses cannot
/// overtake one another or discard distinct-key updates.
pub fn use_control_patch_session(config: ControlPatchConfig) -> ControlPatchSession {
    let ControlPatchConfig {
        target,
        defs,
        set_values,
        initial_version,
        debounce_ms,
        patch,
        on_error,
        recover,
        on_committed,
        flush_guard,
    } = config;

    let optimistic = OptimisticControlSession::new();
    let version = RwSignal::new(initial_version);

    let flush_core = move || {
        if let Some(guard) = flush_guard
            && !guard.run(())
        {
            optimistic.clear_pending();
            return;
        }
        let Some(active_target) = target.get_untracked() else {
            optimistic.clear_pending();
            recover.run(());
            return;
        };
        let mut batch = match optimistic.start_flush_for(&active_target) {
            Ok(Some(batch)) => batch,
            Ok(None) => return,
            Err(()) => {
                recover.run(());
                return;
            }
        };
        let patch = Arc::clone(&patch);
        spawn_mutation(
            async move {
                'batches: loop {
                    if target.get_untracked().as_deref() != Some(batch.target.as_str()) {
                        let Some(next) = next_batch_after_target_change(
                            optimistic,
                            target.get_untracked(),
                            recover,
                        ) else {
                            return Ok(());
                        };
                        batch = next;
                        continue 'batches;
                    }
                    let mut retried = false;
                    loop {
                        let outcome = patch(
                            batch.target.clone(),
                            batch.values.clone(),
                            version.get_untracked(),
                        )
                        .await;
                        if target.get_untracked().as_deref() != Some(batch.target.as_str()) {
                            let Some(next) = next_batch_after_target_change(
                                optimistic,
                                target.get_untracked(),
                                recover,
                            ) else {
                                return Ok(());
                            };
                            batch = next;
                            continue 'batches;
                        }
                        let outcome = outcome?;
                        match reconcile_outcome(&outcome, retried) {
                            ReconcileAction::Adopt { new_version } => {
                                if let Some(next) = new_version {
                                    version.set(Some(next));
                                }
                                break;
                            }
                            ReconcileAction::RetryOnce { current } => {
                                version.set(Some(current));
                                batch.values = merge_retry_batch(
                                    batch.values,
                                    optimistic.take_pending_for_retry(&batch.target),
                                );
                                retried = true;
                            }
                            ReconcileAction::GiveUp { current } => {
                                version.set(Some(current));
                                return Err(ApiError::Http {
                                    status: 412,
                                    message: Some(format!(
                                        "control state changed again at version {current}"
                                    )),
                                });
                            }
                        }
                    }

                    if let Some(guard) = flush_guard
                        && !guard.run(())
                    {
                        optimistic.fail_flush();
                        return Ok(());
                    }
                    let Some(next) = optimistic.complete_flush() else {
                        if let Some(on_committed) = on_committed {
                            on_committed.run(batch.target);
                        }
                        return Ok(());
                    };
                    batch = next;
                }
            },
            |()| {},
            move |error| {
                optimistic.fail_flush();
                recover.run(());
                on_error.run(error.to_string());
            },
        );
    };

    let debounced_flush = use_debounce_fn(flush_core.clone(), debounce_ms);
    let debounced_single_flush = debounced_flush.clone();
    let on_change = Callback::new(move |(name, raw): (String, serde_json::Value)| {
        let Some(target) = target.get_untracked() else {
            return;
        };
        optimistic.admit_raw_update_to(target, set_values, &defs.get_untracked(), &name, &raw);
        debounced_single_flush();
    });
    let on_changes = Callback::new(move |updates: Vec<(String, serde_json::Value)>| {
        let Some(target) = target.get_untracked() else {
            return;
        };
        optimistic.admit_raw_updates_to(target, set_values, &defs.get_untracked(), &updates);
        debounced_flush();
    });
    let flush_now = Callback::new(move |()| flush_core());
    let clear_pending = Callback::new(move |()| optimistic.clear_pending());

    ControlPatchSession {
        on_change,
        on_changes,
        flush_now,
        clear_pending,
        version,
    }
}

fn next_batch_after_target_change(
    optimistic: OptimisticControlSession,
    active_target: Option<String>,
    recover: Callback<()>,
) -> Option<crate::optimistic_controls::ControlMutationBatch> {
    let Some(active_target) = active_target else {
        optimistic.fail_flush();
        recover.run(());
        return None;
    };
    match optimistic.retire_flush_for(&active_target) {
        Ok(next) => next,
        Err(()) => {
            optimistic.fail_flush();
            recover.run(());
            None
        }
    }
}
