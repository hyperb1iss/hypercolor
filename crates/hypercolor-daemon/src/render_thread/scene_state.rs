use hypercolor_core::spatial::SpatialEngine;

use crate::scene_transactions::{SceneTransaction, SceneTransactionQueue};

#[derive(Debug, Clone)]
pub(crate) struct RenderSceneState {
    spatial_engine: SpatialEngine,
    screen_capture_configured: bool,
}

impl RenderSceneState {
    pub(crate) fn new(spatial_engine: SpatialEngine, screen_capture_configured: bool) -> Self {
        Self {
            spatial_engine,
            screen_capture_configured,
        }
    }

    pub(crate) fn apply_transactions(&mut self, scene_transactions: &SceneTransactionQueue) {
        for transaction in scene_transactions.drain() {
            match transaction {
                SceneTransaction::ReplaceLayout(layout) => {
                    self.spatial_engine.update_layout(layout)
                }
                SceneTransaction::SetScreenCaptureConfigured(configured) => {
                    self.screen_capture_configured = configured;
                }
            }
        }
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
    fn render_scene_state_applies_layout_and_capture_transactions() {
        let queue = SceneTransactionQueue::default();
        let mut scene_state =
            RenderSceneState::new(SpatialEngine::new(test_layout("initial", 320)), false);
        queue.push(SceneTransaction::SetScreenCaptureConfigured(true));
        queue.push(SceneTransaction::ReplaceLayout(test_layout("updated", 640)));

        scene_state.apply_transactions(&queue);

        assert!(scene_state.screen_capture_configured());
        assert_eq!(scene_state.spatial_engine().layout().id, "updated");
        assert_eq!(scene_state.spatial_engine().layout().canvas_width, 640);
    }
}
