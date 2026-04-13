use hypercolor_core::types::canvas::{Canvas, PublishedSurface};

#[derive(Debug, Clone)]
pub(crate) enum ProducerFrame {
    Canvas(Canvas),
    Surface(PublishedSurface),
}

impl ProducerFrame {
    #[cfg(feature = "wgpu")]
    pub(crate) fn rgba_bytes(&self) -> &[u8] {
        match self {
            Self::Canvas(canvas) => canvas.as_rgba_bytes(),
            Self::Surface(surface) => surface.rgba_bytes(),
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) const fn width(&self) -> u32 {
        match self {
            Self::Canvas(canvas) => canvas.width(),
            Self::Surface(surface) => surface.width(),
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) const fn height(&self) -> u32 {
        match self {
            Self::Canvas(canvas) => canvas.height(),
            Self::Surface(surface) => surface.height(),
        }
    }

    pub(crate) fn into_render_frame(self) -> (Canvas, Option<PublishedSurface>) {
        match self {
            Self::Canvas(canvas) => (canvas, None),
            Self::Surface(surface) => (Canvas::from_published_surface(&surface), Some(surface)),
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
        self.replace_latest(ProducerSubmission {
            frame,
            fresh: true,
        })
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
    use hypercolor_core::types::canvas::Canvas;

    use super::{ProducerFrame, ProducerFrameState, ProducerQueue};

    #[test]
    fn producer_queue_latches_fresh_then_retains() {
        let mut queue = ProducerQueue::new();
        queue.submit_latest(ProducerFrame::Canvas(Canvas::new(4, 4)));

        let fresh = queue.latch_latest().expect("fresh frame should latch");
        assert_eq!(fresh.state, ProducerFrameState::Fresh);

        let retained = queue
            .latch_latest()
            .expect("latched frame should retain");
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
}
