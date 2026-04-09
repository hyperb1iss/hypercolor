use hypercolor_core::types::canvas::{Canvas, PublishedSurface};

#[derive(Debug, Clone)]
pub(crate) enum ProducerFrame {
    Canvas(Canvas),
    Surface(PublishedSurface),
}

impl ProducerFrame {
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
    generation: u64,
    fresh: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct LatchedProducerFrame {
    #[allow(
        dead_code,
        reason = "fresh vs retained state is asserted in tests and feeds upcoming frame metrics"
    )]
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

    pub(crate) fn clear(&mut self) {
        self.latest = None;
    }

    pub(crate) fn submit(&mut self, frame: ProducerFrame, generation: u64) {
        self.latest = Some(ProducerSubmission {
            frame,
            generation,
            fresh: true,
        });
    }

    pub(crate) fn latch_latest(&mut self) -> Option<LatchedProducerFrame> {
        self.latch_matching(|_| true)
    }

    pub(crate) fn latch_for_generation(
        &mut self,
        expected_generation: u64,
    ) -> Option<LatchedProducerFrame> {
        self.latch_matching(|submission| submission.generation == expected_generation)
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
        queue.submit(ProducerFrame::Canvas(Canvas::new(4, 4)), 1);

        let fresh = queue
            .latch_for_generation(1)
            .expect("fresh frame should latch");
        assert_eq!(fresh.state, ProducerFrameState::Fresh);

        let retained = queue
            .latch_for_generation(1)
            .expect("latched frame should retain");
        assert_eq!(retained.state, ProducerFrameState::Retained);
    }

    #[test]
    fn producer_queue_discards_generation_mismatch() {
        let mut queue = ProducerQueue::new();
        queue.submit(
            ProducerFrame::Surface(PublishedSurface::from_owned_canvas(Canvas::new(2, 2), 1, 1)),
            7,
        );

        assert!(queue.latch_for_generation(8).is_none());
        assert!(queue.latch_for_generation(7).is_none());
    }

    #[test]
    fn producer_queue_latches_latest_without_generation_gate() {
        let mut queue = ProducerQueue::new();
        queue.submit(ProducerFrame::Canvas(Canvas::new(3, 5)), 11);

        let fresh = queue.latch_latest().expect("latest frame should latch");
        assert_eq!(fresh.state, ProducerFrameState::Fresh);

        let retained = queue
            .latch_latest()
            .expect("latest frame should remain retained");
        assert_eq!(retained.state, ProducerFrameState::Retained);
    }
}
