//! WebSocket connection manager -- connects to the daemon's streaming endpoint.
//!
//! Handles both JSON events and binary preview frames.

mod connection;
pub mod input;
pub mod interactive_preview;
pub mod messages;
mod preview;

pub use connection::WsManager;
pub use input::{
    InputEdgeButton, InputEdgeScrollPhase, InputEdgeScrollUnit, InputEdgeState, InputInjectEdge,
};
pub use interactive_preview::{InteractivePreviewLifecycle, InteractivePreviewRequest};
pub use messages::{
    AudioLevel, BackpressureNotice, CanvasFrame, CanvasPixelFormat, ControlSurfaceEventHint,
    DeviceEventHint, EffectErrorHint, ExtensionEventHint, InputSourceStatusEventHint,
    MacosDaemonOwnershipEventHint, PerformanceMetrics, SceneEventHint, ScreenZonesFrame,
};
pub use preview::DEFAULT_PREVIEW_FPS_CAP;
