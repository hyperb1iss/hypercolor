//! Backend-neutral screen capture frame contract.

use std::any::Any;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use thiserror::Error;

/// Stable logical identity of one capture source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CaptureSourceId(Arc<str>);

impl CaptureSourceId {
    /// Create a non-empty source identity.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::EmptySourceId`] for empty or whitespace-only ids.
    pub fn new(value: impl Into<Arc<str>>) -> Result<Self, CaptureFrameError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CaptureFrameError::EmptySourceId);
        }
        Ok(Self(value))
    }

    /// Borrow the stable identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CaptureSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Positive two-dimensional pixel extent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelExtent {
    width: u32,
    height: u32,
}

impl PixelExtent {
    /// Construct a non-empty extent.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::EmptyExtent`] when either dimension is zero.
    pub const fn new(width: u32, height: u32) -> Result<Self, CaptureFrameError> {
        if width == 0 || height == 0 {
            return Err(CaptureFrameError::EmptyExtent { width, height });
        }
        Ok(Self { width, height })
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }
}

/// Signed origin in the physical desktop coordinate space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicalOrigin {
    /// Horizontal coordinate. Displays left of the primary output are negative.
    pub x: i32,
    /// Vertical coordinate. Displays above the primary output are negative.
    pub y: i32,
}

/// Rectangle in native scanout pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    x: u32,
    y: u32,
    extent: PixelExtent,
}

impl PixelRect {
    /// Construct a non-empty pixel rectangle.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::EmptyExtent`] for an empty rectangle.
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Result<Self, CaptureFrameError> {
        let extent = match PixelExtent::new(width, height) {
            Ok(extent) => extent,
            Err(error) => return Err(error),
        };
        Ok(Self { x, y, extent })
    }

    /// Horizontal offset from the scanout origin.
    #[must_use]
    pub const fn x(self) -> u32 {
        self.x
    }

    /// Vertical offset from the scanout origin.
    #[must_use]
    pub const fn y(self) -> u32 {
        self.y
    }

    /// Rectangle extent.
    #[must_use]
    pub const fn extent(self) -> PixelExtent {
        self.extent
    }

    fn fits_within(self, extent: PixelExtent) -> bool {
        self.x
            .checked_add(self.extent.width)
            .is_some_and(|right| right <= extent.width)
            && self
                .y
                .checked_add(self.extent.height)
                .is_some_and(|bottom| bottom <= extent.height)
    }
}

/// Display transform still pending on native scanout pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureRotation {
    /// Pixels already share the logical display orientation.
    #[default]
    Identity,
    /// Rotate 90 degrees clockwise.
    Clockwise90,
    /// Rotate 180 degrees.
    Clockwise180,
    /// Rotate 270 degrees clockwise.
    Clockwise270,
}

impl CaptureRotation {
    /// Logical extent after applying this transform exactly once.
    #[must_use]
    pub const fn apply_to_extent(self, extent: PixelExtent) -> PixelExtent {
        match self {
            Self::Identity | Self::Clockwise180 => extent,
            Self::Clockwise90 | Self::Clockwise270 => PixelExtent {
                width: extent.height,
                height: extent.width,
            },
        }
    }
}

/// Rational mapping from physical pixels to compositor coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceScale {
    numerator: u32,
    denominator: u32,
}

impl SourceScale {
    /// Identity scale.
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    /// Construct a positive rational scale.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::InvalidSourceScale`] when either term is zero.
    pub const fn new(numerator: u32, denominator: u32) -> Result<Self, CaptureFrameError> {
        if numerator == 0 || denominator == 0 {
            return Err(CaptureFrameError::InvalidSourceScale {
                numerator,
                denominator,
            });
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    /// Scale numerator.
    #[must_use]
    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    /// Scale denominator.
    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator
    }
}

/// Physical geometry carried by a capture frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureGeometry {
    origin: PhysicalOrigin,
    native_extent: PixelExtent,
    storage_extent: PixelExtent,
    rotation: CaptureRotation,
    crop: Option<PixelRect>,
    source_scale: SourceScale,
}

impl CaptureGeometry {
    /// Construct validated physical geometry.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::CropOutOfBounds`] when the crop escapes the
    /// native scanout extent.
    pub fn new(
        origin: PhysicalOrigin,
        native_extent: PixelExtent,
        storage_extent: PixelExtent,
        rotation: CaptureRotation,
        crop: Option<PixelRect>,
        source_scale: SourceScale,
    ) -> Result<Self, CaptureFrameError> {
        if let Some(crop) = crop
            && !crop.fits_within(native_extent)
        {
            return Err(CaptureFrameError::CropOutOfBounds {
                crop,
                extent: native_extent,
            });
        }
        Ok(Self {
            origin,
            native_extent,
            storage_extent,
            rotation,
            crop,
            source_scale,
        })
    }

    /// Physical desktop origin.
    #[must_use]
    pub const fn origin(&self) -> PhysicalOrigin {
        self.origin
    }

    /// Native scanout extent before rotation, crop, or backend downsampling.
    #[must_use]
    pub const fn native_extent(&self) -> PixelExtent {
        self.native_extent
    }

    /// Extent of the pixel plane retained in frame storage.
    #[must_use]
    pub const fn storage_extent(&self) -> PixelExtent {
        self.storage_extent
    }

    /// Transform still pending on the stored pixels.
    #[must_use]
    pub const fn rotation(&self) -> CaptureRotation {
        self.rotation
    }

    /// Optional crop in native scanout coordinates.
    #[must_use]
    pub const fn crop(&self) -> Option<PixelRect> {
        self.crop
    }

    /// Mapping from physical pixels to compositor coordinates.
    #[must_use]
    pub const fn source_scale(&self) -> SourceScale {
        self.source_scale
    }
}

/// Color primaries and white point of stored pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureColorSpace {
    /// IEC 61966-2-1 sRGB primaries.
    #[default]
    Srgb,
    /// Display P3 primaries.
    DisplayP3,
    /// ITU-R BT.2020 primaries.
    Rec2020,
    /// Backend did not provide trustworthy color metadata.
    Unknown,
}

/// Transfer function used by stored channel values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureTransferFunction {
    /// Standard sRGB electro-optical transfer function.
    #[default]
    Srgb,
    /// Linear light.
    Linear,
    /// SMPTE ST 2084 perceptual quantizer.
    Pq,
    /// Hybrid log-gamma.
    Hlg,
    /// Backend did not provide trustworthy transfer metadata.
    Unknown,
}

/// Cursor metadata in native scanout coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureCursor {
    /// Whether the cursor is visible in this frame.
    pub visible: bool,
    /// Cursor origin relative to the native scanout surface.
    pub position: Option<PhysicalOrigin>,
    /// Cursor hotspot relative to its shape origin.
    pub hotspot: Option<PhysicalOrigin>,
    /// Cursor shape extent when supplied separately from the pixels.
    pub shape_extent: Option<PixelExtent>,
    /// Backend-specific shape generation, without exposing its native handle.
    pub shape_generation: Option<u64>,
    /// Whether cursor pixels are already composed into storage.
    pub composed: bool,
}

/// Pixel encoding of a capture storage plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturePixelFormat {
    /// Red, green, blue, alpha bytes.
    Rgba8,
    /// Blue, green, red, alpha bytes.
    Bgra8,
}

impl CapturePixelFormat {
    const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 | Self::Bgra8 => 4,
        }
    }
}

trait CaptureBytePlane: Send + Sync {
    fn bytes(&self) -> &[u8];
}

impl<T> CaptureBytePlane for T
where
    T: AsRef<[u8]> + Send + Sync,
{
    fn bytes(&self) -> &[u8] {
        self.as_ref()
    }
}

/// Owned CPU pixel plane with an explicit signed row stride.
#[derive(Clone)]
pub struct CpuCaptureStorage {
    plane: Arc<dyn CaptureBytePlane>,
    format: CapturePixelFormat,
    row_stride: i64,
    row0_offset: usize,
}

impl CpuCaptureStorage {
    /// Construct CPU storage. Bounds are validated against frame geometry by
    /// [`CaptureFrame::new`].
    #[must_use]
    pub fn new(
        bytes: Arc<[u8]>,
        format: CapturePixelFormat,
        row_stride: i64,
        row0_offset: usize,
    ) -> Self {
        Self::from_owner(bytes, format, row_stride, row0_offset)
    }

    /// Retain an ownership-transferable byte plane without copying its pixels.
    #[must_use]
    pub fn from_owner<T>(
        owner: T,
        format: CapturePixelFormat,
        row_stride: i64,
        row0_offset: usize,
    ) -> Self
    where
        T: AsRef<[u8]> + Send + Sync + 'static,
    {
        Self {
            plane: Arc::new(owner),
            format,
            row_stride,
            row0_offset,
        }
    }

    /// Shared pixel bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.plane.bytes()
    }

    /// Pixel encoding.
    #[must_use]
    pub const fn format(&self) -> CapturePixelFormat {
        self.format
    }

    /// Signed byte distance from one row start to the next.
    #[must_use]
    pub const fn row_stride(&self) -> i64 {
        self.row_stride
    }

    /// Byte offset of the first logical row.
    #[must_use]
    pub const fn row0_offset(&self) -> usize {
        self.row0_offset
    }

    pub(crate) fn tightly_packed_rgba8(&self, extent: PixelExtent) -> Option<&[u8]> {
        let row_bytes = usize::try_from(extent.width)
            .ok()?
            .checked_mul(self.format.bytes_per_pixel())?;
        let expected = row_bytes.checked_mul(usize::try_from(extent.height).ok()?)?;
        if self.format != CapturePixelFormat::Rgba8
            || self.row_stride != i64::try_from(row_bytes).ok()?
            || self.row0_offset != 0
            || self.bytes().len() < expected
        {
            return None;
        }
        self.bytes().get(..expected)
    }

    fn validate(&self, extent: PixelExtent) -> Result<(), CaptureFrameError> {
        let row_bytes = usize::try_from(extent.width)
            .ok()
            .and_then(|width| width.checked_mul(self.format.bytes_per_pixel()))
            .ok_or(CaptureFrameError::StorageSizeOverflow)?;
        let row_bytes_i64 =
            i64::try_from(row_bytes).map_err(|_| CaptureFrameError::StorageSizeOverflow)?;
        let stride_magnitude =
            self.row_stride
                .checked_abs()
                .ok_or(CaptureFrameError::InvalidCpuStride {
                    stride: self.row_stride,
                    minimum: row_bytes,
                })?;
        if stride_magnitude < row_bytes_i64 {
            return Err(CaptureFrameError::InvalidCpuStride {
                stride: self.row_stride,
                minimum: row_bytes,
            });
        }

        let row_count = i128::from(extent.height.saturating_sub(1));
        let first =
            i128::try_from(self.row0_offset).map_err(|_| CaptureFrameError::StorageSizeOverflow)?;
        let last = first
            .checked_add(i128::from(self.row_stride).saturating_mul(row_count))
            .ok_or(CaptureFrameError::StorageSizeOverflow)?;
        let lowest = first.min(last);
        let highest = first.max(last);
        let end = highest
            .checked_add(
                i128::try_from(row_bytes).map_err(|_| CaptureFrameError::StorageSizeOverflow)?,
            )
            .ok_or(CaptureFrameError::StorageSizeOverflow)?;
        let buffer_len = i128::try_from(self.bytes().len())
            .map_err(|_| CaptureFrameError::StorageSizeOverflow)?;
        if lowest < 0 || end > buffer_len {
            return Err(CaptureFrameError::CpuBufferOutOfBounds {
                buffer_len: self.bytes().len(),
                row0_offset: self.row0_offset,
                stride: self.row_stride,
                extent,
            });
        }
        Ok(())
    }
}

impl fmt::Debug for CpuCaptureStorage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CpuCaptureStorage")
            .field("len", &self.bytes().len())
            .field("format", &self.format)
            .field("row_stride", &self.row_stride)
            .field("row0_offset", &self.row0_offset)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct CapturePlanePoolInner {
    available: Mutex<Vec<Vec<u8>>>,
    allocations: AtomicUsize,
}

/// Reusable owner pool for capture adapters that must materialize CPU pixels.
#[derive(Clone, Debug, Default)]
pub struct CapturePlanePool {
    inner: Arc<CapturePlanePoolInner>,
}

impl CapturePlanePool {
    /// Acquire exclusive mutable storage with at least `minimum_capacity` bytes.
    #[must_use]
    pub fn acquire(&self, minimum_capacity: usize) -> CapturePlaneLease {
        let mut available = self
            .inner
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = available
            .iter()
            .position(|buffer| buffer.capacity() >= minimum_capacity);
        let buffer = match index {
            Some(index) => available.swap_remove(index),
            None => {
                if let Some(mut buffer) = available.pop() {
                    buffer.reserve(minimum_capacity);
                    buffer
                } else {
                    self.inner.allocations.fetch_add(1, Ordering::Relaxed);
                    Vec::with_capacity(minimum_capacity)
                }
            }
        };
        drop(available);
        CapturePlaneLease {
            buffer: Some(buffer),
            pool: Arc::downgrade(&self.inner),
        }
    }

    /// Number of backing allocations created by this pool.
    #[must_use]
    pub fn allocation_count(&self) -> usize {
        self.inner.allocations.load(Ordering::Relaxed)
    }

    /// Number of buffers currently available for immediate reuse.
    #[must_use]
    pub fn available_count(&self) -> usize {
        self.inner
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

/// Exclusive mutable lease that freezes into an immutable pooled frame plane.
pub struct CapturePlaneLease {
    buffer: Option<Vec<u8>>,
    pool: Weak<CapturePlanePoolInner>,
}

impl CapturePlaneLease {
    /// Freeze the populated buffer for publication.
    #[must_use]
    pub fn freeze(mut self) -> PooledCapturePlane {
        PooledCapturePlane {
            buffer: self.buffer.take(),
            pool: self.pool.clone(),
        }
    }
}

impl Deref for CapturePlaneLease {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        self.buffer
            .as_ref()
            .expect("capture plane lease owns a buffer")
    }
}

impl DerefMut for CapturePlaneLease {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer
            .as_mut()
            .expect("capture plane lease owns a buffer")
    }
}

impl Drop for CapturePlaneLease {
    fn drop(&mut self) {
        recycle_plane(self.buffer.take(), &self.pool);
    }
}

/// Immutable CPU plane whose allocation returns to its pool after publication.
pub struct PooledCapturePlane {
    buffer: Option<Vec<u8>>,
    pool: Weak<CapturePlanePoolInner>,
}

impl AsRef<[u8]> for PooledCapturePlane {
    fn as_ref(&self) -> &[u8] {
        self.buffer
            .as_deref()
            .expect("pooled capture plane owns a buffer")
    }
}

impl Drop for PooledCapturePlane {
    fn drop(&mut self) {
        recycle_plane(self.buffer.take(), &self.pool);
    }
}

fn recycle_plane(buffer: Option<Vec<u8>>, pool: &Weak<CapturePlanePoolInner>) {
    let (Some(mut buffer), Some(pool)) = (buffer, pool.upgrade()) else {
        return;
    };
    buffer.clear();
    pool.available
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(buffer);
}

/// Platform family of an opaque GPU surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformGpuApi {
    /// Direct3D texture shared by a Windows adapter.
    Direct3d11,
    /// Linux DMA-BUF surface.
    DmaBuf,
    /// Vulkan image or exported memory.
    Vulkan,
    /// Metal texture.
    Metal,
    /// Extensible backend name without a native API type.
    Other(Arc<str>),
}

/// Opaque, lifetime-owning GPU surface descriptor.
#[derive(Clone)]
pub struct PlatformGpuSurface {
    api: PlatformGpuApi,
    handle_id: u64,
    extent: PixelExtent,
    format: CapturePixelFormat,
    owner: Arc<dyn Any + Send + Sync>,
}

impl PlatformGpuSurface {
    /// Erase a platform owner behind the neutral capture contract.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::InvalidGpuHandle`] for a zero handle id.
    pub fn new<T>(
        api: PlatformGpuApi,
        handle_id: u64,
        extent: PixelExtent,
        format: CapturePixelFormat,
        owner: Arc<T>,
    ) -> Result<Self, CaptureFrameError>
    where
        T: Any + Send + Sync,
    {
        if handle_id == 0 {
            return Err(CaptureFrameError::InvalidGpuHandle);
        }
        let owner: Arc<dyn Any + Send + Sync> = owner;
        Ok(Self {
            api,
            handle_id,
            extent,
            format,
            owner,
        })
    }

    /// GPU API family without exposing a native handle type.
    #[must_use]
    pub const fn api(&self) -> &PlatformGpuApi {
        &self.api
    }

    /// Adapter-provided stable handle identity.
    #[must_use]
    pub const fn handle_id(&self) -> u64 {
        self.handle_id
    }

    /// Storage extent.
    #[must_use]
    pub const fn extent(&self) -> PixelExtent {
        self.extent
    }

    /// Pixel encoding.
    #[must_use]
    pub const fn format(&self) -> CapturePixelFormat {
        self.format
    }

    /// Strong references retaining the native surface lifetime.
    #[must_use]
    pub fn owner_strong_count(&self) -> usize {
        Arc::strong_count(&self.owner)
    }

    /// Recover a typed owner in a platform adapter without exposing that type
    /// in the backend-neutral contract.
    #[must_use]
    pub fn owner<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        Arc::clone(&self.owner).downcast().ok()
    }
}

impl fmt::Debug for PlatformGpuSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlatformGpuSurface")
            .field("api", &self.api)
            .field("handle_id", &self.handle_id)
            .field("extent", &self.extent)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// Storage backing a capture frame.
#[derive(Clone, Debug)]
pub enum CaptureStorage {
    /// Owned CPU pixel plane.
    Cpu(CpuCaptureStorage),
    /// Opaque GPU surface with an erased lifetime owner.
    Gpu(PlatformGpuSurface),
}

/// Native scanout move operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveRegion {
    /// Source rectangle before the move.
    pub source: PixelRect,
    /// Destination origin after the move.
    pub destination: (u32, u32),
}

/// Optional damage metadata for incremental consumers.
#[derive(Clone, Debug, Default)]
pub struct CaptureDamage {
    dirty_regions: Arc<[PixelRect]>,
    move_regions: Arc<[MoveRegion]>,
}

impl CaptureDamage {
    /// Construct damage metadata. Bounds are checked by [`CaptureFrame::new`].
    #[must_use]
    pub fn new(dirty_regions: Vec<PixelRect>, move_regions: Vec<MoveRegion>) -> Self {
        Self {
            dirty_regions: dirty_regions.into(),
            move_regions: move_regions.into(),
        }
    }

    /// Dirty rectangles in native scanout coordinates.
    #[must_use]
    pub fn dirty_regions(&self) -> &[PixelRect] {
        &self.dirty_regions
    }

    /// Move operations in native scanout coordinates.
    #[must_use]
    pub fn move_regions(&self) -> &[MoveRegion] {
        &self.move_regions
    }
}

/// Runtime stage discriminator for diagnostics and serialization adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureStageKind {
    /// Native scanout pixels with rotation still pending.
    Raw,
    /// Canonical pixels after geometry and color processing.
    Processed,
}

mod stage_sealed {
    pub trait Sealed {}
}

/// Marker implemented by the two legal capture surface stages.
pub trait CaptureSurfaceStage:
    stage_sealed::Sealed + Clone + fmt::Debug + Send + Sync + 'static
{
    /// Runtime diagnostic marker matching the type-level stage.
    const KIND: CaptureStageKind;
}

/// Native scanout surface. Its rotation is pending and must not be normalized by adapters.
#[derive(Clone, Copy, Debug)]
pub struct RawCaptureSurface;

impl stage_sealed::Sealed for RawCaptureSurface {}

impl CaptureSurfaceStage for RawCaptureSurface {
    const KIND: CaptureStageKind = CaptureStageKind::Raw;
}

/// Canonical processed surface. Its rotation must be identity.
#[derive(Clone, Copy, Debug)]
pub struct ProcessedCaptureSurface;

impl stage_sealed::Sealed for ProcessedCaptureSurface {}

impl CaptureSurfaceStage for ProcessedCaptureSurface {
    const KIND: CaptureStageKind = CaptureStageKind::Processed;
}

/// Metadata shared by CPU and GPU capture frames.
#[derive(Clone, Debug)]
pub struct CaptureFrameMetadata {
    /// Stable logical source identity.
    pub source_id: CaptureSourceId,
    /// Generation of the physical source topology.
    pub topology_generation: u64,
    /// Generation of the active capture session.
    pub session_generation: u64,
    /// Monotonic frame sequence within the session.
    pub sequence: u64,
    /// Monotonic acquisition timestamp.
    pub captured_at: Instant,
    /// Deadline after which this frame must not be presented as fresh.
    pub fresh_until: Instant,
    /// Physical scanout geometry and pending transform.
    pub geometry: CaptureGeometry,
    /// Color primaries and white point.
    pub color_space: CaptureColorSpace,
    /// Channel transfer function.
    pub transfer_function: CaptureTransferFunction,
    /// Cursor metadata and composition state.
    pub cursor: CaptureCursor,
}

/// Source and generation tuple used to reject stale worker publications.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureEpoch {
    /// Stable source expected by the adapter.
    pub source_id: CaptureSourceId,
    /// Current topology generation.
    pub topology_generation: u64,
    /// Current capture session generation.
    pub session_generation: u64,
}

/// Validated backend-neutral capture envelope.
#[derive(Clone, Debug)]
pub struct CaptureFrame<S: CaptureSurfaceStage> {
    metadata: CaptureFrameMetadata,
    storage: CaptureStorage,
    damage: CaptureDamage,
    stage: PhantomData<S>,
}

impl CaptureFrame<RawCaptureSurface> {
    /// Validate and construct a raw capture frame.
    ///
    /// # Errors
    ///
    /// Rejects zero generations or sequence, inverted freshness, inconsistent
    /// storage, and out-of-bounds damage.
    pub fn new(
        metadata: CaptureFrameMetadata,
        storage: CaptureStorage,
        damage: CaptureDamage,
    ) -> Result<Self, CaptureFrameError> {
        Self::from_parts(metadata, storage, damage)
    }

    /// Consume a raw frame and publish the canonical output of geometry processing.
    ///
    /// The source identity, epochs, sequence, timestamps, and color metadata are
    /// retained from the raw input. Callers supply only the processed geometry,
    /// storage, and damage map, so a processed frame cannot be fabricated without
    /// consuming the raw frame it came from.
    ///
    /// # Errors
    ///
    /// Rejects processed geometry with a pending crop or rotation and validates
    /// the replacement storage against its stored extent.
    pub fn into_processed(
        self,
        geometry: CaptureGeometry,
        storage: CaptureStorage,
        damage: CaptureDamage,
    ) -> Result<CaptureFrame<ProcessedCaptureSurface>, CaptureFrameError> {
        let mut metadata = self.metadata;
        metadata.geometry = geometry;
        CaptureFrame::from_parts(metadata, storage, damage)
    }
}

impl<S: CaptureSurfaceStage> CaptureFrame<S> {
    fn from_parts(
        metadata: CaptureFrameMetadata,
        storage: CaptureStorage,
        damage: CaptureDamage,
    ) -> Result<Self, CaptureFrameError> {
        validate_metadata::<S>(&metadata, &storage, &damage)?;
        Ok(Self {
            metadata,
            storage,
            damage,
            stage: PhantomData,
        })
    }

    /// Envelope metadata.
    #[must_use]
    pub const fn metadata(&self) -> &CaptureFrameMetadata {
        &self.metadata
    }

    /// Backing CPU pixels or opaque GPU surface.
    #[must_use]
    pub const fn storage(&self) -> &CaptureStorage {
        &self.storage
    }

    /// Optional dirty and move metadata.
    #[must_use]
    pub const fn damage(&self) -> &CaptureDamage {
        &self.damage
    }

    /// Runtime stage corresponding to `S`.
    #[must_use]
    pub const fn stage(&self) -> CaptureStageKind {
        S::KIND
    }

    /// Reject a frame published by an obsolete source topology or worker session.
    ///
    /// # Errors
    ///
    /// Returns the precise source or generation mismatch.
    pub fn validate_epoch(&self, expected: &CaptureEpoch) -> Result<(), CaptureFrameError> {
        if self.metadata.source_id != expected.source_id {
            return Err(CaptureFrameError::SourceMismatch {
                expected: expected.source_id.clone(),
                actual: self.metadata.source_id.clone(),
            });
        }
        if self.metadata.topology_generation != expected.topology_generation {
            return Err(CaptureFrameError::StaleTopology {
                expected: expected.topology_generation,
                actual: self.metadata.topology_generation,
            });
        }
        if self.metadata.session_generation != expected.session_generation {
            return Err(CaptureFrameError::StaleSession {
                expected: expected.session_generation,
                actual: self.metadata.session_generation,
            });
        }
        Ok(())
    }
}

fn validate_metadata<S: CaptureSurfaceStage>(
    metadata: &CaptureFrameMetadata,
    storage: &CaptureStorage,
    damage: &CaptureDamage,
) -> Result<(), CaptureFrameError> {
    if metadata.topology_generation == 0 {
        return Err(CaptureFrameError::ZeroGeneration("topology"));
    }
    if metadata.session_generation == 0 {
        return Err(CaptureFrameError::ZeroGeneration("session"));
    }
    if metadata.sequence == 0 {
        return Err(CaptureFrameError::ZeroSequence);
    }
    if metadata.fresh_until < metadata.captured_at {
        return Err(CaptureFrameError::InvalidFreshness);
    }
    if S::KIND == CaptureStageKind::Processed
        && metadata.geometry.rotation != CaptureRotation::Identity
    {
        return Err(CaptureFrameError::ProcessedRotationPending(
            metadata.geometry.rotation,
        ));
    }
    if S::KIND == CaptureStageKind::Processed
        && let Some(crop) = metadata.geometry.crop
    {
        return Err(CaptureFrameError::ProcessedCropPending(crop));
    }

    match storage {
        CaptureStorage::Cpu(cpu) => cpu.validate(metadata.geometry.storage_extent)?,
        CaptureStorage::Gpu(gpu) if gpu.extent != metadata.geometry.storage_extent => {
            return Err(CaptureFrameError::GpuExtentMismatch {
                storage: gpu.extent,
                geometry: metadata.geometry.storage_extent,
            });
        }
        CaptureStorage::Gpu(_) => {}
    }

    for region in damage.dirty_regions.iter().copied() {
        if !region.fits_within(metadata.geometry.native_extent) {
            return Err(CaptureFrameError::DamageOutOfBounds(region));
        }
    }
    for region in damage.move_regions.iter().copied() {
        let destination = PixelRect {
            x: region.destination.0,
            y: region.destination.1,
            extent: region.source.extent,
        };
        if !region.source.fits_within(metadata.geometry.native_extent)
            || !destination.fits_within(metadata.geometry.native_extent)
        {
            return Err(CaptureFrameError::MoveOutOfBounds(region));
        }
    }
    Ok(())
}

/// Invalid capture metadata rejected at a backend adapter boundary.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CaptureFrameError {
    /// Stable source identities cannot be blank.
    #[error("capture source id cannot be empty")]
    EmptySourceId,
    /// Pixel surfaces and regions must be non-empty.
    #[error("capture extent must be non-zero, got {width}x{height}")]
    EmptyExtent { width: u32, height: u32 },
    /// Source scale must be a positive rational number.
    #[error("source scale must be positive, got {numerator}/{denominator}")]
    InvalidSourceScale { numerator: u32, denominator: u32 },
    /// Crop escaped the native scanout extent.
    #[error("capture crop {crop:?} is outside extent {extent:?}")]
    CropOutOfBounds {
        crop: PixelRect,
        extent: PixelExtent,
    },
    /// Topology and session generations start at one.
    #[error("{0} generation must be non-zero")]
    ZeroGeneration(&'static str),
    /// Frame sequence starts at one for each session.
    #[error("capture frame sequence must be non-zero")]
    ZeroSequence,
    /// Freshness deadline preceded acquisition.
    #[error("capture freshness deadline precedes acquisition")]
    InvalidFreshness,
    /// CPU stride cannot address one complete row.
    #[error("CPU stride {stride} is smaller than the {minimum}-byte row")]
    InvalidCpuStride { stride: i64, minimum: usize },
    /// CPU row addressing escaped the supplied allocation.
    #[error(
        "CPU storage ({buffer_len} bytes, row0 {row0_offset}, stride {stride}) cannot hold {extent:?}"
    )]
    CpuBufferOutOfBounds {
        buffer_len: usize,
        row0_offset: usize,
        stride: i64,
        extent: PixelExtent,
    },
    /// Storage length arithmetic overflowed the host address space.
    #[error("capture storage size overflow")]
    StorageSizeOverflow,
    /// Opaque GPU handles reserve zero as invalid.
    #[error("GPU surface handle id must be non-zero")]
    InvalidGpuHandle,
    /// GPU descriptor and physical geometry disagree.
    #[error("GPU storage extent {storage:?} differs from geometry {geometry:?}")]
    GpuExtentMismatch {
        storage: PixelExtent,
        geometry: PixelExtent,
    },
    /// A processed surface cannot retain a pending display transform.
    #[error("processed capture surface still has pending rotation {0:?}")]
    ProcessedRotationPending(CaptureRotation),
    /// A processed surface cannot retain a pending native crop.
    #[error("processed capture surface still has pending crop {0:?}")]
    ProcessedCropPending(PixelRect),
    /// Dirty region escaped the native scanout extent.
    #[error("dirty region {0:?} is outside the capture extent")]
    DamageOutOfBounds(PixelRect),
    /// Move source or destination escaped the native scanout extent.
    #[error("move region {0:?} is outside the capture extent")]
    MoveOutOfBounds(MoveRegion),
    /// Frame identity differs from the active adapter source.
    #[error("capture source mismatch: expected {expected}, got {actual}")]
    SourceMismatch {
        expected: CaptureSourceId,
        actual: CaptureSourceId,
    },
    /// Frame belongs to an obsolete topology.
    #[error("stale topology generation {actual}; expected {expected}")]
    StaleTopology { expected: u64, actual: u64 },
    /// Frame belongs to an obsolete worker session.
    #[error("stale capture session {actual}; expected {expected}")]
    StaleSession { expected: u64, actual: u64 },
}
