mod color_picker;
mod effect_card;
mod param_slider;
mod spectrum_bar;
mod split;

pub use color_picker::{ColorPickerPopup, hsl_to_rgb, rgb_to_hsl};
pub use effect_card::EffectCard;
pub use param_slider::ParamSlider;
pub use spectrum_bar::SpectrumBar;
pub use split::{Split, SplitDirection};

use ratatui::layout::Rect;

/// Compute a centered, aspect-preserving fit rect for a source image inside
/// a target terminal area.
///
/// Accounts for the typical 2:1 cell aspect ratio (terminal cells are roughly
/// twice as tall as they are wide in pixels). The returned rect is the
/// largest sub-rectangle of `area` that preserves the source aspect ratio,
/// centered on both axes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
#[must_use]
pub fn aspect_fit(src_w: u16, src_h: u16, area: Rect) -> Rect {
    if src_w == 0 || src_h == 0 || area.width == 0 || area.height == 0 {
        return area;
    }
    let sw = u32::from(src_w);
    let sh = u32::from(src_h);
    let tw = u32::from(area.width);
    let th = u32::from(area.height);

    // Each terminal row ≈ 2 source pixel rows (cells are taller than wide)
    let fit_h_pixels = sh * tw / sw;
    let fit_h_rows = fit_h_pixels.div_ceil(2);

    let (rw, rh) = if fit_h_rows <= th {
        (tw as u16, fit_h_rows as u16)
    } else {
        let th2 = th * 2;
        let fit_w = sw * th2 / sh;
        (fit_w.min(tw) as u16, th as u16)
    };

    let x = area.x + (area.width.saturating_sub(rw)) / 2;
    let y = area.y + (area.height.saturating_sub(rh)) / 2;
    Rect::new(x, y, rw, rh)
}
