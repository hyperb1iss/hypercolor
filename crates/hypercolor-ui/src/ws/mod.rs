//! WebSocket connection manager -- connects to the daemon's streaming endpoint.
//!
//! Handles both JSON events and binary preview frames.

mod connection;
pub mod messages;
mod preview;

pub use connection::WsManager;
pub use messages::{
    AudioLevel, BackpressureNotice, CanvasFrame, CanvasPixelFormat, ControlSurfaceEventHint,
    DeviceEventHint, EffectErrorHint, PerformanceMetrics, SceneEventHint,
};
pub use preview::DEFAULT_PREVIEW_FPS_CAP;
