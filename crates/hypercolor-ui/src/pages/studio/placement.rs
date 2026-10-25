//! Where outputs land on a zone canvas, both as they join a zone and when
//! the zone fits its members to the canvas.
//!
//! Every zone keeps its own canvas. An output joining a zone lands where it
//! sits in the rig (the active layout), carried into the zone by the same
//! scale and offset its current members already wear, so it keeps its
//! physical relation to them. A zone's first arrivals fit the canvas
//! instead, so an effect covers them completely; Fit applies that same fit
//! to every member on demand.
//!
//! Geometry runs in canvas pixel proportions (x spans the canvas aspect, y
//! spans 1), so a uniform scale never distorts a box on a wide canvas.

use hypercolor_types::api::scene::{MemberEdit, MemberPlacementHint, MemberState, SceneDocument};
use hypercolor_types::scene::ZoneRole;
use hypercolor_types::spatial::{NormalizedPosition, Orientation, Output, SpatialLayout};

use crate::api::{self, DeviceSummary, SegmentSummary, SegmentTopologySummary};
use crate::layout_geometry;
use crate::layout_utils::channel_name_matches_slot_alias;

/// Share of each canvas dimension a fit leaves empty on every side.
pub const FIT_MARGIN: f32 = 0.04;

/// Smallest extent a scaled box keeps, so placement never sends the
/// daemon a degenerate size.
const MIN_EXTENT: f32 = 0.001;

/// Gap between default footprints, as a share of the largest one.
const DEFAULT_GAP: f32 = 0.12;

/// An output's placement on a canvas, without its hardware binding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Footprint {
    pub position: NormalizedPosition,
    pub size: NormalizedPosition,
    pub rotation: f32,
    pub scale: f32,
    pub orientation: Option<Orientation>,
}

impl Footprint {
    #[must_use]
    pub fn of(output: &Output) -> Self {
        Self {
            position: output.position,
            size: output.size,
            rotation: output.rotation,
            scale: output.scale,
            orientation: output.orientation,
        }
    }

    /// Write this placement onto `output`, leaving its binding alone.
    pub fn apply(self, output: &mut Output) {
        output.position = self.position;
        output.size = self.size;
        output.rotation = self.rotation;
        output.scale = self.scale;
        output.orientation = self.orientation;
    }

    /// The seed the daemon applies to the output it mints for `segment`.
    #[must_use]
    pub fn hint(self, segment: Option<String>) -> MemberPlacementHint {
        MemberPlacementHint {
            segment,
            position: self.position,
            size: self.size,
            rotation: self.rotation,
            scale: self.scale,
            orientation: self.orientation,
        }
    }

    /// Axis-aligned bounds of the drawn box, rotation and scale included.
    #[must_use]
    pub fn bounds(self, aspect: f32) -> Bounds {
        let scale = self.scale.max(0.0);
        let half_width = self.size.x * aspect * scale * 0.5;
        let half_height = self.size.y * scale * 0.5;
        let (sin, cos) = self.rotation.sin_cos();
        let extent_x = (half_width * cos).abs() + (half_height * sin).abs();
        let extent_y = (half_width * sin).abs() + (half_height * cos).abs();
        let center_x = self.position.x * aspect;
        let center_y = self.position.y;
        Bounds {
            min_x: center_x - extent_x,
            max_x: center_x + extent_x,
            min_y: center_y - extent_y,
            max_y: center_y + extent_y,
        }
    }
}

/// An axis-aligned rectangle in canvas pixel proportions: x runs from 0 to
/// the canvas aspect, y from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min_x: f32,
    pub max_x: f32,
    pub min_y: f32,
    pub max_y: f32,
}

impl Bounds {
    #[must_use]
    pub fn width(self) -> f32 {
        self.max_x - self.min_x
    }

    #[must_use]
    pub fn height(self) -> f32 {
        self.max_y - self.min_y
    }

    #[must_use]
    pub fn center(self) -> (f32, f32) {
        (
            (self.min_x + self.max_x) * 0.5,
            (self.min_y + self.max_y) * 0.5,
        )
    }

    fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            max_x: self.max_x.max(other.max_x),
            min_y: self.min_y.min(other.min_y),
            max_y: self.max_y.max(other.max_y),
        }
    }

    fn diagonal(self) -> f32 {
        self.width().hypot(self.height())
    }
}

fn union_of(bounds: impl IntoIterator<Item = Bounds>) -> Option<Bounds> {
    bounds.into_iter().reduce(Bounds::union)
}

/// A uniform scale plus an offset, in canvas pixel proportions.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Similarity {
    scale: f32,
    dx: f32,
    dy: f32,
}

impl Similarity {
    const IDENTITY: Self = Self {
        scale: 1.0,
        dx: 0.0,
        dy: 0.0,
    };

    fn apply(self, footprint: Footprint, aspect: f32) -> Footprint {
        let x = footprint.position.x * aspect * self.scale + self.dx;
        let y = footprint.position.y * self.scale + self.dy;
        Footprint {
            position: NormalizedPosition::new(x / aspect, y),
            size: NormalizedPosition::new(
                (footprint.size.x * self.scale).max(MIN_EXTENT),
                (footprint.size.y * self.scale).max(MIN_EXTENT),
            ),
            ..footprint
        }
    }

    /// The scale and offset that center `bounds` on the canvas and grow or
    /// shrink it until it touches the margin on its tighter axis.
    fn fit(bounds: Bounds, aspect: f32) -> Self {
        let usable = 1.0 - 2.0 * FIT_MARGIN;
        let scale = (aspect * usable / bounds.width().max(MIN_EXTENT))
            .min(usable / bounds.height().max(MIN_EXTENT));
        let (center_x, center_y) = bounds.center();
        Self {
            scale,
            dx: aspect * 0.5 - scale * center_x,
            dy: 0.5 - scale * center_y,
        }
    }

    /// The scale and offset that carry rig bounds onto the bounds the same
    /// outputs occupy in a zone.
    fn between(rig: Bounds, zone: Bounds) -> Self {
        let (rig_diagonal, zone_diagonal) = (rig.diagonal(), zone.diagonal());
        let scale = if rig_diagonal > MIN_EXTENT && zone_diagonal > MIN_EXTENT {
            zone_diagonal / rig_diagonal
        } else {
            1.0
        };
        let (rig_x, rig_y) = rig.center();
        let (zone_x, zone_y) = zone.center();
        Self {
            scale,
            dx: zone_x - scale * rig_x,
            dy: zone_y - scale * rig_y,
        }
    }
}

/// Shrink a box that is larger than the canvas, then slide it until it is
/// fully on the canvas, so a carried arrival can never land out of sight.
fn keep_on_canvas(mut footprint: Footprint, aspect: f32) -> Footprint {
    let bounds = footprint.bounds(aspect);
    let shrink = (aspect / bounds.width().max(MIN_EXTENT))
        .min(1.0 / bounds.height().max(MIN_EXTENT))
        .min(1.0);
    if shrink < 1.0 {
        footprint.size = NormalizedPosition::new(
            (footprint.size.x * shrink).max(MIN_EXTENT),
            (footprint.size.y * shrink).max(MIN_EXTENT),
        );
    }
    let bounds = footprint.bounds(aspect);
    let shift = |min: f32, max: f32, limit: f32| {
        if min < 0.0 {
            -min
        } else if max > limit {
            limit - max
        } else {
            0.0
        }
    };
    footprint.position.x += shift(bounds.min_x, bounds.max_x, aspect) / aspect;
    footprint.position.y += shift(bounds.min_y, bounds.max_y, 1.0);
    footprint
}

/// One footprint spanning several, unrotated.
fn spanning(footprints: impl IntoIterator<Item = Footprint>, aspect: f32) -> Option<Footprint> {
    let mut footprints = footprints.into_iter().peekable();
    let orientation = footprints.peek()?.orientation;
    let bounds = union_of(footprints.map(|footprint| footprint.bounds(aspect)))?;
    let (center_x, center_y) = bounds.center();
    Some(Footprint {
        position: NormalizedPosition::new(center_x / aspect, center_y),
        size: NormalizedPosition::new(
            (bounds.width() / aspect).max(MIN_EXTENT),
            bounds.height().max(MIN_EXTENT),
        ),
        rotation: 0.0,
        scale: 1.0,
        orientation,
    })
}

/// The layouts that say where hardware physically sits, most authoritative
/// first. A single-zone scene's primary zone follows the active layout
/// live, so it leads there; once a scene has several LED zones each keeps
/// its own canvas and only the active layout describes the rig.
#[must_use]
pub fn rig_layouts(
    scene: &SceneDocument,
    active_layout: Option<&SpatialLayout>,
) -> Vec<Vec<Output>> {
    let mut lights = scene
        .zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display);
    let only_light = match (lights.next(), lights.next()) {
        (Some(only), None) => Some(api::zone_outputs(only)),
        _ => None,
    };
    only_light
        .into_iter()
        .chain(active_layout.map(|layout| layout.zones.clone()))
        .collect()
}

/// Where an output sits in the rig. It is found by id, else by device and
/// segment in the first layout that holds the device's segment. A segment
/// the rig splits into component instances spans every instance.
#[must_use]
pub fn rig_footprint(
    rig: &[Vec<Output>],
    output_id: Option<&str>,
    device_id: &str,
    segment: Option<&str>,
    aspect: f32,
) -> Option<Footprint> {
    if let Some(id) = output_id
        && let Some(output) = rig.iter().flatten().find(|output| output.id == id)
    {
        return Some(Footprint::of(output));
    }
    rig.iter().find_map(|layout| {
        let matches: Vec<&Output> = layout
            .iter()
            .filter(|output| {
                output.device_id == device_id
                    && channel_name_matches_slot_alias(output.zone_name.as_deref(), segment)
            })
            .collect();
        let first = matches.first()?;
        spanning(
            matches
                .iter()
                .filter(|output| output.attachment.is_some())
                .map(|output| Footprint::of(output)),
            aspect,
        )
        .or_else(|| Some(Footprint::of(first)))
    })
}

/// Geometry the UI knows for an arrival when the rig does not place it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fallback {
    /// Nothing; the daemon places the output.
    None,
    /// A placement the output already has: where a moving output sits now,
    /// or a device's own seeded footprint. Used as it is.
    Placed(Footprint),
    /// A generic footprint for the output's topology. Only a zone's first
    /// arrivals use it, so the fit has something to cover; later arrivals
    /// leave placement to the daemon.
    Default(Footprint),
}

/// One output joining a zone.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrival {
    /// The output's id when it already exists (a move). An output the
    /// daemon will mint has none.
    pub output_id: Option<String>,
    pub device_id: String,
    pub segment: Option<String>,
    pub fallback: Fallback,
}

/// Decide where each arrival lands in a zone, given the rig, the zone's
/// current members, and the canvas extent. `None` leaves an arrival to the
/// daemon.
///
/// An empty zone fits all of its arrivals to the canvas together. A zone
/// with members carries rig placements through the scale and offset that
/// map those members' rig placements onto their zone placements (identity
/// when none of them is in the rig), so the newcomer keeps its physical
/// relation to them. Fallback placements land as they are. Nothing lands
/// off the canvas.
#[must_use]
pub fn place_arrivals(
    rig: &[Vec<Output>],
    members: &[Output],
    arrivals: &[Arrival],
    canvas: (u32, u32),
) -> Vec<Option<Footprint>> {
    let aspect = layout_geometry::canvas_pixel_aspect(canvas.0, canvas.1);
    let first = members.is_empty();
    let resolved: Vec<Option<(Footprint, bool)>> = arrivals
        .iter()
        .map(|arrival| {
            let physical = rig_footprint(
                rig,
                arrival.output_id.as_deref(),
                &arrival.device_id,
                arrival.segment.as_deref(),
                aspect,
            );
            physical
                .map(|footprint| (footprint, true))
                .or(match arrival.fallback {
                    Fallback::Placed(footprint) => Some((footprint, false)),
                    Fallback::Default(footprint) if first => Some((footprint, false)),
                    Fallback::Default(_) | Fallback::None => None,
                })
        })
        .collect();

    if first {
        let Some(bounds) = union_of(
            resolved
                .iter()
                .flatten()
                .map(|(footprint, _)| footprint.bounds(aspect)),
        ) else {
            return vec![None; arrivals.len()];
        };
        let fit = Similarity::fit(bounds, aspect);
        return resolved
            .into_iter()
            .map(|landing| landing.map(|(footprint, _)| fit.apply(footprint, aspect)))
            .collect();
    }

    let (rig_bounds, zone_bounds): (Vec<Bounds>, Vec<Bounds>) = members
        .iter()
        .filter_map(|member| {
            let rig_placement = rig_footprint(
                rig,
                Some(&member.id),
                &member.device_id,
                member.zone_name.as_deref(),
                aspect,
            )?;
            Some((
                rig_placement.bounds(aspect),
                Footprint::of(member).bounds(aspect),
            ))
        })
        .unzip();
    let carry = match (union_of(rig_bounds), union_of(zone_bounds)) {
        (Some(rig), Some(zone)) => Similarity::between(rig, zone),
        _ => Similarity::IDENTITY,
    };
    resolved
        .into_iter()
        .map(|landing| {
            landing.map(|(footprint, physical)| {
                let landed = if physical {
                    carry.apply(footprint, aspect)
                } else {
                    footprint
                };
                keep_on_canvas(landed, aspect)
            })
        })
        .collect()
}

/// Scale and center `outputs` as one group so they fill the canvas inside
/// [`FIT_MARGIN`], keeping their arrangement. Reports whether there was
/// anything to fit.
pub fn fit_outputs(outputs: &mut [Output], canvas: (u32, u32)) -> bool {
    let aspect = layout_geometry::canvas_pixel_aspect(canvas.0, canvas.1);
    let Some(bounds) = union_of(
        outputs
            .iter()
            .map(|output| Footprint::of(output).bounds(aspect)),
    ) else {
        return false;
    };
    let fit = Similarity::fit(bounds, aspect);
    for output in outputs {
        fit.apply(Footprint::of(output), aspect).apply(output);
    }
    true
}

/// The segments of a device that carry LEDs, the ones an assignment mints.
fn light_segments(device: &DeviceSummary) -> Vec<SegmentSummary> {
    device
        .segments
        .iter()
        .filter(|segment| {
            segment.led_count > 0
                && !matches!(
                    segment.topology_hint,
                    Some(SegmentTopologySummary::Display { .. })
                )
        })
        .cloned()
        .collect()
}

/// Default footprints for a device's segments, laid out in a loose grid so
/// a zone's first arrival fits as a readable arrangement rather than a
/// stack.
fn default_arrangement(
    device: &DeviceSummary,
    segments: &[SegmentSummary],
    canvas: (u32, u32),
) -> Vec<Footprint> {
    let aspect = layout_geometry::canvas_pixel_aspect(canvas.0, canvas.1);
    let visuals: Vec<_> = segments
        .iter()
        .map(|segment| {
            layout_geometry::default_zone_visuals(
                &device.name,
                Some(segment),
                usize::try_from(device.total_leds).unwrap_or(usize::MAX),
                canvas.0,
                canvas.1,
            )
        })
        .collect();
    let cell_width = visuals
        .iter()
        .map(|visual| visual.size.x * aspect)
        .fold(MIN_EXTENT, f32::max)
        * (1.0 + DEFAULT_GAP);
    let cell_height = visuals
        .iter()
        .map(|visual| visual.size.y)
        .fold(MIN_EXTENT, f32::max)
        * (1.0 + DEFAULT_GAP);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "segment counts are tiny"
    )]
    let columns = (visuals.len() as f32).sqrt().ceil().max(1.0) as usize;
    visuals
        .into_iter()
        .enumerate()
        .map(|(index, visual)| {
            #[allow(clippy::cast_precision_loss, reason = "segment counts are tiny")]
            let (column, row) = ((index % columns) as f32, (index / columns) as f32);
            Footprint {
                position: NormalizedPosition::new(
                    (column + 0.5) * cell_width / aspect,
                    (row + 0.5) * cell_height,
                ),
                size: visual.size,
                rotation: 0.0,
                scale: 1.0,
                orientation: visual.orientation,
            }
        })
        .collect()
}

/// Placement hints for assigning every light segment of `device` to the
/// zone `zone_id`: rig positions carried into the zone, a fit for a zone's
/// first devices, a device's own seeded footprint when the rig does not
/// know it, and nothing (the daemon's placement) otherwise.
#[must_use]
pub fn assignment_hints(
    scene: &SceneDocument,
    active_layout: Option<&SpatialLayout>,
    device: &DeviceSummary,
    zone_id: &str,
    canvas: (u32, u32),
) -> Vec<MemberPlacementHint> {
    let Some(zone) = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == zone_id)
    else {
        return Vec::new();
    };
    let segments = light_segments(device);
    if segments.is_empty() {
        return Vec::new();
    }
    let seeded = layout_geometry::seeded_device_layout(
        &device.layout_device_id,
        &device.name,
        &segments,
        canvas.0,
        canvas.1,
        0,
    );
    let arrivals: Vec<Arrival> = segments
        .iter()
        .zip(default_arrangement(device, &segments, canvas))
        .map(|(segment, default)| {
            let seed = seeded.as_ref().and_then(|seeded| {
                seeded
                    .zones
                    .iter()
                    .find(|output| output.zone_name.as_deref() == Some(segment.name.as_str()))
                    .map(Footprint::of)
            });
            Arrival {
                output_id: None,
                device_id: device.layout_device_id.clone(),
                segment: Some(segment.name.clone()),
                fallback: seed.map_or(Fallback::Default(default), Fallback::Placed),
            }
        })
        .collect();
    let landings = place_arrivals(
        &rig_layouts(scene, active_layout),
        &api::zone_outputs(zone),
        &arrivals,
        canvas,
    );
    arrivals
        .into_iter()
        .zip(landings)
        .filter_map(|(arrival, landing)| landing.map(|footprint| footprint.hint(arrival.segment)))
        .collect()
}

/// Membership edits that move the named outputs into `target_zone_id` from
/// whichever zones hold them, each landing per [`place_arrivals`]. An
/// output the rig does not know keeps the placement it has now.
#[must_use]
pub fn move_edits(
    scene: &SceneDocument,
    active_layout: Option<&SpatialLayout>,
    target_zone_id: &str,
    output_ids: &[String],
    canvas: (u32, u32),
) -> Vec<MemberEdit> {
    let Some(target) = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == target_zone_id)
    else {
        return Vec::new();
    };
    let moving: Vec<MemberState> = scene
        .zones
        .iter()
        .filter(|zone| zone.id != target.id)
        .flat_map(|zone| {
            api::zone_outputs(zone)
                .into_iter()
                .enumerate()
                .filter(|(_, output)| output_ids.contains(&output.id))
                .map(|(index, output)| MemberState {
                    zone_id: zone.id,
                    output,
                    index,
                })
        })
        .collect();
    let arrivals: Vec<Arrival> = moving
        .iter()
        .map(|state| Arrival {
            output_id: Some(state.output.id.clone()),
            device_id: state.output.device_id.clone(),
            segment: state.output.zone_name.clone(),
            fallback: Fallback::Placed(Footprint::of(&state.output)),
        })
        .collect();
    let landings = place_arrivals(
        &rig_layouts(scene, active_layout),
        &api::zone_outputs(target),
        &arrivals,
        canvas,
    );
    moving
        .into_iter()
        .zip(landings)
        .enumerate()
        .map(|(offset, (before, landing))| {
            let mut output = before.output.clone();
            if let Some(footprint) = landing {
                footprint.apply(&mut output);
            }
            MemberEdit {
                before: Some(before),
                after: Some(MemberState {
                    zone_id: target.id,
                    output,
                    index: target.members.len() + offset,
                }),
            }
        })
        .collect()
}
