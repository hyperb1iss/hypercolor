//! The configured render resolution, the one size every LED layout renders
//! at.
//!
//! A saved layout has no size of its own. Startup seeds this cell from
//! `daemon.canvas_width` and `daemon.canvas_height`, a live resolution
//! change updates it, and every layout that goes active is put on it first,
//! so applying a layout can never change the resolution and the resolution
//! Settings shows is the one in force.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_types::spatial::SpatialLayout;

#[derive(Clone, Debug)]
pub(crate) struct RenderCanvas(Arc<AtomicU64>);

impl RenderCanvas {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self(Arc::new(AtomicU64::new(pack(width, height))))
    }

    pub(crate) fn get(&self) -> (u32, u32) {
        unpack(self.0.load(Ordering::Acquire))
    }

    pub(crate) fn set(&self, width: u32, height: u32) {
        self.0.store(pack(width, height), Ordering::Release);
    }

    /// Put `layout` on the render canvas. Returns whether its size changed.
    pub(crate) fn stamp(&self, layout: &mut SpatialLayout) -> bool {
        let (width, height) = self.get();
        let changed = layout.canvas_width != width || layout.canvas_height != height;
        layout.canvas_width = width;
        layout.canvas_height = height;
        changed
    }
}

const fn pack(width: u32, height: u32) -> u64 {
    ((width as u64) << 32) | height as u64
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "each half of the packed value holds one u32 side"
)]
const fn unpack(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode};

    fn layout(width: u32, height: u32) -> SpatialLayout {
        SpatialLayout {
            id: "saved".to_owned(),
            name: "Saved".to_owned(),
            description: None,
            canvas_width: width,
            canvas_height: height,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            version: 1,
        }
    }

    #[test]
    fn a_layout_takes_the_render_size_whatever_it_was_saved_with() {
        let canvas = RenderCanvas::new(640, 480);
        let mut saved = layout(640, 630);
        assert!(canvas.stamp(&mut saved));
        assert_eq!((saved.canvas_width, saved.canvas_height), (640, 480));
        assert!(!canvas.stamp(&mut saved));
    }

    #[test]
    fn a_layout_saved_at_another_size_loads_on_the_render_resolution() {
        let dir = tempfile::tempdir().expect("layout store tempdir");
        let layouts_path = dir.path().join("layouts.json");
        let mut stored = std::collections::HashMap::new();
        stored.insert("saved".to_owned(), layout(640, 630));
        std::fs::write(
            &layouts_path,
            serde_json::to_vec(&stored.values().collect::<Vec<_>>())
                .expect("layout fixture should be valid"),
        )
        .expect("seed a legacy stored layout");

        let mut default_layout = layout(640, 480);
        default_layout.id = "default".to_owned();
        let resources = super::super::LayoutContextResources::load(
            layouts_path.clone(),
            dir.path().join("layout-auto-exclusions.json"),
            &default_layout,
        );

        let loaded = &resources.catalog()["saved"];
        assert_eq!((loaded.canvas_width, loaded.canvas_height), (640, 480));
        let rewritten =
            crate::layout_store::load(&layouts_path).expect("the moved catalog is written back");
        let saved = &rewritten["saved"];
        assert_eq!((saved.canvas_width, saved.canvas_height), (0, 0));
        let raw = std::fs::read_to_string(&layouts_path).expect("layout fixture should be valid");
        assert!(!raw.contains("canvas_width"));
        assert!(!raw.contains("canvas_height"));
    }

    #[test]
    fn a_resolution_change_reaches_every_later_stamp() {
        let canvas = RenderCanvas::new(320, 200);
        let shared = canvas.clone();
        shared.set(1920, 1080);
        assert_eq!(canvas.get(), (1920, 1080));
        let mut saved = layout(320, 200);
        canvas.stamp(&mut saved);
        assert_eq!((saved.canvas_width, saved.canvas_height), (1920, 1080));
    }
}
