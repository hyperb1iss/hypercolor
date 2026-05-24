//! Tray icon generation and state management.
//!
//! Pre-baked PNGs of the canonical trinity mark, one per state. Generated
//! by `assets/brand/build.py` from the master brand and committed under
//! `crates/hypercolor-tray/icons/tray/`.

use std::io::Cursor;

use image::ImageReader;
use tray_icon::Icon;

/// Icon size in pixels (width and height). Pre-baked PNGs must match this.
const ICON_SIZE: u32 = 32;

const ICON_ACTIVE: &[u8] = include_bytes!("../icons/tray/tray-active-32.png");
const ICON_PAUSED: &[u8] = include_bytes!("../icons/tray/tray-paused-32.png");
const ICON_DISCONNECTED: &[u8] = include_bytes!("../icons/tray/tray-disconnected-32.png");
const ICON_ERROR: &[u8] = include_bytes!("../icons/tray/tray-error-32.png");

/// Visual states for the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum IconState {
    /// Daemon running, effect active.
    Active,
    /// Daemon running, output paused.
    Paused,
    /// Daemon not reachable.
    Disconnected,
    /// Daemon error state.
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

/// Decode the pre-baked PNG for the given state into an RGBA pixel buffer.
fn render_icon(state: IconState) -> Vec<u8> {
    let png_bytes: &[u8] = match state {
        IconState::Active => ICON_ACTIVE,
        IconState::Paused => ICON_PAUSED,
        IconState::Disconnected => ICON_DISCONNECTED,
        IconState::Error => ICON_ERROR,
    };

    let img = ImageReader::with_format(Cursor::new(png_bytes), image::ImageFormat::Png)
        .decode()
        .expect("compiled-in PNG decodes")
        .to_rgba8();
    debug_assert_eq!(img.width(), ICON_SIZE);
    debug_assert_eq!(img.height(), ICON_SIZE);
    img.into_raw()
}

#[cfg(test)]
#[allow(clippy::as_conversions)]
mod tests {
    use super::*;

    const PIXEL_COUNT: usize = (ICON_SIZE as usize) * (ICON_SIZE as usize) * 4;

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
    fn states_render_distinct_icons() {
        let active = render_icon(IconState::Active);
        let paused = render_icon(IconState::Paused);
        let disconnected = render_icon(IconState::Disconnected);
        let error = render_icon(IconState::Error);
        assert_ne!(active, paused);
        assert_ne!(active, disconnected);
        assert_ne!(active, error);
        assert_ne!(paused, error);
    }
}
