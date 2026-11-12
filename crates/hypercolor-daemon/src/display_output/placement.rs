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
    /// The output of that zone's layout whose box the screen shows, by its
    /// index there, or `None` when the screen shows the zone's whole canvas.
    /// The screen's backdrop is what that output shows: the layers of the
    /// zone that apply to it.
    pub output: Option<usize>,
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

/// The output of `layout` that places the screen, with its index in the
/// layout and how firmly it places the screen.
///
/// `layout_device_ids` are the layout ids of the screen's device.
fn screen_claim<'a>(
    layout: &'a SpatialLayout,
    layout_device_ids: &[String],
) -> Option<(ScreenClaim, usize, &'a Output)> {
    let mut device_output = None;
    let mut whole_output = None;
    for (index, output) in layout.zones.iter().enumerate() {
        if !layout_device_ids.contains(&output.device_id) {
            continue;
        }
        if is_display_zone(output) {
            return Some((ScreenClaim::Screen, index, output));
        }
        device_output.get_or_insert((index, output));
        if whole_output.is_none() && output.zone_name.is_none() {
            whole_output = Some((index, output));
        }
    }
    whole_output
        .map(|(index, output)| (ScreenClaim::Whole, index, output))
        .or_else(|| device_output.map(|(index, output)| (ScreenClaim::Device, index, output)))
}

/// Whether a claim shows the zone's whole canvas instead of its output's
/// box: a device with LED segments whose screen has no box of its own. An
/// LED segment's box says where its LEDs sit, not where the panel does.
fn shows_whole_canvas(claim: ScreenClaim, has_led_segments: bool) -> bool {
    claim == ScreenClaim::Device && has_led_segments
}

fn claimed_viewport(
    claim: ScreenClaim,
    output: &Output,
    layout: &SpatialLayout,
    has_led_segments: bool,
) -> DisplayZoneViewport {
    if shows_whole_canvas(claim, has_led_segments) {
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
    let (claim, _, output) = screen_claim(layout, layout_device_ids)?;
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
    let mut placed: Option<(ScreenClaim, &Zone, usize, &Output)> = None;
    for zone in zones.iter().filter(|zone| zone.display_target.is_none()) {
        let Some((claim, index, output)) = screen_claim(&zone.layout, layout_device_ids) else {
            continue;
        };
        if placed.is_none_or(|(placed_claim, _, _, _)| claim > placed_claim) {
            placed = Some((claim, zone, index, output));
        }
    }
    placed.map(|(claim, zone, index, output)| ScreenPlacement {
        zone_id: zone.id,
        output: (!shows_whole_canvas(claim, has_led_segments)).then_some(index),
        viewport: claimed_viewport(claim, output, &zone.layout, has_led_segments),
    })
}
