use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use arc_swap::ArcSwap;
use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewRuntimeSnapshot {
    pub canvas_receivers: u32,
    pub screen_canvas_receivers: u32,
    pub web_viewport_canvas_receivers: u32,
    pub canvas_frames_published: u64,
    pub screen_canvas_frames_published: u64,
    pub web_viewport_canvas_frames_published: u64,
    pub latest_canvas_frame_number: u32,
    pub latest_canvas_timestamp_ms: u32,
    pub latest_screen_canvas_frame_number: u32,
    pub latest_screen_canvas_timestamp_ms: u32,
    pub latest_web_viewport_canvas_frame_number: u32,
    pub latest_web_viewport_canvas_timestamp_ms: u32,
}

#[derive(Debug, Default)]
struct PreviewRuntimeTelemetry {
    canvas_receivers: Arc<AtomicU32>,
    internal_canvas_receivers: Arc<AtomicU32>,
    screen_canvas_receivers: Arc<AtomicU32>,
    web_viewport_canvas_receivers: Arc<AtomicU32>,
    canvas_frames_published: AtomicU64,
    screen_canvas_frames_published: AtomicU64,
    web_viewport_canvas_frames_published: AtomicU64,
    latest_canvas_frame_number: AtomicU32,
    latest_canvas_timestamp_ms: AtomicU32,
    latest_screen_canvas_frame_number: AtomicU32,
    latest_screen_canvas_timestamp_ms: AtomicU32,
    latest_web_viewport_canvas_frame_number: AtomicU32,
    latest_web_viewport_canvas_timestamp_ms: AtomicU32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PreviewPixelFormat {
    #[default]
    Rgb,
    Rgba,
    Jpeg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewStreamDemand {
    pub fps: u32,
    pub format: PreviewPixelFormat,
    pub width: u32,
    pub height: u32,
}

impl Default for PreviewStreamDemand {
    fn default() -> Self {
        Self {
            fps: 15,
            format: PreviewPixelFormat::Rgb,
            width: 0,
            height: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewDemandSummary {
    pub subscribers: u32,
    pub max_fps: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub any_full_resolution: bool,
    pub any_rgb: bool,
    pub any_rgba: bool,
    pub any_jpeg: bool,
}

#[derive(Debug)]
struct PreviewDemandSummaryState {
    snapshot: ArcSwap<PreviewDemandSummary>,
}

impl Default for PreviewDemandSummaryState {
    fn default() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(PreviewDemandSummary::default()),
        }
    }
}

#[derive(Debug, Default)]
struct PreviewRuntimeDemandState {
    next_subscription_id: AtomicU64,
    canvas: Mutex<Vec<(u64, PreviewStreamDemand)>>,
    internal_canvas: Mutex<Vec<(u64, PreviewStreamDemand)>>,
    screen_canvas: Mutex<Vec<(u64, PreviewStreamDemand)>>,
    web_viewport_canvas: Mutex<Vec<(u64, PreviewStreamDemand)>>,
    canvas_summary: PreviewDemandSummaryState,
    internal_canvas_summary: PreviewDemandSummaryState,
    screen_canvas_summary: PreviewDemandSummaryState,
    web_viewport_canvas_summary: PreviewDemandSummaryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewStreamKind {
    Canvas,
    InternalCanvas,
    ScreenCanvas,
    WebViewportCanvas,
}

#[derive(Debug)]
struct PreviewDemandRegistration {
    kind: PreviewStreamKind,
    id: u64,
    state: Arc<PreviewRuntimeDemandState>,
    demand: PreviewStreamDemand,
}

#[derive(Debug)]
pub struct PreviewFrameReceiver {
    receiver: watch::Receiver<CanvasFrame>,
    counter: Arc<AtomicU32>,
    demand_registration: PreviewDemandRegistration,
}

impl PreviewFrameReceiver {
    fn new(
        receiver: watch::Receiver<CanvasFrame>,
        counter: Arc<AtomicU32>,
        demand_registration: PreviewDemandRegistration,
    ) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self {
            receiver,
            counter,
            demand_registration,
        }
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

    pub fn update_demand(&mut self, demand: PreviewStreamDemand) {
        self.demand_registration.update(demand);
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
    demand_state: Arc<PreviewRuntimeDemandState>,
}

impl PreviewRuntime {
    #[must_use]
    pub fn new(event_bus: Arc<HypercolorBus>) -> Self {
        Self {
            event_bus,
            telemetry: Arc::new(PreviewRuntimeTelemetry {
                canvas_receivers: Arc::new(AtomicU32::new(0)),
                internal_canvas_receivers: Arc::new(AtomicU32::new(0)),
                screen_canvas_receivers: Arc::new(AtomicU32::new(0)),
                web_viewport_canvas_receivers: Arc::new(AtomicU32::new(0)),
                ..PreviewRuntimeTelemetry::default()
            }),
            demand_state: Arc::new(PreviewRuntimeDemandState::default()),
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
            PreviewDemandRegistration::new(
                Arc::clone(&self.demand_state),
                PreviewStreamKind::Canvas,
                PreviewStreamDemand::default(),
            ),
        )
    }

    #[must_use]
    pub fn internal_canvas_receiver(&self, demand: PreviewStreamDemand) -> PreviewFrameReceiver {
        PreviewFrameReceiver::new(
            self.event_bus.canvas_receiver(),
            Arc::clone(&self.telemetry.internal_canvas_receivers),
            PreviewDemandRegistration::new(
                Arc::clone(&self.demand_state),
                PreviewStreamKind::InternalCanvas,
                demand,
            ),
        )
    }

    #[must_use]
    pub fn canvas_receiver_count(&self) -> usize {
        usize::try_from(self.telemetry.canvas_receivers.load(Ordering::Relaxed))
            .unwrap_or(usize::MAX)
    }

    #[must_use]
    pub fn tracked_canvas_receiver_count(&self) -> usize {
        let external = self.telemetry.canvas_receivers.load(Ordering::Relaxed);
        let internal = self
            .telemetry
            .internal_canvas_receivers
            .load(Ordering::Relaxed);
        usize::try_from(external.saturating_add(internal)).unwrap_or(usize::MAX)
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
            PreviewDemandRegistration::new(
                Arc::clone(&self.demand_state),
                PreviewStreamKind::ScreenCanvas,
                PreviewStreamDemand::default(),
            ),
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

    pub fn note_web_viewport_canvas_frame(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .latest_web_viewport_canvas_frame_number
            .store(frame_number, Ordering::Relaxed);
        self.telemetry
            .latest_web_viewport_canvas_timestamp_ms
            .store(timestamp_ms, Ordering::Relaxed);
    }

    pub fn record_web_viewport_canvas_publication(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .web_viewport_canvas_frames_published
            .fetch_add(1, Ordering::Relaxed);
        self.note_web_viewport_canvas_frame(frame_number, timestamp_ms);
    }

    #[must_use]
    pub fn web_viewport_canvas_receiver(&self) -> PreviewFrameReceiver {
        PreviewFrameReceiver::new(
            self.event_bus.web_viewport_canvas_receiver(),
            Arc::clone(&self.telemetry.web_viewport_canvas_receivers),
            PreviewDemandRegistration::new(
                Arc::clone(&self.demand_state),
                PreviewStreamKind::WebViewportCanvas,
                PreviewStreamDemand::default(),
            ),
        )
    }

    #[must_use]
    pub fn web_viewport_canvas_receiver_count(&self) -> usize {
        usize::try_from(
            self.telemetry
                .web_viewport_canvas_receivers
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
            web_viewport_canvas_receivers: self
                .telemetry
                .web_viewport_canvas_receivers
                .load(Ordering::Relaxed),
            canvas_frames_published: self
                .telemetry
                .canvas_frames_published
                .load(Ordering::Relaxed),
            screen_canvas_frames_published: self
                .telemetry
                .screen_canvas_frames_published
                .load(Ordering::Relaxed),
            web_viewport_canvas_frames_published: self
                .telemetry
                .web_viewport_canvas_frames_published
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
            latest_web_viewport_canvas_frame_number: self
                .telemetry
                .latest_web_viewport_canvas_frame_number
                .load(Ordering::Relaxed),
            latest_web_viewport_canvas_timestamp_ms: self
                .telemetry
                .latest_web_viewport_canvas_timestamp_ms
                .load(Ordering::Relaxed),
        }
    }

    #[must_use]
    pub fn canvas_demand(&self) -> PreviewDemandSummary {
        self.demand_state.summary(PreviewStreamKind::Canvas)
    }

    #[must_use]
    pub fn tracked_canvas_demand(&self) -> PreviewDemandSummary {
        merge_preview_demand_summaries(
            self.demand_state.summary(PreviewStreamKind::Canvas),
            self.demand_state.summary(PreviewStreamKind::InternalCanvas),
        )
    }

    #[must_use]
    pub fn screen_canvas_demand(&self) -> PreviewDemandSummary {
        self.demand_state.summary(PreviewStreamKind::ScreenCanvas)
    }

    #[must_use]
    pub fn web_viewport_canvas_demand(&self) -> PreviewDemandSummary {
        self.demand_state
            .summary(PreviewStreamKind::WebViewportCanvas)
    }
}

impl Default for PreviewRuntime {
    fn default() -> Self {
        Self::new(Arc::new(HypercolorBus::new()))
    }
}

impl PreviewRuntimeDemandState {
    fn entries(&self, kind: PreviewStreamKind) -> &Mutex<Vec<(u64, PreviewStreamDemand)>> {
        match kind {
            PreviewStreamKind::Canvas => &self.canvas,
            PreviewStreamKind::InternalCanvas => &self.internal_canvas,
            PreviewStreamKind::ScreenCanvas => &self.screen_canvas,
            PreviewStreamKind::WebViewportCanvas => &self.web_viewport_canvas,
        }
    }

    fn summary_state(&self, kind: PreviewStreamKind) -> &PreviewDemandSummaryState {
        match kind {
            PreviewStreamKind::Canvas => &self.canvas_summary,
            PreviewStreamKind::InternalCanvas => &self.internal_canvas_summary,
            PreviewStreamKind::ScreenCanvas => &self.screen_canvas_summary,
            PreviewStreamKind::WebViewportCanvas => &self.web_viewport_canvas_summary,
        }
    }

    fn register(
        &self,
        kind: PreviewStreamKind,
        id: u64,
        demand: PreviewStreamDemand,
    ) -> PreviewStreamDemand {
        let entries = self.entries(kind);
        let mut entries = entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.push((id, demand));
        store_preview_demand_summary(
            self.summary_state(kind),
            summarize_preview_demands(entries.as_slice()),
        );
        demand
    }

    fn update(&self, kind: PreviewStreamKind, id: u64, demand: PreviewStreamDemand) {
        let entries = self.entries(kind);
        let mut entries = entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, current)) = entries.iter_mut().find(|(entry_id, _)| *entry_id == id) {
            *current = demand;
            store_preview_demand_summary(
                self.summary_state(kind),
                summarize_preview_demands(entries.as_slice()),
            );
        }
    }

    fn unregister(&self, kind: PreviewStreamKind, id: u64) {
        let entries = self.entries(kind);
        let mut entries = entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|(entry_id, _)| *entry_id != id);
        store_preview_demand_summary(
            self.summary_state(kind),
            summarize_preview_demands(entries.as_slice()),
        );
    }

    fn summary(&self, kind: PreviewStreamKind) -> PreviewDemandSummary {
        load_preview_demand_summary(self.summary_state(kind))
    }
}

fn summarize_preview_demands(entries: &[(u64, PreviewStreamDemand)]) -> PreviewDemandSummary {
    let mut summary = PreviewDemandSummary {
        subscribers: u32::try_from(entries.len()).unwrap_or(u32::MAX),
        ..PreviewDemandSummary::default()
    };
    for (_, demand) in entries {
        summary.max_fps = summary.max_fps.max(demand.fps);
        summary.max_width = summary.max_width.max(demand.width);
        summary.max_height = summary.max_height.max(demand.height);
        summary.any_full_resolution |= demand.width == 0 && demand.height == 0;
        summary.any_rgb |= demand.format == PreviewPixelFormat::Rgb;
        summary.any_rgba |= demand.format == PreviewPixelFormat::Rgba;
        summary.any_jpeg |= demand.format == PreviewPixelFormat::Jpeg;
    }
    summary
}

fn merge_preview_demand_summaries(
    external: PreviewDemandSummary,
    internal: PreviewDemandSummary,
) -> PreviewDemandSummary {
    PreviewDemandSummary {
        subscribers: external.subscribers.saturating_add(internal.subscribers),
        max_fps: external.max_fps.max(internal.max_fps),
        max_width: external.max_width.max(internal.max_width),
        max_height: external.max_height.max(internal.max_height),
        any_full_resolution: external.any_full_resolution || internal.any_full_resolution,
        any_rgb: external.any_rgb || internal.any_rgb,
        any_rgba: external.any_rgba || internal.any_rgba,
        any_jpeg: external.any_jpeg || internal.any_jpeg,
    }
}

fn store_preview_demand_summary(state: &PreviewDemandSummaryState, summary: PreviewDemandSummary) {
    state.snapshot.store(Arc::new(summary));
}

fn load_preview_demand_summary(state: &PreviewDemandSummaryState) -> PreviewDemandSummary {
    **state.snapshot.load()
}

impl PreviewDemandRegistration {
    fn new(
        state: Arc<PreviewRuntimeDemandState>,
        kind: PreviewStreamKind,
        demand: PreviewStreamDemand,
    ) -> Self {
        let id = state.next_subscription_id.fetch_add(1, Ordering::Relaxed);
        let demand = state.register(kind, id, demand);
        Self {
            kind,
            id,
            state,
            demand,
        }
    }

    fn update(&mut self, demand: PreviewStreamDemand) {
        if self.demand == demand {
            return;
        }

        self.state.update(self.kind, self.id, demand);
        self.demand = demand;
    }
}

impl Drop for PreviewDemandRegistration {
    fn drop(&mut self) {
        self.state.unregister(self.kind, self.id);
    }
}
