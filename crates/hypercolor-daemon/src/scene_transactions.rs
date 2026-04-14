use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::RwLock;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::spatial::SpatialLayout;

#[derive(Debug, Clone)]
pub enum SceneTransaction {
    ReplaceLayout(SpatialLayout),
    SetScreenCaptureConfigured(bool),
    ResizeCanvas { width: u32, height: u32 },
}

#[derive(Clone, Default)]
pub struct SceneTransactionQueue {
    inner: Arc<StdMutex<VecDeque<SceneTransaction>>>,
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
    scene_transactions: &SceneTransactionQueue,
    layout: SpatialLayout,
) {
    let canvas_width = layout.canvas_width;
    let canvas_height = layout.canvas_height;
    let needs_resize = {
        let spatial = spatial_engine.read().await;
        let current = spatial.layout();
        current.canvas_width != canvas_width || current.canvas_height != canvas_height
    };
    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout.clone());
    }
    scene_transactions.push(SceneTransaction::ReplaceLayout(layout));
    if needs_resize {
        scene_transactions.push(SceneTransaction::ResizeCanvas {
            width: canvas_width,
            height: canvas_height,
        });
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::RwLock;

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
        queue.push(SceneTransaction::ReplaceLayout(test_layout("updated")));

        let transactions = queue.drain();
        assert_eq!(transactions.len(), 2);
        assert!(matches!(
            transactions.first(),
            Some(SceneTransaction::SetScreenCaptureConfigured(true))
        ));
        assert!(matches!(
            transactions.get(1),
            Some(SceneTransaction::ReplaceLayout(layout)) if layout.id == "updated"
        ));
        assert!(queue.drain().is_empty());
    }

    #[tokio::test]
    async fn apply_layout_update_queues_resize_for_layout_canvas() {
        let queue = SceneTransactionQueue::default();
        let spatial_engine = RwLock::new(SpatialEngine::new(test_layout("initial")));
        let layout = SpatialLayout {
            canvas_width: 640,
            canvas_height: 360,
            ..test_layout("updated")
        };

        apply_layout_update(&spatial_engine, &queue, layout.clone()).await;

        let updated = spatial_engine.read().await.layout().as_ref().clone();
        assert_eq!(updated.id, layout.id);
        assert_eq!(updated.canvas_width, layout.canvas_width);
        assert_eq!(updated.canvas_height, layout.canvas_height);

        let transactions = queue.drain();
        assert_eq!(transactions.len(), 2);
        assert!(matches!(
            transactions.first(),
            Some(SceneTransaction::ReplaceLayout(queued)) if queued.id == layout.id
                && queued.canvas_width == layout.canvas_width
                && queued.canvas_height == layout.canvas_height
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
        let layout = SpatialLayout {
            id: "updated".into(),
            name: "updated".into(),
            ..test_layout("initial")
        };

        apply_layout_update(&spatial_engine, &queue, layout.clone()).await;

        let transactions = queue.drain();
        assert_eq!(transactions.len(), 1);
        assert!(matches!(
            transactions.first(),
            Some(SceneTransaction::ReplaceLayout(queued)) if queued.id == layout.id
        ));
    }
}
