//! Owning service for the daemon's authoritative spatial sampling plan.

use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::spatial::SpatialLayout;

/// Cloneable authority for the active spatial engine.
#[derive(Clone)]
pub struct SpatialService(Arc<SpatialServiceInner>);

struct SpatialServiceInner {
    engine: ArcSwap<SpatialEngine>,
}

/// Lock-free access to the latest admitted spatial engine.
#[derive(Clone)]
pub struct SpatialReader(Arc<SpatialServiceInner>);

#[cfg(feature = "persistence-test-hooks")]
pub struct SpatialTestFixture<'a> {
    service: &'a SpatialService,
}

#[cfg(feature = "persistence-test-hooks")]
impl SpatialTestFixture<'_> {
    pub fn replace(&self, layout: SpatialLayout) {
        let mut candidate = self.service.snapshot().as_ref().clone();
        if let Err(error) = candidate.try_update_layout(layout) {
            tracing::warn!(%error, "Rejected spatial fixture layout update");
            return;
        }
        self.service.replace(candidate);
    }
}

impl SpatialReader {
    /// Borrow the latest admitted spatial engine without cloning its `Arc`.
    #[must_use]
    pub fn load(&self) -> Guard<Arc<SpatialEngine>> {
        self.0.engine.load()
    }
}

impl SpatialService {
    /// Own a fully prepared spatial engine.
    #[must_use]
    pub fn new(engine: SpatialEngine) -> Self {
        Self(Arc::new(SpatialServiceInner {
            engine: ArcSwap::from_pointee(engine),
        }))
    }

    /// Capture an owned reference to the current spatial engine.
    #[must_use]
    pub fn snapshot(&self) -> Arc<SpatialEngine> {
        self.0.engine.load_full()
    }

    /// Capture the active immutable layout.
    #[must_use]
    pub fn layout(&self) -> Arc<SpatialLayout> {
        self.0.engine.load().layout()
    }

    /// Create a lock-free reader for render and output workers.
    #[must_use]
    pub fn reader(&self) -> SpatialReader {
        SpatialReader(Arc::clone(&self.0))
    }

    #[cfg(feature = "persistence-test-hooks")]
    #[must_use]
    pub fn test_fixture(&self) -> SpatialTestFixture<'_> {
        SpatialTestFixture { service: self }
    }

    /// Replace the authoritative engine with a fully prepared candidate.
    pub(crate) fn replace(&self, engine: SpatialEngine) {
        self.0.engine.store(Arc::new(engine));
    }

    #[must_use]
    pub(crate) fn has_layout(&self, expected: &SpatialLayout) -> bool {
        self.0.engine.load().layout().as_ref() == expected
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode};

    use super::*;

    fn layout(id: &str, width: u32) -> SpatialLayout {
        SpatialLayout {
            id: id.to_owned(),
            name: id.to_owned(),
            description: None,
            canvas_width: width,
            canvas_height: 120,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            version: 1,
        }
    }

    #[test]
    fn publication_replaces_reader_snapshot_atomically() {
        let service = SpatialService::new(
            SpatialEngine::try_new(layout("initial", 160)).expect("initial layout should prepare"),
        );
        let reader = service.reader();

        service.replace(
            SpatialEngine::try_new(layout("next", 320)).expect("next layout should prepare"),
        );

        assert_eq!(reader.load().layout().id, "next");
        assert!(service.has_layout(&layout("next", 320)));
    }
}
