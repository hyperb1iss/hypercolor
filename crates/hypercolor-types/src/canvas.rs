//! Canvas buffers, surface pooling, and the canvas spelling of the color
//! kernel's pixel types.
//!
//! The pixel surface (`Canvas`) and surface pool live here. Color values and
//! color math live in `hypercolor-color` and are re-exported below, so
//! `hypercolor_types::canvas::{Rgb, Rgba, LinearRgba, Oklab, Oklch}`
//! resolve to the kernel's types.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Opaque lifetime owner retained with one admitted surface backing.
///
/// Higher-level crates use this seam to keep resource reservations alive for
/// exactly as long as a canvas backing can still be observed. The types crate
/// deliberately does not prescribe the accounting implementation.
pub trait SurfaceResourceOwner: std::fmt::Debug + Send + Sync {}

impl<T> SurfaceResourceOwner for T where T: std::fmt::Debug + Send + Sync {}

// ── Canvas Constants ───────────────────────────────────────────────────────

/// The default canvas width used by the render pipeline.
pub const DEFAULT_CANVAS_WIDTH: u32 = 640;

/// The default canvas height used by the render pipeline.
pub const DEFAULT_CANVAS_HEIGHT: u32 = 480;

/// Bytes per pixel in the RGBA format.
pub const BYTES_PER_PIXEL: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SurfaceResourceError {
    #[error("surface dimensions must be non-zero, got {width}x{height}")]
    EmptyDimensions { width: u32, height: u32 },

    #[error("surface byte length overflows addressable memory for {width}x{height}")]
    ByteLengthOverflow { width: u32, height: u32 },

    #[error("surface buffer length {actual} does not match expected length {expected}")]
    BufferLengthMismatch { expected: usize, actual: usize },

    #[error("could not allocate {byte_len} bytes for a {width}x{height} surface")]
    AllocationFailed {
        width: u32,
        height: u32,
        byte_len: usize,
    },

    #[error(
        "screen analysis needs {requested_bytes} bytes for {width}x{height} sectors; capacity is {capacity_bytes} bytes"
    )]
    AnalysisCapacityExceeded {
        width: u32,
        height: u32,
        requested_bytes: u64,
        capacity_bytes: u64,
    },

    #[error(
        "surface pool byte length overflows addressable memory for {slot_count} slots of {width}x{height}"
    )]
    PoolByteLengthOverflow {
        width: u32,
        height: u32,
        slot_count: usize,
    },

    #[error(
        "could not allocate {byte_len} bytes for {slot_count} surface slots of {width}x{height}"
    )]
    PoolAllocationFailed {
        width: u32,
        height: u32,
        slot_count: usize,
        byte_len: usize,
    },
}

static NEXT_PUBLISHED_SURFACE_STORAGE_ID: AtomicU64 = AtomicU64::new(1);
const EMPTY_PUBLISHED_SURFACE_STORAGE_ID: u64 = 0;

fn next_published_surface_storage_id() -> u64 {
    NEXT_PUBLISHED_SURFACE_STORAGE_ID.fetch_add(1, Ordering::Relaxed)
}

// ── Color types (re-exported from the color kernel) ───────────────────────

// Spec 76 §1: `hypercolor-color` is the single home for color math. These
// paths stay importable from `hypercolor_types::canvas` so the absorption
// is a type swap for every consumer.
pub use hypercolor_color::lut::{linear_to_srgb_u8, srgb_u8_to_linear};
pub use hypercolor_color::{
    LinearRgba, Oklab, Oklch, Rgb, Rgba, linear_to_output_u8, linear_to_srgb, srgb_to_linear,
};

// ── Color (alias) ──────────────────────────────────────────────────────────

/// High-level color type — linear sRGB with float precision.
///
/// Canvas vocabulary for [`LinearRgba`], used throughout the effect
/// pipeline where "Color" is the natural word.
pub type Color = LinearRgba;

// ── ColorFormat ────────────────────────────────────────────────────────────

/// Wire color format for device backends.
///
/// Different hardware speaks different pixel formats. The spatial sampler
/// produces [`Rgb`], and backends convert to their native format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorFormat {
    /// Standard 8-bit RGB (3 bytes per LED).
    #[default]
    Rgb,
    /// RGBW with a dedicated white channel (4 bytes per LED).
    Rgbw,
    /// RGBW with 16-bit white for high-dynamic-range whites (5 bytes per LED).
    RgbW16,
}

// ── SamplingMethod ─────────────────────────────────────────────────────────

/// Interpolation strategy for canvas sampling.
///
/// Controls how sub-pixel LED positions are resolved from the canvas buffer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SamplingMethod {
    /// Snap to the nearest pixel. Fastest, but aliased at low LED density.
    Nearest,

    /// Weighted average of the 4 surrounding pixels.
    /// Default. Good balance of quality and speed.
    #[default]
    Bilinear,

    /// Average all pixels within a rectangular area centered on the sample point.
    /// Best quality for zones spanning many canvas pixels.
    Area {
        /// Half-width of the sample area in canvas pixels.
        /// A value of 5.0 samples an 11x11 pixel box.
        radius: f32,
    },
}

// ── Canvas ─────────────────────────────────────────────────────────────────

/// The canonical render surface for all effects.
///
/// A 2D RGBA pixel buffer at a fixed resolution (default 320x200).
/// Both the wgpu and Servo render paths write to this format.
/// The spatial sampler reads from it to produce LED colors.
///
/// Memory layout: row-major, top-left origin, 4 bytes per pixel (R, G, B, A).
/// Total size at 320x200: 256,000 bytes (250 KB).
#[derive(Clone)]
pub struct Canvas {
    /// Horizontal pixel count.
    width: u32,

    /// Vertical pixel count.
    height: u32,

    /// Row-major RGBA pixel data.
    /// Length is always `width * height * 4`.
    pixels: Arc<Vec<u8>>,

    storage_id: u64,

    resource_owner: Option<Arc<dyn SurfaceResourceOwner>>,
}

impl Canvas {
    /// Create a new canvas filled with opaque black.
    ///
    /// Allocates `width * height * 4` bytes zeroed, then sets every alpha byte to 255.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self::try_new(width, height).expect("canvas dimensions must fit available memory")
    }

    pub fn try_new(width: u32, height: u32) -> Result<Self, SurfaceResourceError> {
        let descriptor = SurfaceDescriptor::rgba8888(width, height);
        let len = descriptor.try_non_empty_byte_len()?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(len)
            .map_err(|_| SurfaceResourceError::AllocationFailed {
                width,
                height,
                byte_len: len,
            })?;
        pixels.resize(len, 0);
        for chunk in pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
            chunk[3] = 255;
        }
        Ok(Self {
            width,
            height,
            pixels: Arc::new(pixels),
            storage_id: next_published_surface_storage_id(),
            resource_owner: None,
        })
    }

    /// Create from a raw RGBA byte slice.
    ///
    /// # Panics
    ///
    /// Panics if `data.len() != width * height * 4`.
    #[must_use]
    pub fn from_rgba(data: &[u8], width: u32, height: u32) -> Self {
        Self::try_from_rgba(data, width, height)
            .expect("RGBA data length does not match addressable canvas dimensions")
    }

    pub fn try_from_rgba(
        data: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Self, SurfaceResourceError> {
        let expected = SurfaceDescriptor::rgba8888(width, height).try_non_empty_byte_len()?;
        if data.len() != expected {
            return Err(SurfaceResourceError::BufferLengthMismatch {
                expected,
                actual: data.len(),
            });
        }
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(expected)
            .map_err(|_| SurfaceResourceError::AllocationFailed {
                width,
                height,
                byte_len: expected,
            })?;
        pixels.extend_from_slice(data);
        Ok(Self {
            width,
            height,
            pixels: Arc::new(pixels),
            storage_id: next_published_surface_storage_id(),
            resource_owner: None,
        })
    }

    /// Wrap an existing `Vec<u8>` without copying. Takes ownership.
    ///
    /// # Panics
    ///
    /// Panics if `data.len() != width * height * 4`.
    #[must_use]
    pub fn from_vec(data: Vec<u8>, width: u32, height: u32) -> Self {
        Self::try_from_vec(data, width, height)
            .expect("RGBA vector length does not match addressable canvas dimensions")
    }

    pub fn try_from_vec(
        data: Vec<u8>,
        width: u32,
        height: u32,
    ) -> Result<Self, SurfaceResourceError> {
        let expected = SurfaceDescriptor::rgba8888(width, height).try_non_empty_byte_len()?;
        if data.len() != expected {
            return Err(SurfaceResourceError::BufferLengthMismatch {
                expected,
                actual: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            pixels: Arc::new(data),
            storage_id: next_published_surface_storage_id(),
            resource_owner: None,
        })
    }

    /// Alias an immutable published surface as a read-mostly canvas handle.
    #[must_use]
    pub fn from_published_surface(surface: &PublishedSurface) -> Self {
        Self {
            width: surface.width(),
            height: surface.height(),
            pixels: Arc::clone(surface.storage.cpu_rgba()),
            storage_id: surface.storage.id(),
            resource_owner: surface.storage.resource_owner().cloned(),
        }
    }

    /// Retain an opaque resource owner with this backing and future aliases.
    pub fn set_resource_owner(&mut self, owner: Arc<dyn SurfaceResourceOwner>) {
        self.resource_owner = Some(owner);
    }

    /// Whether this backing carries an external resource owner.
    #[must_use]
    pub fn has_resource_owner(&self) -> bool {
        self.resource_owner.is_some()
    }

    /// Copy a published surface into this canvas without invalidating readers.
    ///
    /// Matching uniquely owned storage is reused. A shared or differently sized
    /// canvas is replaced only after the new allocation succeeds, so failure
    /// leaves both this canvas and every published reader unchanged.
    pub fn try_copy_from_published_surface(
        &mut self,
        surface: &PublishedSurface,
    ) -> Result<(), SurfaceResourceError> {
        if self.width == surface.width() && self.height == surface.height() && !self.is_shared() {
            self.as_rgba_bytes_mut()
                .copy_from_slice(surface.rgba_bytes());
            return Ok(());
        }

        *self = Self::try_from_rgba(surface.rgba_bytes(), surface.width(), surface.height())?;
        Ok(())
    }

    /// Horizontal pixel count.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Vertical pixel count.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Raw pixel slice for zero-copy handoff to the spatial sampler.
    #[must_use]
    pub fn as_rgba_bytes(&self) -> &[u8] {
        self.pixels.as_slice()
    }

    /// Stable identity for the current storage backing this canvas.
    #[must_use]
    pub const fn storage_identity(&self) -> PublishedSurfaceStorageIdentity {
        PublishedSurfaceStorageIdentity::CpuRgba {
            id: self.storage_id,
        }
    }

    /// Whether the pixel buffer is currently shared with another canvas handle.
    #[must_use]
    pub fn is_shared(&self) -> bool {
        Arc::strong_count(&self.pixels) > 1
    }

    /// Number of live handles sharing this pixel buffer.
    #[must_use]
    pub fn shared_ref_count(&self) -> usize {
        Arc::strong_count(&self.pixels)
    }

    /// Raw RGBA byte length for the current canvas dimensions.
    #[must_use]
    pub fn rgba_len(&self) -> usize {
        self.pixels.len()
    }

    /// Mutable pixel slice for renderers writing directly into the buffer.
    pub fn as_rgba_bytes_mut(&mut self) -> &mut [u8] {
        self.pixels_mut().as_mut_slice()
    }

    /// Consume the canvas and return the owned RGBA byte buffer.
    #[must_use]
    pub fn into_rgba_bytes(self) -> Vec<u8> {
        self.into_rgba_bytes_with_copy_info().0
    }

    /// Consume the canvas and report whether ownership required a backing clone.
    #[must_use]
    pub fn into_rgba_bytes_with_copy_info(self) -> (Vec<u8>, bool) {
        let Self {
            pixels,
            resource_owner,
            ..
        } = self;
        if resource_owner.is_some() {
            return (pixels.as_ref().clone(), true);
        }
        match Arc::try_unwrap(pixels) {
            Ok(pixels) => (pixels, false),
            Err(pixels) => (pixels.as_ref().clone(), true),
        }
    }

    /// View pixel data as `[u8; 4]` RGBA tuples.
    ///
    /// Returns a slice of length `width * height`.
    #[must_use]
    pub fn pixels(&self) -> impl ExactSizeIterator<Item = [u8; 4]> + '_ {
        self.pixels
            .as_slice()
            .chunks_exact(BYTES_PER_PIXEL)
            .map(|chunk| {
                // chunks_exact guarantees exactly BYTES_PER_PIXEL elements
                [chunk[0], chunk[1], chunk[2], chunk[3]]
            })
    }

    /// Read a single pixel. Returns opaque black for out-of-bounds coordinates.
    #[must_use]
    #[allow(clippy::as_conversions)]
    pub fn get_pixel(&self, x: u32, y: u32) -> Rgba {
        if x >= self.width || y >= self.height {
            return Rgba::BLACK;
        }
        let idx = (y as usize * self.width as usize + x as usize) * BYTES_PER_PIXEL;
        Rgba {
            r: self.pixels[idx],
            g: self.pixels[idx + 1],
            b: self.pixels[idx + 2],
            a: self.pixels[idx + 3],
        }
    }

    /// Write a single pixel. No-op for out-of-bounds coordinates.
    #[allow(clippy::as_conversions)]
    pub fn set_pixel(&mut self, x: u32, y: u32, color: Rgba) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = (y as usize * self.width as usize + x as usize) * BYTES_PER_PIXEL;
        let pixels = self.pixels_mut();
        pixels[idx] = color.r;
        pixels[idx + 1] = color.g;
        pixels[idx + 2] = color.b;
        pixels[idx + 3] = color.a;
    }

    /// Fill the entire canvas with a single color.
    pub fn fill(&mut self, color: Rgba) {
        for chunk in self.pixels_mut().chunks_exact_mut(BYTES_PER_PIXEL) {
            chunk[0] = color.r;
            chunk[1] = color.g;
            chunk[2] = color.b;
            chunk[3] = color.a;
        }
    }

    /// Reset to opaque black. Reuses the existing allocation.
    pub fn clear(&mut self) {
        self.fill(Rgba::BLACK);
    }

    fn pixels_mut(&mut self) -> &mut Vec<u8> {
        if self.resource_owner.is_some() && Arc::strong_count(&self.pixels) > 1 {
            self.pixels = Arc::new(self.pixels.as_ref().clone());
            self.resource_owner = None;
        }
        self.storage_id = next_published_surface_storage_id();
        Arc::make_mut(&mut self.pixels)
    }

    /// Sample the canvas at normalized coordinates (0.0..=1.0).
    ///
    /// `nx` and `ny` are in `[0.0, 1.0]` where (0,0) is top-left
    /// and (1,1) is bottom-right. Values outside this range are clamped.
    ///
    /// Returns an [`Rgba`] pixel using the specified interpolation method.
    #[must_use]
    pub fn sample(&self, nx: f32, ny: f32, method: SamplingMethod) -> Rgba {
        let nx = nx.clamp(0.0, 1.0);
        let ny = ny.clamp(0.0, 1.0);
        match method {
            SamplingMethod::Nearest => self.sample_nearest(nx, ny),
            SamplingMethod::Bilinear => self.sample_bilinear(nx, ny),
            SamplingMethod::Area { radius } => self.sample_area(nx, ny, radius),
        }
    }

    /// Sample with nearest-neighbor interpolation.
    ///
    /// Snaps `(nx, ny)` to the closest integer pixel coordinate.
    /// Cost: 1 pixel read.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::as_conversions
    )]
    pub fn sample_nearest(&self, nx: f32, ny: f32) -> Rgba {
        let x = (nx * (self.width - 1) as f32).round() as u32;
        let y = (ny * (self.height - 1) as f32).round() as u32;
        self.get_pixel(x.min(self.width - 1), y.min(self.height - 1))
    }

    /// Sample with bilinear interpolation.
    ///
    /// Reads the 4 pixels surrounding the fractional coordinate and blends
    /// by distance. Produces smooth gradients between pixels.
    /// Cost: 4 pixel reads + 12 multiplies.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::as_conversions
    )]
    pub fn sample_bilinear(&self, nx: f32, ny: f32) -> Rgba {
        const BILINEAR_ONE: u32 = 256;

        let fx = nx * (self.width - 1) as f32;
        let fy = ny * (self.height - 1) as f32;

        let x0 = fx.floor() as u32;
        let y0 = fy.floor() as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);

        let frac_x = (fx.fract() * BILINEAR_ONE as f32).clamp(0.0, BILINEAR_ONE as f32)
            / BILINEAR_ONE as f32;
        let frac_y = (fy.fract() * BILINEAR_ONE as f32).clamp(0.0, BILINEAR_ONE as f32)
            / BILINEAR_ONE as f32;

        let top = self
            .get_pixel(x0, y0)
            .to_linear()
            .lerp(self.get_pixel(x1, y0).to_linear(), frac_x);
        let bottom = self
            .get_pixel(x0, y1)
            .to_linear()
            .lerp(self.get_pixel(x1, y1).to_linear(), frac_x);

        top.lerp(bottom, frac_y).to_encoded()
    }

    /// Sample with area averaging.
    ///
    /// Averages all pixels within a `(2*radius+1)` square centered on the
    /// sample point. Best for low-resolution LED zones that map to large
    /// canvas regions — prevents moire patterns and aliasing.
    /// Cost: `(2*radius+1)^2` pixel reads.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        clippy::cast_possible_wrap,
        clippy::as_conversions
    )]
    pub fn sample_area(&self, nx: f32, ny: f32, radius: f32) -> Rgba {
        let cx = nx * (self.width - 1) as f32;
        let cy = ny * (self.height - 1) as f32;

        let r = radius.ceil() as i32;
        let mut sum_r = 0.0_f32;
        let mut sum_g = 0.0_f32;
        let mut sum_b = 0.0_f32;
        let mut sum_a = 0.0_f32;
        let mut count = 0.0_f32;

        for dy in -r..=r {
            for dx in -r..=r {
                let px = (cx as i32 + dx).clamp(0, self.width as i32 - 1) as u32;
                let py = (cy as i32 + dy).clamp(0, self.height as i32 - 1) as u32;
                let p = self.get_pixel(px, py).to_linear();
                sum_r += p.r;
                sum_g += p.g;
                sum_b += p.b;
                sum_a += p.a;
                count += 1.0;
            }
        }

        LinearRgba::new(sum_r / count, sum_g / count, sum_b / count, sum_a / count).to_encoded()
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT)
    }
}

impl std::fmt::Debug for Canvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixel_count", &(self.width * self.height))
            .finish_non_exhaustive()
    }
}

// ── Render Surfaces ───────────────────────────────────────────────────────

/// Pixel storage format for published render surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceFormat {
    /// Standard 8-bit RGBA surface.
    #[default]
    Rgba8888,
}

/// Immutable dimensions and format for a surface family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct SurfaceDescriptor {
    pub width: u32,
    pub height: u32,
    pub format: SurfaceFormat,
}

impl SurfaceDescriptor {
    /// Create a descriptor for an RGBA8888 surface.
    #[must_use]
    pub const fn rgba8888(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            format: SurfaceFormat::Rgba8888,
        }
    }

    /// Total byte size for one surface when it fits the host address space.
    #[must_use]
    pub fn checked_byte_len(self) -> Option<usize> {
        usize::try_from(self.width)
            .ok()?
            .checked_mul(usize::try_from(self.height).ok()?)?
            .checked_mul(BYTES_PER_PIXEL)
    }

    pub fn try_byte_len(self) -> Result<usize, SurfaceResourceError> {
        self.checked_byte_len()
            .ok_or(SurfaceResourceError::ByteLengthOverflow {
                width: self.width,
                height: self.height,
            })
    }

    pub fn try_non_empty_byte_len(self) -> Result<usize, SurfaceResourceError> {
        if self.width == 0 || self.height == 0 {
            return Err(SurfaceResourceError::EmptyDimensions {
                width: self.width,
                height: self.height,
            });
        }
        self.try_byte_len()
    }
}

/// Immutable published surface shared across consumers.
#[derive(Clone, Debug)]
pub struct PublishedSurface {
    descriptor: SurfaceDescriptor,
    generation: u64,
    frame_number: u32,
    timestamp_ms: u32,
    storage: PublishedSurfaceStorage,
}

#[derive(Clone, Debug)]
enum PublishedSurfaceStorage {
    CpuRgba {
        id: u64,
        rgba: Arc<Vec<u8>>,
        content_digest: Arc<OnceLock<u64>>,
        resource_owner: Option<Arc<dyn SurfaceResourceOwner>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublishedSurfaceStorageIdentity {
    CpuRgba { id: u64 },
}

impl PublishedSurfaceStorage {
    fn new_cpu_rgba(
        id: u64,
        rgba: Arc<Vec<u8>>,
        resource_owner: Option<Arc<dyn SurfaceResourceOwner>>,
    ) -> Self {
        Self::CpuRgba {
            id,
            rgba,
            content_digest: Arc::new(OnceLock::new()),
            resource_owner,
        }
    }

    fn cpu_rgba(&self) -> &Arc<Vec<u8>> {
        match self {
            Self::CpuRgba { rgba, .. } => rgba,
        }
    }

    fn identity(&self) -> PublishedSurfaceStorageIdentity {
        match self {
            Self::CpuRgba { id, .. } => PublishedSurfaceStorageIdentity::CpuRgba { id: *id },
        }
    }

    fn id(&self) -> u64 {
        match self {
            Self::CpuRgba { id, .. } => *id,
        }
    }

    fn resource_owner(&self) -> Option<&Arc<dyn SurfaceResourceOwner>> {
        match self {
            Self::CpuRgba { resource_owner, .. } => resource_owner.as_ref(),
        }
    }

    fn content_digest(&self, width: u32, height: u32) -> u64 {
        match self {
            Self::CpuRgba {
                rgba,
                content_digest,
                ..
            } => *content_digest.get_or_init(|| {
                let mut hasher = DefaultHasher::new();
                width.hash(&mut hasher);
                height.hash(&mut hasher);
                rgba.hash(&mut hasher);
                hasher.finish()
            }),
        }
    }

    fn cached_content_digest(&self) -> Option<u64> {
        match self {
            Self::CpuRgba { content_digest, .. } => content_digest.get().copied(),
        }
    }
}

impl PublishedSurface {
    /// Create an empty published surface.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            descriptor: SurfaceDescriptor::default(),
            generation: 0,
            frame_number: 0,
            timestamp_ms: 0,
            storage: PublishedSurfaceStorage::new_cpu_rgba(
                EMPTY_PUBLISHED_SURFACE_STORAGE_ID,
                Arc::new(Vec::new()),
                None,
            ),
        }
    }

    /// Snapshot a shared published surface from a borrowed canvas.
    #[must_use]
    pub fn from_canvas(canvas: &Canvas, frame_number: u32, timestamp_ms: u32) -> Self {
        Self {
            descriptor: SurfaceDescriptor::rgba8888(canvas.width(), canvas.height()),
            generation: 0,
            frame_number,
            timestamp_ms,
            storage: PublishedSurfaceStorage::new_cpu_rgba(
                next_published_surface_storage_id(),
                Arc::new(canvas.as_rgba_bytes().to_vec()),
                None,
            ),
        }
    }

    /// Create a shared published surface from owned RGBA bytes.
    ///
    /// # Panics
    ///
    /// Panics if `rgba.len() != width * height * 4`.
    #[must_use]
    #[allow(clippy::as_conversions)]
    pub fn from_vec(
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        frame_number: u32,
        timestamp_ms: u32,
    ) -> Self {
        let expected = width as usize * height as usize * BYTES_PER_PIXEL;
        assert_eq!(
            rgba.len(),
            expected,
            "RGBA data length {} does not match {}x{}x4 = {}",
            rgba.len(),
            width,
            height,
            expected,
        );
        Self {
            descriptor: SurfaceDescriptor::rgba8888(width, height),
            generation: 0,
            frame_number,
            timestamp_ms,
            storage: PublishedSurfaceStorage::new_cpu_rgba(
                next_published_surface_storage_id(),
                Arc::new(rgba),
                None,
            ),
        }
    }

    /// Create a shared published surface from owned canvas storage.
    #[must_use]
    pub fn from_owned_canvas(canvas: Canvas, frame_number: u32, timestamp_ms: u32) -> Self {
        Self::from_owned_canvas_with_copy_info(canvas, frame_number, timestamp_ms).0
    }

    /// Create a shared published surface from owned canvas storage.
    #[must_use]
    pub fn from_owned_canvas_with_copy_info(
        canvas: Canvas,
        frame_number: u32,
        timestamp_ms: u32,
    ) -> (Self, bool) {
        let Canvas {
            width,
            height,
            pixels,
            storage_id,
            resource_owner,
        } = canvas;
        let descriptor = SurfaceDescriptor::rgba8888(width, height);
        (
            Self {
                descriptor,
                generation: 0,
                frame_number,
                timestamp_ms,
                storage: PublishedSurfaceStorage::new_cpu_rgba(storage_id, pixels, resource_owner),
            },
            false,
        )
    }

    /// Descriptor for this published surface.
    #[must_use]
    pub const fn descriptor(&self) -> SurfaceDescriptor {
        self.descriptor
    }

    /// Monotonic generation for slot-backed surfaces.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Frame number associated with this publish.
    #[must_use]
    pub const fn frame_number(&self) -> u32 {
        self.frame_number
    }

    /// Timestamp associated with this publish.
    #[must_use]
    pub const fn timestamp_ms(&self) -> u32 {
        self.timestamp_ms
    }

    /// Surface width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.descriptor.width
    }

    /// Surface height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.descriptor.height
    }

    /// Surface format.
    #[must_use]
    pub const fn format(&self) -> SurfaceFormat {
        self.descriptor.format
    }

    /// Published RGBA bytes.
    #[must_use]
    pub fn rgba_bytes(&self) -> &[u8] {
        self.storage.cpu_rgba().as_slice()
    }

    /// Stable identity for the current storage backing this surface.
    #[must_use]
    pub fn storage_identity(&self) -> PublishedSurfaceStorageIdentity {
        self.storage.identity()
    }

    /// Content digest cached on this surface's immutable backing storage.
    #[must_use]
    pub fn content_digest(&self) -> u64 {
        self.storage.content_digest(self.width(), self.height())
    }

    /// Previously computed content digest without forcing a full-buffer hash.
    #[must_use]
    pub fn cached_content_digest(&self) -> Option<u64> {
        self.storage.cached_content_digest()
    }

    /// Published RGBA byte length.
    #[must_use]
    pub fn rgba_len(&self) -> usize {
        self.storage.cpu_rgba().len()
    }

    /// Read a single pixel. Returns opaque black for out-of-bounds coordinates.
    #[must_use]
    #[allow(clippy::as_conversions)]
    pub fn get_pixel(&self, x: u32, y: u32) -> Rgba {
        if x >= self.width() || y >= self.height() {
            return Rgba::BLACK;
        }
        let idx = (y as usize * self.width() as usize + x as usize) * BYTES_PER_PIXEL;
        let rgba = self.storage.cpu_rgba();
        Rgba {
            r: rgba[idx],
            g: rgba[idx + 1],
            b: rgba[idx + 2],
            a: rgba[idx + 3],
        }
    }

    /// Clone the surface handle with updated frame metadata.
    #[must_use]
    pub fn with_frame_metadata(&self, frame_number: u32, timestamp_ms: u32) -> Self {
        Self {
            descriptor: self.descriptor,
            generation: self.generation,
            frame_number,
            timestamp_ms,
            storage: self.storage.clone(),
        }
    }
}

/// Lifecycle state for a slot in the render surface pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceState {
    /// Available for dequeue.
    #[default]
    Free,
    /// Currently leased for mutation.
    Dequeued,
    /// Published and still potentially shared by readers.
    Published,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceStateCounts {
    pub free: usize,
    pub dequeued: usize,
    pub published: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceSharingCounts {
    pub shared_published: usize,
    pub max_ref_count: usize,
}

const DEFAULT_RENDER_SURFACE_SLOTS: usize = 3;

/// Absolute floor for a pool's growth cap. Even a one-slot pool must be
/// permitted to absorb transient double-pinning without entering the
/// realloc-every-frame failure mode.
const MIN_POOL_CAP: usize = 16;

/// Growth factor applied to the initial slot count when the caller does
/// not pass an explicit cap. Four absorbs typical fan-out swings while
/// staying small enough that a leak shows up as a visible `grown_slots`
/// climb well before it starves RAM.
const DEFAULT_POOL_GROWTH_FACTOR: usize = 4;

fn default_pool_cap(initial_slots: usize) -> usize {
    initial_slots
        .saturating_mul(DEFAULT_POOL_GROWTH_FACTOR)
        .max(MIN_POOL_CAP)
}

#[derive(Clone)]
struct SurfaceSlot {
    canvas: Option<Canvas>,
    generation: u64,
    state: SurfaceState,
}

impl SurfaceSlot {
    fn try_new(descriptor: SurfaceDescriptor) -> Result<Self, SurfaceResourceError> {
        Ok(Self {
            canvas: Some(Canvas::try_new(descriptor.width, descriptor.height)?),
            generation: 0,
            state: SurfaceState::Free,
        })
    }

    const fn vacant() -> Self {
        Self {
            canvas: None,
            generation: 0,
            state: SurfaceState::Free,
        }
    }

    /// Prepare the slot for a new producer write. Returns `true` if the pool
    /// had to allocate a fresh canvas because the previous slot was still
    /// shared downstream (a sign the pool is undersized for current load).
    fn try_begin_dequeue(
        &mut self,
        descriptor: SurfaceDescriptor,
    ) -> Result<bool, SurfaceResourceError> {
        let reused_shared = self.state == SurfaceState::Published
            && self.canvas.as_ref().is_some_and(Canvas::is_shared);
        let needs_realloc = reused_shared
            || self.canvas.as_ref().is_none_or(|canvas| {
                canvas.width() != descriptor.width || canvas.height() != descriptor.height
            });
        if needs_realloc {
            drop(self.canvas.take());
            self.canvas = Some(Canvas::try_new(descriptor.width, descriptor.height)?);
        }

        self.state = SurfaceState::Dequeued;
        Ok(reused_shared)
    }
}

/// Elastic-capacity pool for reusable render surfaces.
///
/// The pool starts at its configured initial slot count and grows up to
/// `max_slots` on demand. A dequeue that cannot find a free slot first
/// tries to append a new slot (one-time `Canvas::new` per high-water
/// mark); only when the pool is at its cap does it fall back to reusing
/// a still-shared Published slot, which forces a `Canvas::new` every
/// frame until downstream releases its pins. That fallback path feeds
/// `saturation_reallocs` so the cap can be flagged as undersized in
/// metrics.
#[derive(Clone)]
pub struct RenderSurfacePool {
    descriptor: SurfaceDescriptor,
    slots: Vec<SurfaceSlot>,
    next_slot: usize,
    materialization: SurfaceMaterialization,
    /// Hard cap for `slots.len()`. Growth above this falls back to the
    /// realloc path; serves as a safety bound against runaway Arc leaks.
    max_slots: usize,
    /// Total count of slots appended after construction. Each growth
    /// event is a one-time `Canvas::new`; a high value simply means the
    /// pool has settled into its working set.
    grown_slots: u32,
    /// Total count of dequeues that hit the cap and had to reuse a
    /// still-shared Published slot (forcing a fresh `Canvas::new`). A
    /// rising value means `max_slots` is too small for current fan-out.
    saturation_reallocs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceMaterialization {
    Eager,
    Lazy,
}

impl RenderSurfacePool {
    /// Create a new render surface pool with the default slot count.
    #[must_use]
    pub fn new(descriptor: SurfaceDescriptor) -> Self {
        Self::with_slot_count(descriptor, DEFAULT_RENDER_SURFACE_SLOTS)
    }

    /// Fallible counterpart to [`Self::new`] for runtime-negotiated descriptors.
    pub fn try_new(descriptor: SurfaceDescriptor) -> Result<Self, SurfaceResourceError> {
        Self::try_with_slot_count(descriptor, DEFAULT_RENDER_SURFACE_SLOTS)
    }

    /// Create a render surface pool with an explicit initial slot count
    /// and an auto-computed growth cap.
    #[must_use]
    pub fn with_slot_count(descriptor: SurfaceDescriptor, slot_count: usize) -> Self {
        Self::with_slot_count_and_cap(descriptor, slot_count, default_pool_cap(slot_count))
    }

    /// Fallible counterpart to [`Self::with_slot_count`].
    pub fn try_with_slot_count(
        descriptor: SurfaceDescriptor,
        slot_count: usize,
    ) -> Result<Self, SurfaceResourceError> {
        Self::try_with_slot_count_and_cap(descriptor, slot_count, default_pool_cap(slot_count))
    }

    /// Create a pool whose initial slots allocate their canvases on first dequeue.
    pub fn try_with_lazy_slot_count(
        descriptor: SurfaceDescriptor,
        slot_count: usize,
    ) -> Result<Self, SurfaceResourceError> {
        Self::try_with_lazy_slot_count_and_cap(descriptor, slot_count, default_pool_cap(slot_count))
    }

    /// Create a lazily materialized pool with an explicit growth cap.
    pub fn try_with_lazy_slot_count_and_cap(
        descriptor: SurfaceDescriptor,
        slot_count: usize,
        max_slots: usize,
    ) -> Result<Self, SurfaceResourceError> {
        let slot_count = slot_count.max(1);
        let max_slots = max_slots.max(slot_count);
        let pool_byte_len = checked_pool_byte_len(descriptor, slot_count)?;
        let mut slots = Vec::new();
        slots.try_reserve_exact(slot_count).map_err(|_| {
            SurfaceResourceError::PoolAllocationFailed {
                width: descriptor.width,
                height: descriptor.height,
                slot_count,
                byte_len: pool_byte_len,
            }
        })?;
        slots.resize_with(slot_count, SurfaceSlot::vacant);
        Ok(Self {
            descriptor,
            slots,
            next_slot: 0,
            materialization: SurfaceMaterialization::Lazy,
            max_slots,
            grown_slots: 0,
            saturation_reallocs: 0,
        })
    }

    /// Create a render surface pool with explicit initial slot count and
    /// growth cap. `max_slots` is clamped to at least `slot_count` (and
    /// at least 1).
    #[must_use]
    pub fn with_slot_count_and_cap(
        descriptor: SurfaceDescriptor,
        slot_count: usize,
        max_slots: usize,
    ) -> Self {
        Self::try_with_slot_count_and_cap(descriptor, slot_count, max_slots)
            .expect("surface pool dimensions must fit available memory")
    }

    /// Fallible pool construction for runtime-negotiated descriptors.
    pub fn try_with_slot_count_and_cap(
        descriptor: SurfaceDescriptor,
        slot_count: usize,
        max_slots: usize,
    ) -> Result<Self, SurfaceResourceError> {
        let slot_count = slot_count.max(1);
        let max_slots = max_slots.max(slot_count);
        let pool_byte_len = checked_pool_byte_len(descriptor, slot_count)?;
        let mut slots = Vec::new();
        slots.try_reserve_exact(slot_count).map_err(|_| {
            SurfaceResourceError::PoolAllocationFailed {
                width: descriptor.width,
                height: descriptor.height,
                slot_count,
                byte_len: pool_byte_len,
            }
        })?;
        for _ in 0..slot_count {
            slots.push(SurfaceSlot::try_new(descriptor).map_err(|_| {
                SurfaceResourceError::PoolAllocationFailed {
                    width: descriptor.width,
                    height: descriptor.height,
                    slot_count,
                    byte_len: pool_byte_len,
                }
            })?);
        }
        Ok(Self {
            descriptor,
            slots,
            next_slot: 0,
            materialization: SurfaceMaterialization::Eager,
            max_slots,
            grown_slots: 0,
            saturation_reallocs: 0,
        })
    }

    /// Hard growth cap for this pool.
    #[must_use]
    pub const fn max_slots(&self) -> usize {
        self.max_slots
    }

    /// Count of slots appended since construction. Each event is a
    /// one-time `Canvas::new`; a settled pool converges on a fixed value.
    #[must_use]
    pub const fn grown_slots(&self) -> u32 {
        self.grown_slots
    }

    /// Cumulative count of dequeues that hit the cap and had to reuse a
    /// still-shared slot, forcing a fresh `Canvas::new` on every call.
    #[must_use]
    pub const fn saturation_reallocs(&self) -> u64 {
        self.saturation_reallocs
    }

    /// Surface descriptor shared by all slots.
    #[must_use]
    pub const fn descriptor(&self) -> SurfaceDescriptor {
        self.descriptor
    }

    /// Number of slots in the pool.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Number of slots whose canvas storage has been materialized.
    #[must_use]
    pub fn materialized_slot_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.canvas.is_some())
            .count()
    }

    /// Ensure the pool has at least the requested number of slots.
    pub fn ensure_slot_count(&mut self, slot_count: usize) {
        self.try_ensure_slot_count(slot_count)
            .expect("surface pool growth must fit available memory");
    }

    /// Grow the pool transactionally, leaving the current slots unchanged on failure.
    pub fn try_ensure_slot_count(&mut self, slot_count: usize) -> Result<(), SurfaceResourceError> {
        if slot_count <= self.slots.len() {
            return Ok(());
        }

        let pool_byte_len = checked_pool_byte_len(self.descriptor, slot_count)?;
        let additional = slot_count - self.slots.len();
        let mut new_slots = Vec::new();
        new_slots.try_reserve_exact(additional).map_err(|_| {
            SurfaceResourceError::PoolAllocationFailed {
                width: self.descriptor.width,
                height: self.descriptor.height,
                slot_count,
                byte_len: pool_byte_len,
            }
        })?;
        for _ in 0..additional {
            let slot =
                match self.materialization {
                    SurfaceMaterialization::Eager => SurfaceSlot::try_new(self.descriptor)
                        .map_err(|_| SurfaceResourceError::PoolAllocationFailed {
                            width: self.descriptor.width,
                            height: self.descriptor.height,
                            slot_count,
                            byte_len: pool_byte_len,
                        })?,
                    SurfaceMaterialization::Lazy => SurfaceSlot::vacant(),
                };
            new_slots.push(slot);
        }
        self.slots.extend(new_slots);
        self.max_slots = self.max_slots.max(slot_count);
        Ok(())
    }

    /// Current visible state of all pool slots.
    #[must_use]
    pub fn slot_states(&mut self) -> Vec<SurfaceState> {
        self.reclaim_published_slots();
        self.slots.iter().map(|slot| slot.state).collect()
    }

    /// Aggregate slot counts without allocating an intermediate state vector.
    #[must_use]
    pub fn slot_counts(&mut self) -> SurfaceStateCounts {
        self.reclaim_published_slots();
        let mut counts = SurfaceStateCounts::default();
        for slot in &self.slots {
            match slot.state {
                SurfaceState::Free => counts.free = counts.free.saturating_add(1),
                SurfaceState::Dequeued => {
                    counts.dequeued = counts.dequeued.saturating_add(1);
                }
                SurfaceState::Published => {
                    counts.published = counts.published.saturating_add(1);
                }
            }
        }
        counts
    }

    /// Count published slots still pinned by downstream surface handles.
    #[must_use]
    pub fn sharing_counts(&mut self) -> SurfaceSharingCounts {
        self.reclaim_published_slots();
        let mut counts = SurfaceSharingCounts::default();
        for slot in &self.slots {
            if slot.state != SurfaceState::Published {
                continue;
            }
            let Some(canvas) = slot.canvas.as_ref() else {
                continue;
            };
            let ref_count = canvas.shared_ref_count();
            counts.max_ref_count = counts.max_ref_count.max(ref_count);
            if ref_count > 1 {
                counts.shared_published = counts.shared_published.saturating_add(1);
            }
        }
        counts
    }

    /// Lease the next available surface slot for mutation.
    ///
    /// Preference order:
    /// 1. An already-Free materialized slot — no allocation.
    /// 2. A configured vacant slot — one-time `Canvas::new` only when
    ///    every materialized slot is busy.
    /// 3. Appending a new slot (one-time `Canvas::new`) if we are under
    ///    `max_slots`. Converts the chronic realloc pattern from an
    ///    undersized pool into a single high-water-mark growth event.
    /// 4. Reusing a still-shared Published slot — forces a
    ///    `Canvas::new` every call; bumps `saturation_reallocs` so it
    ///    surfaces in metrics.
    pub fn dequeue(&mut self) -> Option<SurfaceLease<'_>> {
        self.try_dequeue()
            .expect("surface pool dequeue must fit available memory")
    }

    /// Fallible dequeue for pools whose descriptor or fan-out is runtime-negotiated.
    pub fn try_dequeue(&mut self) -> Result<Option<SurfaceLease<'_>>, SurfaceResourceError> {
        self.try_dequeue_descriptor(self.descriptor, true)
    }

    /// Fallible dequeue that never replaces storage still pinned by readers.
    ///
    /// The pool may materialize configured slots and grow to `max_slots`, but
    /// returns `None` once every bounded slot is pinned or dequeued.
    pub fn try_dequeue_bounded(
        &mut self,
    ) -> Result<Option<SurfaceLease<'_>>, SurfaceResourceError> {
        self.try_dequeue_descriptor(self.descriptor, false)
    }

    /// Fallible bounded dequeue for a runtime-selected descriptor.
    ///
    /// Descriptor churn reuses only unpinned slots. Published readers keep
    /// their old backing while still counting against the same global slot
    /// bound, so alternating aspect ratios cannot create hidden generations.
    pub fn try_dequeue_bounded_for_descriptor(
        &mut self,
        descriptor: SurfaceDescriptor,
    ) -> Result<Option<SurfaceLease<'_>>, SurfaceResourceError> {
        descriptor.try_non_empty_byte_len()?;
        self.try_dequeue_descriptor(descriptor, false)
    }

    fn try_dequeue_descriptor(
        &mut self,
        descriptor: SurfaceDescriptor,
        replace_pinned: bool,
    ) -> Result<Option<SurfaceLease<'_>>, SurfaceResourceError> {
        self.reclaim_published_slots();

        for offset in 0..self.slots.len() {
            let index = (self.next_slot + offset) % self.slots.len();
            if self.slots[index].state != SurfaceState::Free || self.slots[index].canvas.is_none() {
                continue;
            }

            let _ = self.slots[index].try_begin_dequeue(descriptor)?;
            self.descriptor = descriptor;
            self.next_slot = (index + 1) % self.slots.len();
            return Ok(Some(SurfaceLease {
                descriptor,
                slot: &mut self.slots[index],
            }));
        }

        for offset in 0..self.slots.len() {
            let index = (self.next_slot + offset) % self.slots.len();
            if self.slots[index].state != SurfaceState::Free {
                continue;
            }

            let _ = self.slots[index].try_begin_dequeue(descriptor)?;
            self.descriptor = descriptor;
            self.next_slot = (index + 1) % self.slots.len();
            return Ok(Some(SurfaceLease {
                descriptor,
                slot: &mut self.slots[index],
            }));
        }

        if self.slots.len() < self.max_slots {
            let slot_count = self.slots.len() + 1;
            let pool_byte_len = checked_pool_byte_len(descriptor, slot_count)?;
            let mut fresh = SurfaceSlot::try_new(descriptor)?;
            let _ = fresh.try_begin_dequeue(descriptor)?;
            self.slots
                .try_reserve(1)
                .map_err(|_| SurfaceResourceError::PoolAllocationFailed {
                    width: descriptor.width,
                    height: descriptor.height,
                    slot_count,
                    byte_len: pool_byte_len,
                })?;
            self.slots.push(fresh);
            self.grown_slots = self.grown_slots.saturating_add(1);
            let index = self.slots.len() - 1;
            self.descriptor = descriptor;
            self.next_slot = (index + 1) % self.slots.len();
            return Ok(Some(SurfaceLease {
                descriptor,
                slot: &mut self.slots[index],
            }));
        }

        if !replace_pinned {
            return Ok(None);
        }

        for offset in 0..self.slots.len() {
            let index = (self.next_slot + offset) % self.slots.len();
            if self.slots[index].state != SurfaceState::Published {
                continue;
            }

            if self.slots[index].try_begin_dequeue(descriptor)? {
                self.saturation_reallocs = self.saturation_reallocs.saturating_add(1);
            }
            self.descriptor = descriptor;
            self.next_slot = (index + 1) % self.slots.len();
            return Ok(Some(SurfaceLease {
                descriptor,
                slot: &mut self.slots[index],
            }));
        }

        Ok(None)
    }

    fn reclaim_published_slots(&mut self) {
        for slot in &mut self.slots {
            if slot.state == SurfaceState::Published
                && slot
                    .canvas
                    .as_ref()
                    .is_some_and(|canvas| !canvas.is_shared())
            {
                slot.state = SurfaceState::Free;
            }
        }
    }
}

fn checked_pool_byte_len(
    descriptor: SurfaceDescriptor,
    slot_count: usize,
) -> Result<usize, SurfaceResourceError> {
    descriptor
        .try_non_empty_byte_len()?
        .checked_mul(slot_count)
        .ok_or(SurfaceResourceError::PoolByteLengthOverflow {
            width: descriptor.width,
            height: descriptor.height,
            slot_count,
        })
}

impl std::fmt::Debug for RenderSurfacePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderSurfacePool")
            .field("descriptor", &self.descriptor)
            .field("slot_count", &self.slots.len())
            .field("materialization", &self.materialization)
            .finish_non_exhaustive()
    }
}

/// Unique mutable lease to one render surface slot.
pub struct SurfaceLease<'a> {
    descriptor: SurfaceDescriptor,
    slot: &'a mut SurfaceSlot,
}

impl SurfaceLease<'_> {
    /// Descriptor for the leased surface.
    #[must_use]
    pub const fn descriptor(&self) -> SurfaceDescriptor {
        self.descriptor
    }

    /// Current slot generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.slot.generation
    }

    /// Mutable canvas view for direct CPU rendering.
    pub fn canvas_mut(&mut self) -> &mut Canvas {
        self.slot
            .canvas
            .as_mut()
            .expect("dequeued surface slot must own a canvas")
    }

    /// Publish the leased surface and return an immutable shared handle.
    #[must_use]
    pub fn submit(self, frame_number: u32, timestamp_ms: u32) -> PublishedSurface {
        self.slot.generation = self.slot.generation.saturating_add(1);
        self.slot.state = SurfaceState::Published;
        let canvas = self
            .slot
            .canvas
            .as_ref()
            .expect("dequeued surface slot must own a canvas");
        let descriptor = SurfaceDescriptor::rgba8888(canvas.width(), canvas.height());
        PublishedSurface {
            descriptor,
            generation: self.slot.generation,
            frame_number,
            timestamp_ms,
            storage: PublishedSurfaceStorage::new_cpu_rgba(
                canvas.storage_id,
                Arc::clone(&canvas.pixels),
                canvas.resource_owner.clone(),
            ),
        }
    }

    /// Return the leased slot to the pool without publishing.
    pub fn release(self) {
        self.slot.state = SurfaceState::Free;
    }
}
