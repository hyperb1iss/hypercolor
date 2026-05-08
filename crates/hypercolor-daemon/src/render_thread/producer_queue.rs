#[cfg(feature = "servo-gpu-import")]
use hypercolor_core::effect::ImportedEffectFrame;
use hypercolor_core::types::canvas::{Canvas, PublishedSurface};
use std::sync::atomic::{AtomicU64, Ordering};

static PRODUCER_CPU_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static PRODUCER_GPU_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProducerFrameCounts {
    pub(crate) cpu_frames_total: u64,
    pub(crate) gpu_frames_total: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum ProducerFrame {
    Canvas(Canvas),
    Surface(PublishedSurface),
    #[cfg(feature = "servo-gpu-import")]
    Gpu(ImportedEffectFrame),
}

impl ProducerFrame {
    #[cfg(feature = "wgpu")]
    pub(crate) fn rgba_bytes(&self) -> &[u8] {
        match self {
            Self::Canvas(canvas) => canvas.as_rgba_bytes(),
            Self::Surface(surface) => surface.rgba_bytes(),
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(_) => {
                panic!("GPU producer frames do not expose CPU RGBA bytes")
            }
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) const fn width(&self) -> u32 {
        match self {
            Self::Canvas(canvas) => canvas.width(),
            Self::Surface(surface) => surface.width(),
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(frame) => frame.width,
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) const fn height(&self) -> u32 {
        match self {
            Self::Canvas(canvas) => canvas.height(),
            Self::Surface(surface) => surface.height(),
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(frame) => frame.height,
        }
    }

    pub(crate) fn into_render_frame(self) -> (Canvas, Option<PublishedSurface>) {
        match self {
            Self::Canvas(canvas) => (canvas, None),
            Self::Surface(surface) => (Canvas::from_published_surface(&surface), Some(surface)),
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(_) => {
                panic!("GPU producer frames must be handled before CPU materialization")
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
            #[cfg(feature = "servo-gpu-import")]
            (Self::Gpu(left), Self::Gpu(right)) => {
                left.width == right.width
                    && left.height == right.height
                    && left.storage_id == right.storage_id
            }
            _ => false,
        }
    }
}

pub(crate) fn producer_frame_counts() -> ProducerFrameCounts {
    ProducerFrameCounts {
        cpu_frames_total: PRODUCER_CPU_FRAMES_TOTAL.load(Ordering::Relaxed),
        gpu_frames_total: PRODUCER_GPU_FRAMES_TOTAL.load(Ordering::Relaxed),
    }
}

fn record_producer_frame(frame: &ProducerFrame) {
    match frame {
        ProducerFrame::Canvas(_) | ProducerFrame::Surface(_) => {
            let _ = PRODUCER_CPU_FRAMES_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => {
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
}
