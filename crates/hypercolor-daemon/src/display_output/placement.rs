//! Which zone places a screen, and the part of that zone's canvas the screen
//! shows.
//!
//! A screen's backdrop is the frame of the zone that places it, seen through
//! that zone's placement of the screen (Spec 95 §5.10). The placement is read
//! from the zone's own layout, the layout its frame is drawn in, and from the
//! zones of the frame being rendered, so it always belongs to that frame.

use hypercolor_core::bus::DisplayZoneViewport;
use hypercolor_core::spatial::is_display_zone;
use hypercolor_types::scene::{Zone, ZoneId};
use hypercolor_types::spatial::{NormalizedPosition, Output, SpatialLayout};

/// The zone that places a screen and its viewport for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPlacement {
    /// The zone whose layout holds the screen's output.
    pub zone_id: ZoneId,
    /// The region of that zone's canvas the screen shows.
    pub viewport: DisplayZoneViewport,
}

/// How firmly an output of a layout places a screen, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ScreenClaim {
    /// Some output of the screen's device; its own box is not laid out.
    Device,
    /// An output for the whole device.
    Whole,
    /// The output named for the device's screen.
    Screen,
}

/// The output of `layout` that places the screen, with how firmly it does.
///
/// `layout_device_ids` are the layout ids of the screen's device.
fn screen_claim<'a>(
    layout: &'a SpatialLayout,
    layout_device_ids: &[String],
) -> Option<(ScreenClaim, &'a Output)> {
    let mut device_output = None;
    let mut whole_output = None;
    for output in &layout.zones {
        if !layout_device_ids.contains(&output.device_id) {
            continue;
        }
        if is_display_zone(output) {
            return Some((ScreenClaim::Screen, output));
        }
        device_output.get_or_insert(output);
        if whole_output.is_none() && output.zone_name.is_none() {
            whole_output = Some(output);
        }
    }
    whole_output
        .map(|output| (ScreenClaim::Whole, output))
        .or_else(|| device_output.map(|output| (ScreenClaim::Device, output)))
}

fn claimed_viewport(
    claim: ScreenClaim,
    output: &Output,
    layout: &SpatialLayout,
    has_led_segments: bool,
) -> DisplayZoneViewport {
    // A device with LED segments whose screen has no box of its own shows the
    // whole canvas: an LED segment's box says where its LEDs sit, not where
    // the panel does.
    if claim == ScreenClaim::Device && has_led_segments {
        return DisplayZoneViewport {
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            edge_behavior: layout.default_edge_behavior,
        };
    }
    DisplayZoneViewport {
        position: output.position,
        size: output.size,
        rotation: output.rotation,
        scale: output.scale,
        edge_behavior: output.edge_behavior.unwrap_or(layout.default_edge_behavior),
    }
}

/// The viewport one layout gives a screen, or `None` when no output of the
/// layout belongs to the screen's device.
///
/// `layout_device_ids` are the layout ids of the screen's device, and
/// `has_led_segments` says whether that device also drives LEDs.
#[must_use]
pub fn screen_viewport_in_layout(
    layout: &SpatialLayout,
    layout_device_ids: &[String],
    has_led_segments: bool,
) -> Option<DisplayZoneViewport> {
    let (claim, output) = screen_claim(layout, layout_device_ids)?;
    Some(claimed_viewport(claim, output, layout, has_led_segments))
}

/// The zone that places a screen, and its viewport for it.
///
/// Display zones place nothing: their layouts hold a face, not outputs. Among
/// the scene's other zones the firmest claim wins, and the earlier zone wins
/// a tie, so a screen whose box one zone holds is placed by that zone even
/// when another zone holds the same device's LEDs.
#[must_use]
pub fn screen_placement(
    zones: &[Zone],
    layout_device_ids: &[String],
    has_led_segments: bool,
) -> Option<ScreenPlacement> {
    let mut placed: Option<(ScreenClaim, &Zone, &Output)> = None;
    for zone in zones.iter().filter(|zone| zone.display_target.is_none()) {
        let Some((claim, output)) = screen_claim(&zone.layout, layout_device_ids) else {
            continue;
        };
        if placed.is_none_or(|(placed_claim, _, _)| claim > placed_claim) {
            placed = Some((claim, zone, output));
        }
    }
    placed.map(|(claim, zone, output)| ScreenPlacement {
        zone_id: zone.id,
        viewport: claimed_viewport(claim, output, &zone.layout, has_led_segments),
    })
}
