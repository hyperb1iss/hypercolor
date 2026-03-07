//! Spatial layout types — zones, positions, topologies.
//!
//! Everything lives in normalized `[0.0, 1.0]` canvas space.
//! The spatial engine maps effect canvas pixels to physical LED positions,
//! bridging beautiful pixels and physical photons.

use serde::{Deserialize, Serialize};

// ── NormalizedPosition ──────────────────────────────────────────────────────

/// A position in normalized `[0.0, 1.0]` canvas space.
///
/// - `(0.0, 0.0)` = top-left corner of the canvas
/// - `(1.0, 1.0)` = bottom-right corner of the canvas
/// - `(0.5, 0.5)` = center of the canvas
///
/// Values outside `[0.0, 1.0]` are permitted — they represent positions
/// beyond the canvas bounds and are handled by [`EdgeBehavior`].
///
/// Used for zone positions and sizes on the canvas, LED positions within
/// a zone's bounding box, and space regions in multi-room layouts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormalizedPosition {
    /// Horizontal position. 0.0 = left edge, 1.0 = right edge.
    pub x: f32,
    /// Vertical position. 0.0 = top edge, 1.0 = bottom edge.
    pub y: f32,
}

impl NormalizedPosition {
    /// Create a new normalized position.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Create from pixel coordinates given canvas dimensions.
    ///
    /// Maps pixel centers to normalized space: pixel 0 maps to 0.0,
    /// pixel `(W-1)` maps to 1.0. A 1-pixel canvas maps to 0.5.
    #[must_use]
    #[allow(clippy::as_conversions, clippy::cast_precision_loss)]
    pub fn from_pixel(px: f32, py: f32, canvas_width: u32, canvas_height: u32) -> Self {
        Self {
            x: if canvas_width <= 1 {
                0.5
            } else {
                px / (canvas_width - 1) as f32
            },
            y: if canvas_height <= 1 {
                0.5
            } else {
                py / (canvas_height - 1) as f32
            },
        }
    }

    /// Convert to fractional pixel coordinates suitable for bilinear sampling.
    ///
    /// Returns values in the range `[0.0, W-1] x [0.0, H-1]`.
    #[must_use]
    #[allow(clippy::as_conversions, clippy::cast_precision_loss)]
    pub fn to_pixel(self, canvas_width: u32, canvas_height: u32) -> (f32, f32) {
        (
            self.x * (canvas_width.saturating_sub(1)) as f32,
            self.y * (canvas_height.saturating_sub(1)) as f32,
        )
    }

    /// Convert to integer pixel coordinates (nearest pixel), clamped to canvas bounds.
    #[must_use]
    #[allow(
        clippy::as_conversions,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn to_pixel_rounded(self, canvas_width: u32, canvas_height: u32) -> (u32, u32) {
        let (px, py) = self.to_pixel(canvas_width, canvas_height);
        (
            px.round()
                .clamp(0.0, (canvas_width.saturating_sub(1)) as f32) as u32,
            py.round()
                .clamp(0.0, (canvas_height.saturating_sub(1)) as f32) as u32,
        )
    }

    /// Linearly interpolate between two positions.
    #[must_use]
    pub fn lerp(a: Self, b: Self, t: f32) -> Self {
        Self {
            x: a.x + (b.x - a.x) * t,
            y: a.y + (b.y - a.y) * t,
        }
    }

    /// Euclidean distance between two normalized positions.
    #[must_use]
    pub fn distance(a: Self, b: Self) -> f32 {
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        (dx * dx + dy * dy).sqrt()
    }

    /// Clamp both components to `[0.0, 1.0]`.
    #[must_use]
    pub fn clamp_to_canvas(self) -> Self {
        Self {
            x: self.x.clamp(0.0, 1.0),
            y: self.y.clamp(0.0, 1.0),
        }
    }

    /// Check if the position is within the `[0.0, 1.0]` canvas bounds.
    #[must_use]
    pub fn is_on_canvas(&self) -> bool {
        self.x >= 0.0 && self.x <= 1.0 && self.y >= 0.0 && self.y <= 1.0
    }
}

impl Default for NormalizedPosition {
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

// ── NormalizedRect ──────────────────────────────────────────────────────────

/// Normalized rectangle in `[0.0, 1.0]` canvas space.
///
/// Used for space regions in multi-room layouts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormalizedRect {
    /// Left edge x-coordinate.
    pub x: f32,
    /// Top edge y-coordinate.
    pub y: f32,
    /// Width as fraction of canvas.
    pub width: f32,
    /// Height as fraction of canvas.
    pub height: f32,
}

// ── LedTopology ─────────────────────────────────────────────────────────────

/// LED arrangement within a zone's bounding rectangle.
///
/// Each variant computes zone-local positions in normalized `[0.0, 1.0]` space.
/// The topology determines how many LEDs exist and where they sit within
/// the zone's rectangular bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LedTopology {
    /// Linear strip: LEDs in a straight line across the zone.
    ///
    /// The strip runs along one axis; the perpendicular axis is fixed at 0.5
    /// (the zone midline).
    Strip {
        /// Total number of LEDs.
        count: u32,
        /// Which direction LED index 0 starts from.
        direction: StripDirection,
    },

    /// 2D grid of LEDs (WLED matrix, Strimer, LED panel).
    ///
    /// Row-major indexing. The `serpentine` flag affects output buffer
    /// ordering only, NOT spatial positions.
    Matrix {
        /// Columns in the grid.
        width: u32,
        /// Rows in the grid.
        height: u32,
        /// Alternating row direction for serpentine wiring.
        serpentine: bool,
        /// Which corner is LED index 0.
        start_corner: Corner,
    },

    /// LEDs arranged in a circle (fan ring, LED halo).
    ///
    /// When the zone is non-square, the ring becomes an ellipse.
    Ring {
        /// Number of LEDs on the ring.
        count: u32,
        /// Angle of LED 0 in radians. 0 = right (3 o'clock).
        start_angle: f32,
        /// Clockwise or counter-clockwise winding.
        direction: Winding,
    },

    /// Concentric rings (dual-ring fans like Corsair QL120).
    ///
    /// LEDs are emitted ring-by-ring (outermost first).
    ConcentricRings {
        /// Ring definitions from outermost to innermost.
        rings: Vec<RingDef>,
    },

    /// Rectangular perimeter loop (monitor backlight, ambilight-style).
    ///
    /// LEDs trace the rectangular perimeter of the zone.
    PerimeterLoop {
        /// LED count on top edge.
        top: u32,
        /// LED count on right edge.
        right: u32,
        /// LED count on bottom edge.
        bottom: u32,
        /// LED count on left edge.
        left: u32,
        /// Which corner begins the LED chain.
        start_corner: Corner,
        /// Traversal direction.
        direction: Winding,
    },

    /// Single point source (smart bulbs, single-LED indicators).
    ///
    /// Always produces exactly 1 LED at zone center `(0.5, 0.5)`.
    Point,

    /// Arbitrary LED positions defined manually or imported.
    ///
    /// Positions are normalized `[0.0, 1.0]` within the zone bounding box.
    Custom {
        /// Directly-stored LED positions.
        positions: Vec<NormalizedPosition>,
    },
}

impl LedTopology {
    /// Returns the number of LEDs this topology produces.
    #[must_use]
    pub fn led_count(&self) -> u32 {
        match self {
            Self::Strip { count, .. } | Self::Ring { count, .. } => *count,
            Self::Matrix { width, height, .. } => width * height,
            Self::ConcentricRings { rings } => rings.iter().map(|r| r.count).sum(),
            Self::PerimeterLoop {
                top,
                right,
                bottom,
                left,
                ..
            } => top + right + bottom + left,
            Self::Point => 1,
            #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
            Self::Custom { positions } => positions.len() as u32,
        }
    }
}

/// Direction for strip LED indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StripDirection {
    /// LED 0 at the left, ascending rightward.
    LeftToRight,
    /// LED 0 at the right, ascending leftward.
    RightToLeft,
    /// LED 0 at the top, ascending downward.
    TopToBottom,
    /// LED 0 at the bottom, ascending upward.
    BottomToTop,
}

/// Corner for matrix start position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    /// Origin at top-left.
    TopLeft,
    /// Origin at top-right.
    TopRight,
    /// Origin at bottom-left.
    BottomLeft,
    /// Origin at bottom-right.
    BottomRight,
}

/// Winding direction for circular topologies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Winding {
    /// LED indices increase clockwise.
    Clockwise,
    /// LED indices increase counter-clockwise.
    CounterClockwise,
}

/// Definition for a single ring within [`LedTopology::ConcentricRings`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RingDef {
    /// Number of LEDs in this ring.
    pub count: u32,
    /// Radius as a fraction of the zone's half-size. 0.0 = center, 1.0 = zone edge.
    pub radius: f32,
    /// Angle of LED 0 in radians.
    pub start_angle: f32,
    /// Winding direction.
    pub direction: Winding,
}

/// Named collection of zones for editor organization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneGroup {
    /// Unique group identifier within a layout.
    pub id: String,
    /// Human-readable group name.
    pub name: String,
    /// Optional hex color used by the editor for visual distinction.
    #[serde(default)]
    pub color: Option<String>,
}

/// Attachment metadata carried by imported layout zones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneAttachment {
    /// Bound attachment template identifier.
    pub template_id: String,
    /// Source slot ID on the physical controller.
    pub slot_id: String,
    /// Zero-based attachment instance index within the binding.
    #[serde(default)]
    pub instance: u32,
    /// Optional spatial-order -> physical-order LED remapping.
    #[serde(default)]
    pub led_mapping: Option<Vec<u32>>,
}

// ── DeviceZone ──────────────────────────────────────────────────────────────

/// A device zone: the spatial binding between a physical device and a
/// region of the effect canvas.
///
/// The zone's bounding rectangle is defined by `position` (center) and
/// `size` (width, height), both in normalized `[0.0, 1.0]` canvas coordinates.
/// LED positions within the zone are computed from the `topology` and stored
/// in `led_positions` as zone-local normalized coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceZone {
    // ── Identity ──────────────────────────────────────────────────────
    /// Unique identifier within the layout.
    pub id: String,

    /// Human-readable name (e.g., "ATX Strimer", "Front Fan 1").
    pub name: String,

    /// Backend device identifier.
    /// Format: `"<backend>:<device_id>"` (e.g., `"hid:prism-s-1"`, `"wled:192.168.1.42"`).
    pub device_id: String,

    /// Sub-device channel or segment name (e.g., `"ch1"`, `"atx"`, `"segment-0"`).
    /// `None` for single-zone devices.
    pub zone_name: Option<String>,

    /// Group this zone belongs to. `None` means ungrouped.
    #[serde(default)]
    pub group_id: Option<String>,

    // ── Placement ─────────────────────────────────────────────────────
    /// Center position of the zone on the canvas. Normalized `[0.0, 1.0]`.
    pub position: NormalizedPosition,

    /// Zone dimensions on the canvas. Normalized `[0.0, 1.0]` relative to canvas size.
    pub size: NormalizedPosition,

    /// Rotation in radians around the zone's center point.
    /// Positive = counter-clockwise (standard math convention).
    pub rotation: f32,

    /// Scale factor applied uniformly. Default 1.0.
    #[serde(default = "default_scale")]
    pub scale: f32,

    /// Zone orientation hint for the editor. Does not affect sampling.
    pub orientation: Option<Orientation>,

    // ── Topology ──────────────────────────────────────────────────────
    /// LED arrangement within the zone's bounding rectangle.
    pub topology: LedTopology,

    /// Precomputed LED positions in zone-local normalized coordinates.
    /// Derived from `topology` during layout build — not serialized.
    #[serde(skip)]
    pub led_positions: Vec<NormalizedPosition>,

    /// Optional spatial-index -> physical-index remap applied before device writes.
    ///
    /// Attachment templates use this to preserve non-sequential wiring orders
    /// without baking transport details into topology coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_mapping: Option<Vec<u32>>,

    // ── Sampling ──────────────────────────────────────────────────────
    /// Per-zone sampling mode. `None` inherits from layout default.
    pub sampling_mode: Option<SamplingMode>,

    /// Per-zone edge behavior. `None` inherits from layout default.
    pub edge_behavior: Option<EdgeBehavior>,

    // ── Shape ─────────────────────────────────────────────────────────
    /// Shape descriptor for the editor's visual appearance.
    pub shape: Option<ZoneShape>,

    /// Shape preset ID from the device library (e.g., `"strimer-atx-24pin"`).
    pub shape_preset: Option<String>,

    /// Attachment metadata for zones imported from attachment profiles.
    #[serde(default)]
    pub attachment: Option<ZoneAttachment>,
}

fn default_scale() -> f32 {
    1.0
}

/// Visual shape of the zone in the editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape_type", rename_all = "snake_case")]
pub enum ZoneShape {
    /// Rectangular bounding box (default for strips, matrices).
    Rectangle,
    /// Circular arc or full circle (fans).
    Arc {
        /// Start angle in radians.
        start_angle: f32,
        /// Sweep angle in radians.
        sweep_angle: f32,
    },
    /// Full ring (fan rings).
    Ring,
    /// Arbitrary polygon defined by normalized vertices.
    Custom {
        /// Polygon vertices in normalized coordinates.
        vertices: Vec<NormalizedPosition>,
    },
}

/// Orientation hint for the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// Wider than tall.
    Horizontal,
    /// Taller than wide.
    Vertical,
    /// Angled placement.
    Diagonal,
    /// Radial/circular arrangement.
    Radial,
}

// ── SpatialLayout ───────────────────────────────────────────────────────────

/// Top-level spatial layout container.
///
/// Defines the complete mapping from a 2D effect canvas to the physical LED
/// positions of every connected device. All coordinates use normalized
/// `[0.0, 1.0]` space where `(0,0)` is top-left and `(1,1)` is bottom-right.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialLayout {
    // ── Identity ──────────────────────────────────────────────────────
    /// Unique layout identifier (UUID or slug).
    pub id: String,

    /// Human-readable name (e.g., "Bliss's PC Case", "Full Room").
    pub name: String,

    /// Optional description for the layout editor UI.
    pub description: Option<String>,

    // ── Canvas ────────────────────────────────────────────────────────
    /// Canvas width in pixels. Standard: 320.
    pub canvas_width: u32,

    /// Canvas height in pixels. Standard: 200.
    pub canvas_height: u32,

    // ── Zones ─────────────────────────────────────────────────────────
    /// All device zones in this layout, ordered by rendering priority.
    pub zones: Vec<DeviceZone>,

    /// Named groups for organizing zones in the editor.
    #[serde(default)]
    pub groups: Vec<ZoneGroup>,

    // ── Defaults ──────────────────────────────────────────────────────
    /// Default sampling mode for zones that don't specify one.
    #[serde(default = "default_sampling_mode")]
    pub default_sampling_mode: SamplingMode,

    /// Default edge behavior for zones that don't specify one.
    #[serde(default = "default_edge_behavior")]
    pub default_edge_behavior: EdgeBehavior,

    // ── Multi-Room ────────────────────────────────────────────────────
    /// Space hierarchy for multi-room layouts.
    /// `None` means all zones live in a flat canvas (device/desk scale).
    pub spaces: Option<Vec<SpaceDefinition>>,

    // ── Metadata ──────────────────────────────────────────────────────
    /// Schema version for forward-compatible migrations.
    pub version: u32,
}

fn default_sampling_mode() -> SamplingMode {
    SamplingMode::Bilinear
}

fn default_edge_behavior() -> EdgeBehavior {
    EdgeBehavior::Clamp
}

// ── SamplingMode ────────────────────────────────────────────────────────────

/// Sampling algorithm for canvas-to-LED color extraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SamplingMode {
    /// Snap to nearest integer pixel. O(1), 1 pixel read.
    Nearest,

    /// Bilinear interpolation of 4 surrounding pixels. O(1), 4 pixel reads.
    Bilinear,

    /// Flat average of a rectangular region. O(1) with summed-area table.
    AreaAverage {
        /// Half-width of the averaging rectangle in pixels.
        radius_x: f32,
        /// Half-height of the averaging rectangle in pixels.
        radius_y: f32,
    },

    /// Gaussian-weighted average for natural ambient falloff.
    GaussianArea {
        /// Standard deviation of the Gaussian kernel.
        sigma: f32,
        /// Kernel half-size in pixels (full kernel = `(2*radius+1)^2`).
        radius: u32,
    },
}

// ── EdgeBehavior ────────────────────────────────────────────────────────────

/// Edge behavior for out-of-bounds LED positions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeBehavior {
    /// Clamp coordinates to canvas bounds (default).
    Clamp,

    /// Wrap around to the opposite edge for seamless loop effects.
    Wrap,

    /// Fade to black outside canvas bounds. `falloff` controls fade rate.
    FadeToBlack {
        /// Higher values produce a sharper cutoff.
        falloff: f32,
    },

    /// Mirror coordinates at canvas edges for symmetric reflections.
    Mirror,
}

// ── Multi-Room Types ────────────────────────────────────────────────────────

/// A physical space (room) containing a subset of zones.
///
/// Used for multi-room orchestration and per-room canvas rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceDefinition {
    /// Unique space identifier.
    pub id: String,

    /// Human-readable name (e.g., "Office", "Living Room").
    pub name: String,

    /// Physical dimensions of the room. Optional when measurements are unknown.
    pub dimensions: Option<RoomDimensions>,

    /// Region of the unified canvas this space occupies. Normalized coordinates.
    pub canvas_region: Option<NormalizedRect>,

    /// IDs of zones belonging to this space.
    pub zone_ids: Vec<String>,

    /// Neighboring spaces that share walls with this one.
    pub adjacency: Vec<RoomAdjacency>,
}

/// Physical room dimensions in centimeters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RoomDimensions {
    /// X-axis (left to right).
    pub width: f64,
    /// Y-axis (floor to ceiling).
    pub height: f64,
    /// Z-axis (front to back).
    pub depth: f64,
}

/// Declares adjacency between two rooms for cross-room effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomAdjacency {
    /// ID of the neighboring space.
    pub neighbor_id: String,
    /// Which wall is shared.
    pub shared_wall: Wall,
    /// Canvas pixels for cross-room blending zone.
    pub blend_width: u32,
}

/// Cardinal wall for room adjacency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wall {
    /// Top wall.
    North,
    /// Bottom wall.
    South,
    /// Right wall.
    East,
    /// Left wall.
    West,
}
