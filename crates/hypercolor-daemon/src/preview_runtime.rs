use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
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
    canvas_receivers: Arc<AtomicU32>,
    screen_canvas_receivers: Arc<AtomicU32>,
    canvas_frames_published: AtomicU64,
    screen_canvas_frames_published: AtomicU64,
    latest_canvas_frame_number: AtomicU32,
    latest_canvas_timestamp_ms: AtomicU32,
    latest_screen_canvas_frame_number: AtomicU32,
    latest_screen_canvas_timestamp_ms: AtomicU32,
}

#[derive(Debug)]
pub struct PreviewFrameReceiver {
    receiver: watch::Receiver<CanvasFrame>,
    counter: Arc<AtomicU32>,
}

impl PreviewFrameReceiver {
    fn new(receiver: watch::Receiver<CanvasFrame>, counter: Arc<AtomicU32>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { receiver, counter }
    }

    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.receiver.changed().await
    }

    pub fn borrow(&self) -> watch::Ref<'_, CanvasFrame> {
        self.receiver.borrow()
    }

    pub fn borrow_and_update(&mut self) -> watch::Ref<'_, CanvasFrame> {
        self.receiver.borrow_and_update()
    }
}

impl Drop for PreviewFrameReceiver {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
pub struct PreviewRuntime {
    event_bus: Arc<HypercolorBus>,
    telemetry: Arc<PreviewRuntimeTelemetry>,
}

impl PreviewRuntime {
    #[must_use]
    pub fn new(event_bus: Arc<HypercolorBus>) -> Self {
        Self {
            event_bus,
            telemetry: Arc::new(PreviewRuntimeTelemetry {
                canvas_receivers: Arc::new(AtomicU32::new(0)),
                screen_canvas_receivers: Arc::new(AtomicU32::new(0)),
                ..PreviewRuntimeTelemetry::default()
            }),
        }
    }

    pub fn note_canvas_frame(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .latest_canvas_frame_number
            .store(frame_number, Ordering::Relaxed);
        self.telemetry
            .latest_canvas_timestamp_ms
            .store(timestamp_ms, Ordering::Relaxed);
    }

    pub fn record_canvas_publication(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .canvas_frames_published
            .fetch_add(1, Ordering::Relaxed);
        self.note_canvas_frame(frame_number, timestamp_ms);
    }

    #[must_use]
    pub fn canvas_receiver(&self) -> PreviewFrameReceiver {
        PreviewFrameReceiver::new(
            self.event_bus.canvas_receiver(),
            Arc::clone(&self.telemetry.canvas_receivers),
        )
    }

    #[must_use]
    pub fn canvas_receiver_count(&self) -> usize {
        usize::try_from(self.telemetry.canvas_receivers.load(Ordering::Relaxed))
            .unwrap_or(usize::MAX)
    }

    pub fn note_screen_canvas_frame(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .latest_screen_canvas_frame_number
            .store(frame_number, Ordering::Relaxed);
        self.telemetry
            .latest_screen_canvas_timestamp_ms
            .store(timestamp_ms, Ordering::Relaxed);
    }

    pub fn record_screen_canvas_publication(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .screen_canvas_frames_published
            .fetch_add(1, Ordering::Relaxed);
        self.note_screen_canvas_frame(frame_number, timestamp_ms);
    }

    #[must_use]
    pub fn screen_canvas_receiver(&self) -> PreviewFrameReceiver {
        PreviewFrameReceiver::new(
            self.event_bus.screen_canvas_receiver(),
            Arc::clone(&self.telemetry.screen_canvas_receivers),
        )
    }

    #[must_use]
    pub fn screen_canvas_receiver_count(&self) -> usize {
        usize::try_from(
            self.telemetry
                .screen_canvas_receivers
                .load(Ordering::Relaxed),
        )
        .unwrap_or(usize::MAX)
    }

    #[must_use]
    pub fn snapshot(&self) -> PreviewRuntimeSnapshot {
        PreviewRuntimeSnapshot {
            canvas_receivers: self.telemetry.canvas_receivers.load(Ordering::Relaxed),
            screen_canvas_receivers: self
                .telemetry
                .screen_canvas_receivers
                .load(Ordering::Relaxed),
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
        Self::new(Arc::new(HypercolorBus::new()))
    }
}
