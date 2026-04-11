mod clock;
mod common;
mod image;
mod pixmap;
mod sensor;
mod text;

use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use thiserror::Error;
use tiny_skia::Pixmap;

use hypercolor_types::sensor::SystemSnapshot;

pub use clock::ClockRenderer;
pub use image::ImageRenderer;
pub use pixmap::overlay_buffer_from_pixmap;
pub use sensor::SensorRenderer;
pub use text::TextRenderer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayBuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl OverlayBuffer {
    #[must_use]
    pub fn new(size: OverlaySize) -> Self {
        Self {
            width: size.width,
            height: size.height,
            pixels: vec![0; pixel_len(size.width, size.height)],
        }
    }

    pub fn resize(&mut self, size: OverlaySize) {
        self.width = size.width;
        self.height = size.height;
        self.pixels.resize(pixel_len(size.width, size.height), 0);
    }

    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    pub fn copy_from_pixmap(&mut self, pixmap: &Pixmap) -> Result<()> {
        let width = pixmap.width();
        let height = pixmap.height();
        if self.width != width || self.height != height {
            bail!(
                "pixmap size {}x{} did not match overlay buffer {}x{}",
                width,
                height,
                self.width,
                self.height
            );
        }

        self.pixels.copy_from_slice(pixmap.data());
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlaySize {
    pub width: u32,
    pub height: u32,
}

impl OverlaySize {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

pub struct OverlayInput<'a> {
    pub now: SystemTime,
    pub display_width: u32,
    pub display_height: u32,
    pub circular: bool,
    pub sensors: &'a SystemSnapshot,
    pub elapsed_secs: f32,
    pub frame_number: u64,
}

#[derive(Debug, Error)]
pub enum OverlayError {
    #[error("asset error: {0}")]
    Asset(#[from] anyhow::Error),
    #[error("transient: {0}")]
    Transient(String),
    #[error("fatal: {0}")]
    Fatal(String),
}

pub trait OverlayRenderer: Send {
    fn init(&mut self, target_size: OverlaySize) -> Result<()>;

    fn resize(&mut self, target_size: OverlaySize) -> Result<()>;

    fn render_into(
        &mut self,
        input: &OverlayInput<'_>,
        target: &mut OverlayBuffer,
    ) -> Result<(), OverlayError>;

    fn content_changed(&self, _input: &OverlayInput<'_>) -> bool {
        true
    }

    fn next_refresh_after(&self) -> Option<Duration> {
        None
    }

    fn destroy(&mut self) {}
}

fn pixel_len(width: u32, height: u32) -> usize {
    usize::try_from(width)
        .unwrap_or_default()
        .saturating_mul(usize::try_from(height).unwrap_or_default())
        .saturating_mul(4)
}
