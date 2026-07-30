use std::sync::Arc;
use std::time::Duration;

use super::{
    LayoutActivationControl, LayoutActivationDecision, LayoutTransactionRejection,
    LayoutUpdateError, PreparedLayoutUpdate, SceneTransaction, SceneTransactionQueue,
    apply_layout_update, apply_prepared_layout_update_under_guard_with_persistence,
};
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::{SpatialEngine, SpatialPlanError};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use tempfile::TempDir;
use tokio::sync::RwLock;

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

fn state(
    initial: SpatialLayout,
) -> (
    Arc<RwLock<SpatialEngine>>,
    Arc<RwLock<SceneManager>>,
    SceneTransactionQueue,
) {
    let spatial_engine =
        SpatialEngine::try_new(initial.clone()).expect("test spatial layout should be addressable");
    (
        Arc::new(RwLock::new(spatial_engine)),
        Arc::new(RwLock::new(SceneManager::with_default_layout(initial))),
        SceneTransactionQueue::default(),
    )
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
    let update_spatial_engine = Arc::clone(&spatial_engine);
    let update_scene_manager = Arc::clone(&scene_manager);
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
    let activation = accept_preparation(transaction);
    complete_commit(&activation).await;
    update
        .await
        .expect("layout coordinator should not panic")
        .expect("layout should commit after renderer acceptance");

    let authoritative = spatial_engine.read().await;
    assert!(Arc::ptr_eq(&renderer_plan, &authoritative.sampling_plan()));
    assert_eq!(authoritative.layout().id, "prepared");
    drop(authoritative);
    let manager = scene_manager.read().await;
    assert_eq!(
        manager
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .expect("default scene should retain a primary group")
            .layout
            .id,
        "prepared"
    );
}

#[tokio::test]
async fn renderer_rejection_preserves_state_and_a_retry_can_commit() {
    let initial = layout("initial", 320, 200);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();

    let rejected_spatial_engine = Arc::clone(&spatial_engine);
    let rejected_scene_manager = Arc::clone(&scene_manager);
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
    assert_eq!(spatial_engine.read().await.layout().as_ref(), &initial);

    let retry_spatial_engine = Arc::clone(&spatial_engine);
    let retry_scene_manager = Arc::clone(&scene_manager);
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
    let activation = accept_preparation(transaction);
    complete_commit(&activation).await;
    retry
        .await
        .expect("layout coordinator should not panic")
        .expect("retry should commit");
    assert_eq!(spatial_engine.read().await.layout().id, "retry");
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
    assert_eq!(spatial_engine.read().await.layout().as_ref(), &initial);
}

#[tokio::test]
async fn persistence_failure_aborts_prepared_renderer_resources() {
    let initial = layout("initial", 320, 200);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let guard = queue.acquire_layout_update_guard().await;
    let prepared = PreparedLayoutUpdate::try_new(layout("candidate", 640, 480))
        .expect("candidate layout should prepare");
    let update_spatial_engine = Arc::clone(&spatial_engine);
    let update_scene_manager = Arc::clone(&scene_manager);
    let update_queue = queue.clone();
    let update = tokio::spawn(async move {
        apply_prepared_layout_update_under_guard_with_persistence(
            update_spatial_engine,
            update_scene_manager,
            update_queue,
            &guard,
            prepared,
            |_| async { anyhow::bail!("synthetic persistence failure") },
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
    assert_eq!(spatial_engine.read().await.layout().as_ref(), &initial);
    let manager_layout = scene_manager.read().await.active_render_groups()[0]
        .layout
        .clone();
    assert_eq!(manager_layout.id, initial.id);
    assert_eq!(manager_layout.canvas_width, initial.canvas_width);
    assert_eq!(manager_layout.canvas_height, initial.canvas_height);
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
    let update_spatial_engine = Arc::clone(&spatial_engine);
    let update_scene_manager = Arc::clone(&scene_manager);
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
                    let state = match phase {
                        super::LayoutPersistencePhase::Commit(state)
                        | super::LayoutPersistencePhase::Rollback(state) => state,
                    };
                    std::fs::write(path, state.layout.id)?;
                    Ok(())
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

    assert_eq!(
        std::fs::read_to_string(&persisted_path).expect("candidate persisted"),
        candidate.id
    );
    activation.complete(Ok(()));
    update
        .await
        .expect("layout coordinator should not panic")
        .expect("armed candidate should commit");
    assert_eq!(spatial_engine.read().await.layout().id, candidate.id);
    assert_eq!(
        scene_manager.read().await.active_render_groups()[0]
            .layout
            .id,
        candidate.id
    );
}

#[tokio::test]
async fn renderer_shutdown_after_persistence_rolls_disk_back_to_live_generation() {
    let initial = layout("initial", 320, 200);
    let candidate = layout("candidate", 7_680, 4_320);
    let (spatial_engine, scene_manager, queue) = state(initial.clone());
    let _consumer = queue.consumer();
    let tempdir = TempDir::new().expect("transaction tempdir");
    let persisted_path = tempdir.path().join("active-layout");
    std::fs::write(&persisted_path, &initial.id).expect("seed persisted generation");
    let guard = queue.acquire_layout_update_guard().await;
    let prepared = PreparedLayoutUpdate::try_new(candidate).expect("candidate prepares");
    let update_spatial_engine = Arc::clone(&spatial_engine);
    let update_scene_manager = Arc::clone(&scene_manager);
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
                    let state = match phase {
                        super::LayoutPersistencePhase::Commit(state)
                        | super::LayoutPersistencePhase::Rollback(state) => state,
                    };
                    std::fs::write(path, state.layout.id)?;
                    Ok(())
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

    assert!(matches!(
        update.await.expect("layout coordinator should not panic"),
        Err(LayoutUpdateError::Transaction(
            LayoutTransactionRejection::RendererStopped
        ))
    ));
    assert_eq!(
        std::fs::read_to_string(&persisted_path).expect("rollback persisted"),
        initial.id
    );
    assert_eq!(spatial_engine.read().await.layout().id, initial.id);
    assert_eq!(
        scene_manager.read().await.active_render_groups()[0]
            .layout
            .id,
        initial.id
    );
}
