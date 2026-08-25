use std::sync::Arc;

use hypercolor_core::bus::ScreenZonesFrame;
use hypercolor_core::input::ScreenBranchPublication;
use hypercolor_core::input::screen::consumer::{PixelExtent, ScreenBranchPayload};

use super::RenderThreadState;

/// Latest exact zones publication with the source geometry of its branch.
#[derive(Clone, Debug)]
pub(crate) struct ScreenZonesSnapshot {
    pub(crate) publication: Arc<ScreenBranchPublication>,
    pub(crate) source_extent: PixelExtent,
}

/// Publish the latest exact zones publication to the bus when anyone is
/// watching.
///
/// Redundant frames (identical zone content) are skipped so relays only wake
/// when colors actually change.
pub(crate) fn publish_screen_zones(
    state: &RenderThreadState,
    zones: Option<&ScreenZonesSnapshot>,
    frame_number: u32,
    timestamp_ms: u32,
) {
    if state.event_bus.screen_zones_receiver_count() == 0 {
        return;
    }

    let frame = zones
        .and_then(|snapshot| screen_zones_frame(snapshot, frame_number, timestamp_ms))
        .unwrap_or_default();

    state
        .event_bus
        .screen_zones_lane()
        .send_if_modified(|current| {
            if current.same_content(&frame) {
                false
            } else {
                *current = frame;
                true
            }
        });
}

/// Project one exact zones publication onto the wire frame.
///
/// The exact pipeline already applied the branch's content-bars policy
/// before publishing, so the grid is the effective grid and the letterbox
/// field stays zero. Source geometry comes from the committed descriptor.
pub(crate) fn screen_zones_frame(
    snapshot: &ScreenZonesSnapshot,
    frame_number: u32,
    timestamp_ms: u32,
) -> Option<ScreenZonesFrame> {
    let ScreenBranchPayload::Zones(zones) = snapshot.publication.payload() else {
        return None;
    };
    let cols = zones.columns().get();
    let rows = zones.rows().get();
    let row_count = usize::try_from(rows).ok()?;
    let col_count = usize::try_from(cols).ok()?;
    let cell_count = checked_cell_count(row_count, col_count)?;
    if zones.colors().len() != cell_count {
        return None;
    }
    let mut colors = try_zone_color_buffer(cell_count)?;
    colors.extend_from_slice(zones.colors());
    let source_width = snapshot.source_extent.width();
    let source_height = snapshot.source_extent.height();

    Some(ScreenZonesFrame {
        frame_number,
        timestamp_ms,
        source_width,
        source_height,
        grid_cols: cols,
        grid_rows: rows,
        letterbox: [0; 4],
        colors: Arc::new(colors),
    })
}

fn checked_cell_count(rows: usize, cols: usize) -> Option<usize> {
    rows.checked_mul(cols)
}

fn try_zone_color_buffer(cell_count: usize) -> Option<Vec<[u8; 3]>> {
    let mut colors = Vec::new();
    colors.try_reserve_exact(cell_count).ok()?;
    Some(colors)
}

#[cfg(test)]
mod tests;
