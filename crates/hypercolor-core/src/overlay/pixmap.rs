use anyhow::Result;
use tiny_skia::Pixmap;

use super::{OverlayBuffer, OverlaySize};

pub fn overlay_buffer_from_pixmap(pixmap: &Pixmap) -> Result<OverlayBuffer> {
    let mut buffer = OverlayBuffer::new(OverlaySize::new(pixmap.width(), pixmap.height()));
    buffer.copy_from_pixmap(pixmap)?;
    Ok(buffer)
}
