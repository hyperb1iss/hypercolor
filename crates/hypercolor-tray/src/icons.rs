//! Tray icon generation and state management.
//!
//! Generates simple colored circle icons programmatically using the `image`
//! crate. Each icon state maps to a distinct color from the `SilkCircuit` palette.

use tray_icon::Icon;

/// Icon size in pixels (width and height).
const ICON_SIZE: u32 = 32;

/// Radius of the circle relative to the icon center.
const CIRCLE_RADIUS: f64 = 13.0;

/// Visual states for the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum IconState {
    /// Daemon running, effect active. Bright magenta/purple (#b537f2).
    Active,
    /// Daemon running, output paused. Dimmed gray (#808080).
    Paused,
    /// Daemon not reachable. Transparent outline circle.
    Disconnected,
    /// Daemon error state. Red (#e53e3e).
    Error,
}

/// Build a `tray_icon::Icon` for the given state.
///
/// # Errors
///
/// Returns an error if the icon RGBA data cannot be converted to a
/// platform icon (should not happen with valid 32x32 data).
pub fn build_icon(state: IconState) -> anyhow::Result<Icon> {
    let rgba = render_icon(state);
    let icon = Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE)
        .map_err(|e| anyhow::anyhow!("Failed to create tray icon: {e}"))?;
    Ok(icon)
}

/// Render a 32x32 RGBA pixel buffer for the given icon state.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn render_icon(state: IconState) -> Vec<u8> {
    let (r, g, b, filled) = match state {
        IconState::Active => (0xb5, 0x37, 0xf2, true),
        IconState::Paused => (0x80, 0x80, 0x80, true),
        IconState::Disconnected => (0x80, 0x80, 0x80, false),
        IconState::Error => (0xe5, 0x3e, 0x3e, true),
    };

    let center = f64::from(ICON_SIZE) / 2.0;
    let mut pixels = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];

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

/// Write a single pixel for a filled circle with anti-aliased edge.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
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

/// Write a single pixel for a ring (outline-only circle) with anti-aliased edges.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
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
        // Center of the ring should be transparent.
        assert_eq!(rgba[CENTER_OFFSET + 3], 0);
    }
}
