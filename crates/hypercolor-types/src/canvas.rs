//! Canvas buffer, color types, blend modes, and perceptual color space conversions.
//!
//! This module contains the core pixel surface (`Canvas`), integer and floating-point
//! color representations (`Rgba`, `RgbaF32`, `Rgb`), blend mode compositing (`BlendMode`),
//! and Oklab/Oklch perceptual color spaces for smooth interpolation.

use serde::{Deserialize, Serialize};

// ── Canvas Constants ───────────────────────────────────────────────────────

/// The default canvas width, matching `SignalRGB`'s standard resolution.
pub const DEFAULT_CANVAS_WIDTH: u32 = 320;

/// The default canvas height, matching `SignalRGB`'s standard resolution.
pub const DEFAULT_CANVAS_HEIGHT: u32 = 200;

/// Bytes per pixel in the RGBA format.
pub const BYTES_PER_PIXEL: usize = 4;

// ── Rgba ───────────────────────────────────────────────────────────────────

/// A single pixel value — 8-bit RGBA.
///
/// This is the canonical pixel type for canvas storage and device output.
/// Values are in sRGB gamma space for storage; use [`RgbaF32`] for linear math.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba {
    /// Red channel (0–255).
    pub r: u8,
    /// Green channel (0–255).
    pub g: u8,
    /// Blue channel (0–255).
    pub b: u8,
    /// Alpha channel (0 = transparent, 255 = opaque).
    pub a: u8,
}

impl Rgba {
    /// Opaque black.
    pub const BLACK: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };

    /// Opaque white.
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };

    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    /// Create an RGBA pixel from individual channel values.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Convert to floating-point representation for interpolation math.
    ///
    /// Each channel is mapped from `[0, 255]` to `[0.0, 1.0]`.
    #[must_use]
    pub fn to_f32(self) -> RgbaF32 {
        RgbaF32 {
            r: f32::from(self.r) / 255.0,
            g: f32::from(self.g) / 255.0,
            b: f32::from(self.b) / 255.0,
            a: f32::from(self.a) / 255.0,
        }
    }

    /// Extract RGB only, discarding alpha.
    #[must_use]
    pub const fn to_rgb(self) -> Rgb {
        Rgb {
            r: self.r,
            g: self.g,
            b: self.b,
        }
    }
}

impl Default for Rgba {
    fn default() -> Self {
        Self::BLACK
    }
}

// ── Rgb ────────────────────────────────────────────────────────────────────

/// Device-facing RGB color (no alpha). This is what backends receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rgb {
    /// Red channel (0–255).
    pub r: u8,
    /// Green channel (0–255).
    pub g: u8,
    /// Blue channel (0–255).
    pub b: u8,
}

impl Rgb {
    /// Create an RGB color from individual channel values.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Promote to RGBA with full opacity.
    #[must_use]
    pub const fn to_rgba(self) -> Rgba {
        Rgba {
            r: self.r,
            g: self.g,
            b: self.b,
            a: 255,
        }
    }
}

// ── RgbaF32 (Color) ───────────────────────────────────────────────────────

/// Floating-point RGBA color in linear sRGB space.
///
/// Values are `0.0..=1.0` per channel. Used for interpolation, blending,
/// and color space conversions. Clamped on conversion back to [`Rgba`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RgbaF32 {
    /// Red channel (0.0–1.0, linear).
    pub r: f32,
    /// Green channel (0.0–1.0, linear).
    pub g: f32,
    /// Blue channel (0.0–1.0, linear).
    pub b: f32,
    /// Alpha channel (0.0 = transparent, 1.0 = opaque).
    pub a: f32,
}

impl RgbaF32 {
    /// Create a new floating-point color.
    #[must_use]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Create from 8-bit sRGB values, converting to linear float.
    ///
    /// Applies the sRGB transfer function (gamma decoding) to each RGB channel.
    /// Alpha is linearly mapped from `[0, 255]` to `[0.0, 1.0]`.
    #[must_use]
    pub fn from_srgb_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: srgb_to_linear(f32::from(r) / 255.0),
            g: srgb_to_linear(f32::from(g) / 255.0),
            b: srgb_to_linear(f32::from(b) / 255.0),
            a: f32::from(a) / 255.0,
        }
    }

    /// Convert back to 8-bit sRGB, applying gamma encoding.
    ///
    /// Applies the inverse sRGB transfer function to each RGB channel
    /// and clamps to `[0, 255]`.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    pub fn to_srgb_u8(self) -> [u8; 4] {
        [
            (linear_to_srgb(self.r) * 255.0).clamp(0.0, 255.0) as u8,
            (linear_to_srgb(self.g) * 255.0).clamp(0.0, 255.0) as u8,
            (linear_to_srgb(self.b) * 255.0).clamp(0.0, 255.0) as u8,
            (self.a * 255.0).clamp(0.0, 255.0) as u8,
        ]
    }

    /// Convert back to byte [`Rgba`], applying sRGB gamma encoding.
    ///
    /// This is the correct conversion for effect output headed to canvas
    /// storage or LED devices, where byte pixels are stored in sRGB space.
    #[must_use]
    pub fn to_srgba(self) -> Rgba {
        let [r, g, b, a] = self.to_srgb_u8();
        Rgba { r, g, b, a }
    }

    /// Convert back to byte [`Rgba`], clamping each channel to `[0, 255]`.
    ///
    /// This is a direct (non-gamma-corrected) conversion — each float channel
    /// is simply scaled by 255.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    pub fn to_rgba(self) -> Rgba {
        Rgba {
            r: (self.r * 255.0).clamp(0.0, 255.0) as u8,
            g: (self.g * 255.0).clamp(0.0, 255.0) as u8,
            b: (self.b * 255.0).clamp(0.0, 255.0) as u8,
            a: (self.a * 255.0).clamp(0.0, 255.0) as u8,
        }
    }

    /// Linear interpolation between two colors.
    ///
    /// `t = 0.0` returns `a`, `t = 1.0` returns `b`.
    #[must_use]
    pub fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        Self {
            r: a.r + (b.r - a.r) * t,
            g: a.g + (b.g - a.g) * t,
            b: a.b + (b.b - a.b) * t,
            a: a.a + (b.a - a.a) * t,
        }
    }

    /// Blend this color (source) onto `dst` (destination) using the given blend mode.
    ///
    /// The blend is modulated by `opacity` (0.0 = invisible, 1.0 = full effect).
    #[must_use]
    pub fn blend(self, dst: Self, mode: BlendMode, opacity: f32) -> Self {
        let src_arr = [self.r, self.g, self.b, self.a];
        let dst_arr = [dst.r, dst.g, dst.b, dst.a];
        let result = mode.blend(dst_arr, src_arr, opacity);
        Self {
            r: result[0],
            g: result[1],
            b: result[2],
            a: result[3],
        }
    }

    /// Convert to Oklab perceptual color space.
    #[must_use]
    pub fn to_oklab(self) -> Oklab {
        linear_srgb_to_oklab(self.r, self.g, self.b, self.a)
    }

    /// Create from Oklab perceptual color space.
    #[must_use]
    pub fn from_oklab(lab: Oklab) -> Self {
        oklab_to_linear_srgb(lab)
    }

    /// Convert to Oklch (perceptual lightness, chroma, hue).
    #[must_use]
    pub fn to_oklch(self) -> Oklch {
        self.to_oklab().to_oklch()
    }

    /// Create from Oklch perceptual color space.
    #[must_use]
    pub fn from_oklch(lch: Oklch) -> Self {
        Self::from_oklab(lch.to_oklab())
    }
}

impl Default for RgbaF32 {
    fn default() -> Self {
        Self {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    }
}

// ── Color (alias) ──────────────────────────────────────────────────────────

/// High-level color type — linear sRGB with float precision.
///
/// This is a convenience alias for [`RgbaF32`], used throughout the effect
/// pipeline where "Color" is the natural vocabulary.
pub type Color = RgbaF32;

// ── sRGB Transfer Functions ────────────────────────────────────────────────

/// Convert a single sRGB gamma-encoded channel to linear.
///
/// Implements the official sRGB EOTF (IEC 61966-2-1).
#[must_use]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Convert a single linear channel to sRGB gamma-encoded.
///
/// Implements the inverse sRGB EOTF (IEC 61966-2-1).
#[must_use]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

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
    pixels: Vec<u8>,
}

impl Canvas {
    /// Create a new canvas filled with opaque black.
    ///
    /// Allocates `width * height * 4` bytes zeroed, then sets every alpha byte to 255.
    #[must_use]
    #[allow(clippy::as_conversions)]
    pub fn new(width: u32, height: u32) -> Self {
        let len = width as usize * height as usize * BYTES_PER_PIXEL;
        let mut pixels = vec![0u8; len];
        // Set alpha channel to 255 (opaque) for every pixel
        for chunk in pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
            chunk[3] = 255;
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Create from a raw RGBA byte slice.
    ///
    /// # Panics
    ///
    /// Panics if `data.len() != width * height * 4`.
    #[must_use]
    #[allow(clippy::as_conversions)]
    pub fn from_rgba(data: &[u8], width: u32, height: u32) -> Self {
        let expected = width as usize * height as usize * BYTES_PER_PIXEL;
        assert_eq!(
            data.len(),
            expected,
            "RGBA data length {} does not match {}x{}x4 = {}",
            data.len(),
            width,
            height,
            expected,
        );
        Self {
            width,
            height,
            pixels: data.to_vec(),
        }
    }

    /// Wrap an existing `Vec<u8>` without copying. Takes ownership.
    ///
    /// # Panics
    ///
    /// Panics if `data.len() != width * height * 4`.
    #[must_use]
    #[allow(clippy::as_conversions)]
    pub fn from_vec(data: Vec<u8>, width: u32, height: u32) -> Self {
        let expected = width as usize * height as usize * BYTES_PER_PIXEL;
        assert_eq!(
            data.len(),
            expected,
            "Vec length {} does not match {}x{}x4 = {}",
            data.len(),
            width,
            height,
            expected,
        );
        Self {
            width,
            height,
            pixels: data,
        }
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
        &self.pixels
    }

    /// Mutable pixel slice for renderers writing directly into the buffer.
    pub fn as_rgba_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// View pixel data as `[u8; 4]` RGBA tuples.
    ///
    /// Returns a slice of length `width * height`.
    #[must_use]
    pub fn pixels(&self) -> impl ExactSizeIterator<Item = [u8; 4]> + '_ {
        self.pixels.chunks_exact(BYTES_PER_PIXEL).map(|chunk| {
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
        self.pixels[idx] = color.r;
        self.pixels[idx + 1] = color.g;
        self.pixels[idx + 2] = color.b;
        self.pixels[idx + 3] = color.a;
    }

    /// Fill the entire canvas with a single color.
    pub fn fill(&mut self, color: Rgba) {
        for chunk in self.pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
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
        let fx = nx * (self.width - 1) as f32;
        let fy = ny * (self.height - 1) as f32;

        let x0 = fx.floor() as u32;
        let y0 = fy.floor() as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);

        let frac_x = fx.fract();
        let frac_y = fy.fract();

        let tl = self.get_pixel(x0, y0).to_f32();
        let tr = self.get_pixel(x1, y0).to_f32();
        let bl = self.get_pixel(x0, y1).to_f32();
        let br = self.get_pixel(x1, y1).to_f32();

        // Horizontal lerp, then vertical lerp
        let top = RgbaF32::lerp(&tl, &tr, frac_x);
        let bot = RgbaF32::lerp(&bl, &br, frac_x);
        RgbaF32::lerp(&top, &bot, frac_y).to_rgba()
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
        let mut sum_r = 0u32;
        let mut sum_g = 0u32;
        let mut sum_b = 0u32;
        let mut sum_a = 0u32;
        let mut count = 0u32;

        for dy in -r..=r {
            for dx in -r..=r {
                let px = (cx as i32 + dx).clamp(0, self.width as i32 - 1) as u32;
                let py = (cy as i32 + dy).clamp(0, self.height as i32 - 1) as u32;
                let p = self.get_pixel(px, py);
                sum_r += u32::from(p.r);
                sum_g += u32::from(p.g);
                sum_b += u32::from(p.b);
                sum_a += u32::from(p.a);
                count += 1;
            }
        }

        #[allow(clippy::cast_possible_truncation)]
        Rgba {
            r: (sum_r / count) as u8,
            g: (sum_g / count) as u8,
            b: (sum_b / count) as u8,
            a: (sum_a / count) as u8,
        }
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

// ── BlendMode ──────────────────────────────────────────────────────────────

/// Blend modes for layer compositing.
///
/// All blend operations work on premultiplied-alpha RGBA pixels in `[0.0, 1.0]`.
/// At 320x200 (64,000 pixels), blending is trivially fast on CPU.
/// The wgpu path runs compositing as a compute shader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    /// Standard source-over alpha compositing.
    #[default]
    Normal,

    /// Additive blending: `dst + src`. Great for glow and flash effects.
    /// Result is clamped to 1.0.
    Add,

    /// Screen: `1 - (1-dst)(1-src)`. Brightens without blowing out.
    Screen,

    /// Multiply: `dst * src`. Darkens, useful for tinting.
    Multiply,

    /// Overlay: Screen if `dst > 0.5`, Multiply otherwise.
    /// Increases contrast.
    Overlay,

    /// Soft Light: Subtle tinting, less harsh than Overlay.
    SoftLight,

    /// Color Dodge: `dst / (1 - src)`. Creates intense highlights.
    ColorDodge,

    /// Difference: `|dst - src|`. Psychedelic color inversion.
    Difference,
}

impl BlendMode {
    /// Blend a source pixel onto a destination pixel.
    ///
    /// Both `dst` and `src` are RGBA arrays in `[0.0, 1.0]` range.
    /// `opacity` modulates the source alpha (0.0 = invisible, 1.0 = full).
    #[must_use]
    pub fn blend(self, dst: [f32; 4], src: [f32; 4], opacity: f32) -> [f32; 4] {
        let a = src[3] * opacity;
        let blend_channel = |d: f32, s: f32| -> f32 {
            let blended = match self {
                Self::Normal => s,
                Self::Add => (d + s).min(1.0),
                Self::Screen => 1.0 - (1.0 - d) * (1.0 - s),
                Self::Multiply => d * s,
                Self::Overlay => {
                    if d < 0.5 {
                        2.0 * d * s
                    } else {
                        1.0 - 2.0 * (1.0 - d) * (1.0 - s)
                    }
                }
                Self::SoftLight => {
                    if s < 0.5 {
                        d - (1.0 - 2.0 * s) * d * (1.0 - d)
                    } else {
                        d + (2.0 * s - 1.0) * (d.sqrt() - d)
                    }
                }
                Self::ColorDodge => {
                    if s >= 1.0 {
                        1.0
                    } else {
                        (d / (1.0 - s)).min(1.0)
                    }
                }
                Self::Difference => (d - s).abs(),
            };
            d.mul_add(1.0 - a, blended * a)
        };

        [
            blend_channel(dst[0], src[0]),
            blend_channel(dst[1], src[1]),
            blend_channel(dst[2], src[2]),
            (dst[3] + a - dst[3] * a).min(1.0),
        ]
    }
}

// ── Oklab ──────────────────────────────────────────────────────────────────

/// Oklab perceptual color space.
///
/// Oklab is a perceptually uniform color space designed by Björn Ottosson.
/// It provides linear interpolation that matches human perception of color
/// differences, making it ideal for smooth gradients and color transitions.
///
/// - `l`: Perceived lightness (0.0 = black, 1.0 = white)
/// - `a`: Green-red axis (negative = green, positive = red)
/// - `b`: Blue-yellow axis (negative = blue, positive = yellow)
/// - `alpha`: Opacity (0.0–1.0)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Oklab {
    /// Perceived lightness (0.0–1.0).
    pub l: f32,
    /// Green-red opponent channel.
    pub a: f32,
    /// Blue-yellow opponent channel.
    pub b: f32,
    /// Alpha/opacity (0.0–1.0).
    pub alpha: f32,
}

impl Oklab {
    /// Create a new Oklab color.
    #[must_use]
    pub const fn new(l: f32, a: f32, b: f32, alpha: f32) -> Self {
        Self { l, a, b, alpha }
    }

    /// Linear interpolation in Oklab space (perceptually smooth).
    ///
    /// `t = 0.0` returns `self`, `t = 1.0` returns `other`.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            l: self.l + (other.l - self.l) * t,
            a: self.a + (other.a - self.a) * t,
            b: self.b + (other.b - self.b) * t,
            alpha: self.alpha + (other.alpha - self.alpha) * t,
        }
    }

    /// Convert to Oklch (lightness, chroma, hue) representation.
    #[must_use]
    pub fn to_oklch(self) -> Oklch {
        let c = (self.a * self.a + self.b * self.b).sqrt();
        let h = self.b.atan2(self.a).to_degrees();
        // Normalize hue to [0, 360)
        let h = if h < 0.0 { h + 360.0 } else { h };
        Oklch {
            l: self.l,
            c,
            h,
            alpha: self.alpha,
        }
    }

    /// Convert back to linear sRGB.
    #[must_use]
    pub fn to_linear_srgb(self) -> RgbaF32 {
        oklab_to_linear_srgb(self)
    }
}

impl Default for Oklab {
    fn default() -> Self {
        Self {
            l: 0.0,
            a: 0.0,
            b: 0.0,
            alpha: 1.0,
        }
    }
}

// ── Oklch ──────────────────────────────────────────────────────────────────

/// Oklch perceptual color space (polar form of Oklab).
///
/// Oklch is the cylindrical representation of Oklab, providing intuitive
/// control over lightness, saturation (chroma), and hue. Ideal for
/// palette generation and hue-based operations.
///
/// - `l`: Perceived lightness (0.0 = black, 1.0 = white)
/// - `c`: Chroma / saturation (0.0 = gray, higher = more vivid)
/// - `h`: Hue angle in degrees (0–360)
/// - `alpha`: Opacity (0.0–1.0)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Oklch {
    /// Perceived lightness (0.0–1.0).
    pub l: f32,
    /// Chroma / saturation (0.0+).
    pub c: f32,
    /// Hue angle in degrees (0–360).
    pub h: f32,
    /// Alpha/opacity (0.0–1.0).
    pub alpha: f32,
}

impl Oklch {
    /// Create a new Oklch color.
    #[must_use]
    pub const fn new(l: f32, c: f32, h: f32, alpha: f32) -> Self {
        Self { l, c, h, alpha }
    }

    /// Convert to Oklab cartesian representation.
    #[must_use]
    pub fn to_oklab(self) -> Oklab {
        let h_rad = self.h.to_radians();
        Oklab {
            l: self.l,
            a: self.c * h_rad.cos(),
            b: self.c * h_rad.sin(),
            alpha: self.alpha,
        }
    }

    /// Convert to linear sRGB.
    #[must_use]
    pub fn to_linear_srgb(self) -> RgbaF32 {
        self.to_oklab().to_linear_srgb()
    }

    /// Interpolate in Oklch space with shortest-path hue interpolation.
    ///
    /// `t = 0.0` returns `self`, `t = 1.0` returns `other`.
    /// Hue interpolation takes the shortest arc around the color wheel.
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        // Shortest-path hue interpolation
        let mut dh = other.h - self.h;
        if dh > 180.0 {
            dh -= 360.0;
        } else if dh < -180.0 {
            dh += 360.0;
        }
        let h = self.h + dh * t;
        // Normalize to [0, 360)
        let h = ((h % 360.0) + 360.0) % 360.0;

        Self {
            l: self.l + (other.l - self.l) * t,
            c: self.c + (other.c - self.c) * t,
            h,
            alpha: self.alpha + (other.alpha - self.alpha) * t,
        }
    }
}

impl Default for Oklch {
    fn default() -> Self {
        Self {
            l: 0.0,
            c: 0.0,
            h: 0.0,
            alpha: 1.0,
        }
    }
}

// ── Oklab Conversion Functions ─────────────────────────────────────────────

/// Convert linear sRGB to Oklab.
///
/// Uses the Oklab forward transform (Björn Ottosson, 2020).
/// Input RGB values should be in linear light (not gamma-encoded).
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn linear_srgb_to_oklab(r: f32, g: f32, b: f32, alpha: f32) -> Oklab {
    // Linear sRGB -> LMS (using Oklab's M1 matrix)
    let lms_l = 0.412_221_46_f32.mul_add(r, 0.536_332_55_f32.mul_add(g, 0.051_445_99 * b));
    let lms_m = 0.211_903_5_f32.mul_add(r, 0.680_699_5_f32.mul_add(g, 0.107_396_96 * b));
    let lms_s = 0.088_302_46_f32.mul_add(r, 0.281_718_84_f32.mul_add(g, 0.629_978_7 * b));

    // Cube root (perceptual compression)
    let l_ = lms_l.cbrt();
    let m_ = lms_m.cbrt();
    let s_ = lms_s.cbrt();

    // LMS -> Lab (using Oklab's M2 matrix)
    Oklab {
        l: 0.210_454_26_f32.mul_add(l_, 0.793_617_8_f32.mul_add(m_, -0.004_072_047 * s_)),
        a: 1.977_998_5_f32.mul_add(l_, (-2.428_592_2_f32).mul_add(m_, 0.450_593_7 * s_)),
        b: 0.025_904_037_f32.mul_add(l_, 0.782_771_8_f32.mul_add(m_, -0.808_675_77 * s_)),
        alpha,
    }
}

/// Convert Oklab to linear sRGB.
///
/// Uses the Oklab inverse transform. Output RGB values are in linear light.
/// Values may fall outside `[0, 1]` for out-of-gamut colors — clamp if needed.
#[must_use]
pub fn oklab_to_linear_srgb(lab: Oklab) -> RgbaF32 {
    // Lab -> LMS (inverse of M2)
    let lms_l = lab
        .l
        .mul_add(1.0, 0.396_337_78_f32.mul_add(lab.a, 0.215_803_76 * lab.b));
    let lms_m = lab.l.mul_add(
        1.0,
        (-0.105_561_346_f32).mul_add(lab.a, -0.063_854_17 * lab.b),
    );
    let lms_s = lab.l.mul_add(
        1.0,
        (-0.089_484_18_f32).mul_add(lab.a, -1.291_485_5 * lab.b),
    );

    // Undo cube root
    let lin_l = lms_l * lms_l * lms_l;
    let lin_m = lms_m * lms_m * lms_m;
    let lin_s = lms_s * lms_s * lms_s;

    // LMS -> linear sRGB (inverse of M1)
    RgbaF32 {
        r: 4.076_741_7_f32.mul_add(
            lin_l,
            (-3.307_711_6_f32).mul_add(lin_m, 0.230_969_94 * lin_s),
        ),
        g: (-1.268_438_f32).mul_add(lin_l, 2.609_757_4_f32.mul_add(lin_m, -0.341_319_38 * lin_s)),
        b: (-0.004_196_086_3_f32).mul_add(
            lin_l,
            (-0.703_418_6_f32).mul_add(lin_m, 1.707_614_7 * lin_s),
        ),
        a: lab.alpha,
    }
}
