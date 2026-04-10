use leptos::prelude::*;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreviewPresenterTelemetry {
    pub runtime_mode: Option<&'static str>,
    pub present_fps: f32,
    pub arrival_to_present_ms: f64,
    pub skipped_frames: u32,
    pub last_frame_number: Option<u32>,
}

#[derive(Clone, Copy)]
pub struct PreviewTelemetryContext {
    pub presenter: ReadSignal<PreviewPresenterTelemetry>,
    pub set_presenter: WriteSignal<PreviewPresenterTelemetry>,
}
