//! Device classes, matrix geometry, and the mapping from LIFX zones to
//! Hypercolor segments and LED positions (Spec 88 §3, §6.4, §7).

use std::net::SocketAddr;

use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint,
    FingerprintNamespace, SegmentInfo, SegmentLayoutHint,
};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition};

use crate::products::{LifxProduct, MatrixQuirk};
use crate::protocol::{FirmwareVersion, Serial, TileRecord};

/// Driver id, fingerprint namespace, and device family slug.
pub const LIFX_DRIVER_ID: &str = "lifx";

const PREFERRED_ZONE_SPAN: f32 = 0.26;
const MIN_ZONE_SPAN: f32 = 0.04;

/// Tile orientation derived from the accelerometer (Spec 88 §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    RightSideUp,
    UpsideDown,
    RotatedLeft,
    RotatedRight,
    FaceUp,
    FaceDown,
}

impl Orientation {
    /// Nearest orientation for an accelerometer reading.
    #[must_use]
    pub fn from_accel(x: i16, y: i16, z: i16) -> Self {
        if x == -1 && y == -1 && z == -1 {
            return Self::RightSideUp;
        }
        let (abs_x, abs_y, abs_z) = (x.unsigned_abs(), y.unsigned_abs(), z.unsigned_abs());
        if abs_x > abs_y && abs_x > abs_z {
            if x > 0 {
                Self::RotatedRight
            } else {
                Self::RotatedLeft
            }
        } else if abs_z > abs_x && abs_z > abs_y {
            if z > 0 { Self::FaceDown } else { Self::FaceUp }
        } else if y > 0 {
            Self::UpsideDown
        } else {
            Self::RightSideUp
        }
    }

    /// Whether device zone order already matches the upright image.
    #[must_use]
    pub const fn is_identity(self) -> bool {
        matches!(self, Self::RightSideUp | Self::FaceUp | Self::FaceDown)
    }

    /// Upright image position of device zone `(dx, dy)` on a `size` x `size`
    /// tile. Inverts the documented image-to-device remap.
    #[must_use]
    pub const fn upright_position(self, dx: u32, dy: u32, size: u32) -> (u32, u32) {
        let last = size.saturating_sub(1);
        match self {
            Self::RightSideUp | Self::FaceUp | Self::FaceDown => (dx, dy),
            Self::UpsideDown => (last.saturating_sub(dx), last.saturating_sub(dy)),
            Self::RotatedLeft => (dy, last.saturating_sub(dx)),
            Self::RotatedRight => (last.saturating_sub(dy), dx),
        }
    }
}

/// Geometry of one matrix device in a chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileGeometry {
    pub width: u8,
    pub height: u8,
    pub user_x: f32,
    pub user_y: f32,
    pub orientation: Orientation,
    pub supported_frame_buffers: u8,
}

impl TileGeometry {
    /// Zones on this tile.
    #[must_use]
    pub fn zone_count(&self) -> usize {
        usize::from(self.width) * usize::from(self.height)
    }
}

impl From<&TileRecord> for TileGeometry {
    fn from(record: &TileRecord) -> Self {
        Self {
            width: record.width,
            height: record.height,
            user_x: if record.user_x.is_finite() {
                record.user_x
            } else {
                0.0
            },
            user_y: if record.user_y.is_finite() {
                record.user_y
            } else {
                0.0
            },
            orientation: Orientation::from_accel(
                record.accel_meas_x,
                record.accel_meas_y,
                record.accel_meas_z,
            ),
            supported_frame_buffers: record.supported_frame_buffers,
        }
    }
}

/// How a device is painted.
#[derive(Debug, Clone, PartialEq)]
pub enum LifxTopology {
    /// One zone, painted with SetColor.
    SingleZone,
    /// Linear zones; `extended` selects 510 over 501.
    Multizone { zones: u16, extended: bool },
    /// Matrix tiles in chain order.
    Matrix {
        tiles: Vec<TileGeometry>,
        quirk: MatrixQuirk,
        uplight: Option<(u8, u8)>,
    },
}

impl LifxTopology {
    /// Short machine-readable class name for metadata.
    #[must_use]
    pub const fn class_name(&self) -> &'static str {
        match self {
            Self::SingleZone => "single_zone",
            Self::Multizone {
                extended: false, ..
            } => "legacy_multizone",
            Self::Multizone { extended: true, .. } => "extended_multizone",
            Self::Matrix { .. } => "matrix",
        }
    }

    /// Number of LEDs the engine addresses for this topology.
    #[must_use]
    pub fn led_count(&self) -> usize {
        match self {
            Self::SingleZone => 1,
            Self::Multizone { zones, .. } => usize::from(*zones),
            Self::Matrix { .. } => self.matrix_layout().map_or(0, |layout| layout.slots.len()),
        }
    }

    /// LED-to-zone map and segments for a matrix topology.
    #[must_use]
    pub fn matrix_layout(&self) -> Option<MatrixLayout> {
        let Self::Matrix {
            tiles,
            quirk,
            uplight,
        } = self
        else {
            return None;
        };
        Some(MatrixLayout::new(tiles, *quirk, *uplight))
    }
}

/// One engine LED's destination: a zone on a tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MatrixSlot {
    /// Position in the chain.
    pub tile: u8,
    /// Row-major zone index inside the tile's frame buffer.
    pub zone: u16,
}

/// Engine LED order for a matrix device and the segments describing it.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixLayout {
    /// Destination for each engine LED, in frame order.
    pub slots: Vec<MatrixSlot>,
    pub segments: Vec<SegmentInfo>,
}

impl MatrixLayout {
    fn new(tiles: &[TileGeometry], quirk: MatrixQuirk, uplight: Option<(u8, u8)>) -> Self {
        if let [tile] = tiles {
            if quirk == MatrixQuirk::CandleTopZone && tile.width == 5 && tile.height == 6 {
                return Self::candle();
            }
            if let Some((x, y)) = uplight
                && x < tile.width
                && y < tile.height
            {
                return Self::with_uplight(tile, x, y);
            }
            if tile.orientation.is_identity() || tile.width != tile.height {
                return Self::single_tile(tile);
            }
        }
        Self::chain(tiles)
    }

    fn single_tile(tile: &TileGeometry) -> Self {
        let zones = tile_zone_count(tile);
        Self {
            slots: (0..zones)
                .map(|zone| MatrixSlot { tile: 0, zone })
                .collect(),
            segments: vec![SegmentInfo {
                name: "Matrix".to_owned(),
                led_count: u32::from(zones),
                topology: DeviceTopologyHint::Matrix {
                    rows: u32::from(tile.height),
                    cols: u32::from(tile.width),
                },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            }],
        }
    }

    fn candle() -> Self {
        let mut slots = vec![MatrixSlot { tile: 0, zone: 0 }];
        let mut coordinates = vec![(2, 0)];
        for y in 1..6_u16 {
            for x in 0..5_u16 {
                slots.push(MatrixSlot {
                    tile: 0,
                    zone: y * 5 + x,
                });
                coordinates.push((u32::from(x), u32::from(y)));
            }
        }
        Self {
            segments: vec![SegmentInfo {
                name: "Candle".to_owned(),
                led_count: u32::try_from(slots.len()).unwrap_or(u32::MAX),
                topology: DeviceTopologyHint::Custom,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: Some(SegmentLayoutHint::custom_grid(5, 6, &coordinates)),
            }],
            slots,
        }
    }

    fn with_uplight(tile: &TileGeometry, uplight_x: u8, uplight_y: u8) -> Self {
        let width = u16::from(tile.width);
        let uplight_zone = u16::from(uplight_y) * width + u16::from(uplight_x);
        let mut slots = Vec::with_capacity(tile.zone_count());
        let mut coordinates = Vec::with_capacity(tile.zone_count());
        for zone in 0..tile_zone_count(tile) {
            if zone == uplight_zone {
                continue;
            }
            slots.push(MatrixSlot { tile: 0, zone });
            coordinates.push((u32::from(zone % width), u32::from(zone / width)));
        }
        let downlight = SegmentInfo {
            name: "Downlight".to_owned(),
            led_count: u32::try_from(slots.len()).unwrap_or(u32::MAX),
            topology: DeviceTopologyHint::Custom,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: Some(SegmentLayoutHint::custom_grid(
                u32::from(tile.width),
                u32::from(tile.height),
                &coordinates,
            )),
        };
        slots.push(MatrixSlot {
            tile: 0,
            zone: uplight_zone,
        });
        let uplight = SegmentInfo {
            name: "Uplight".to_owned(),
            led_count: 1,
            topology: DeviceTopologyHint::Point,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: Some(SegmentLayoutHint::default().co_located()),
        };
        Self {
            slots,
            segments: vec![downlight, uplight],
        }
    }

    fn chain(tiles: &[TileGeometry]) -> Self {
        let centers = chain_centers(tiles);
        let mut slots = Vec::new();
        let mut points = Vec::new();
        for (index, (tile, center)) in tiles.iter().zip(&centers).enumerate() {
            let tile_index = u8::try_from(index).unwrap_or(u8::MAX);
            let width = u32::from(tile.width);
            let height = u32::from(tile.height);
            let unit = 1.0 / f32_from(width);
            let half_height = 0.5 * f32_from(height) * unit;
            for zone in 0..tile_zone_count(tile) {
                let dx = u32::from(zone) % width;
                let dy = u32::from(zone) / width;
                let (x, y) = if width == height {
                    tile.orientation.upright_position(dx, dy, width)
                } else {
                    (dx, dy)
                };
                slots.push(MatrixSlot {
                    tile: tile_index,
                    zone,
                });
                points.push((
                    center.0 - 0.5 + (f32_from(x) + 0.5) * unit,
                    center.1 - half_height + (f32_from(y) + 0.5) * unit,
                ));
            }
        }

        let bounds = chain_bounds(tiles, &centers);
        let span_x = (bounds.max_x - bounds.min_x).max(f32::EPSILON);
        let span_y = (bounds.max_y - bounds.min_y).max(f32::EPSILON);
        let positions = points
            .iter()
            .map(|(x, y)| {
                NormalizedPosition::new(
                    ((x - bounds.min_x) / span_x).clamp(0.0, 1.0),
                    ((y - bounds.min_y) / span_y).clamp(0.0, 1.0),
                )
            })
            .collect();
        let aspect = span_x / span_y;
        let size = if aspect >= 1.0 {
            NormalizedPosition::new(
                PREFERRED_ZONE_SPAN,
                (PREFERRED_ZONE_SPAN / aspect).max(MIN_ZONE_SPAN),
            )
        } else {
            NormalizedPosition::new(
                (PREFERRED_ZONE_SPAN * aspect).max(MIN_ZONE_SPAN),
                PREFERRED_ZONE_SPAN,
            )
        };

        Self {
            segments: vec![SegmentInfo {
                name: "Tiles".to_owned(),
                led_count: u32::try_from(slots.len()).unwrap_or(u32::MAX),
                topology: DeviceTopologyHint::Custom,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: Some(
                    SegmentLayoutHint {
                        topology: Some(LedTopology::Custom { positions }),
                        ..SegmentLayoutHint::default()
                    }
                    .with_size(size),
                ),
            }],
            slots,
        }
    }
}

struct Bounds {
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

/// Tile centers on the canvas in tile widths. LIFX `user_y` is taken to
/// grow upward, so it is negated. Chains whose tiles all share one center
/// (never arranged) are laid out left to right instead.
fn chain_centers(tiles: &[TileGeometry]) -> Vec<(f32, f32)> {
    let arranged = tiles.windows(2).any(|pair| {
        (pair[0].user_x - pair[1].user_x).abs() > f32::EPSILON
            || (pair[0].user_y - pair[1].user_y).abs() > f32::EPSILON
    });
    tiles
        .iter()
        .enumerate()
        .map(|(index, tile)| {
            if arranged {
                (tile.user_x, -tile.user_y)
            } else {
                (f32_from(u32::try_from(index).unwrap_or(u32::MAX)), 0.0)
            }
        })
        .collect()
}

fn chain_bounds(tiles: &[TileGeometry], centers: &[(f32, f32)]) -> Bounds {
    let mut bounds = Bounds {
        min_x: f32::INFINITY,
        max_x: f32::NEG_INFINITY,
        min_y: f32::INFINITY,
        max_y: f32::NEG_INFINITY,
    };
    for (tile, (x, y)) in tiles.iter().zip(centers) {
        let half_height = 0.5 * f32::from(tile.height) / f32::from(tile.width.max(1));
        bounds.min_x = bounds.min_x.min(x - 0.5);
        bounds.max_x = bounds.max_x.max(x + 0.5);
        bounds.min_y = bounds.min_y.min(y - half_height);
        bounds.max_y = bounds.max_y.max(y + half_height);
    }
    if !bounds.min_x.is_finite() {
        bounds = Bounds {
            min_x: 0.0,
            max_x: 1.0,
            min_y: 0.0,
            max_y: 1.0,
        };
    }
    bounds
}

fn tile_zone_count(tile: &TileGeometry) -> u16 {
    u16::from(tile.width) * u16::from(tile.height)
}

fn f32_from(value: u32) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

/// Everything learned about one device by probing it.
#[derive(Debug, Clone, PartialEq)]
pub struct LifxDeviceProfile {
    pub serial: Serial,
    pub addr: SocketAddr,
    pub label: String,
    pub vendor_id: u32,
    pub product_id: u32,
    pub product: Option<&'static LifxProduct>,
    pub firmware: FirmwareVersion,
    pub topology: LifxTopology,
}

impl LifxDeviceProfile {
    /// Stable device id minted from the serial.
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        fingerprint_for_serial(self.serial).stable_device_id()
    }

    /// Display name: the device label, else the product name, else the serial.
    #[must_use]
    pub fn display_name(&self) -> String {
        if !self.label.trim().is_empty() {
            return self.label.trim().to_owned();
        }
        self.product.map_or_else(
            || format!("LIFX {}", self.serial),
            |product| format!("{} {}", product.name, short_serial(self.serial)),
        )
    }

    /// Product name for the model field.
    #[must_use]
    pub fn model_name(&self) -> String {
        self.product.map_or_else(
            || format!("LIFX product {}", self.product_id),
            |product| product.name.to_owned(),
        )
    }

    /// Whether hue and saturation reach the light.
    #[must_use]
    pub fn color_capable(&self) -> bool {
        self.product.is_none_or(LifxProduct::color_capable)
    }

    /// Kelvin range output is clamped to.
    #[must_use]
    pub fn kelvin_range(&self) -> (u16, u16) {
        self.product.map_or(
            crate::products::FALLBACK_KELVIN_RANGE,
            LifxProduct::kelvin_range_or_fallback,
        )
    }

    /// Build the `DeviceInfo` the host tracks for this device.
    #[must_use]
    pub fn device_info(&self, max_fps: u32) -> DeviceInfo {
        let segments = segments_for(&self.topology);
        let led_count = segments.iter().map(|segment| segment.led_count).sum();
        DeviceInfo {
            id: self.device_id(),
            name: self.display_name(),
            vendor: "LIFX".to_owned(),
            family: DeviceFamily::new_static(LIFX_DRIVER_ID, "LIFX"),
            model: Some(self.model_name()),
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native(LIFX_DRIVER_ID, LIFX_DRIVER_ID, ConnectionType::Network),
            segments,
            firmware_version: Some(self.firmware.to_string()),
            capabilities: DeviceCapabilities {
                led_count,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps,
                color_space: DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        }
    }
}

/// Fingerprint for a LIFX serial.
#[must_use]
pub fn fingerprint_for_serial(serial: Serial) -> DeviceFingerprint {
    DeviceFingerprint::mint(
        FingerprintNamespace::Net,
        LIFX_DRIVER_ID,
        &serial.to_string(),
    )
}

/// Segments describing a topology, in engine LED order.
#[must_use]
pub fn segments_for(topology: &LifxTopology) -> Vec<SegmentInfo> {
    match topology {
        LifxTopology::SingleZone => vec![SegmentInfo {
            name: "Light".to_owned(),
            led_count: 1,
            topology: DeviceTopologyHint::Point,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        LifxTopology::Multizone { zones, .. } => vec![SegmentInfo {
            name: "Zones".to_owned(),
            led_count: u32::from(*zones),
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        LifxTopology::Matrix { .. } => topology
            .matrix_layout()
            .map(|layout| layout.segments)
            .unwrap_or_default(),
    }
}

fn short_serial(serial: Serial) -> String {
    let full = serial.to_string();
    full[full.len().saturating_sub(4)..].to_owned()
}
