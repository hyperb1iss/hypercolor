#[cfg(feature = "servo-gpu-import")]
use hypercolor_core::effect::ImportedEffectFrame;
#[cfg(all(feature = "wgpu", target_os = "macos", feature = "screen-capture"))]
use hypercolor_core::input::screen::PlatformGpuSurfaceOwner;
#[cfg(all(
    feature = "wgpu",
    any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    )
))]
use hypercolor_core::input::screen::ScreenResourceLifetime;
use hypercolor_core::input::screen::{
    CapturePixelFormat, ScreenBranchPayload, ScreenBranchPublication, ScreenSurfacePayload,
};
use hypercolor_core::types::canvas::{Canvas, PublishedSurface};
#[cfg(all(feature = "wgpu", target_os = "macos", feature = "screen-capture"))]
use hypercolor_macos_capture::MacosCaptureFrame;
#[cfg(all(feature = "wgpu", target_os = "macos", feature = "screen-capture"))]
use hypercolor_macos_gpu_interop::ImportedMacosScreenFrame;
#[cfg(all(feature = "wgpu", target_os = "windows"))]
use hypercolor_windows_gpu_interop::ScreenTextureCopy;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "wgpu")]
#[derive(Debug, Clone)]
pub(crate) struct GpuTextureFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) storage_id: u64,
    pub(crate) content_generation: u64,
    pub(crate) origin: GpuTextureFrameOrigin,
    pub(crate) texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    #[allow(
        dead_code,
        reason = "the Arc value is retained for its lifetime rather than read in production"
    )]
    pub(crate) immutable_lease: Option<Arc<GpuTextureFrameLease>>,
    #[cfg(target_os = "windows")]
    pub(crate) windows_screen_lease: Option<WindowsScreenTextureLease>,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    pub(crate) macos_screen_lease: Option<MacosScreenTextureLease>,
}

#[cfg(feature = "wgpu")]
#[derive(Debug)]
pub(crate) struct GpuTextureFrameLease;

#[cfg(all(feature = "wgpu", target_os = "windows"))]
#[derive(Debug, Clone)]
pub(crate) struct WindowsScreenTextureLease {
    _copy: ScreenTextureCopy,
    target_lifetime: ScreenResourceLifetime,
    _capture_lifetime: ScreenResourceLifetime,
}

#[cfg(all(feature = "wgpu", target_os = "windows"))]
impl WindowsScreenTextureLease {
    pub(crate) fn new(
        copy: ScreenTextureCopy,
        target_lifetime: ScreenResourceLifetime,
        capture_lifetime: ScreenResourceLifetime,
    ) -> Self {
        Self {
            _copy: copy,
            target_lifetime,
            _capture_lifetime: capture_lifetime,
        }
    }
}
#[cfg(all(feature = "wgpu", target_os = "macos", feature = "screen-capture"))]
#[derive(Clone)]
pub(crate) struct MacosScreenTextureLease {
    _imported: ImportedMacosScreenFrame,
    _capture_owner: PlatformGpuSurfaceOwner<MacosCaptureFrame>,
    _target_owner: PlatformGpuSurfaceOwner<
        crate::render_thread::sparkleflinger::gpu::PreparedMacosScreenTarget,
    >,
    _target_lifetime: ScreenResourceLifetime,
}

#[cfg(all(feature = "wgpu", target_os = "macos", feature = "screen-capture"))]
impl MacosScreenTextureLease {
    pub(crate) fn new(
        imported: ImportedMacosScreenFrame,
        capture_owner: PlatformGpuSurfaceOwner<MacosCaptureFrame>,
        target_owner: PlatformGpuSurfaceOwner<
            crate::render_thread::sparkleflinger::gpu::PreparedMacosScreenTarget,
        >,
        target_lifetime: ScreenResourceLifetime,
    ) -> Self {
        Self {
            _imported: imported,
            _capture_owner: capture_owner,
            _target_owner: target_owner,
            _target_lifetime: target_lifetime,
        }
    }
}
#[cfg(all(feature = "wgpu", target_os = "macos", feature = "screen-capture"))]
impl std::fmt::Debug for MacosScreenTextureLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MacosScreenTextureLease")
            .finish_non_exhaustive()
    }
}

#[cfg(all(
    feature = "wgpu",
    any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    )
))]
#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "cache lease payloads are retained for ownership rather than inspected"
)]
pub(crate) enum NativeScreenCacheLease {
    #[cfg(target_os = "windows")]
    Windows(ScreenResourceLifetime),
    #[cfg(target_os = "macos")]
    Macos(MacosScreenTextureLease),
}

#[cfg(all(
    feature = "wgpu",
    any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    )
))]
impl GpuTextureFrame {
    pub(crate) fn native_screen_cache_lease(&self) -> Option<NativeScreenCacheLease> {
        #[cfg(target_os = "windows")]
        {
            self.windows_screen_lease
                .as_ref()
                .map(|lease| NativeScreenCacheLease::Windows(lease.target_lifetime.clone()))
        }
        #[cfg(target_os = "macos")]
        {
            self.macos_screen_lease
                .as_ref()
                .cloned()
                .map(NativeScreenCacheLease::Macos)
        }
    }
}

#[cfg(feature = "wgpu")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuTextureFrameOrigin {
    CompositorOutput,
    ImmutableSnapshot,
    ProjectionSnapshot,
    ProducerTexture,
}

static PRODUCER_CPU_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static PRODUCER_GPU_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static PRODUCER_GPU_CPU_MATERIALIZATION_BLOCKED_TOTAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProducerFrameCounts {
    pub(crate) cpu_frames: u64,
    pub(crate) gpu_frames: u64,
    pub(crate) gpu_cpu_materialization_blocked: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum ProducerFrame {
    Canvas(Canvas),
    Surface(PublishedSurface),
    ScreenPublication(ScreenPublicationFrame),
    #[cfg(feature = "servo-gpu-import")]
    Gpu(ImportedEffectFrame),
    #[cfg(feature = "wgpu")]
    GpuTexture(GpuTextureFrame),
}

#[derive(Debug, Clone)]
pub(crate) struct ScreenPublicationFrame {
    publication: Arc<ScreenBranchPublication>,
}

impl ScreenPublicationFrame {
    fn try_new(publication: Arc<ScreenBranchPublication>) -> Option<Self> {
        let ScreenBranchPayload::Surface(surface) = publication.payload() else {
            return None;
        };
        if surface.pixel_format() != CapturePixelFormat::Rgba8 {
            return None;
        }
        Some(Self { publication })
    }

    pub(crate) fn surface(&self) -> ScreenSurfacePayload<'_> {
        let ScreenBranchPayload::Surface(surface) = self.publication.payload() else {
            unreachable!("screen publication frames are validated as CPU surfaces")
        };
        surface
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn plan_generation(&self) -> u64 {
        self.publication.plan_generation().get()
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn branch_sequence(&self) -> u64 {
        self.publication.branch_sequence().get()
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn descriptor_identity(&self) -> u64 {
        self.publication.descriptor_identity().get()
    }
}

impl ProducerFrame {
    pub(crate) fn screen_publication(publication: Arc<ScreenBranchPublication>) -> Option<Self> {
        ScreenPublicationFrame::try_new(publication).map(Self::ScreenPublication)
    }

    #[cfg_attr(
        not(any(feature = "wgpu", feature = "servo-gpu-import")),
        allow(
            dead_code,
            reason = "GPU residency helpers are inert without GPU producers"
        )
    )]
    pub(crate) const fn is_gpu_resident(&self) -> bool {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(_) => true,
            #[cfg(feature = "wgpu")]
            Self::GpuTexture(_) => true,
            Self::Canvas(_) | Self::Surface(_) | Self::ScreenPublication(_) => false,
        }
    }

    #[cfg_attr(
        not(any(feature = "wgpu", feature = "servo-gpu-import")),
        allow(
            dead_code,
            reason = "GPU residency helpers are inert without GPU producers"
        )
    )]
    pub(crate) fn record_cpu_materialization_blocked(&self) {
        if self.is_gpu_resident() {
            record_gpu_cpu_materialization_blocked();
        }
    }

    #[cfg_attr(
        not(feature = "servo-gpu-import"),
        allow(
            clippy::unnecessary_wraps,
            reason = "keeps the CPU and GPU producer frame API feature-stable"
        )
    )]
    pub(crate) fn cpu_rgba_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Canvas(canvas) => Some(canvas.as_rgba_bytes()),
            Self::Surface(surface) => Some(surface.rgba_bytes()),
            Self::ScreenPublication(publication) => Some(publication.surface().pixels()),
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(_) => {
                self.record_cpu_materialization_blocked();
                None
            }
            #[cfg(feature = "wgpu")]
            Self::GpuTexture(_) => {
                self.record_cpu_materialization_blocked();
                None
            }
        }
    }

    pub(crate) fn width(&self) -> u32 {
        match self {
            Self::Canvas(canvas) => canvas.width(),
            Self::Surface(surface) => surface.width(),
            Self::ScreenPublication(publication) => publication.surface().extent().width(),
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(frame) => frame.width,
            #[cfg(feature = "wgpu")]
            Self::GpuTexture(frame) => frame.width,
        }
    }

    pub(crate) fn height(&self) -> u32 {
        match self {
            Self::Canvas(canvas) => canvas.height(),
            Self::Surface(surface) => surface.height(),
            Self::ScreenPublication(publication) => publication.surface().extent().height(),
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(frame) => frame.height,
            #[cfg(feature = "wgpu")]
            Self::GpuTexture(frame) => frame.height,
        }
    }

    #[cfg_attr(
        not(feature = "servo-gpu-import"),
        allow(
            clippy::unnecessary_wraps,
            reason = "keeps the CPU and GPU producer frame API feature-stable"
        )
    )]
    pub(crate) fn into_cpu_render_frame(self) -> Option<(Canvas, Option<PublishedSurface>)> {
        match self {
            Self::Canvas(canvas) => Some((canvas, None)),
            Self::Surface(surface) => {
                Some((Canvas::from_published_surface(&surface), Some(surface)))
            }
            Self::ScreenPublication(publication) => {
                let surface = publication.surface();
                let mut canvas = Canvas::new(surface.extent().width(), surface.extent().height());
                canvas.as_rgba_bytes_mut().copy_from_slice(surface.pixels());
                Some((canvas, None))
            }
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(frame) => {
                let frame = Self::Gpu(frame);
                frame.record_cpu_materialization_blocked();
                None
            }
            #[cfg(feature = "wgpu")]
            Self::GpuTexture(frame) => {
                let frame = Self::GpuTexture(frame);
                frame.record_cpu_materialization_blocked();
                None
            }
        }
    }

    fn stable_identity_matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Canvas(left), Self::Canvas(right)) => {
                left.width() == right.width()
                    && left.height() == right.height()
                    && left.storage_identity() == right.storage_identity()
            }
            (Self::Surface(left), Self::Surface(right)) => {
                left.width() == right.width()
                    && left.height() == right.height()
                    && left.generation() == right.generation()
                    && left.storage_identity() == right.storage_identity()
            }
            (Self::ScreenPublication(left), Self::ScreenPublication(right)) => {
                Arc::ptr_eq(&left.publication, &right.publication)
            }
            #[cfg(feature = "servo-gpu-import")]
            (Self::Gpu(left), Self::Gpu(right)) => {
                left.width == right.width
                    && left.height == right.height
                    && left.storage_id == right.storage_id
            }
            #[cfg(feature = "wgpu")]
            (Self::GpuTexture(left), Self::GpuTexture(right)) => {
                left.width == right.width
                    && left.height == right.height
                    && left.storage_id == right.storage_id
                    && left.content_generation == right.content_generation
            }
            _ => false,
        }
    }
}

pub(crate) fn producer_frame_counts() -> ProducerFrameCounts {
    ProducerFrameCounts {
        cpu_frames: PRODUCER_CPU_FRAMES_TOTAL.load(Ordering::Relaxed),
        gpu_frames: PRODUCER_GPU_FRAMES_TOTAL.load(Ordering::Relaxed),
        gpu_cpu_materialization_blocked: PRODUCER_GPU_CPU_MATERIALIZATION_BLOCKED_TOTAL
            .load(Ordering::Relaxed),
    }
}

#[cfg_attr(
    not(any(feature = "wgpu", feature = "servo-gpu-import")),
    allow(
        dead_code,
        reason = "GPU residency counters are inert without GPU producers"
    )
)]
fn record_gpu_cpu_materialization_blocked() {
    let _ = PRODUCER_GPU_CPU_MATERIALIZATION_BLOCKED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_producer_frame(frame: &ProducerFrame) {
    match frame {
        ProducerFrame::Canvas(_)
        | ProducerFrame::Surface(_)
        | ProducerFrame::ScreenPublication(_) => {
            let _ = PRODUCER_CPU_FRAMES_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => {
            let _ = PRODUCER_GPU_FRAMES_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(_) => {
            let _ = PRODUCER_GPU_FRAMES_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProducerFrameState {
    Fresh,
    Retained,
}

#[derive(Debug, Clone)]
struct ProducerSubmission {
    frame: ProducerFrame,
    fresh: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct LatchedProducerFrame {
    pub state: ProducerFrameState,
    pub frame: ProducerFrame,
}

#[derive(Debug, Default)]
pub(crate) struct ProducerQueue {
    latest: Option<ProducerSubmission>,
}

impl ProducerQueue {
    pub(crate) const fn new() -> Self {
        Self { latest: None }
    }

    pub(crate) fn submit_latest(&mut self, frame: ProducerFrame) -> Option<ProducerFrame> {
        record_producer_frame(&frame);
        self.replace_latest(ProducerSubmission { frame, fresh: true })
    }

    #[cfg(any(
        test,
        all(
            feature = "wgpu",
            any(
                target_os = "windows",
                all(target_os = "macos", feature = "screen-capture")
            )
        )
    ))]
    pub(crate) const fn has_latest(&self) -> bool {
        self.latest.is_some()
    }

    pub(crate) fn clear_latest(&mut self) -> Option<ProducerFrame> {
        self.latest.take().map(|submission| submission.frame)
    }

    pub(crate) fn latch_latest(&mut self) -> Option<LatchedProducerFrame> {
        self.latch_matching(|_| true)
    }

    fn latch_matching(
        &mut self,
        predicate: impl FnOnce(&ProducerSubmission) -> bool,
    ) -> Option<LatchedProducerFrame> {
        let matches = predicate(self.latest.as_ref()?);
        if !matches {
            self.latest = None;
            return None;
        }

        let submission = self
            .latest
            .as_mut()
            .expect("matching submissions stay available until they are cleared");
        let state = if submission.fresh {
            submission.fresh = false;
            ProducerFrameState::Fresh
        } else {
            ProducerFrameState::Retained
        };

        Some(LatchedProducerFrame {
            state,
            frame: submission.frame.clone(),
        })
    }

    fn replace_latest(&mut self, submission: ProducerSubmission) -> Option<ProducerFrame> {
        if self
            .latest
            .as_ref()
            .is_some_and(|current| current.frame.stable_identity_matches(&submission.frame))
        {
            return Some(submission.frame);
        }

        self.latest
            .replace(submission)
            .map(|previous| previous.frame)
    }
}

impl ProducerFrameState {
    pub(crate) const fn is_retained(self) -> bool {
        matches!(self, Self::Retained)
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::types::canvas::{Canvas, PublishedSurface};

    use super::{ProducerFrame, ProducerFrameState, ProducerQueue};

    #[test]
    fn producer_queue_latches_fresh_then_retains() {
        let mut queue = ProducerQueue::new();
        queue.submit_latest(ProducerFrame::Canvas(Canvas::new(4, 4)));

        let fresh = queue.latch_latest().expect("fresh frame should latch");
        assert_eq!(fresh.state, ProducerFrameState::Fresh);

        let retained = queue.latch_latest().expect("latched frame should retain");
        assert_eq!(retained.state, ProducerFrameState::Retained);
    }

    #[test]
    fn producer_queue_latches_latest_without_generation_gate() {
        let mut queue = ProducerQueue::new();
        queue.submit_latest(ProducerFrame::Canvas(Canvas::new(3, 5)));

        let fresh = queue.latch_latest().expect("latest frame should latch");
        assert_eq!(fresh.state, ProducerFrameState::Fresh);

        let retained = queue
            .latch_latest()
            .expect("latest frame should remain retained");
        assert_eq!(retained.state, ProducerFrameState::Retained);
    }

    #[test]
    fn producer_queue_submit_returns_replaced_frame() {
        let mut queue = ProducerQueue::new();
        let first = Canvas::new(3, 5);
        let second = Canvas::new(3, 5);
        queue.submit_latest(ProducerFrame::Canvas(first.clone()));

        let replaced = queue.submit_latest(ProducerFrame::Canvas(second));
        let Some(ProducerFrame::Canvas(replaced)) = replaced else {
            panic!("expected replaced canvas frame");
        };
        assert_eq!(replaced.width(), first.width());
        assert_eq!(replaced.height(), first.height());
    }

    #[test]
    fn producer_queue_keeps_duplicate_surface_submissions_retained() {
        let mut queue = ProducerQueue::new();
        let surface = PublishedSurface::from_owned_canvas(Canvas::new(3, 5), 7, 11);
        queue.submit_latest(ProducerFrame::Surface(surface.clone()));

        let fresh = queue.latch_latest().expect("fresh surface should latch");
        assert_eq!(fresh.state, ProducerFrameState::Fresh);

        let duplicate = queue.submit_latest(ProducerFrame::Surface(surface.clone()));
        let Some(ProducerFrame::Surface(duplicate)) = duplicate else {
            panic!("duplicate surface should be returned to the caller");
        };
        assert_eq!(duplicate.storage_identity(), surface.storage_identity());

        let retained = queue
            .latch_latest()
            .expect("duplicate surface should leave the previous frame retained");
        assert_eq!(retained.state, ProducerFrameState::Retained);
    }

    #[test]
    fn producer_queue_keeps_duplicate_canvas_submissions_retained() {
        let mut queue = ProducerQueue::new();
        let canvas = Canvas::new(3, 5);
        queue.submit_latest(ProducerFrame::Canvas(canvas.clone()));

        let fresh = queue.latch_latest().expect("fresh canvas should latch");
        assert_eq!(fresh.state, ProducerFrameState::Fresh);

        let duplicate = queue.submit_latest(ProducerFrame::Canvas(canvas.clone()));
        let Some(ProducerFrame::Canvas(duplicate)) = duplicate else {
            panic!("duplicate canvas should be returned to the caller");
        };
        assert_eq!(duplicate.storage_identity(), canvas.storage_identity());

        let retained = queue
            .latch_latest()
            .expect("duplicate canvas should leave the previous frame retained");
        assert_eq!(retained.state, ProducerFrameState::Retained);
    }

    #[test]
    fn producer_queue_clear_releases_the_retained_frame() {
        let mut queue = ProducerQueue::new();
        assert!(!queue.has_latest());
        assert!(queue.clear_latest().is_none());

        queue.submit_latest(ProducerFrame::Canvas(Canvas::new(3, 5)));
        assert!(queue.has_latest());
        assert!(queue.clear_latest().is_some());
        assert!(!queue.has_latest());
        assert!(queue.latch_latest().is_none());
    }
}
