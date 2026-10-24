use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use arc_swap::{ArcSwap, ArcSwapOption};
use hypercolor_core::bus::{
    CanvasFrame, HypercolorBus, PreviewKind, WatchLaneStats, ZonePreviewFrame,
};
use hypercolor_types::scene::{SceneId, Zone, ZoneId};
use tokio::sync::watch;
use uuid::Uuid;

use crate::interactive_preview::InteractivePreviewExecutor;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewLaneSnapshot {
    pub receivers: u32,
    pub frames_published: u64,
    pub revision: u64,
    pub latest_frame_number: u32,
    pub latest_timestamp_ms: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewLaneSnapshots {
    canvas: PreviewLaneSnapshot,
    scene_canvas: PreviewLaneSnapshot,
    screen_canvas: PreviewLaneSnapshot,
    web_viewport_canvas: PreviewLaneSnapshot,
}

impl PreviewLaneSnapshots {
    #[must_use]
    pub const fn get(&self, kind: PreviewKind) -> PreviewLaneSnapshot {
        match kind {
            PreviewKind::Canvas => self.canvas,
            PreviewKind::SceneCanvas => self.scene_canvas,
            PreviewKind::ScreenCanvas => self.screen_canvas,
            PreviewKind::WebViewportCanvas => self.web_viewport_canvas,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewRuntimeSnapshot {
    pub previews: PreviewLaneSnapshots,
    pub zone_preview: PreviewLaneSnapshot,
}

impl PreviewRuntimeSnapshot {
    #[must_use]
    pub const fn preview(&self, kind: PreviewKind) -> PreviewLaneSnapshot {
        self.previews.get(kind)
    }
}

#[derive(Debug, Default)]
struct PreviewObservation {
    latest_frame_number: AtomicU32,
    latest_timestamp_ms: AtomicU32,
}

impl PreviewObservation {
    fn note(&self, frame_number: u32, timestamp_ms: u32) {
        self.latest_frame_number
            .store(frame_number, Ordering::Relaxed);
        self.latest_timestamp_ms
            .store(timestamp_ms, Ordering::Relaxed);
    }

    fn snapshot(&self, stats: WatchLaneStats) -> PreviewLaneSnapshot {
        PreviewLaneSnapshot {
            receivers: u32::try_from(stats.receivers).unwrap_or(u32::MAX),
            frames_published: stats.published,
            revision: stats.revision,
            latest_frame_number: self.latest_frame_number.load(Ordering::Relaxed),
            latest_timestamp_ms: self.latest_timestamp_ms.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Default)]
struct PreviewRuntimeTelemetry {
    canvas: PreviewObservation,
    scene_canvas: PreviewObservation,
    screen_canvas: PreviewObservation,
    web_viewport_canvas: PreviewObservation,
    zone_preview: PreviewObservation,
    /// Frame number of the first frame after the render loop last woke
    /// from a park, tagged with [`RENDER_RESUME_FRAME_SET`]; zero until the
    /// loop first wakes.
    render_resume_frame: AtomicU64,
}

const RENDER_RESUME_FRAME_SET: u64 = 1 << 32;

impl PreviewRuntimeTelemetry {
    fn preview(&self, kind: PreviewKind) -> &PreviewObservation {
        match kind {
            PreviewKind::Canvas => &self.canvas,
            PreviewKind::SceneCanvas => &self.scene_canvas,
            PreviewKind::ScreenCanvas => &self.screen_canvas,
            PreviewKind::WebViewportCanvas => &self.web_viewport_canvas,
        }
    }
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

/// Which scene zones a zone-preview subscriber wants delivered.
///
/// Subscribers name zones on the wire (`zones` on `zone_preview`), and the
/// render thread stages, reads back, and copies only the union of what
/// every subscriber names, so a zone nobody previews costs no preview
/// work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ZonePreviewSelection {
    /// Every zone of the live scene.
    #[default]
    All,
    /// Only these zones, sorted and deduplicated. Empty selects nothing.
    Zones(Arc<[ZoneId]>),
}

impl ZonePreviewSelection {
    /// The selection a wire `zones` list names. `"all"` anywhere selects
    /// every zone; entries that are not zone ids select nothing, the same
    /// way the `frames` topic treats names that match no zone.
    #[must_use]
    pub fn from_wire(zones: &[String]) -> Self {
        if zones.iter().any(|zone| zone == "all") {
            return Self::All;
        }
        Self::from_zone_ids(
            zones
                .iter()
                .filter_map(|zone| Uuid::parse_str(zone.trim()).ok())
                .map(ZoneId),
        )
    }

    /// A selection of exactly these zones.
    #[must_use]
    pub fn from_zone_ids(zones: impl IntoIterator<Item = ZoneId>) -> Self {
        let mut zones = zones.into_iter().collect::<Vec<_>>();
        zones.sort_unstable_by_key(|zone| zone.0);
        zones.dedup();
        Self::Zones(zones.into())
    }

    /// Whether the selection delivers `zone_id`.
    #[must_use]
    pub fn includes(&self, zone_id: &ZoneId) -> bool {
        match self {
            Self::All => true,
            Self::Zones(zones) => zones
                .binary_search_by_key(&zone_id.0, |zone| zone.0)
                .is_ok(),
        }
    }

    /// Whether the selection names every zone.
    #[must_use]
    pub const fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    /// The smallest selection covering every one of `selections`.
    fn union<'a>(selections: impl IntoIterator<Item = &'a Self>) -> Self {
        let mut zones = Vec::new();
        for selection in selections {
            match selection {
                Self::All => return Self::All,
                Self::Zones(selected) => zones.extend(selected.iter().copied()),
            }
        }
        Self::from_zone_ids(zones)
    }
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
    scene_canvas: Mutex<Vec<(u64, PreviewStreamDemand)>>,
    screen_canvas: Mutex<Vec<(u64, PreviewStreamDemand)>>,
    web_viewport_canvas: Mutex<Vec<(u64, PreviewStreamDemand)>>,
    zone_preview: Mutex<Vec<(u64, PreviewStreamDemand)>>,
    canvas_summary: PreviewDemandSummaryState,
    internal_canvas_summary: PreviewDemandSummaryState,
    scene_canvas_summary: PreviewDemandSummaryState,
    screen_canvas_summary: PreviewDemandSummaryState,
    web_viewport_canvas_summary: PreviewDemandSummaryState,
    zone_preview_summary: PreviewDemandSummaryState,
    zone_preview_selections: Mutex<Vec<(u64, ZonePreviewSelection)>>,
    zone_preview_selection_union: ZonePreviewSelectionState,
}

#[derive(Debug)]
struct ZonePreviewSelectionState {
    union: ArcSwap<ZonePreviewSelection>,
}

impl Default for ZonePreviewSelectionState {
    fn default() -> Self {
        Self {
            union: ArcSwap::from_pointee(ZonePreviewSelection::from_zone_ids([])),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewStreamKind {
    Canvas,
    InternalCanvas,
    SceneCanvas,
    ScreenCanvas,
    WebViewportCanvas,
    ZonePreview,
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
    demand_registration: PreviewDemandRegistration,
}

#[derive(Debug)]
pub struct ZonePreviewFrameReceiver {
    receiver: watch::Receiver<Arc<[ZonePreviewFrame]>>,
    demand_registration: PreviewDemandRegistration,
    selection_registration: ZonePreviewSelectionRegistration,
}

/// One subscriber's zone selection, held for as long as its receiver
/// lives. Dropping it removes the selection from the union.
#[derive(Debug)]
struct ZonePreviewSelectionRegistration {
    id: u64,
    state: Arc<PreviewRuntimeDemandState>,
    selection: ZonePreviewSelection,
}

impl PreviewFrameReceiver {
    fn new(
        receiver: watch::Receiver<CanvasFrame>,
        demand_registration: PreviewDemandRegistration,
    ) -> Self {
        Self {
            receiver,
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

    pub fn has_changed(&self) -> Result<bool, watch::error::RecvError> {
        self.receiver.has_changed()
    }

    pub fn update_demand(&mut self, demand: PreviewStreamDemand) {
        self.demand_registration.update(demand);
    }
}

impl ZonePreviewFrameReceiver {
    /// Registers the demand and the selection before subscribing to the
    /// lane: the render thread treats a bus receiver it cannot match to a
    /// registration as one that wants every zone at full size.
    fn new(
        event_bus: &HypercolorBus,
        demand_registration: PreviewDemandRegistration,
        selection: ZonePreviewSelection,
    ) -> Self {
        let selection_registration = ZonePreviewSelectionRegistration::new(
            Arc::clone(&demand_registration.state),
            demand_registration.id,
            selection,
        );
        Self {
            receiver: event_bus.zone_preview_receiver(),
            demand_registration,
            selection_registration,
        }
    }

    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.receiver.changed().await
    }

    pub fn borrow(&self) -> watch::Ref<'_, Arc<[ZonePreviewFrame]>> {
        self.receiver.borrow()
    }

    pub fn borrow_and_update(&mut self) -> watch::Ref<'_, Arc<[ZonePreviewFrame]>> {
        self.receiver.borrow_and_update()
    }

    pub fn update_demand(&mut self, demand: PreviewStreamDemand) {
        self.demand_registration.update(demand);
    }

    /// Narrow or widen the zones this subscriber receives. A receiver from
    /// [`PreviewRuntime::zone_preview_receiver`] selects every zone until
    /// its first update.
    pub fn update_selection(&mut self, selection: &ZonePreviewSelection) {
        self.selection_registration.update(selection);
    }
}

/// Test seam: one armed hold on the render thread's next frame publication.
#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug, Default)]
struct FramePublicationHoldSlot {
    armed: std::sync::atomic::AtomicBool,
    gate: Mutex<Option<FramePublicationGate>>,
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug)]
struct FramePublicationGate {
    reached: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

/// A test's handle on one held frame publication. Dropping it releases the
/// frame, so a failing test never leaves the render thread wedged.
#[cfg(feature = "persistence-test-hooks")]
#[doc(hidden)]
#[derive(Debug)]
pub struct FramePublicationHold {
    reached: Option<tokio::sync::oneshot::Receiver<()>>,
    release: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(feature = "persistence-test-hooks")]
impl FramePublicationHold {
    /// Wait until the render thread holds a rendered frame in front of its
    /// publication.
    pub async fn reached(&mut self) {
        if let Some(reached) = self.reached.take() {
            let _ = reached.await;
        }
    }

    /// Let the held frame publish.
    pub fn release(self) {
        drop(self);
    }
}

#[cfg(feature = "persistence-test-hooks")]
impl Drop for FramePublicationHold {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreviewRuntime {
    event_bus: Arc<HypercolorBus>,
    telemetry: Arc<PreviewRuntimeTelemetry>,
    demand_state: Arc<PreviewRuntimeDemandState>,
    interactive_executor: Arc<ArcSwapOption<InteractivePreviewExecutor>>,
    #[cfg(feature = "persistence-test-hooks")]
    publication_hold: Arc<FramePublicationHoldSlot>,
}

impl PreviewRuntime {
    #[must_use]
    pub fn new(event_bus: Arc<HypercolorBus>) -> Self {
        Self {
            event_bus,
            telemetry: Arc::new(PreviewRuntimeTelemetry::default()),
            demand_state: Arc::new(PreviewRuntimeDemandState::default()),
            interactive_executor: Arc::new(ArcSwapOption::empty()),
            #[cfg(feature = "persistence-test-hooks")]
            publication_hold: Arc::default(),
        }
    }

    /// Hold the next frame the render thread renders while awake just
    /// before it publishes, so a test can land an output hold while that
    /// frame is in flight. The frame has already written its outputs.
    #[cfg(feature = "persistence-test-hooks")]
    #[doc(hidden)]
    #[must_use]
    pub fn hold_next_frame_publication(&self) -> FramePublicationHold {
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self
            .publication_hold
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(FramePublicationGate {
            reached: reached_tx,
            release: release_rx,
        });
        self.publication_hold.armed.store(true, Ordering::Release);
        FramePublicationHold {
            reached: Some(reached_rx),
            release: Some(release_tx),
        }
    }

    /// Wait out a hold armed by [`Self::hold_next_frame_publication`].
    #[cfg(feature = "persistence-test-hooks")]
    pub(crate) async fn pass_frame_publication_hold(&self) {
        if !self.publication_hold.armed.swap(false, Ordering::AcqRel) {
            return;
        }
        let gate = self
            .publication_hold
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(gate) = gate else {
            return;
        };
        let _ = gate.reached.send(());
        let _ = gate.release.await;
    }

    pub fn install_interactive_executor(&self, executor: Arc<InteractivePreviewExecutor>) {
        self.interactive_executor.store(Some(executor));
    }

    #[must_use]
    pub fn interactive_executor(&self) -> Option<Arc<InteractivePreviewExecutor>> {
        self.interactive_executor.load_full()
    }

    pub fn clear_interactive_executor(&self) {
        self.interactive_executor.store(None);
    }

    /// Record the frame number the render loop renders first after waking
    /// from an output-demand park.
    pub fn note_render_resume(&self, frame_number: u32) {
        self.telemetry.render_resume_frame.store(
            RENDER_RESUME_FRAME_SET | u64::from(frame_number),
            Ordering::Release,
        );
    }

    /// Whether `frame_number` is the first frame after the most recent wake
    /// from a park.
    ///
    /// Asynchronous producers deliver one tick late, so that frame composes
    /// producer output requested before the park. Consumers that need output
    /// rendered after they asked, such as canvas captures, wait one more
    /// frame.
    #[must_use]
    pub fn is_render_resume_frame(&self, frame_number: u32) -> bool {
        self.telemetry.render_resume_frame.load(Ordering::Acquire)
            == RENDER_RESUME_FRAME_SET | u64::from(frame_number)
    }

    pub fn note_canvas_frame(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .preview(PreviewKind::Canvas)
            .note(frame_number, timestamp_ms);
    }

    #[must_use]
    pub fn canvas_receiver(&self) -> PreviewFrameReceiver {
        PreviewFrameReceiver::new(
            self.event_bus.canvas_receiver(),
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
            PreviewDemandRegistration::new(
                Arc::clone(&self.demand_state),
                PreviewStreamKind::InternalCanvas,
                demand,
            ),
        )
    }

    #[must_use]
    pub fn canvas_receiver_count(&self) -> usize {
        usize::try_from(self.canvas_demand().subscribers).unwrap_or(usize::MAX)
    }

    #[must_use]
    pub fn tracked_canvas_receiver_count(&self) -> usize {
        usize::try_from(self.tracked_canvas_demand().subscribers).unwrap_or(usize::MAX)
    }

    pub fn note_scene_canvas_frame(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .preview(PreviewKind::SceneCanvas)
            .note(frame_number, timestamp_ms);
    }

    #[must_use]
    pub fn scene_canvas_receiver(&self) -> PreviewFrameReceiver {
        PreviewFrameReceiver::new(
            self.event_bus.scene_canvas_receiver(),
            PreviewDemandRegistration::new(
                Arc::clone(&self.demand_state),
                PreviewStreamKind::SceneCanvas,
                PreviewStreamDemand {
                    fps: 60,
                    ..PreviewStreamDemand::default()
                },
            ),
        )
    }

    #[must_use]
    pub fn scene_canvas_receiver_count(&self) -> usize {
        usize::try_from(self.scene_canvas_demand().subscribers).unwrap_or(usize::MAX)
    }

    pub fn note_screen_canvas_frame(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .preview(PreviewKind::ScreenCanvas)
            .note(frame_number, timestamp_ms);
    }

    #[must_use]
    pub fn screen_canvas_receiver(&self) -> PreviewFrameReceiver {
        PreviewFrameReceiver::new(
            self.event_bus.screen_canvas_receiver(),
            PreviewDemandRegistration::new(
                Arc::clone(&self.demand_state),
                PreviewStreamKind::ScreenCanvas,
                PreviewStreamDemand::default(),
            ),
        )
    }

    #[must_use]
    pub fn screen_canvas_receiver_count(&self) -> usize {
        usize::try_from(self.screen_canvas_demand().subscribers).unwrap_or(usize::MAX)
    }

    /// Subscribe to ambilight screen-zone frames straight from the bus.
    ///
    /// Receiver-count bookkeeping happens on the bus watch itself; capture
    /// demand reads `screen_zones_receiver_count` there.
    #[must_use]
    pub fn screen_zones_receiver(
        &self,
    ) -> tokio::sync::watch::Receiver<hypercolor_core::bus::ScreenZonesFrame> {
        self.event_bus.screen_zones_receiver()
    }

    pub fn note_web_viewport_canvas_frame(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry
            .preview(PreviewKind::WebViewportCanvas)
            .note(frame_number, timestamp_ms);
    }

    #[must_use]
    pub fn web_viewport_canvas_receiver(&self) -> PreviewFrameReceiver {
        PreviewFrameReceiver::new(
            self.event_bus.web_viewport_canvas_receiver(),
            PreviewDemandRegistration::new(
                Arc::clone(&self.demand_state),
                PreviewStreamKind::WebViewportCanvas,
                PreviewStreamDemand::default(),
            ),
        )
    }

    #[must_use]
    pub fn web_viewport_canvas_receiver_count(&self) -> usize {
        usize::try_from(self.web_viewport_canvas_demand().subscribers).unwrap_or(usize::MAX)
    }

    pub fn note_zone_preview_frame(&self, frame_number: u32, timestamp_ms: u32) {
        self.telemetry.zone_preview.note(frame_number, timestamp_ms);
    }

    #[must_use]
    pub fn zone_preview_receiver(&self) -> ZonePreviewFrameReceiver {
        self.selected_zone_preview_receiver(
            ZonePreviewSelection::All,
            PreviewStreamDemand::default(),
        )
    }

    /// A zone-preview receiver that registers `selection` at `demand`, for a
    /// subscriber that knows its zones up front. Unlike a receiver narrowed
    /// after the fact, it never asks the render thread for every zone at
    /// full size while its first update is on the way.
    #[must_use]
    pub fn selected_zone_preview_receiver(
        &self,
        selection: ZonePreviewSelection,
        demand: PreviewStreamDemand,
    ) -> ZonePreviewFrameReceiver {
        ZonePreviewFrameReceiver::new(
            &self.event_bus,
            PreviewDemandRegistration::new(
                Arc::clone(&self.demand_state),
                PreviewStreamKind::ZonePreview,
                demand,
            ),
            selection,
        )
    }

    #[must_use]
    pub fn zone_preview_receiver_count(&self) -> usize {
        usize::try_from(self.zone_preview_demand().subscribers).unwrap_or(usize::MAX)
    }

    /// Show one static frame as every scene zone's preview: the frame a
    /// sleep, pause, or stop puts on every output. Those holds publish once
    /// and then stop rendering, so this runs outside the preview cadence,
    /// which could skip the one publication and leave the last live zone
    /// previews up while the outputs sit dark. It covers every zone rather
    /// than the selected ones: each entry shares the one static surface, so
    /// it costs no pixel work, and a subscriber that arrives or retargets
    /// during the hold still finds its zone dark.
    pub fn publish_static_zone_previews(
        &self,
        scene_id: Option<SceneId>,
        zones: &[Zone],
        frame: &CanvasFrame,
    ) {
        let Some(scene_id) = scene_id else {
            return;
        };
        let previews = zones
            .iter()
            .map(|zone| ZonePreviewFrame {
                scene_id,
                zone_id: zone.id,
                frame: frame.clone(),
            })
            .collect::<Vec<_>>();
        let published = u64::try_from(previews.len()).unwrap_or(u64::MAX);
        self.event_bus
            .zone_preview_lane()
            .send_replace_weighted(previews.into(), published);
    }

    /// The union of every tracked zone-preview subscriber's selection.
    /// Selects nothing while no tracked subscriber exists; a caller that
    /// also sees untracked bus receivers must treat them as selecting
    /// every zone.
    #[must_use]
    pub fn zone_preview_selection(&self) -> ZonePreviewSelection {
        ZonePreviewSelection::clone(&self.demand_state.zone_preview_selection_union.union.load())
    }

    #[must_use]
    pub fn snapshot(&self) -> PreviewRuntimeSnapshot {
        let preview = |kind| {
            self.telemetry
                .preview(kind)
                .snapshot(self.event_bus.lanes().preview(kind).stats())
        };
        PreviewRuntimeSnapshot {
            previews: PreviewLaneSnapshots {
                canvas: preview(PreviewKind::Canvas),
                scene_canvas: preview(PreviewKind::SceneCanvas),
                screen_canvas: preview(PreviewKind::ScreenCanvas),
                web_viewport_canvas: preview(PreviewKind::WebViewportCanvas),
            },
            zone_preview: self
                .telemetry
                .zone_preview
                .snapshot(self.event_bus.lanes().zone_preview().stats()),
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
    pub fn scene_canvas_demand(&self) -> PreviewDemandSummary {
        self.demand_state.summary(PreviewStreamKind::SceneCanvas)
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

    #[must_use]
    pub fn zone_preview_demand(&self) -> PreviewDemandSummary {
        self.demand_state.summary(PreviewStreamKind::ZonePreview)
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
            PreviewStreamKind::SceneCanvas => &self.scene_canvas,
            PreviewStreamKind::ScreenCanvas => &self.screen_canvas,
            PreviewStreamKind::WebViewportCanvas => &self.web_viewport_canvas,
            PreviewStreamKind::ZonePreview => &self.zone_preview,
        }
    }

    fn summary_state(&self, kind: PreviewStreamKind) -> &PreviewDemandSummaryState {
        match kind {
            PreviewStreamKind::Canvas => &self.canvas_summary,
            PreviewStreamKind::InternalCanvas => &self.internal_canvas_summary,
            PreviewStreamKind::SceneCanvas => &self.scene_canvas_summary,
            PreviewStreamKind::ScreenCanvas => &self.screen_canvas_summary,
            PreviewStreamKind::WebViewportCanvas => &self.web_viewport_canvas_summary,
            PreviewStreamKind::ZonePreview => &self.zone_preview_summary,
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

    fn register_zone_selection(&self, id: u64, selection: ZonePreviewSelection) {
        let mut selections = self
            .zone_preview_selections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        selections.push((id, selection));
        self.store_zone_selection_union(&selections);
    }

    fn update_zone_selection(&self, id: u64, selection: &ZonePreviewSelection) {
        let mut selections = self
            .zone_preview_selections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, current)) = selections.iter_mut().find(|(entry_id, _)| *entry_id == id) {
            current.clone_from(selection);
            self.store_zone_selection_union(&selections);
        }
    }

    fn unregister_zone_selection(&self, id: u64) {
        let mut selections = self
            .zone_preview_selections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        selections.retain(|(entry_id, _)| *entry_id != id);
        self.store_zone_selection_union(&selections);
    }

    fn store_zone_selection_union(&self, selections: &[(u64, ZonePreviewSelection)]) {
        let union = ZonePreviewSelection::union(selections.iter().map(|(_, selection)| selection));
        self.zone_preview_selection_union
            .union
            .store(Arc::new(union));
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

impl ZonePreviewSelectionRegistration {
    fn new(
        state: Arc<PreviewRuntimeDemandState>,
        id: u64,
        selection: ZonePreviewSelection,
    ) -> Self {
        state.register_zone_selection(id, selection.clone());
        Self {
            id,
            state,
            selection,
        }
    }

    fn update(&mut self, selection: &ZonePreviewSelection) {
        if self.selection == *selection {
            return;
        }
        self.state.update_zone_selection(self.id, selection);
        self.selection.clone_from(selection);
    }
}

impl Drop for ZonePreviewSelectionRegistration {
    fn drop(&mut self) {
        self.state.unregister_zone_selection(self.id);
    }
}
