use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use hypercolor_core::bus::CanvasFrame;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewRuntimeSnapshot {
    pub canvas_receivers: u32,
    pub screen_canvas_receivers: u32,
    pub canvas_frames_published: u64,
    pub screen_canvas_frames_published: u64,
    pub latest_canvas_frame_number: u32,
    pub latest_canvas_timestamp_ms: u32,
    pub latest_screen_canvas_frame_number: u32,
    pub latest_screen_canvas_timestamp_ms: u32,
}

#[derive(Debug, Default)]
struct PreviewRuntimeTelemetry {
    canvas_frames_published: AtomicU64,
    screen_canvas_frames_published: AtomicU64,
    latest_canvas_frame_number: AtomicU32,
    latest_canvas_timestamp_ms: AtomicU32,
    latest_screen_canvas_frame_number: AtomicU32,
    latest_screen_canvas_timestamp_ms: AtomicU32,
}

#[derive(Clone, Debug)]
pub struct PreviewRuntime {
    canvas: watch::Sender<CanvasFrame>,
    screen_canvas: watch::Sender<CanvasFrame>,
    telemetry: Arc<PreviewRuntimeTelemetry>,
}

impl PreviewRuntime {
    #[must_use]
    pub fn new() -> Self {
        let (canvas, _) = watch::channel(CanvasFrame::empty());
        let (screen_canvas, _) = watch::channel(CanvasFrame::empty());
        Self {
            canvas,
            screen_canvas,
            telemetry: Arc::new(PreviewRuntimeTelemetry::default()),
        }
    }

    pub fn publish_canvas(&self, frame: CanvasFrame) {
        self.telemetry
            .canvas_frames_published
            .fetch_add(1, Ordering::Relaxed);
        self.telemetry
            .latest_canvas_frame_number
            .store(frame.frame_number, Ordering::Relaxed);
        self.telemetry
            .latest_canvas_timestamp_ms
            .store(frame.timestamp_ms, Ordering::Relaxed);
        let _ = self.canvas.send(frame);
    }

    #[must_use]
    pub fn canvas_receiver(&self) -> watch::Receiver<CanvasFrame> {
        self.canvas.subscribe()
    }

    #[must_use]
    pub fn canvas_receiver_count(&self) -> usize {
        self.canvas.receiver_count()
    }

    pub fn publish_screen_canvas(&self, frame: CanvasFrame) {
        self.telemetry
            .screen_canvas_frames_published
            .fetch_add(1, Ordering::Relaxed);
        self.telemetry
            .latest_screen_canvas_frame_number
            .store(frame.frame_number, Ordering::Relaxed);
        self.telemetry
            .latest_screen_canvas_timestamp_ms
            .store(frame.timestamp_ms, Ordering::Relaxed);
        let _ = self.screen_canvas.send(frame);
    }

    #[must_use]
    pub fn screen_canvas_receiver(&self) -> watch::Receiver<CanvasFrame> {
        self.screen_canvas.subscribe()
    }

    #[must_use]
    pub fn screen_canvas_receiver_count(&self) -> usize {
        self.screen_canvas.receiver_count()
    }

    #[must_use]
    pub fn snapshot(&self) -> PreviewRuntimeSnapshot {
        PreviewRuntimeSnapshot {
            canvas_receivers: u32::try_from(self.canvas.receiver_count()).unwrap_or(u32::MAX),
            screen_canvas_receivers: u32::try_from(self.screen_canvas.receiver_count())
                .unwrap_or(u32::MAX),
            canvas_frames_published: self
                .telemetry
                .canvas_frames_published
                .load(Ordering::Relaxed),
            screen_canvas_frames_published: self
                .telemetry
                .screen_canvas_frames_published
                .load(Ordering::Relaxed),
            latest_canvas_frame_number: self
                .telemetry
                .latest_canvas_frame_number
                .load(Ordering::Relaxed),
            latest_canvas_timestamp_ms: self
                .telemetry
                .latest_canvas_timestamp_ms
                .load(Ordering::Relaxed),
            latest_screen_canvas_frame_number: self
                .telemetry
                .latest_screen_canvas_frame_number
                .load(Ordering::Relaxed),
            latest_screen_canvas_timestamp_ms: self
                .telemetry
                .latest_screen_canvas_timestamp_ms
                .load(Ordering::Relaxed),
        }
    }
}

impl Default for PreviewRuntime {
    fn default() -> Self {
        Self::new()
    }
}
