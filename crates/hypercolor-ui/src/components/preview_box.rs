//! The one sizing rule every live preview shares.
//!
//! A preview shows its source at the source's own aspect: the frame header
//! on the canvas path, the video's dimensions under Remote. Only the render
//! resolution (or a zone's canvas, which follows it) changes that shape. The
//! host decides how big the picture is, never what shape it has, so dragging
//! a splitter, going fullscreen, or turning a phone never stretches it.

/// How a preview box sits in its host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PreviewFit {
    /// Fit inside a host bounded on both axes (the dashboard hero, fullscreen,
    /// a phone's hero slot). The host becomes a size container and the box
    /// takes the largest rectangle of the source's aspect that fits in it.
    Contain,
    /// Fill the host's width and let the height follow the source, for
    /// unbounded columns such as the effects page and the sidebar player.
    #[default]
    FillWidth,
}

impl PreviewFit {
    /// Stable name for the `data-preview-fit` attribute.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contain => "contain",
            Self::FillWidth => "fill-width",
        }
    }
}

/// Classes for the host a [`PreviewFit::Contain`] box measures against.
/// It fills its positioned parent and centers the box on both axes.
pub const CONTAIN_HOST_CLASS: &str = "absolute inset-0 flex items-center justify-center";

/// Inline style for the [`PreviewFit::Contain`] host: a size container, so
/// the box can resolve `cqw` and `cqh` against it.
pub const CONTAIN_HOST_STYLE: &str = "container-type: size;";

/// The extent a preview is sized by: the decoded source once a frame has
/// arrived, else the source the page expects. Zero sides fall through, so a
/// half-known size never collapses the box.
#[must_use]
pub fn preview_source_size(decoded: Option<(u32, u32)>, expected: (u32, u32)) -> (u32, u32) {
    decoded
        .filter(|&(width, height)| width > 0 && height > 0)
        .unwrap_or((expected.0.max(1), expected.1.max(1)))
}

/// `WxH` for the `data-preview-source` attribute.
#[must_use]
pub fn preview_source_label((width, height): (u32, u32)) -> String {
    format!("{width}x{height}")
}

/// Inline style for a preview box showing a source whose CSS aspect is
/// `aspect_css` (`"W / H"`).
///
/// `Contain` sizes the width as the smaller of the host's width and the
/// width the host's height allows at this aspect, and lets `aspect-ratio`
/// derive the height, so the box never exceeds the host on either axis and
/// never changes shape. `FillWidth` takes the host's width.
#[must_use]
pub fn preview_box_style(fit: PreviewFit, aspect_css: &str, max_width: &str) -> String {
    match fit {
        PreviewFit::Contain => format!(
            "aspect-ratio: {aspect_css}; width: min(100cqw, calc(100cqh * ({aspect_css}))); \
             max-width: {max_width};"
        ),
        PreviewFit::FillWidth => {
            format!("aspect-ratio: {aspect_css}; width: 100%; max-width: {max_width};")
        }
    }
}

/// The width a box of `source` aspect takes when contained in a
/// `host_width` x `host_height` host, in the host's units.
#[must_use]
pub fn contained_width(source: (u32, u32), host_width: f64, host_height: f64) -> f64 {
    let (width, height) = (f64::from(source.0.max(1)), f64::from(source.1.max(1)));
    host_width.min(host_height * width / height).max(0.0)
}

/// Chrome a cabinet adds around its picture, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CabinetChrome {
    /// Left and right borders together.
    pub width: f64,
    /// Top and bottom borders plus the preset strip.
    pub height: f64,
}

/// The cabinet extent that hugs a `source` picture: as wide as asked up to
/// the width whose picture fills `max_height`, and no taller than its
/// picture plus chrome. Extra width goes back to the cabinet's neighbours
/// instead of turning into black side bars, and a wide source shortens the
/// row instead of growing bars above and below. `min_width` keeps a very
/// tall source from squeezing the preset strip; such a source pillarboxes.
#[must_use]
pub fn hug_cabinet(
    source: (u32, u32),
    requested_width: f64,
    max_height: f64,
    chrome: CabinetChrome,
    min_width: f64,
) -> (f64, f64) {
    let aspect = f64::from(source.0.max(1)) / f64::from(source.1.max(1));
    let picture_height = (max_height - chrome.height).max(1.0);
    let widest = (picture_height * aspect + chrome.width).max(min_width);
    let width = requested_width
        .min(widest)
        .max(min_width.min(requested_width));
    let height = ((width - chrome.width).max(1.0) / aspect + chrome.height).min(max_height);
    (width, height)
}
