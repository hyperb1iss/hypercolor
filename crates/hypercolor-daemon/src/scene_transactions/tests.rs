use std::sync::Arc;
use std::time::Duration;

use super::{
    LayoutActivationControl, LayoutActivationDecision, LayoutPersistenceOutcome,
    LayoutPersistencePhase, LayoutTransactionRejection, LayoutUpdateError, PreparedLayoutUpdate,
    SceneTransaction, SceneTransactionQueue, apply_layout_update,
    apply_prepared_layout_update_under_guard_with_persistence, publish_prepared_layout_activation,
};
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::{SpatialEngine, SpatialPlanError};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use tempfile::TempDir;
use tokio::sync::Notify;

use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;

fn layout(id: &str, width: u32, height: u32) -> SpatialLayout {
    SpatialLayout {
        id: id.to_owned(),
        name: id.to_owned(),
        description: None,
        canvas_width: width,
        canvas_height: height,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn state(initial: SpatialLayout) -> (SpatialService, SceneService, SceneTransactionQueue) {
    let spatial_engine =
        SpatialEngine::try_new(initial.clone()).expect("test spatial layout should be addressable");
    (
        SpatialService::new(spatial_engine),
        SceneService::in_memory(
            SceneManager::with_default_layout(initial),
            Arc::new(HypercolorBus::new()),
        ),
        SceneTransactionQueue::default(),
    )
}

async fn commit_scene_mutation(
    scene_manager: &SceneService,
    mutation: crate::domain::scene::SceneMutation,
) {
    scene_manager
        .commit_mutation(mutation)
        .await
        .expect("scene transaction mutation should commit");
}

#[tokio::test]
async fn scene_activation_guard_serializes_the_post_commit_pipeline() {
    let queue = SceneTransactionQueue::default();
    let first = queue.acquire_scene_activation_guard().await;
    let waiting = Arc::new(Notify::new());
    let task_waiting = Arc::clone(&waiting);
    let task_queue = queue.clone();
    let second = tokio::spawn(async move {
        task_waiting.notify_one();
        task_queue.acquire_scene_activation_guard().await
    });

    waiting.notified().await;
    tokio::task::yield_now().await;
    assert!(!second.is_finished());

    drop(first);
    tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .expect("second activation should acquire the released guard")
        .expect("guard waiter should not panic");
}

async fn wait_for_pending(queue: &SceneTransactionQueue, expected: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while queue.pending_len() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("layout transaction should be enqueued");
}

fn accept_preparation(transaction: super::PrepareLayoutTransaction) -> LayoutActivationControl {
    let activation = transaction.activation();
    transaction.accept();
    activation
}

struct AcceptedPublication {
    activation: LayoutActivationControl,
    spatial_engine: SpatialEngine,
    expected_layout: SpatialLayout,
    active_scene_id: Option<hypercolor_types::scene::SceneId>,
    source_active_render_groups_revision: u64,
}

fn accept_publication(transaction: super::PrepareLayoutTransaction) -> AcceptedPublication {
    let accepted = AcceptedPublication {
        activation: transaction.activation(),
        spatial_engine: transaction.spatial_engine().clone(),
        expected_layout: transaction.expected_layout().clone(),
        active_scene_id: transaction.active_scene_id(),
        source_active_render_groups_revision: transaction.source_active_render_groups_revision(),
    };
    transaction.accept();
    accepted
}

async fn publish_commit(
    accepted: AcceptedPublication,
    spatial_engine: &SpatialService,
    scene_manager: &SceneService,
) -> Result<(), LayoutTransactionRejection> {
    wait_for_decision(&accepted.activation, LayoutActivationDecision::Commit).await;
    let result = publish_prepared_layout_activation(
        spatial_engine,
        scene_manager,
        accepted.spatial_engine,
        &accepted.expected_layout,
        accepted.active_scene_id,
        accepted.source_active_render_groups_revision,
        |_| {},
    )
    .await;
    accepted.activation.complete(result.clone());
    result
}

async fn complete_commit(activation: &LayoutActivationControl) {
    wait_for_decision(activation, LayoutActivationDecision::Commit).await;
    activation.complete(Ok(()));
}

async fn wait_for_decision(
    activation: &LayoutActivationControl,
    expected: LayoutActivationDecision,
) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while activation.decision() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("layout activation decision should resolve");
}

#[tokio::test]
async fn queue_drains_ordered_atomic_layout_transactions_without_coalescing() {
    let (spatial_engine, scene_manager, queue) = state(layout("initial", 320, 200));
    let _consumer = queue.consumer();
    queue
        .push(SceneTransaction::SetScreenCaptureConfigured(true))
        .expect("renderer queue should accept transactions");

    let update_queue = queue.clone();
    let update = tokio::spawn(async move {
        apply_layout_update(
            &spatial_engine,
            &scene_manager,
            &update_queue,
            layout("wide", 7_680, 4_320),
        )
        .await
    });
    wait_for_pending(&queue, 2).await;
    queue
        .push(SceneTransaction::SetScreenCaptureConfigured(false))
        .expect("renderer queue should accept transactions");

    let transactions = queue.drain();
    assert_eq!(transactions.len(), 3);
    assert!(matches!(
        transactions.first(),
        Some(SceneTransaction::SetScreenCaptureConfigured(true))
    ));
    assert!(matches!(
        transactions.last(),
        Some(SceneTransaction::SetScreenCaptureConfigured(false))
    ));
    let SceneTransaction::PrepareLayout(transaction) = transactions
        .into_iter()
        .nth(1)
        .expect("atomic layout transaction should remain ordered")
    else {
        panic!("middle transaction should atomically carry the layout");
    };
    assert_eq!(transaction.spatial_engine().layout().canvas_width, 7_680);
    assert_eq!(transaction.spatial_engine().layout().canvas_height, 4_320);
    let activation = accept_preparation(transaction);
    complete_commit(&activation).await;
    update
        .await
        .expect("layout coordinator should not panic")
        .expect("layout should commit after renderer acceptance");
}

#[tokio::test]
async fn renderer_and_authoritative_state_reuse_the_exact_prepared_sampling_plan() {
    let (spatial_engine, scene_manager, queue) = state(layout("initial", 320, 200));
    let _consumer = queue.consumer();
    let update_spatial_engine = spatial_engine.clone();
    let update_scene_manager = scene_manager.clone();
    let update_queue = queue.clone();
    let update = tokio::spawn(async move {
        apply_layout_update(
            &update_spatial_engine,
            &update_scene_manager,
            &update_queue,
            layout("prepared", 1_920, 1_080),
        )
        .await
    });
    wait_for_pending(&queue, 1).await;

    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout transaction should be queued")
    else {
        panic!("queued transaction should apply a layout");
    };
    let renderer_plan = transaction.spatial_engine().sampling_plan();
    let accepted = accept_publication(transaction);
    publish_commit(accepted, &spatial_engine, &scene_manager)
        .await
        .expect("prepared layout should publish");
    update
        .await
        .expect("layout coordinator should not panic")
        .expect("layout should commit after renderer acceptance");

    let authoritative = spatial_engine.snapshot();
    assert!(Arc::ptr_eq(&renderer_plan, &authoritative.sampling_plan()));
    assert_eq!(authoritative.layout().id, "prepared");
    drop(authoritative);
    let manager = scene_manager.snapshot().await;
    assert_eq!(
        manager
            .active_scene()
            .and_then(|scene| scene.primary_zone())
            .expect("default scene should retain a primary group")
            .layout
            .id,
        "prepared"
    );
}

#[tokio::test]
async fn render_snapshot_reads_old_generation_while_activation_waits() {
    let initial = layout("initial", 320, 200);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let update_spatial_engine = spatial_engine.clone();
    let update_scene_manager = scene_manager.clone();
    let update_queue = queue.clone();
    let update = tokio::spawn(async move {
        apply_layout_update(
            &update_spatial_engine,
            &update_scene_manager,
            &update_queue,
            layout("candidate", 7_680, 4_320),
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout transaction should be queued")
    else {
        panic!("queued transaction should apply a layout");
    };
    let accepted = accept_publication(transaction);
    wait_for_decision(&accepted.activation, LayoutActivationDecision::Commit).await;

    tokio::time::timeout(Duration::from_secs(1), async {
        let manager = scene_manager.snapshot().await;
        let authoritative = spatial_engine.snapshot();
        assert_eq!(manager.active_render_groups()[0].layout.id, initial.id);
        assert_eq!(authoritative.layout().id, initial.id);
    })
    .await
    .expect("render snapshot should not wait behind layout activation");

    publish_commit(accepted, &spatial_engine, &scene_manager)
        .await
        .expect("candidate layout should publish");
    update
        .await
        .expect("layout coordinator should not panic")
        .expect("layout should commit after renderer publication");
}

#[tokio::test]
async fn newer_scene_state_supersedes_prepared_layout_before_publication() {
    let initial = layout("initial", 320, 200);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let update_spatial_engine = spatial_engine.clone();
    let update_scene_manager = scene_manager.clone();
    let update_queue = queue.clone();
    let update = tokio::spawn(async move {
        apply_layout_update(
            &update_spatial_engine,
            &update_scene_manager,
            &update_queue,
            layout("superseded", 1_920, 1_080),
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout transaction should be queued")
    else {
        panic!("queued transaction should apply a layout");
    };
    let accepted = accept_publication(transaction);
    wait_for_decision(&accepted.activation, LayoutActivationDecision::Commit).await;
    let mut mutation = scene_manager.begin_mutation().await;
    mutation.invalidate_active_zones();
    commit_scene_mutation(&scene_manager, mutation).await;

    assert_eq!(
        publish_commit(accepted, &spatial_engine, &scene_manager).await,
        Err(LayoutTransactionRejection::Superseded)
    );
    assert!(matches!(
        update.await.expect("layout coordinator should not panic"),
        Err(LayoutUpdateError::Transaction(
            LayoutTransactionRejection::Superseded
        ))
    ));
    assert_eq!(spatial_engine.snapshot().layout().as_ref(), &initial);
    assert_eq!(
        scene_manager.snapshot().await.active_render_groups()[0]
            .layout
            .id,
        initial.id
    );
}

#[tokio::test]
async fn effect_id_migration_supersedes_queued_layout_publication() {
    let initial = layout("initial-migration", 320, 200);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let update_spatial_engine = spatial_engine.clone();
    let update_scene_manager = scene_manager.clone();
    let update_queue = queue.clone();
    let update = tokio::spawn(async move {
        apply_layout_update(
            &update_spatial_engine,
            &update_scene_manager,
            &update_queue,
            layout("stale-after-migration", 1_920, 1_080),
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout transaction should be queued")
    else {
        panic!("queued transaction should apply a layout");
    };
    let accepted = accept_publication(transaction);
    wait_for_decision(&accepted.activation, LayoutActivationDecision::Commit).await;

    let legacy_id = hypercolor_types::effect::EffectId::new(uuid::Uuid::now_v7());
    let canonical_id = hypercolor_types::effect::EffectId::new(uuid::Uuid::now_v7());
    let migration = scene_manager
        .prepare_effect_id_migration(&std::collections::HashMap::from([(
            legacy_id,
            canonical_id,
        )]))
        .await
        .expect("scene migration should prepare")
        .persist()
        .expect("in-memory scene migration should persist");
    let publication = scene_manager
        .prepare_effect_id_migration_publication(migration)
        .await
        .expect("scene migration should prepare publication");
    scene_manager.publish_effect_id_migration(publication);

    assert_eq!(
        publish_commit(accepted, &spatial_engine, &scene_manager).await,
        Err(LayoutTransactionRejection::Superseded)
    );
    assert!(matches!(
        update.await.expect("layout coordinator should not panic"),
        Err(LayoutUpdateError::Transaction(
            LayoutTransactionRejection::Superseded
        ))
    ));
    assert_eq!(spatial_engine.snapshot().layout().as_ref(), &initial);
}

#[tokio::test]
async fn renderer_rejection_preserves_state_and_a_retry_can_commit() {
    let initial = layout("initial", 320, 200);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();

    let rejected_spatial_engine = spatial_engine.clone();
    let rejected_scene_manager = scene_manager.clone();
    let rejected_queue = queue.clone();
    let rejected = tokio::spawn(async move {
        apply_layout_update(
            &rejected_spatial_engine,
            &rejected_scene_manager,
            &rejected_queue,
            layout("retry", 3_840, 2_160),
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout transaction should be queued")
    else {
        panic!("queued transaction should apply a layout");
    };
    transaction.reject(LayoutTransactionRejection::PreparationFailed {
        message: "synthetic allocation failure".to_owned(),
    });
    assert!(matches!(
        rejected.await.expect("layout coordinator should not panic"),
        Err(LayoutUpdateError::Transaction(
            LayoutTransactionRejection::PreparationFailed { .. }
        ))
    ));
    assert_eq!(spatial_engine.snapshot().layout().as_ref(), &initial);

    let retry_spatial_engine = spatial_engine.clone();
    let retry_scene_manager = scene_manager.clone();
    let retry_queue = queue.clone();
    let retry = tokio::spawn(async move {
        apply_layout_update(
            &retry_spatial_engine,
            &retry_scene_manager,
            &retry_queue,
            layout("retry", 3_840, 2_160),
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("retry transaction should be queued")
    else {
        panic!("retry transaction should apply a layout");
    };
    let accepted = accept_publication(transaction);
    publish_commit(accepted, &spatial_engine, &scene_manager)
        .await
        .expect("retry layout should publish");
    retry
        .await
        .expect("layout coordinator should not panic")
        .expect("retry should commit");
    assert_eq!(spatial_engine.snapshot().layout().id, "retry");
}

#[tokio::test]
async fn renderer_exit_rejects_pending_and_future_layout_updates() {
    let (spatial_engine, scene_manager, queue) = state(layout("initial", 320, 200));
    let consumer = queue.consumer();
    let update_queue = queue.clone();
    let update = tokio::spawn(async move {
        apply_layout_update(
            &spatial_engine,
            &scene_manager,
            &update_queue,
            layout("pending", 1_280, 720),
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    drop(consumer);

    assert!(matches!(
        update.await.expect("layout coordinator should not panic"),
        Err(LayoutUpdateError::Transaction(
            LayoutTransactionRejection::RendererStopped
        ))
    ));
    assert!(queue.is_closed());
    assert!(matches!(
        queue.push(SceneTransaction::SetScreenCaptureConfigured(true)),
        Err(LayoutTransactionRejection::RendererStopped)
    ));
}

#[tokio::test]
async fn invalid_preparation_never_reaches_the_renderer_or_mutates_state() {
    let initial = layout("initial", 320, 200);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let mut invalid = layout("invalid", 320, 200);
    invalid.canvas_width = u32::MAX;
    invalid.canvas_height = u32::MAX;

    let result = apply_layout_update(&spatial_engine, &scene_manager, &queue, invalid).await;

    assert!(matches!(
        result,
        Err(LayoutUpdateError::SpatialPlan(
            SpatialPlanError::CanvasByteLengthOverflow {
                width: u32::MAX,
                height: u32::MAX,
            }
        ))
    ));
    assert_eq!(queue.pending_len(), 0);
    assert_eq!(spatial_engine.snapshot().layout().as_ref(), &initial);
}

#[tokio::test]
async fn admitted_persistence_failure_aborts_and_persists_fresh_rollback() {
    let initial = layout("initial", 320, 200);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let guard = queue.acquire_layout_update_guard().await;
    let prepared = PreparedLayoutUpdate::try_new(layout("candidate", 640, 480))
        .expect("candidate layout should prepare");
    let update_spatial_engine = spatial_engine.clone();
    let update_scene_manager = scene_manager.clone();
    let update_queue = queue.clone();
    let phases = Arc::new(std::sync::Mutex::new(Vec::new()));
    let update_phases = Arc::clone(&phases);
    let update = tokio::spawn(async move {
        apply_prepared_layout_update_under_guard_with_persistence(
            update_spatial_engine,
            update_scene_manager,
            update_queue,
            &guard,
            prepared,
            move |phase| {
                let phases = Arc::clone(&update_phases);
                async move {
                    match phase {
                        LayoutPersistencePhase::Precommit(_) => {
                            phases
                                .lock()
                                .expect("phase log should lock")
                                .push("precommit");
                            LayoutPersistenceOutcome::RetryArmed(
                                "synthetic persistence failure".to_owned(),
                            )
                        }
                        LayoutPersistencePhase::Rollback => {
                            phases
                                .lock()
                                .expect("phase log should lock")
                                .push("rollback");
                            LayoutPersistenceOutcome::Written
                        }
                        LayoutPersistencePhase::Converge => {
                            panic!("failed precommit must not converge")
                        }
                    }
                }
            },
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout preparation should be queued")
    else {
        panic!("queued transaction should prepare a layout");
    };
    let activation = accept_preparation(transaction);
    wait_for_decision(&activation, LayoutActivationDecision::Abort).await;
    activation.complete(Ok(()));

    assert!(matches!(
        update.await.expect("layout coordinator should not panic"),
        Err(LayoutUpdateError::Persistence(message))
            if message.contains("synthetic persistence failure")
    ));
    assert_eq!(
        *phases.lock().expect("phase log should lock"),
        ["precommit", "rollback"]
    );
    assert_eq!(spatial_engine.snapshot().layout().as_ref(), &initial);
    let manager_layout = scene_manager.snapshot().await.active_render_groups()[0]
        .layout
        .clone();
    assert_eq!(manager_layout.id, initial.id);
    assert_eq!(manager_layout.canvas_width, initial.canvas_width);
    assert_eq!(manager_layout.canvas_height, initial.canvas_height);
}

#[tokio::test]
async fn superseded_precommit_aborts_renderer_admission() {
    let initial = layout("initial", 320, 200);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let guard = queue.acquire_layout_update_guard().await;
    let prepared = PreparedLayoutUpdate::try_new(layout("candidate", 640, 480))
        .expect("candidate layout should prepare");
    let update_spatial_engine = spatial_engine.clone();
    let update_scene_manager = scene_manager.clone();
    let update_queue = queue.clone();
    let update = tokio::spawn(async move {
        apply_prepared_layout_update_under_guard_with_persistence(
            update_spatial_engine,
            update_scene_manager,
            update_queue,
            &guard,
            prepared,
            |phase| async move {
                match phase {
                    LayoutPersistencePhase::Precommit(_) => LayoutPersistenceOutcome::Superseded,
                    LayoutPersistencePhase::Rollback => {
                        panic!("superseded precommit must not roll back")
                    }
                    LayoutPersistencePhase::Converge => {
                        panic!("superseded precommit must not converge")
                    }
                }
            },
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout preparation should be queued")
    else {
        panic!("queued transaction should prepare a layout");
    };
    let activation = accept_preparation(transaction);
    wait_for_decision(&activation, LayoutActivationDecision::Abort).await;
    activation.complete(Ok(()));

    assert!(matches!(
        update.await.expect("layout coordinator should not panic"),
        Err(LayoutUpdateError::PersistenceSuperseded)
    ));
    assert_eq!(spatial_engine.snapshot().layout().as_ref(), &initial);
    assert_eq!(
        scene_manager.snapshot().await.active_render_groups()[0]
            .layout
            .id,
        initial.id
    );
}

#[tokio::test]
async fn persistence_finishes_before_armed_renderer_publication() {
    let initial = layout("initial", 320, 200);
    let candidate = layout("candidate", 3_840, 2_160);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let tempdir = TempDir::new().expect("transaction tempdir");
    let persisted_path = tempdir.path().join("active-layout");
    std::fs::write(&persisted_path, &initial.id).expect("seed persisted generation");
    let guard = queue.acquire_layout_update_guard().await;
    let prepared = PreparedLayoutUpdate::try_new(candidate.clone()).expect("candidate prepares");
    let update_spatial_engine = spatial_engine.clone();
    let update_scene_manager = scene_manager.clone();
    let update_queue = queue.clone();
    let update_path = persisted_path.clone();
    let update = tokio::spawn(async move {
        apply_prepared_layout_update_under_guard_with_persistence(
            update_spatial_engine,
            update_scene_manager,
            update_queue,
            &guard,
            prepared,
            move |phase| {
                let path = update_path.clone();
                async move {
                    let LayoutPersistencePhase::Precommit(state) = phase else {
                        panic!("successful renderer publication must not roll back")
                    };
                    std::fs::write(path, state.layout.id).expect("test persistence should write");
                    LayoutPersistenceOutcome::Written
                }
            },
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout preparation should be queued")
    else {
        panic!("queued transaction should prepare a layout");
    };
    let accepted = accept_publication(transaction);
    wait_for_decision(&accepted.activation, LayoutActivationDecision::Commit).await;

    assert_eq!(
        std::fs::read_to_string(&persisted_path).expect("candidate persisted"),
        candidate.id
    );
    assert_eq!(scene_manager.revision(), 0);
    publish_commit(accepted, &spatial_engine, &scene_manager)
        .await
        .expect("persisted layout should publish");
    assert_eq!(scene_manager.revision(), 1);
    update
        .await
        .expect("layout coordinator should not panic")
        .expect("armed candidate should commit");
    assert_eq!(spatial_engine.snapshot().layout().id, candidate.id);
    assert_eq!(
        scene_manager.snapshot().await.active_render_groups()[0]
            .layout
            .id,
        candidate.id
    );
}

#[tokio::test]
async fn renderer_shutdown_after_persistence_rolls_disk_back_to_live_generation() {
    let initial = layout("initial", 320, 200);
    let candidate = layout("candidate", 7_680, 4_320);
    let newer = layout("newer-live", 2_560, 1_440);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let tempdir = TempDir::new().expect("transaction tempdir");
    let persisted_path = tempdir.path().join("active-layout");
    std::fs::write(&persisted_path, &initial.id).expect("seed persisted generation");
    let guard = queue.acquire_layout_update_guard().await;
    let prepared = PreparedLayoutUpdate::try_new(candidate).expect("candidate prepares");
    let update_spatial_engine = spatial_engine.clone();
    let update_scene_manager = scene_manager.clone();
    let update_queue = queue.clone();
    let update_path = persisted_path.clone();
    let rollback_spatial_engine = spatial_engine.clone();
    let update = tokio::spawn(async move {
        apply_prepared_layout_update_under_guard_with_persistence(
            update_spatial_engine,
            update_scene_manager,
            update_queue,
            &guard,
            prepared,
            move |phase| {
                let path = update_path.clone();
                let spatial_engine = rollback_spatial_engine.clone();
                async move {
                    let layout_id = match phase {
                        LayoutPersistencePhase::Precommit(state) => state.layout.id,
                        LayoutPersistencePhase::Rollback => {
                            spatial_engine.snapshot().layout().id.clone()
                        }
                        LayoutPersistencePhase::Converge => {
                            panic!("rejected renderer publication must not converge")
                        }
                    };
                    std::fs::write(path, layout_id).expect("test persistence should write");
                    LayoutPersistenceOutcome::Written
                }
            },
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout preparation should be queued")
    else {
        panic!("queued transaction should prepare a layout");
    };
    let activation = accept_preparation(transaction);
    wait_for_decision(&activation, LayoutActivationDecision::Commit).await;
    let source_layout = spatial_engine.snapshot().layout().as_ref().clone();
    let scenes = scene_manager.snapshot().await;
    scene_manager
        .publish_layout_activation(
            &spatial_engine,
            SpatialEngine::try_new(newer.clone()).expect("newer live layout should prepare"),
            &source_layout,
            scenes.active_scene_id().copied(),
            scenes.active_render_groups_revision(),
            |_| {},
        )
        .await
        .expect("newer live layout should publish");
    activation.complete(Err(LayoutTransactionRejection::RendererStopped));

    assert!(matches!(
        update.await.expect("layout coordinator should not panic"),
        Err(LayoutUpdateError::Transaction(
            LayoutTransactionRejection::RendererStopped
        ))
    ));
    assert_eq!(
        std::fs::read_to_string(&persisted_path).expect("rollback persisted"),
        newer.id
    );
    assert_eq!(spatial_engine.snapshot().layout().id, newer.id);
    assert_eq!(
        scene_manager.snapshot().await.active_render_groups()[0]
            .layout
            .id,
        newer.id
    );
}

async fn renderer_rejection_with_rollback_outcome(
    rollback_outcome: LayoutPersistenceOutcome,
) -> LayoutUpdateError {
    let (spatial_engine, scene_manager, queue) = state(layout("initial", 320, 200));
    let _consumer = queue.consumer();
    let guard = queue.acquire_layout_update_guard().await;
    let prepared = PreparedLayoutUpdate::try_new(layout("candidate", 640, 480))
        .expect("candidate layout should prepare");
    let rollback_outcome = Arc::new(std::sync::Mutex::new(Some(rollback_outcome)));
    let update_rollback_outcome = Arc::clone(&rollback_outcome);
    let update_queue = queue.clone();
    let update = tokio::spawn(async move {
        apply_prepared_layout_update_under_guard_with_persistence(
            spatial_engine,
            scene_manager,
            update_queue,
            &guard,
            prepared,
            move |phase| {
                let rollback_outcome = Arc::clone(&update_rollback_outcome);
                async move {
                    match phase {
                        LayoutPersistencePhase::Precommit(_) => LayoutPersistenceOutcome::Written,
                        LayoutPersistencePhase::Rollback => rollback_outcome
                            .lock()
                            .expect("rollback outcome should lock")
                            .take()
                            .expect("rollback should run once"),
                        LayoutPersistencePhase::Converge => {
                            panic!("rejected renderer publication must not converge")
                        }
                    }
                }
            },
        )
        .await
    });
    wait_for_pending(&queue, 1).await;
    let SceneTransaction::PrepareLayout(transaction) = queue
        .drain()
        .into_iter()
        .next()
        .expect("layout preparation should be queued")
    else {
        panic!("queued transaction should prepare a layout");
    };
    let activation = accept_preparation(transaction);
    wait_for_decision(&activation, LayoutActivationDecision::Commit).await;
    activation.complete(Err(LayoutTransactionRejection::RendererStopped));

    update
        .await
        .expect("layout coordinator should not panic")
        .expect_err("unsafe rollback outcome must fail")
}

#[tokio::test]
async fn renderer_rejection_rejects_retry_armed_rollback() {
    let error = renderer_rejection_with_rollback_outcome(LayoutPersistenceOutcome::RetryArmed(
        "rollback retry remains armed".to_owned(),
    ))
    .await;

    assert!(matches!(
        error,
        LayoutUpdateError::PersistenceRollback(message)
            if message.contains("RetryArmed(\"rollback retry remains armed\")")
    ));
}

#[tokio::test]
async fn renderer_rejection_rejects_superseded_rollback() {
    let error =
        renderer_rejection_with_rollback_outcome(LayoutPersistenceOutcome::Superseded).await;

    assert!(matches!(
        error,
        LayoutUpdateError::PersistenceRollback(message)
            if message.contains("rollback outcome: Superseded")
    ));
}
