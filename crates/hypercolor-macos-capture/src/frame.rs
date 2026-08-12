use std::fmt;
use std::sync::Arc;

use thiserror::Error;

use crate::geometry::{
    MacosCaptureGeometry, MacosGeometryError, MacosPixelExtent, MacosPixelRect, MacosPointRect,
    MacosScale,
};

pub const MACOS_STREAM_QUEUE_DEPTH: usize = 8;

const BGRA8: u32 = 0x4247_5241;
const ARGB2101010: u32 = 0x5231_306b;
const RGBA16_FLOAT: u32 = 0x5247_6841;
const YUV420_VIDEO_RANGE: u32 = 0x3432_3076;
const YUV420_FULL_RANGE: u32 = 0x3432_3066;
const YUV44410_VIDEO_RANGE: u32 = 0x7834_3434;
const YUV44410_FULL_RANGE: u32 = 0x7866_3434;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosProtectedSourceState {
    Disabled,
    NeedsUserAction,
    PermissionDenied,
    NeedsProcessRestart,
    NeedsSelection,
    ReadyIdle,
    Starting,
    Live,
    Interrupted,
    Revoked,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosFrameStatus {
    Complete,
    Idle,
    Blank,
    Suspended,
    Started,
    Stopped,
}

impl TryFrom<i64> for MacosFrameStatus {
    type Error = MacosCaptureError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Complete),
            1 => Ok(Self::Idle),
            2 => Ok(Self::Blank),
            3 => Ok(Self::Suspended),
            4 => Ok(Self::Started),
            5 => Ok(Self::Stopped),
            _ => Err(MacosCaptureError::UnknownFrameStatus(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosCapturePixelFormat {
    Bgra8,
    Argb2101010,
    Rgba16Float,
    Yuv420VideoRange,
    Yuv420FullRange,
    Yuv44410BiPlanar,
}

impl MacosCapturePixelFormat {
    pub fn from_fourcc(fourcc: u32) -> Result<Self, MacosCaptureError> {
        match fourcc {
            BGRA8 => Ok(Self::Bgra8),
            ARGB2101010 => Ok(Self::Argb2101010),
            RGBA16_FLOAT => Ok(Self::Rgba16Float),
            YUV420_VIDEO_RANGE => Ok(Self::Yuv420VideoRange),
            YUV420_FULL_RANGE => Ok(Self::Yuv420FullRange),
            YUV44410_VIDEO_RANGE | YUV44410_FULL_RANGE => Ok(Self::Yuv44410BiPlanar),
            _ => Err(MacosCaptureError::UnsupportedPixelFormat(fourcc)),
        }
    }

    pub fn fourcc(self, range: MacosColorRange) -> Result<u32, MacosCaptureError> {
        match (self, range) {
            (Self::Bgra8, MacosColorRange::Full) => Ok(BGRA8),
            (Self::Argb2101010, MacosColorRange::Full) => Ok(ARGB2101010),
            (Self::Rgba16Float, MacosColorRange::Full) => Ok(RGBA16_FLOAT),
            (Self::Yuv420VideoRange, MacosColorRange::Video) => Ok(YUV420_VIDEO_RANGE),
            (Self::Yuv420FullRange, MacosColorRange::Full) => Ok(YUV420_FULL_RANGE),
            (Self::Yuv44410BiPlanar, MacosColorRange::Video) => Ok(YUV44410_VIDEO_RANGE),
            (Self::Yuv44410BiPlanar, MacosColorRange::Full) => Ok(YUV44410_FULL_RANGE),
            _ => Err(MacosCaptureError::ColorMetadataMismatch),
        }
    }

    fn plane_layout(self, storage: MacosPixelExtent) -> Vec<(MacosPixelExtent, u64)> {
        match self {
            Self::Bgra8 | Self::Argb2101010 => vec![(storage, 4)],
            Self::Rgba16Float => vec![(storage, 8)],
            Self::Yuv420VideoRange | Self::Yuv420FullRange => {
                let chroma = MacosPixelExtent {
                    width: storage.width.div_ceil(2),
                    height: storage.height.div_ceil(2),
                };
                vec![(storage, 1), (chroma, 2)]
            }
            Self::Yuv44410BiPlanar => vec![(storage, 2), (storage, 4)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosColorPrimaries {
    Srgb,
    DisplayP3,
    Rec2020,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosTransferFunction {
    Srgb,
    Linear,
    Pq,
    Hlg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosYuvMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosColorRange {
    Full,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosChromaLocation {
    Left,
    Center,
    TopLeft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosCaptureColorimetry {
    pub primaries: MacosColorPrimaries,
    pub transfer: MacosTransferFunction,
    pub matrix: Option<MacosYuvMatrix>,
    pub range: MacosColorRange,
    pub chroma_location: Option<MacosChromaLocation>,
}

impl MacosCaptureColorimetry {
    pub fn validate_for(self, format: MacosCapturePixelFormat) -> Result<(), MacosCaptureError> {
        let rgb = matches!(
            format,
            MacosCapturePixelFormat::Bgra8
                | MacosCapturePixelFormat::Argb2101010
                | MacosCapturePixelFormat::Rgba16Float
        );
        if rgb {
            if self.matrix.is_some()
                || self.chroma_location.is_some()
                || self.range != MacosColorRange::Full
            {
                return Err(MacosCaptureError::ColorMetadataMismatch);
            }
            return Ok(());
        }
        if self.matrix.is_none() || self.chroma_location.is_none() {
            return Err(MacosCaptureError::MissingYuvColorMetadata);
        }
        match format {
            MacosCapturePixelFormat::Yuv420VideoRange if self.range != MacosColorRange::Video => {
                Err(MacosCaptureError::ColorMetadataMismatch)
            }
            MacosCapturePixelFormat::Yuv420FullRange if self.range != MacosColorRange::Full => {
                Err(MacosCaptureError::ColorMetadataMismatch)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosRawCapturePlane {
    pub index: u32,
    pub extent: MacosPixelExtent,
    pub bytes_per_row: usize,
    pub length_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosCapturePlane {
    pub index: u32,
    pub extent: MacosPixelExtent,
    pub bytes_per_row: usize,
    pub length_bytes: u64,
}

#[derive(Clone)]
pub struct MacosCaptureSurface {
    pub iosurface_id: u32,
    pub allocation_bytes: u64,
    owner: Arc<MacosRetainedPixelBuffer>,
}

impl MacosCaptureSurface {
    #[cfg(feature = "capture-fixtures")]
    pub fn new_fixture(
        iosurface_id: u32,
        allocation_bytes: u64,
        fixture_id: u64,
    ) -> Result<Self, MacosCaptureError> {
        if iosurface_id == 0 || allocation_bytes == 0 {
            return Err(MacosCaptureError::InvalidSurface);
        }
        Ok(Self {
            iosurface_id,
            allocation_bytes,
            owner: Arc::new(MacosRetainedPixelBuffer { fixture_id }),
        })
    }

    pub fn retained_owner_count(&self) -> usize {
        Arc::strong_count(&self.owner)
    }

    #[cfg(feature = "capture-fixtures")]
    pub fn fixture_id(&self) -> u64 {
        self.owner.fixture_id
    }
}

impl fmt::Debug for MacosCaptureSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosCaptureSurface")
            .field("iosurface_id", &self.iosurface_id)
            .field("allocation_bytes", &self.allocation_bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct MacosRetainedPixelBuffer {
    #[cfg(feature = "capture-fixtures")]
    fixture_id: u64,
}

#[derive(Debug, Clone)]
pub struct MacosCaptureFrame {
    pub epoch: u64,
    pub sequence: u64,
    pub display_time: u64,
    pub storage_extent: MacosPixelExtent,
    pub planes: Arc<[MacosCapturePlane]>,
    pub pixel_format: MacosCapturePixelFormat,
    pub color: MacosCaptureColorimetry,
    pub geometry: MacosCaptureGeometry,
    pub damage: Arc<[MacosPixelRect]>,
    pub cursor_composed: bool,
    pub surface: MacosCaptureSurface,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MacosAttachment<T> {
    Missing,
    Malformed,
    Value(T),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacosRawFrameAttachments {
    pub status: MacosAttachment<i64>,
    pub display_time: MacosAttachment<u64>,
    pub display_scale_factor: MacosAttachment<f64>,
    pub content_scale: MacosAttachment<f64>,
    pub content_rect: MacosAttachment<MacosPointRect>,
    pub dirty_rects: MacosAttachment<Vec<MacosPixelRect>>,
    pub screen_rect: MacosAttachment<MacosPointRect>,
    pub bounding_rect: MacosAttachment<MacosPointRect>,
}

#[derive(Debug, Clone)]
pub struct MacosRawCompleteFrame {
    pub storage_extent: MacosPixelExtent,
    pub planes: Vec<MacosRawCapturePlane>,
    pub pixel_format_fourcc: u32,
    pub color: MacosCaptureColorimetry,
    pub cursor_composed: bool,
    pub surface: MacosCaptureSurface,
}

#[derive(Debug, Clone)]
pub struct MacosRawCaptureSample {
    pub frame: Option<MacosRawCompleteFrame>,
    pub attachments: MacosRawFrameAttachments,
}

#[derive(Debug, Clone)]
pub enum MacosFrameEvent {
    Frame(Box<MacosCaptureFrame>),
    Lifecycle(MacosFrameStatus),
}

#[derive(Debug, Clone)]
pub struct MacosFrameDecoder {
    epoch: u64,
    next_sequence: u64,
}

impl MacosFrameDecoder {
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            next_sequence: 0,
        }
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn decode(
        &mut self,
        sample: MacosRawCaptureSample,
    ) -> Result<MacosFrameEvent, MacosCaptureError> {
        let status =
            MacosFrameStatus::try_from(required(sample.attachments.status.clone(), "status")?)?;
        if status != MacosFrameStatus::Complete {
            return Ok(MacosFrameEvent::Lifecycle(status));
        }

        let MacosRawCompleteFrame {
            storage_extent,
            planes: raw_planes,
            pixel_format_fourcc,
            color,
            cursor_composed,
            surface,
        } = sample.frame.ok_or(MacosCaptureError::MissingFramePayload)?;
        let pixel_format = MacosCapturePixelFormat::from_fourcc(pixel_format_fourcc)?;
        color.validate_for(pixel_format)?;
        if pixel_format.fourcc(color.range)? != pixel_format_fourcc {
            return Err(MacosCaptureError::ColorMetadataMismatch);
        }
        let planes = validate_planes(
            storage_extent,
            pixel_format,
            raw_planes,
            surface.allocation_bytes,
        )?;
        let geometry = validate_geometry(storage_extent, &sample.attachments)?;
        let damage = validate_damage(storage_extent, &geometry, sample.attachments.dirty_rects)?;
        let display_time = required(sample.attachments.display_time, "display_time")?;
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;

        Ok(MacosFrameEvent::Frame(Box::new(MacosCaptureFrame {
            epoch: self.epoch,
            sequence,
            display_time,
            storage_extent,
            planes: planes.into(),
            pixel_format,
            color,
            geometry,
            damage: damage.into(),
            cursor_composed,
            surface,
        })))
    }
}

fn validate_planes(
    storage: MacosPixelExtent,
    format: MacosCapturePixelFormat,
    raw_planes: Vec<MacosRawCapturePlane>,
    allocation_bytes: u64,
) -> Result<Vec<MacosCapturePlane>, MacosCaptureError> {
    let expected = format.plane_layout(storage);
    if raw_planes.len() != expected.len() {
        return Err(MacosCaptureError::PlaneCount {
            expected: expected.len(),
            actual: raw_planes.len(),
        });
    }
    let mut total_length = 0_u64;
    let mut planes = Vec::with_capacity(raw_planes.len());
    for (position, (plane, (expected_extent, bytes_per_pixel))) in
        raw_planes.into_iter().zip(expected).enumerate()
    {
        if usize::try_from(plane.index).ok() != Some(position) {
            return Err(MacosCaptureError::InvalidPlaneIndex {
                expected: position,
                actual: plane.index,
            });
        }
        if plane.extent != expected_extent {
            return Err(MacosCaptureError::InvalidPlaneExtent {
                plane: plane.index,
                expected: expected_extent,
                actual: plane.extent,
            });
        }
        let minimum_stride = u64::from(expected_extent.width)
            .checked_mul(bytes_per_pixel)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let stride = u64::try_from(plane.bytes_per_row)
            .map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
        if stride < minimum_stride {
            return Err(MacosCaptureError::StrideTooSmall {
                plane: plane.index,
                minimum: minimum_stride,
                actual: stride,
            });
        }
        let minimum_length = stride
            .checked_mul(u64::from(expected_extent.height))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        if plane.length_bytes < minimum_length {
            return Err(MacosCaptureError::PlaneLengthTooSmall {
                plane: plane.index,
                minimum: minimum_length,
                actual: plane.length_bytes,
            });
        }
        total_length = total_length
            .checked_add(plane.length_bytes)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        planes.push(MacosCapturePlane {
            index: plane.index,
            extent: plane.extent,
            bytes_per_row: plane.bytes_per_row,
            length_bytes: plane.length_bytes,
        });
    }
    if total_length > allocation_bytes {
        return Err(MacosCaptureError::AllocationTooSmall {
            required: total_length,
            actual: allocation_bytes,
        });
    }
    Ok(planes)
}

fn validate_geometry(
    storage: MacosPixelExtent,
    attachments: &MacosRawFrameAttachments,
) -> Result<MacosCaptureGeometry, MacosCaptureError> {
    let display_scale_factor = MacosScale::display(required(
        attachments.display_scale_factor.clone(),
        "scale_factor",
    )?)?;
    let content_scale = MacosScale::new(required(
        attachments.content_scale.clone(),
        "content_scale",
    )?)?;
    let content_rect_points = required(attachments.content_rect.clone(), "content_rect")?;
    let content_rect_pixels = content_rect_points.to_pixel_rect(display_scale_factor)?;
    if !content_rect_pixels.fits_within(storage) {
        return Err(MacosCaptureError::GeometryOutsideStorage("content_rect"));
    }
    let screen_rect_points = optional(attachments.screen_rect.clone(), "screen_rect")?;
    let bounding_rect_points = optional(attachments.bounding_rect.clone(), "bounding_rect")?;
    let bounding_rect_pixels = bounding_rect_points
        .map(|rect| rect.to_pixel_rect(display_scale_factor))
        .transpose()?;
    if bounding_rect_pixels.is_some_and(|rect| !rect.fits_within(storage)) {
        return Err(MacosCaptureError::GeometryOutsideStorage("bounding_rect"));
    }
    Ok(MacosCaptureGeometry {
        display_scale_factor,
        content_scale,
        content_rect_points,
        content_rect_pixels,
        screen_rect_points,
        bounding_rect_points,
        bounding_rect_pixels,
    })
}

fn validate_damage(
    storage: MacosPixelExtent,
    geometry: &MacosCaptureGeometry,
    dirty_rects: MacosAttachment<Vec<MacosPixelRect>>,
) -> Result<Vec<MacosPixelRect>, MacosCaptureError> {
    match dirty_rects {
        MacosAttachment::Missing => Ok(vec![geometry.content_rect_pixels]),
        MacosAttachment::Malformed => Err(MacosCaptureError::MalformedAttachment("dirty_rects")),
        MacosAttachment::Value(rects) => rects
            .into_iter()
            .map(|rect| rect.clip_to(storage).map_err(MacosCaptureError::from))
            .collect(),
    }
}

fn required<T>(attachment: MacosAttachment<T>, name: &'static str) -> Result<T, MacosCaptureError> {
    match attachment {
        MacosAttachment::Missing => Err(MacosCaptureError::MissingAttachment(name)),
        MacosAttachment::Malformed => Err(MacosCaptureError::MalformedAttachment(name)),
        MacosAttachment::Value(value) => Ok(value),
    }
}

fn optional<T>(
    attachment: MacosAttachment<T>,
    name: &'static str,
) -> Result<Option<T>, MacosCaptureError> {
    match attachment {
        MacosAttachment::Missing => Ok(None),
        MacosAttachment::Malformed => Err(MacosCaptureError::MalformedAttachment(name)),
        MacosAttachment::Value(value) => Ok(Some(value)),
    }
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum MacosCaptureError {
    #[error("missing ScreenCaptureKit attachment: {0}")]
    MissingAttachment(&'static str),
    #[error("malformed ScreenCaptureKit attachment: {0}")]
    MalformedAttachment(&'static str),
    #[error("unknown ScreenCaptureKit frame status {0}")]
    UnknownFrameStatus(i64),
    #[error("unsupported Core Video pixel format {0:#010x}")]
    UnsupportedPixelFormat(u32),
    #[error("complete frame has no image payload")]
    MissingFramePayload,
    #[error("pixel plane count mismatch: expected {expected}, got {actual}")]
    PlaneCount { expected: usize, actual: usize },
    #[error("pixel plane index mismatch: expected {expected}, got {actual}")]
    InvalidPlaneIndex { expected: usize, actual: u32 },
    #[error("pixel plane {plane} extent mismatch: expected {expected:?}, got {actual:?}")]
    InvalidPlaneExtent {
        plane: u32,
        expected: MacosPixelExtent,
        actual: MacosPixelExtent,
    },
    #[error("pixel plane {plane} stride is {actual}, minimum is {minimum}")]
    StrideTooSmall {
        plane: u32,
        minimum: u64,
        actual: u64,
    },
    #[error("pixel plane {plane} length is {actual}, minimum is {minimum}")]
    PlaneLengthTooSmall {
        plane: u32,
        minimum: u64,
        actual: u64,
    },
    #[error("pixel plane arithmetic overflowed")]
    ArithmeticOverflow,
    #[error("IOSurface allocation is {actual} bytes, minimum is {required}")]
    AllocationTooSmall { required: u64, actual: u64 },
    #[error("pixel format and color metadata disagree")]
    ColorMetadataMismatch,
    #[error("YUV frames require matrix and chroma-location metadata")]
    MissingYuvColorMetadata,
    #[error("{0} exceeds pixel storage")]
    GeometryOutsideStorage(&'static str),
    #[error("IOSurface identity and allocation must be nonzero")]
    InvalidSurface,
    #[error("complete-frame sequence exhausted")]
    SequenceExhausted,
    #[error(transparent)]
    Geometry(#[from] MacosGeometryError),
}
