//! Tests for the pure decision logic of the shared control-patch
//! session: outcome reconciliation (version adoption, the single
//! Stale rebase-and-retry) and retry-batch merging.

use hypercolor_ui::api::client::MutationOutcome;
use hypercolor_ui::control_session::{ReconcileAction, merge_retry_batch, reconcile_outcome};
use hypercolor_ui::optimistic_controls::RawControlUpdates;

#[test]
fn applied_outcome_adopts_returned_version() {
    let action = reconcile_outcome(&MutationOutcome::Applied(Some(7)), false);
    assert_eq!(
        action,
        ReconcileAction::Adopt {
            new_version: Some(7)
        }
    );
}

#[test]
fn applied_outcome_on_unversioned_route_adopts_nothing() {
    let action = reconcile_outcome(&MutationOutcome::Applied(None), false);
    assert_eq!(action, ReconcileAction::Adopt { new_version: None });
}

#[test]
fn applied_outcome_adopts_even_after_a_retry() {
    let action = reconcile_outcome(&MutationOutcome::Applied(Some(12)), true);
    assert_eq!(
        action,
        ReconcileAction::Adopt {
            new_version: Some(12)
        }
    );
}

#[test]
fn first_stale_reply_earns_one_rebase_and_retry() {
    let action = reconcile_outcome(&MutationOutcome::Stale { current: 41 }, false);
    assert_eq!(action, ReconcileAction::RetryOnce { current: 41 });
}

#[test]
fn second_stale_reply_gives_up_with_the_fresh_token() {
    let action = reconcile_outcome(&MutationOutcome::Stale { current: 42 }, true);
    assert_eq!(action, ReconcileAction::GiveUp { current: 42 });
}

#[test]
fn merge_retry_batch_keeps_failed_edits_no_newer_edit_touched() {
    let failed = RawControlUpdates::from([
        ("speed".to_owned(), serde_json::json!(0.5)),
        ("hue".to_owned(), serde_json::json!(120)),
    ]);
    let newer = RawControlUpdates::new();

    let merged = merge_retry_batch(failed.clone(), newer);
    assert_eq!(merged, failed);
}

#[test]
fn merge_retry_batch_prefers_newer_edits_per_key() {
    let failed = RawControlUpdates::from([
        ("speed".to_owned(), serde_json::json!(0.5)),
        ("hue".to_owned(), serde_json::json!(120)),
    ]);
    let newer = RawControlUpdates::from([("speed".to_owned(), serde_json::json!(0.9))]);

    let merged = merge_retry_batch(failed, newer);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged.get("speed"), Some(&serde_json::json!(0.9)));
    assert_eq!(merged.get("hue"), Some(&serde_json::json!(120)));
}

#[test]
fn merge_retry_batch_carries_brand_new_keys_from_the_newer_batch() {
    let failed = RawControlUpdates::from([("speed".to_owned(), serde_json::json!(0.5))]);
    let newer = RawControlUpdates::from([("brightness".to_owned(), serde_json::json!(0.8))]);

    let merged = merge_retry_batch(failed, newer);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged.get("speed"), Some(&serde_json::json!(0.5)));
    assert_eq!(merged.get("brightness"), Some(&serde_json::json!(0.8)));
}
