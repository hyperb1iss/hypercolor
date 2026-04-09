use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::RwLock;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::spatial::SpatialLayout;

#[derive(Debug, Clone)]
pub enum SceneTransaction {
    ReplaceLayout(SpatialLayout),
    SetScreenCaptureConfigured(bool),
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
    {
        let mut spatial = spatial_engine.write().await;
        spatial.update_layout(layout.clone());
    }
    scene_transactions.push(SceneTransaction::ReplaceLayout(layout));
}

#[cfg(test)]
mod tests {
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

    use super::{SceneTransaction, SceneTransactionQueue};

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
}
