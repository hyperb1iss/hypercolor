//! Where a layer's source lands on its zone's canvas.
//!
//! A layer with a scope fills the frame of its outputs instead of the zone
//! canvas: one output's own placed rectangle, rotation included, or the
//! bounding box of several. The layer's transform then places the source
//! inside that frame. [`LayerPlacement`] carries both as two affines, zone
//! pixels to frame units and frame units to source pixels, and it is the
//! only description of that mapping: the compositor draws through it and
//! input inverts a touch through it, so the point an effect is told about
//! is the point its pixels land on.
//!
//! Spaces, in the order a source pixel travels: source pixels, frame pixels
//! (the frame's own axes, sized in zone pixels), frame units (the unit
//! square), normalized canvas units (where output placements live), and zone
//! pixels. Pixel spaces are continuous, with pixel `i` covering `[i, i + 1)`.

use hypercolor_types::layer::{LayerScope, LayerTransform};
use hypercolor_types::spatial::{NormalizedPosition, Output, SpatialLayout};
use hypercolor_types::viewport::FitMode;

/// Shortest side a framed layer renders at, in pixels.
const MIN_RENDER_SIDE: u32 = 16;

/// How far outside the crop, in source pixels, a point still counts as on
/// the source. The frame's edge maps onto the crop's edge exactly, and the
/// arithmetic that gets there can land a hair past it. The compose shader
/// holds the same number.
const CROP_SLACK: f32 = 0.01;

/// Row-major 2 by 3 affine: `x' = m[0] x + m[1] y + m[2]`,
/// `y' = m[3] x + m[4] y + m[5]`.
type Affine = [f32; 6];

/// The rectangle of a zone's canvas a layer's source fills, in normalized
/// canvas units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerFrame {
    /// Centre of the rectangle.
    pub center: NormalizedPosition,
    /// Width and height along the rectangle's own axes.
    pub extent: NormalizedPosition,
    /// Rotation of those axes in normalized canvas units, in radians.
    pub rotation: f32,
}

impl LayerFrame {
    /// The whole zone canvas: the frame of a layer with no scope.
    pub const ZONE: Self = Self {
        center: NormalizedPosition { x: 0.5, y: 0.5 },
        extent: NormalizedPosition { x: 1.0, y: 1.0 },
        rotation: 0.0,
    };

    /// One output's placed rectangle, rotation included, so a layer framed
    /// to it is upright relative to the device. `None` when the output has
    /// no finite, positive extent.
    #[must_use]
    pub fn of_output(output: &Output) -> Option<Self> {
        let frame = Self {
            center: output.position,
            extent: NormalizedPosition::new(
                output.size.x * output.scale,
                output.size.y * output.scale,
            ),
            rotation: output.rotation,
        };
        frame.is_placeable().then_some(frame)
    }

    /// The frame of the outputs of `layout` that `scope` names: the one
    /// output's own rectangle, or the axis-aligned bounding box of several.
    /// `None` when the scope names no output the layout places.
    #[must_use]
    pub fn of_scope(layout: &SpatialLayout, scope: &LayerScope) -> Option<Self> {
        let mut frames =
            scoped_outputs(layout, scope).filter_map(|(_, output)| Self::of_output(output));
        let first = frames.next()?;
        let Some(second) = frames.next() else {
            return Some(first);
        };
        let mut bounds = first.bounds();
        for frame in std::iter::once(second).chain(frames) {
            let other = frame.bounds();
            bounds = [
                bounds[0].min(other[0]),
                bounds[1].min(other[1]),
                bounds[2].max(other[2]),
                bounds[3].max(other[3]),
            ];
        }
        let frame = Self {
            center: NormalizedPosition::new(
                f32::midpoint(bounds[0], bounds[2]),
                f32::midpoint(bounds[1], bounds[3]),
            ),
            extent: NormalizedPosition::new(bounds[2] - bounds[0], bounds[3] - bounds[1]),
            rotation: 0.0,
        };
        frame.is_placeable().then_some(frame)
    }

    /// The frame's width and height in zone pixels: the lengths of its two
    /// edges once placed on a `zone_size` canvas. On a canvas that is not
    /// square a rotated frame's edges are not the extent times the canvas
    /// size, because the rotation happens in normalized units.
    #[must_use]
    pub fn pixel_size(&self, zone_size: (u32, u32)) -> Option<[f32; 2]> {
        let (edge_x, edge_y) = self.pixel_edges(zone_size)?;
        Some([edge_x[0].hypot(edge_x[1]), edge_y[0].hypot(edge_y[1])])
    }

    /// The size a layer framed here renders at: the frame's aspect ratio
    /// with its long side equal to the zone canvas's long side, and its
    /// short side an even number of pixels, never under sixteen.
    #[must_use]
    pub fn render_extent(&self, zone_size: (u32, u32)) -> Option<(u32, u32)> {
        let [width, height] = self.pixel_size(zone_size)?;
        let long = zone_size.0.max(zone_size.1);
        let short_for = |ratio: f32| {
            let short = even_round(to_f32(long) * ratio).max(MIN_RENDER_SIDE);
            short.min(long.max(MIN_RENDER_SIDE))
        };
        Some(if width >= height {
            (long, short_for(height / width))
        } else {
            (short_for(width / height), long)
        })
    }

    fn is_placeable(&self) -> bool {
        [
            self.center.x,
            self.center.y,
            self.extent.x,
            self.extent.y,
            self.rotation,
        ]
        .iter()
        .all(|value| value.is_finite())
            && self.extent.x > 0.0
            && self.extent.y > 0.0
    }

    /// Axis-aligned bounds of the placed rectangle in normalized canvas
    /// units: `[min_x, min_y, max_x, max_y]`.
    fn bounds(&self) -> [f32; 4] {
        let (sin, cos) = self.rotation.sin_cos();
        let half_x = self.extent.x * 0.5;
        let half_y = self.extent.y * 0.5;
        let reach_x = (half_x * cos).abs() + (half_y * sin).abs();
        let reach_y = (half_x * sin).abs() + (half_y * cos).abs();
        [
            self.center.x - reach_x,
            self.center.y - reach_y,
            self.center.x + reach_x,
            self.center.y + reach_y,
        ]
    }

    /// The frame's two edge vectors in zone pixels.
    fn pixel_edges(&self, zone_size: (u32, u32)) -> Option<([f32; 2], [f32; 2])> {
        if zone_size.0 == 0 || zone_size.1 == 0 || !self.is_placeable() {
            return None;
        }
        let (width, height) = (to_f32(zone_size.0), to_f32(zone_size.1));
        let (sin, cos) = self.rotation.sin_cos();
        let edge_x = [self.extent.x * cos * width, self.extent.x * sin * height];
        let edge_y = [-self.extent.y * sin * width, self.extent.y * cos * height];
        Some((edge_x, edge_y))
    }

    /// Zone pixels to frame units.
    fn zone_to_frame(&self, zone_size: (u32, u32)) -> Option<Affine> {
        if zone_size.0 == 0 || zone_size.1 == 0 || !self.is_placeable() {
            return None;
        }
        let (width, height) = (to_f32(zone_size.0), to_f32(zone_size.1));
        let (sin, cos) = self.rotation.sin_cos();
        Some([
            cos / (self.extent.x * width),
            sin / (self.extent.x * height),
            0.5 - cos.mul_add(self.center.x, sin * self.center.y) / self.extent.x,
            -sin / (self.extent.y * width),
            cos / (self.extent.y * height),
            0.5 - (-sin).mul_add(self.center.x, cos * self.center.y) / self.extent.y,
        ])
    }
}

/// The outputs of `layout` that `scope` names, with their index in the
/// layout, in layout order. An id that names no output of the layout is
/// dormant and yields nothing.
pub fn scoped_outputs<'a>(
    layout: &'a SpatialLayout,
    scope: &'a LayerScope,
) -> impl Iterator<Item = (usize, &'a Output)> + 'a {
    layout
        .zones
        .iter()
        .enumerate()
        .filter(|(_, output)| scope.contains(&output.id))
}

/// How a zone frames one of its layers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LayerFraming {
    /// No scope: the source fills the zone canvas.
    Zone,
    /// The source fills the frame of the outputs its scope names.
    Framed(LayerFrame),
    /// The scope names no output the zone places, so the layer renders
    /// nothing. A scope is never widened to the zone.
    Dormant,
}

impl LayerFraming {
    /// The framing `layout` gives a layer with `scope`.
    #[must_use]
    pub fn of(layout: &SpatialLayout, scope: Option<&LayerScope>) -> Self {
        match scope {
            None => Self::Zone,
            Some(scope) => LayerFrame::of_scope(layout, scope).map_or(Self::Dormant, Self::Framed),
        }
    }

    /// The frame the layer's source fills, or `None` for a dormant layer.
    #[must_use]
    pub const fn frame(self) -> Option<LayerFrame> {
        match self {
            Self::Zone => Some(LayerFrame::ZONE),
            Self::Framed(frame) => Some(frame),
            Self::Dormant => None,
        }
    }

    /// The size a layer framed this way renders at on a `zone_size` canvas,
    /// or `None` for a dormant layer.
    #[must_use]
    pub fn render_extent(self, zone_size: (u32, u32)) -> Option<(u32, u32)> {
        match self {
            Self::Zone => Some(zone_size),
            Self::Framed(frame) => frame.render_extent(zone_size),
            Self::Dormant => None,
        }
    }

    /// Whether the layer renders nothing.
    #[must_use]
    pub const fn is_dormant(self) -> bool {
        matches!(self, Self::Dormant)
    }
}

/// How a source repeats beyond its own extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceRepeat {
    /// The source shows once; beyond the crop nothing is drawn.
    None,
    /// The source repeats.
    Tile,
    /// The source repeats, every other copy mirrored.
    Mirror,
}

/// What a placed layer shows beyond its frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameEdge {
    /// The frame's edge runs on: a pixel outside the frame shows what the
    /// nearest point of the frame's edge shows.
    ///
    /// A device's LEDs sit on the border of its rectangle, and a sampling
    /// footprint wider than a point straddles that border. Running the edge
    /// on lets a border LED read the layer and nothing of what lies around
    /// it. Only outputs the layer applies to ever sample the canvas it is
    /// drawn on, so the run-on reaches nothing else.
    Extend,
    /// Nothing shows outside the frame.
    Clip,
}

/// Where one layer's source lands on its zone's canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerPlacement {
    /// Zone pixels to frame units, row-major 2 by 3. A zone pixel is inside
    /// the frame when both results are within 0 and 1.
    pub zone_to_frame: [f32; 6],
    /// Frame units to source pixels, row-major 2 by 3.
    pub frame_to_source: [f32; 6],
    /// The source rectangle the fit mode shows, in source pixels: left, top,
    /// width, height.
    pub source_crop: [f32; 4],
    pub repeat: SourceRepeat,
    pub source_size: [u32; 2],
    pub edge: FrameEdge,
}

impl LayerPlacement {
    /// The placement of a `source_size` source framed by `frame` and placed
    /// inside it by `transform`, on a `zone_size` canvas. The frame's edge
    /// runs on beyond it.
    ///
    /// `None` when the frame or the transform is degenerate.
    #[must_use]
    pub fn new(
        frame: &LayerFrame,
        transform: LayerTransform,
        source_size: (u32, u32),
        zone_size: (u32, u32),
    ) -> Option<Self> {
        if source_size.0 == 0 || source_size.1 == 0 {
            return None;
        }
        let zone_to_frame = frame.zone_to_frame(zone_size)?;
        let [frame_width, frame_height] = frame.pixel_size(zone_size)?;
        if !(frame_width.is_finite() && frame_height.is_finite())
            || frame_width <= 0.0
            || frame_height <= 0.0
        {
            return None;
        }
        let transform = transform.normalized();
        let source = [to_f32(source_size.0), to_f32(source_size.1)];
        let fit = FitGeometry::new(transform.fit, source, [frame_width, frame_height]);
        let anchor = [
            transform.anchor.x * frame_width,
            transform.anchor.y * frame_height,
        ];
        let (sin, cos) = transform.rotation.sin_cos();
        let [scale_x, scale_y] = transform.scale;
        // Frame pixels to source pixels: undo the anchor, the rotation, and
        // the scale, then map the drawn rectangle onto the crop.
        let (gain, offset) = if fit.repeat == SourceRepeat::None {
            (
                [
                    fit.crop[2] / (scale_x * fit.draw[0]),
                    fit.crop[3] / (scale_y * fit.draw[1]),
                ],
                [
                    fit.crop[2].mul_add(0.5, fit.crop[0]),
                    fit.crop[3].mul_add(0.5, fit.crop[1]),
                ],
            )
        } else {
            ([1.0 / scale_x, 1.0 / scale_y], anchor)
        };
        let frame_px_to_source: Affine = [
            gain[0] * cos,
            gain[0] * sin,
            gain[0].mul_add(-cos.mul_add(anchor[0], sin * anchor[1]), offset[0]),
            -gain[1] * sin,
            gain[1] * cos,
            gain[1].mul_add(-(-sin).mul_add(anchor[0], cos * anchor[1]), offset[1]),
        ];
        let frame_to_frame_px: Affine = [frame_width, 0.0, 0.0, 0.0, frame_height, 0.0];
        Self {
            zone_to_frame,
            frame_to_source: compose(frame_px_to_source, frame_to_frame_px),
            source_crop: fit.crop,
            repeat: fit.repeat,
            source_size: [source_size.0, source_size.1],
            edge: FrameEdge::Extend,
        }
        .finite()
    }

    /// The placement of a zone-sized source seen through `frame`: each zone
    /// pixel inside the frame shows the source pixel at its own position,
    /// and nothing shows outside it.
    ///
    /// `None` when the frame is degenerate.
    #[must_use]
    pub fn window(frame: &LayerFrame, zone_size: (u32, u32)) -> Option<Self> {
        let zone_to_frame = frame.zone_to_frame(zone_size)?;
        let (edge_x, edge_y) = frame.pixel_edges(zone_size)?;
        let center = [
            frame.center.x * to_f32(zone_size.0),
            frame.center.y * to_f32(zone_size.1),
        ];
        Self {
            zone_to_frame,
            // Frame units back to the zone pixel they came from.
            frame_to_source: [
                edge_x[0],
                edge_y[0],
                center[0] - f32::midpoint(edge_x[0], edge_y[0]),
                edge_x[1],
                edge_y[1],
                center[1] - f32::midpoint(edge_x[1], edge_y[1]),
            ],
            source_crop: [0.0, 0.0, to_f32(zone_size.0), to_f32(zone_size.1)],
            repeat: SourceRepeat::None,
            source_size: [zone_size.0, zone_size.1],
            edge: FrameEdge::Clip,
        }
        .finite()
    }

    /// The placement by the bits of its numbers, for a key that must
    /// compare exactly.
    #[must_use]
    pub fn key(&self) -> LayerPlacementKey {
        LayerPlacementKey {
            zone_to_frame: self.zone_to_frame.map(f32::to_bits),
            frame_to_source: self.frame_to_source.map(f32::to_bits),
            source_crop: self.source_crop.map(f32::to_bits),
            repeat: self.repeat,
            source_size: self.source_size,
            edge: self.edge,
        }
    }

    fn finite(self) -> Option<Self> {
        self.zone_to_frame
            .iter()
            .chain(&self.frame_to_source)
            .all(|value| value.is_finite())
            .then_some(self)
    }

    /// Where a zone pixel falls in the frame, in frame units, or `None`
    /// when it is outside the frame.
    #[must_use]
    pub fn frame_point(&self, zone_px: [f32; 2]) -> Option<[f32; 2]> {
        let point = apply(self.zone_to_frame, zone_px);
        ((0.0..=1.0).contains(&point[0]) && (0.0..=1.0).contains(&point[1])).then_some(point)
    }

    /// Whether the layer draws on a zone pixel at all: everywhere when its
    /// edge runs on, inside the frame when it is clipped.
    #[must_use]
    pub fn covers(&self, zone_px: [f32; 2]) -> bool {
        self.edge == FrameEdge::Extend || self.frame_point(zone_px).is_some()
    }

    /// The source point a zone pixel shows, or `None` where the layer shows
    /// nothing: outside a clipped frame, or where the placed source does
    /// not reach. A pixel beyond a frame whose edge runs on shows the
    /// nearest point of that edge. A repeating source answers with the
    /// point of the copy the pixel shows.
    #[must_use]
    pub fn source_point(&self, zone_px: [f32; 2]) -> Option<[f32; 2]> {
        let point = self.unbounded_source_point(zone_px)?;
        let size = [to_f32(self.source_size[0]), to_f32(self.source_size[1])];
        match self.repeat {
            SourceRepeat::None => self.crop_contains(point).then_some(point),
            SourceRepeat::Tile => {
                Some([point[0].rem_euclid(size[0]), point[1].rem_euclid(size[1])])
            }
            SourceRepeat::Mirror => {
                Some([mirrored(point[0], size[0]), mirrored(point[1], size[1])])
            }
        }
    }

    /// The source texel a zone pixel shows: the texel holding
    /// [`Self::source_point`]. The compositor's shader takes the same steps
    /// in the same order, so both pick the same texel.
    #[must_use]
    pub fn source_texel(&self, zone_px: [f32; 2]) -> Option<[u32; 2]> {
        let point = self.unbounded_source_point(zone_px)?;
        let [width, height] = self.source_size;
        match self.repeat {
            SourceRepeat::None => self.crop_contains(point).then(|| {
                [
                    clamped_texel(point[0], width),
                    clamped_texel(point[1], height),
                ]
            }),
            SourceRepeat::Tile => Some([
                repeated_texel(point[0], width, false),
                repeated_texel(point[1], height, false),
            ]),
            SourceRepeat::Mirror => Some([
                repeated_texel(point[0], width, true),
                repeated_texel(point[1], height, true),
            ]),
        }
    }

    /// The point of the unbounded source plane a zone pixel shows.
    fn unbounded_source_point(&self, zone_px: [f32; 2]) -> Option<[f32; 2]> {
        let frame = apply(self.zone_to_frame, zone_px);
        let inside = (0.0..=1.0).contains(&frame[0]) && (0.0..=1.0).contains(&frame[1]);
        if !inside && self.edge == FrameEdge::Clip {
            return None;
        }
        let point = apply(
            self.frame_to_source,
            [frame[0].clamp(0.0, 1.0), frame[1].clamp(0.0, 1.0)],
        );
        (point[0].is_finite() && point[1].is_finite()).then_some(point)
    }

    fn crop_contains(&self, point: [f32; 2]) -> bool {
        let [left, top, width, height] = self.source_crop;
        point[0] >= left - CROP_SLACK
            && point[0] <= left + width + CROP_SLACK
            && point[1] >= top - CROP_SLACK
            && point[1] <= top + height + CROP_SLACK
    }

    /// The axis-aligned box of zone pixels that holds the frame: left, top,
    /// right, bottom. `None` when the frame cannot be inverted.
    #[must_use]
    pub fn frame_bounds(&self) -> Option<[f32; 4]> {
        let corners = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
            .map(|corner| invert(self.zone_to_frame, corner));
        let mut bounds = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for corner in corners {
            let [x, y] = corner?;
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
        }
        Some(bounds)
    }

    /// The zone pixel a source point lands on, ignoring the frame's edge
    /// and the crop: the inverse of the two affines alone. `None` when the
    /// mapping cannot be inverted.
    #[must_use]
    pub fn zone_point(&self, source_px: [f32; 2]) -> Option<[f32; 2]> {
        invert(self.zone_to_frame, invert(self.frame_to_source, source_px)?)
    }
}

/// The point `affine` maps onto `image`, or `None` when it has no inverse.
fn invert(affine: Affine, image: [f32; 2]) -> Option<[f32; 2]> {
    let determinant = affine[0].mul_add(affine[4], -(affine[1] * affine[3]));
    if !determinant.is_finite() || determinant.abs() <= f32::MIN_POSITIVE {
        return None;
    }
    let offset = [image[0] - affine[2], image[1] - affine[5]];
    let point = [
        affine[4].mul_add(offset[0], -(affine[1] * offset[1])) / determinant,
        affine[0].mul_add(offset[1], -(affine[3] * offset[0])) / determinant,
    ];
    (point[0].is_finite() && point[1].is_finite()).then_some(point)
}

/// A [`LayerPlacement`] by the bits of its numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerPlacementKey {
    zone_to_frame: [u32; 6],
    frame_to_source: [u32; 6],
    source_crop: [u32; 4],
    repeat: SourceRepeat,
    source_size: [u32; 2],
    edge: FrameEdge,
}

/// What a fit mode draws of a source into a target, before the layer's own
/// anchor, scale, and rotation.
struct FitGeometry {
    /// Size the crop is drawn at, in target pixels.
    draw: [f32; 2],
    /// The source rectangle drawn: left, top, width, height.
    crop: [f32; 4],
    repeat: SourceRepeat,
}

impl FitGeometry {
    fn new(fit: FitMode, source: [f32; 2], target: [f32; 2]) -> Self {
        let whole = [0.0, 0.0, source[0], source[1]];
        let source_aspect = source[0] / source[1];
        let target_aspect = target[0] / target[1];
        match fit {
            FitMode::Stretch => Self {
                draw: target,
                crop: whole,
                repeat: SourceRepeat::None,
            },
            FitMode::Tile => Self {
                draw: target,
                crop: whole,
                repeat: SourceRepeat::Tile,
            },
            FitMode::Mirror => Self {
                draw: target,
                crop: whole,
                repeat: SourceRepeat::Mirror,
            },
            FitMode::Contain => Self {
                draw: if target_aspect > source_aspect {
                    [target[1] * source_aspect, target[1]]
                } else {
                    [target[0], target[0] / source_aspect]
                },
                crop: whole,
                repeat: SourceRepeat::None,
            },
            FitMode::Cover => {
                let crop = if target_aspect > source_aspect {
                    let height = source[0] / target_aspect;
                    [0.0, (source[1] - height) * 0.5, source[0], height]
                } else {
                    let width = source[1] * target_aspect;
                    [(source[0] - width) * 0.5, 0.0, width, source[1]]
                };
                Self {
                    draw: target,
                    crop,
                    repeat: SourceRepeat::None,
                }
            }
        }
    }
}

/// `outer` applied after `inner`.
fn compose(outer: Affine, inner: Affine) -> Affine {
    [
        outer[0].mul_add(inner[0], outer[1] * inner[3]),
        outer[0].mul_add(inner[1], outer[1] * inner[4]),
        outer[0].mul_add(inner[2], outer[1].mul_add(inner[5], outer[2])),
        outer[3].mul_add(inner[0], outer[4] * inner[3]),
        outer[3].mul_add(inner[1], outer[4] * inner[4]),
        outer[3].mul_add(inner[2], outer[4].mul_add(inner[5], outer[5])),
    ]
}

fn apply(affine: Affine, point: [f32; 2]) -> [f32; 2] {
    [
        affine[0] * point[0] + affine[1] * point[1] + affine[2],
        affine[3] * point[0] + affine[4] * point[1] + affine[5],
    ]
}

fn mirrored(value: f32, extent: f32) -> f32 {
    let phase = value.rem_euclid(extent * 2.0);
    if phase < extent {
        phase
    } else {
        extent * 2.0 - phase
    }
}

fn clamped_texel(value: f32, extent: u32) -> u32 {
    let last = extent.saturating_sub(1);
    (value.floor().clamp(0.0, to_f32(last)) as u32).min(last)
}

/// The texel of a repeating source that holds `value`.
fn repeated_texel(value: f32, extent: u32, mirror: bool) -> u32 {
    let index = value.floor() as i64;
    let extent_i = i64::from(extent.max(1));
    let texel = if mirror && extent > 1 {
        let period = extent_i * 2;
        let phase = index.rem_euclid(period);
        if phase < extent_i {
            phase
        } else {
            period - 1 - phase
        }
    } else {
        index.rem_euclid(extent_i)
    };
    u32::try_from(texel).unwrap_or(0)
}

fn even_round(value: f32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    ((value * 0.5).round() as u32).saturating_mul(2)
}

const fn to_f32(value: u32) -> f32 {
    value as f32
}
