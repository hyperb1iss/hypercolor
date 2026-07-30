use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{Mutex, RwLock};

use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::{SpatialEngine, SpatialPlanError};
use hypercolor_types::spatial::SpatialLayout;

#[derive(Debug, Clone)]
pub enum SceneTransaction {
    ReplaceSpatialEngine(SpatialEngine),
    SetScreenCaptureConfigured(bool),
    ResizeCanvas { width: u32, height: u32 },
}

#[derive(Clone, Default)]
pub struct SceneTransactionQueue {
    inner: Arc<StdMutex<VecDeque<SceneTransaction>>>,
    layout_update_lock: Arc<Mutex<()>>,
}

impl SceneTransactionQueue {
    pub fn push(&self, transaction: SceneTransaction) {
        self.inner
            .lock()
            .expect("scene transaction queue should lock")
            .push_back(transaction);
    }

    #[must_use]
    pub fn drain(&self) -> Vec<SceneTransaction> {
        self.inner
            .lock()
            .expect("scene transaction queue should lock")
            .drain(..)
            .collect()
    }
}

pub async fn apply_layout_update(
    spatial_engine: &RwLock<SpatialEngine>,
    scene_manager: &RwLock<SceneManager>,
    scene_transactions: &SceneTransactionQueue,
    layout: SpatialLayout,
) -> Result<(), SpatialPlanError> {
    let _transaction = scene_transactions.layout_update_lock.lock().await;
    let canvas_width = layout.canvas_width;
    let canvas_height = layout.canvas_height;
    let (prepared_engine, needs_resize) = {
        let mut spatial = spatial_engine.write().await;
        let current = spatial.layout();
        let needs_resize =
            current.canvas_width != canvas_width || current.canvas_height != canvas_height;
        spatial.try_update_layout(layout.clone())?;
        (spatial.clone(), needs_resize)
    };
    {
        let mut manager = scene_manager.write().await;
        manager.sync_primary_group_layout(&layout);
    }
    scene_transactions.push(SceneTransaction::ReplaceSpatialEngine(prepared_engine));
    if needs_resize {
        scene_transactions.push(SceneTransaction::ResizeCanvas {
            width: canvas_width,
            height: canvas_height,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::RwLock;

    use hypercolor_core::scene::SceneManager;
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

    use super::{SceneTransaction, SceneTransactionQueue, apply_layout_update};

    fn test_layout(id: &str) -> SpatialLayout {
        SpatialLayout {
            id: id.into(),
            name: id.into(),
            description: None,
            canvas_width: 320,
            canvas_height: 200,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    #[test]
    fn scene_transaction_queue_drains_in_submission_order() {
        let queue = SceneTransactionQueue::default();
        queue.push(SceneTransaction::SetScreenCaptureConfigured(true));
        queue.push(SceneTransaction::ReplaceSpatialEngine(SpatialEngine::new(
            test_layout("updated"),
        )));

        let transactions = queue.drain();
        assert_eq!(transactions.len(), 2);
        assert!(matches!(
            transactions.first(),
            Some(SceneTransaction::SetScreenCaptureConfigured(true))
        ));
        assert!(matches!(
            transactions.get(1),
            Some(SceneTransaction::ReplaceSpatialEngine(engine))
                if engine.layout().id == "updated"
        ));
        assert!(queue.drain().is_empty());
    }

    #[tokio::test]
    async fn apply_layout_update_queues_resize_for_layout_canvas() {
        let queue = SceneTransactionQueue::default();
        let spatial_engine = RwLock::new(SpatialEngine::new(test_layout("initial")));
        let scene_manager = RwLock::new(SceneManager::with_default());
        let layout = SpatialLayout {
            canvas_width: 640,
            canvas_height: 360,
            ..test_layout("updated")
        };

        apply_layout_update(&spatial_engine, &scene_manager, &queue, layout.clone())
            .await
            .expect("valid layout should apply");

        let updated = spatial_engine.read().await.layout().as_ref().clone();
        assert_eq!(updated.id, layout.id);
        assert_eq!(updated.canvas_width, layout.canvas_width);
        assert_eq!(updated.canvas_height, layout.canvas_height);

        let transactions = queue.drain();
        assert_eq!(transactions.len(), 2);
        assert!(matches!(
            transactions.first(),
            Some(SceneTransaction::ReplaceSpatialEngine(engine))
                if engine.layout().id == layout.id
                    && engine.layout().canvas_width == layout.canvas_width
                    && engine.layout().canvas_height == layout.canvas_height
        ));
        assert!(matches!(
            transactions.get(1),
            Some(SceneTransaction::ResizeCanvas { width, height })
                if *width == layout.canvas_width && *height == layout.canvas_height
        ));
    }

    #[tokio::test]
    async fn apply_layout_update_skips_resize_when_canvas_dimensions_match() {
        let queue = SceneTransactionQueue::default();
        let spatial_engine = RwLock::new(SpatialEngine::new(test_layout("initial")));
        let scene_manager = RwLock::new(SceneManager::with_default());
        let layout = SpatialLayout {
            id: "updated".into(),
            name: "updated".into(),
            ..test_layout("initial")
        };

        apply_layout_update(&spatial_engine, &scene_manager, &queue, layout.clone())
            .await
            .expect("valid layout should apply");

        let transactions = queue.drain();
        assert_eq!(transactions.len(), 1);
        assert!(matches!(
            transactions.first(),
            Some(SceneTransaction::ReplaceSpatialEngine(engine))
                if engine.layout().id == layout.id
        ));
    }

    #[tokio::test]
    async fn rejected_layout_preserves_spatial_scene_and_transaction_state() {
        let queue = SceneTransactionQueue::default();
        let initial = test_layout("initial");
        let spatial_engine = RwLock::new(SpatialEngine::new(initial.clone()));
        let scene_manager = RwLock::new(SceneManager::with_default());
        let primary_layout_before = scene_manager
            .read()
            .await
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .map(|group| group.layout.clone());
        let mut invalid = test_layout("invalid");
        invalid.canvas_width = u32::MAX;
        invalid.canvas_height = u32::MAX;

        let result = apply_layout_update(&spatial_engine, &scene_manager, &queue, invalid).await;

        assert!(result.is_err());
        assert_eq!(spatial_engine.read().await.layout().as_ref(), &initial);
        assert_eq!(
            scene_manager
                .read()
                .await
                .active_scene()
                .and_then(|scene| scene.primary_group())
                .map(|group| group.layout.clone()),
            primary_layout_before
        );
        assert!(queue.drain().is_empty());
    }

    #[tokio::test]
    async fn concurrent_layout_updates_keep_engine_scene_and_queue_ordered() {
        let queue = SceneTransactionQueue::default();
        let spatial_engine = Arc::new(RwLock::new(SpatialEngine::new(test_layout("initial"))));
        let scene_manager = Arc::new(RwLock::new(SceneManager::with_default()));
        let first = apply_layout_update(
            &spatial_engine,
            &scene_manager,
            &queue,
            test_layout("first"),
        );
        let second = apply_layout_update(
            &spatial_engine,
            &scene_manager,
            &queue,
            test_layout("second"),
        );

        let (first_result, second_result) = tokio::join!(first, second);

        first_result.expect("first layout should apply");
        second_result.expect("second layout should apply");
        let active_id = spatial_engine.read().await.layout().id.clone();
        let scene_id = scene_manager
            .read()
            .await
            .active_scene()
            .and_then(|scene| scene.primary_group())
            .map(|group| group.layout.id.clone());
        let queued_id = queue.drain().into_iter().rev().find_map(|transaction| {
            let SceneTransaction::ReplaceSpatialEngine(engine) = transaction else {
                return None;
            };
            Some(engine.layout().id.clone())
        });

        assert_eq!(scene_id.as_deref(), Some(active_id.as_str()));
        assert_eq!(queued_id.as_deref(), Some(active_id.as_str()));
    }
}
