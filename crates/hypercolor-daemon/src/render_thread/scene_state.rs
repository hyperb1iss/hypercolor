use hypercolor_core::spatial::SpatialEngine;

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

    pub(crate) fn replace_spatial_engine(&mut self, spatial_engine: SpatialEngine) {
        self.spatial_engine = spatial_engine;
    }

    pub(crate) fn set_screen_capture_configured(&mut self, configured: bool) {
        self.screen_capture_configured = configured;
    }

    pub(crate) fn spatial_engine(&self) -> &SpatialEngine {
        &self.spatial_engine
    }

    pub(crate) fn screen_capture_configured(&self) -> bool {
        self.screen_capture_configured
    }
}
