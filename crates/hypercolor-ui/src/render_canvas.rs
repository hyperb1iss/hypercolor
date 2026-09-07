//! The daemon's render canvas extent, as the Studio sees it.
//!
//! A zone's layout carries placements only; the canvas those placements
//! sit on is the daemon's (`daemon.canvas_width` × `daemon.canvas_height`).
//! Every Studio surface that needs a real aspect ratio (the stage viewport,
//! the live preview, auto-placement defaults) resolves it here so the
//! canvas is never guessed as square or 4:3.

use hypercolor_types::config::HypercolorConfig;
use leptos::prelude::*;

use crate::app::WsContext;
use crate::config_state::ConfigContext;

/// The canvas extent the daemon ships when nothing else is known.
pub const DEFAULT_RENDER_CANVAS: (u32, u32) = (640, 480);

/// Resolve the render canvas extent from the two live sources, most
/// authoritative first: the daemon's config, then the most recent preview
/// frame. Either source is ignored when it carries a zero dimension.
#[must_use]
pub fn resolve_render_canvas_size(
    config: Option<(u32, u32)>,
    frame: Option<(u32, u32)>,
) -> (u32, u32) {
    config
        .filter(|&(w, h)| w > 0 && h > 0)
        .or_else(|| frame.filter(|&(w, h)| w > 0 && h > 0))
        .unwrap_or(DEFAULT_RENDER_CANVAS)
}

fn config_canvas(config: &HypercolorConfig) -> (u32, u32) {
    (config.daemon.canvas_width, config.daemon.canvas_height)
}

/// Reactive render canvas extent. Dedupes through a memo so a 60 Hz frame
/// stream never re-renders consumers that only care about the size.
#[must_use]
pub fn use_render_canvas_size() -> Memo<(u32, u32)> {
    let config = use_context::<ConfigContext>();
    let ws = use_context::<WsContext>();
    Memo::new(move |_| {
        let from_config =
            config.and_then(|ctx| ctx.config.with(|cfg| cfg.as_ref().map(config_canvas)));
        let from_frame = ws.and_then(|ws| {
            ws.canvas_frame
                .with(|frame| frame.as_ref().map(|frame| (frame.width, frame.height)))
        });
        resolve_render_canvas_size(from_config, from_frame)
    })
}

/// CSS `aspect-ratio` value for a canvas extent.
#[must_use]
pub fn aspect_ratio_css((width, height): (u32, u32)) -> String {
    format!("{} / {}", width.max(1), height.max(1))
}
