use hypercolor_core::spatial::SpatialEngine;

use crate::scene_transactions::{SceneTransaction, SceneTransactionQueue};

#[derive(Debug, Clone)]
pub(crate) struct RenderSceneState {
    spatial_engine: SpatialEngine,
    screen_capture_configured: bool,
}

pub(crate) struct PendingSceneTransactions {
    pub(crate) spatial_engine: Option<SpatialEngine>,
    pub(crate) resize: Option<(u32, u32)>,
}

impl RenderSceneState {
    pub(crate) fn new(spatial_engine: SpatialEngine, screen_capture_configured: bool) -> Self {
        Self {
            spatial_engine,
            screen_capture_configured,
        }
    }

    /// Drain queued transactions and return shape-dependent changes for admission.
    pub(crate) fn drain_transactions(
        &mut self,
        scene_transactions: &SceneTransactionQueue,
    ) -> PendingSceneTransactions {
        let mut pending_spatial_engine = None;
        let mut pending_resize = None;
        for transaction in scene_transactions.drain() {
            match transaction {
                SceneTransaction::ReplaceSpatialEngine(spatial_engine) => {
                    pending_spatial_engine = Some(spatial_engine);
                }
                SceneTransaction::SetScreenCaptureConfigured(configured) => {
                    self.screen_capture_configured = configured;
                }
                SceneTransaction::ResizeCanvas { width, height } => {
                    pending_resize = Some((width, height));
                }
            }
        }
        PendingSceneTransactions {
            spatial_engine: pending_spatial_engine,
            resize: pending_resize,
        }
    }

    pub(crate) fn replace_spatial_engine(&mut self, spatial_engine: SpatialEngine) {
        self.spatial_engine = spatial_engine;
    }

    pub(crate) fn spatial_engine(&self) -> &SpatialEngine {
        &self.spatial_engine
    }

    pub(crate) fn screen_capture_configured(&self) -> bool {
        self.screen_capture_configured
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

    use crate::scene_transactions::{SceneTransaction, SceneTransactionQueue};

    use super::RenderSceneState;

    fn test_layout(id: &str, width: u32) -> SpatialLayout {
        SpatialLayout {
            id: id.into(),
            name: id.into(),
            description: None,
            canvas_width: width,
            canvas_height: 200,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    #[test]
    fn render_scene_state_defers_layout_until_resize_admission() {
        let queue = SceneTransactionQueue::default();
        let mut scene_state =
            RenderSceneState::new(SpatialEngine::new(test_layout("initial", 320)), false);
        queue.push(SceneTransaction::SetScreenCaptureConfigured(true));
        queue.push(SceneTransaction::ReplaceSpatialEngine(SpatialEngine::new(
            test_layout("updated", 640),
        )));
        queue.push(SceneTransaction::ResizeCanvas {
            width: 640,
            height: 200,
        });

        let pending = scene_state.drain_transactions(&queue);

        assert!(scene_state.screen_capture_configured());
        assert_eq!(scene_state.spatial_engine().layout().id, "initial");
        assert_eq!(pending.resize, Some((640, 200)));

        scene_state.replace_spatial_engine(
            pending
                .spatial_engine
                .expect("spatial engine should be pending"),
        );

        assert_eq!(scene_state.spatial_engine().layout().id, "updated");
        assert_eq!(scene_state.spatial_engine().layout().canvas_width, 640);
    }
}
