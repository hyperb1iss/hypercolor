//! Shared debounced control-patch session.
//!
//! Every live control surface (effect controls, display face controls,
//! Studio layer controls) follows the same shape: an input edit ticks an
//! optimistic local `ControlValue` map immediately, the raw JSON edit is
//! queued into a pending batch keyed by control id (last write per key
//! wins, so a slider drag sends only its final position), and a debounced
//! flush PATCHes the coalesced batch to the daemon. Versioned routes echo
//! an `If-Match` token and rebase on a `412 Stale` reply.
//!
//! [`use_control_patch_session`] owns those mechanics once — debounce,
//! pending-batch coalescing, optimistic application, and epoch/version
//! reconciliation — while each surface supplies only its patch request,
//! its error toast, and (optionally) a flush guard. Cadences are passed
//! per surface and are product contracts: do not slow them down for
//! convenience.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use hypercolor_types::effect::ControlDefinition;
use leptos::prelude::*;
use leptos_use::use_debounce_fn;

use crate::api::client::MutationOutcome;
use crate::async_helpers::spawn_mutation;
use crate::optimistic_controls::{
    ControlValueMap, OptimisticControlSession, RawControlUpdates, raw_control_updates_payload,
};

/// Future returned by a surface's patch function. Not `Send` — it runs on
/// the single-threaded WASM executor via `spawn_local`.
pub type ControlPatchFuture =
    Pin<Box<dyn Future<Output = Result<MutationOutcome<Option<u64>>, String>>>>;

/// One control-surface PATCH: `(coalesced_payload, if_match_version)` →
/// outcome. A versioned route returns `Applied(Some(new_version))` so the
/// session can chain the next `If-Match` without a refetch; an unversioned
/// route returns `Applied(None)`. `Stale { current }` triggers the
/// session's rebase-and-retry path.
pub type ControlPatchFn =
    Arc<dyn Fn(serde_json::Value, Option<u64>) -> ControlPatchFuture + Send + Sync>;

/// Configuration for [`use_control_patch_session`].
pub struct ControlPatchConfig {
    /// Control schema used to normalize raw JSON edits into typed
    /// [`ControlValue`](hypercolor_types::effect::ControlValue)s for the
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
    /// `current` and stop so two racing writers cannot ping-pong forever.
    GiveUp { current: u64 },
}

/// Decide how to reconcile one patch outcome. A `Stale` reply earns
/// exactly one rebase-and-retry; a second `Stale` (a genuine concurrent
/// writer) adopts the fresh token and stops.
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
pub fn merge_retry_batch(failed: RawControlUpdates, newer: RawControlUpdates) -> RawControlUpdates {
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
/// pending edits). A response only reconciles when no newer flush started
/// while it was in flight, so out-of-order replies cannot regress the
/// version token.
pub fn use_control_patch_session(config: ControlPatchConfig) -> ControlPatchSession {
    let ControlPatchConfig {
        defs,
        set_values,
        initial_version,
        debounce_ms,
        patch,
        on_error,
        flush_guard,
    } = config;

    let optimistic = OptimisticControlSession::new();
    let version = RwSignal::new(initial_version);
    // Monotonic flush counter. Each flush claims the next epoch; its
    // response reconciles only while it is still the newest flush.
    let flush_epoch = StoredValue::new(0_u64);

    let flush_core = move || {
        if let Some(guard) = flush_guard
            && !guard.run(())
        {
            optimistic.clear_pending();
            return;
        }
        let batch = optimistic.take_pending();
        if batch.is_empty() {
            return;
        }
        let epoch = flush_epoch
            .try_update_value(|value| {
                *value = value.wrapping_add(1);
                *value
            })
            .unwrap_or_default();
        let patch = Arc::clone(&patch);
        spawn_mutation(
            async move {
                let mut batch = batch;
                let mut retried = false;
                loop {
                    let payload = raw_control_updates_payload(batch.clone());
                    let outcome = patch(payload, version.get_untracked()).await?;
                    let is_latest = flush_epoch.get_value() == epoch;
                    match reconcile_outcome(&outcome, retried) {
                        ReconcileAction::Adopt { new_version } => {
                            if is_latest && let Some(next) = new_version {
                                version.set(Some(next));
                            }
                            return Ok(());
                        }
                        ReconcileAction::RetryOnce { current } => {
                            if !is_latest {
                                // A newer flush owns reconciliation now;
                                // its batch carries the newest values.
                                return Ok(());
                            }
                            version.set(Some(current));
                            batch = merge_retry_batch(batch, optimistic.take_pending());
                            retried = true;
                        }
                        ReconcileAction::GiveUp { current } => {
                            if is_latest {
                                version.set(Some(current));
                            }
                            return Ok(());
                        }
                    }
                }
            },
            |()| {},
            move |error| on_error.run(error),
        );
    };

    let debounced_flush = use_debounce_fn(flush_core.clone(), debounce_ms);
    let on_change = Callback::new(move |(name, raw): (String, serde_json::Value)| {
        optimistic.apply_raw_update_to(set_values, &defs.get_untracked(), &name, &raw);
        optimistic.queue_raw_update(name, raw);
        debounced_flush();
    });
    let flush_now = Callback::new(move |()| flush_core());
    let clear_pending = Callback::new(move |()| optimistic.clear_pending());

    ControlPatchSession {
        on_change,
        flush_now,
        clear_pending,
        version,
    }
}
