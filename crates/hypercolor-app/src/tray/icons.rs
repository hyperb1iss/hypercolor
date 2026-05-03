//! Tray icon generation and state mapping.

use tauri::image::Image;

use crate::state::AppState;

/// Icon size in pixels.
const ICON_SIZE: u32 = 32;

/// Radius of the circle relative to the icon center.
const CIRCLE_RADIUS: f64 = 13.0;

/// Visual states for the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    /// Daemon running with live output.
    Active,
    /// Daemon running with rendering paused.
    Paused,
    /// Daemon not reachable.
    Disconnected,
    /// Daemon error state.
    Error,
}

/// Pick the tray icon state from the current app state.
#[must_use]
pub fn icon_state_for(state: &AppState) -> IconState {
    if !state.connected {
        IconState::Disconnected
    } else if state.paused {
        IconState::Paused
    } else {
        IconState::Active
    }
}

/// Build a Tauri image for the given tray icon state.
#[must_use]
pub fn build_icon(state: IconState) -> Image<'static> {
    Image::new_owned(render_icon(state), ICON_SIZE, ICON_SIZE)
}

/// Render a 32x32 RGBA pixel buffer for the given icon state.
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn render_icon(state: IconState) -> Vec<u8> {
    let (r, g, b, filled) = match state {
        IconState::Active => (0xb5, 0x37, 0xf2, true),
        IconState::Paused => (0x80, 0x80, 0x80, true),
        IconState::Disconnected => (0x80, 0x80, 0x80, false),
        IconState::Error => (0xe5, 0x3e, 0x3e, true),
    };

    let center = f64::from(ICON_SIZE) / 2.0;
    let mut pixels = vec![0_u8; (ICON_SIZE * ICON_SIZE * 4) as usize];

    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = f64::from(x) - center + 0.5;
            let dy = f64::from(y) - center + 0.5;
            let dist = (dx * dx + dy * dy).sqrt();
            let offset = ((y * ICON_SIZE + x) * 4) as usize;

            if filled {
                write_filled_pixel(&mut pixels, offset, r, g, b, dist);
            } else {
                write_ring_pixel(&mut pixels, offset, r, g, b, dist);
            }
        }
    }

    pixels
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn write_filled_pixel(pixels: &mut [u8], offset: usize, r: u8, g: u8, b: u8, dist: f64) {
    let alpha = if dist <= CIRCLE_RADIUS - 1.0 {
        255.0
    } else if dist <= CIRCLE_RADIUS {
        (CIRCLE_RADIUS - dist) * 255.0
    } else {
        0.0
    };

    let a = alpha.round().clamp(0.0, 255.0) as u8;
    pixels[offset] = r;
    pixels[offset + 1] = g;
    pixels[offset + 2] = b;
    pixels[offset + 3] = a;
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn write_ring_pixel(pixels: &mut [u8], offset: usize, r: u8, g: u8, b: u8, dist: f64) {
    let ring_outer = CIRCLE_RADIUS;
    let ring_inner = CIRCLE_RADIUS - 2.0;

    let outer_alpha = if dist <= ring_outer - 1.0 {
        255.0
    } else if dist <= ring_outer {
        (ring_outer - dist) * 255.0
    } else {
        0.0
    };

    let inner_alpha = if dist <= ring_inner - 1.0 {
        255.0
    } else if dist <= ring_inner {
        (ring_inner - dist) * 255.0
    } else {
        0.0
    };

    let ring_alpha = outer_alpha - inner_alpha;
    let a = ring_alpha.round().clamp(0.0, 255.0) as u8;
    pixels[offset] = r;
    pixels[offset + 1] = g;
    pixels[offset + 2] = b;
    pixels[offset + 3] = a;
}

#[cfg(test)]
#[allow(clippy::as_conversions)]
mod tests {
    use super::*;

    const PIXEL_COUNT: usize = ICON_SIZE as usize * ICON_SIZE as usize * 4;
    const CENTER_OFFSET: usize =
        ((ICON_SIZE as usize / 2) * ICON_SIZE as usize + ICON_SIZE as usize / 2) * 4;

    #[test]
    fn render_icon_produces_correct_size() {
        for state in [
            IconState::Active,
            IconState::Paused,
            IconState::Disconnected,
            IconState::Error,
        ] {
            let rgba = render_icon(state);
            assert_eq!(rgba.len(), PIXEL_COUNT);
        }
    }

    #[test]
    fn active_icon_has_nonzero_alpha_at_center() {
        let rgba = render_icon(IconState::Active);
        assert_eq!(rgba[CENTER_OFFSET + 3], 255);
    }

    #[test]
    fn disconnected_icon_has_zero_alpha_at_center() {
        let rgba = render_icon(IconState::Disconnected);
        assert_eq!(rgba[CENTER_OFFSET + 3], 0);
    }
}
