//! Immutable screen-publication requests and independently resolved descriptors.

use std::cmp::Ordering;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use super::{
    CaptureColorSpace, CaptureColorimetry, CaptureColorimetryError, CaptureDynamicRange,
    CaptureEpoch, CaptureGeometry, CaptureLuminanceContext, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CaptureTransferFunction, KnownCaptureColorimetry, PixelExtent, PixelRect,
    PlatformGpuApi,
};

/// Selector used by a consumer before capture-source resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenSourceSelector {
    /// Follow the daemon's configured source policy.
    Configured,
    /// Follow the current primary display.
    Primary,
    /// Select one stable source identity.
    Exact(CaptureSourceId),
}

/// Reflection still participating in source normalization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenSourceReflection {
    /// No reflection.
    #[default]
    None,
    /// Reflect around the vertical axis.
    Horizontal,
    /// Reflect around the horizontal axis.
    Vertical,
    /// Reflect around both axes.
    Both,
}

/// Capture backend that owns the source session.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenCaptureBackend {
    /// Windows Desktop Duplication capture.
    WindowsDesktopDuplication,
    /// Wayland portal and `PipeWire` capture.
    WaylandPipeWire,
    /// macOS `ScreenCaptureKit` capture.
    MacosScreenCaptureKit,
    /// Deterministic CPU or fixture source.
    Synthetic,
    /// Extensible backend identity.
    Other(Arc<str>),
}

/// Resource API backing a resolved capture source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenResourceApi {
    /// CPU-addressable source storage.
    Cpu,
    /// Platform GPU resource.
    PlatformGpu(PlatformGpuApi),
}

impl Ord for ScreenResourceApi {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Cpu, Self::Cpu) => Ordering::Equal,
            (Self::Cpu, Self::PlatformGpu(_)) => Ordering::Less,
            (Self::PlatformGpu(_), Self::Cpu) => Ordering::Greater,
            (Self::PlatformGpu(left), Self::PlatformGpu(right)) => {
                platform_gpu_api_cmp(left, right)
            }
        }
    }
}

impl PartialOrd for ScreenResourceApi {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Exact backend resource identity fencing reusable publication work.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenBackendResourceIdentity {
    backend: ScreenCaptureBackend,
    api: ScreenResourceApi,
    device_generation: u64,
    resource_generation: u64,
}

impl ScreenBackendResourceIdentity {
    /// Construct a complete backend resource fence.
    #[must_use]
    pub const fn new(
        backend: ScreenCaptureBackend,
        api: ScreenResourceApi,
        device_generation: u64,
        resource_generation: u64,
    ) -> Self {
        Self {
            backend,
            api,
            device_generation,
            resource_generation,
        }
    }

    /// Capture backend identity.
    #[must_use]
    pub const fn backend(&self) -> &ScreenCaptureBackend {
        &self.backend
    }

    /// Resource API identity.
    #[must_use]
    pub const fn api(&self) -> &ScreenResourceApi {
        &self.api
    }

    /// Generation of the backend device or adapter.
    #[must_use]
    pub const fn device_generation(&self) -> u64 {
        self.device_generation
    }

    /// Generation of the concrete resource set.
    #[must_use]
    pub const fn resource_generation(&self) -> u64 {
        self.resource_generation
    }
}

/// Complete source metadata participating in byte-equivalence identity.
///
/// Capture adapters create a fresh value after geometry, format, backend, or
/// resource-generation changes. Prepared-plan transitions retain the old and
/// candidate snapshots independently; this value is never mutated in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedScreenSourceConfig {
    geometry: CaptureGeometry,
    logical_extent: PixelExtent,
    reflection: ScreenSourceReflection,
    pixel_format: CapturePixelFormat,
    colorimetry: CaptureColorimetry,
    cursor_capabilities: ScreenCursorCapabilities,
    resources: ScreenBackendResourceIdentity,
}

impl ResolvedScreenSourceConfig {
    /// Construct one complete resolved-source configuration.
    #[must_use]
    pub const fn new(
        geometry: CaptureGeometry,
        logical_extent: PixelExtent,
        reflection: ScreenSourceReflection,
        pixel_format: CapturePixelFormat,
        colorimetry: CaptureColorimetry,
        resources: ScreenBackendResourceIdentity,
    ) -> Self {
        Self {
            geometry,
            logical_extent,
            reflection,
            pixel_format,
            colorimetry,
            cursor_capabilities: ScreenCursorCapabilities::clean_only(),
            resources,
        }
    }

    /// Construct a source with explicit cursor storage capabilities.
    #[must_use]
    pub const fn new_with_cursor_capabilities(
        geometry: CaptureGeometry,
        logical_extent: PixelExtent,
        reflection: ScreenSourceReflection,
        pixel_format: CapturePixelFormat,
        colorimetry: CaptureColorimetry,
        cursor_capabilities: ScreenCursorCapabilities,
        resources: ScreenBackendResourceIdentity,
    ) -> Self {
        Self {
            geometry,
            logical_extent,
            reflection,
            pixel_format,
            colorimetry,
            cursor_capabilities,
            resources,
        }
    }

    /// Native and storage geometry, origin, rotation, crop, and source scale.
    #[must_use]
    pub const fn geometry(&self) -> CaptureGeometry {
        self.geometry
    }

    /// Processed native logical extent.
    #[must_use]
    pub const fn logical_extent(&self) -> PixelExtent {
        self.logical_extent
    }

    /// Reflection participating in normalization.
    #[must_use]
    pub const fn reflection(&self) -> ScreenSourceReflection {
        self.reflection
    }

    /// Native source pixel format.
    #[must_use]
    pub const fn pixel_format(&self) -> CapturePixelFormat {
        self.pixel_format
    }

    /// Native source color metadata.
    #[must_use]
    pub const fn colorimetry(&self) -> CaptureColorimetry {
        self.colorimetry
    }

    /// Cursor storage modes the backend can truthfully provide.
    #[must_use]
    pub const fn cursor_capabilities(&self) -> ScreenCursorCapabilities {
        self.cursor_capabilities
    }

    /// Backend API and device/resource generation fence.
    #[must_use]
    pub const fn resources(&self) -> &ScreenBackendResourceIdentity {
        &self.resources
    }
}

impl Ord for ResolvedScreenSourceConfig {
    fn cmp(&self, other: &Self) -> Ordering {
        capture_geometry_cmp(self.geometry, other.geometry)
            .then_with(|| extent_key(self.logical_extent).cmp(&extent_key(other.logical_extent)))
            .then_with(|| self.reflection.cmp(&other.reflection))
            .then_with(|| {
                pixel_format_rank(self.pixel_format).cmp(&pixel_format_rank(other.pixel_format))
            })
            .then_with(|| self.colorimetry.cmp(&other.colorimetry))
            .then_with(|| self.cursor_capabilities.cmp(&other.cursor_capabilities))
            .then_with(|| self.resources.cmp(&other.resources))
    }
}

impl PartialOrd for ResolvedScreenSourceConfig {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Whether a bounded request may produce more pixels than its source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenUpscalePolicy {
    /// Treat dimensions as upper bounds and never manufacture source pixels.
    #[default]
    Never,
    /// Permit resampling above the source's logical extent.
    Allow,
}

/// Valid non-native bounds with at least one requested axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenBoundedExtent {
    max_width: Option<NonZeroU32>,
    max_height: Option<NonZeroU32>,
    upscale: ScreenUpscalePolicy,
}

impl ScreenBoundedExtent {
    /// Maximum output width, or an aspect-derived width when absent.
    #[must_use]
    pub const fn max_width(self) -> Option<NonZeroU32> {
        self.max_width
    }

    /// Maximum output height, or an aspect-derived height when absent.
    #[must_use]
    pub const fn max_height(self) -> Option<NonZeroU32> {
        self.max_height
    }

    /// Whether resolution may exceed the processed source.
    #[must_use]
    pub const fn upscale(self) -> ScreenUpscalePolicy {
        self.upscale
    }
}

/// Requested logical extent before source resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenExtentRequest {
    /// Use the source's processed native logical extent.
    #[default]
    Native,
    /// Preserve aspect while fitting at least one requested axis.
    Bounded(ScreenBoundedExtent),
}

impl ScreenExtentRequest {
    /// Construct a bounded request, canonicalizing two absent axes to native.
    #[must_use]
    pub const fn bounded(
        max_width: Option<NonZeroU32>,
        max_height: Option<NonZeroU32>,
        upscale: ScreenUpscalePolicy,
    ) -> Self {
        if max_width.is_none() && max_height.is_none() {
            Self::Native
        } else {
            Self::Bounded(ScreenBoundedExtent {
                max_width,
                max_height,
                upscale,
            })
        }
    }

    /// Return the structurally valid bounds, or `None` for native resolution.
    #[must_use]
    pub const fn bounded_extent(self) -> Option<ScreenBoundedExtent> {
        match self {
            Self::Native => None,
            Self::Bounded(bounds) => Some(bounds),
        }
    }
}

/// Geometric fit rule used to resolve a bounded extent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenAspectPolicy {
    /// Preserve the full source inside the requested bounds.
    #[default]
    Contain,
    /// Fill a two-axis output by cropping the source around its center.
    Cover,
}

/// Kind of publication produced by one independent branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenPublicationKind {
    /// Publish a raster surface.
    Surface,
    /// Publish reduced colors over a logical zone grid.
    Zones {
        /// Number of grid columns.
        columns: NonZeroU32,
        /// Number of grid rows.
        rows: NonZeroU32,
    },
}

/// Storage residency required by one exact publication descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenPublicationResidency {
    /// Host-addressable storage.
    Cpu,
    /// Opaque platform GPU storage retained by its native owner.
    PlatformGpu(PlatformGpuApi),
}

impl Ord for ScreenPublicationResidency {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Cpu, Self::Cpu) => Ordering::Equal,
            (Self::Cpu, Self::PlatformGpu(_)) => Ordering::Less,
            (Self::PlatformGpu(_), Self::Cpu) => Ordering::Greater,
            (Self::PlatformGpu(left), Self::PlatformGpu(right)) => {
                platform_gpu_api_cmp(left, right)
            }
        }
    }
}

impl PartialOrd for ScreenPublicationResidency {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Executor requested by one logical publication consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenPublicationExecutorRequest {
    /// Materialize exact host-addressable output, reading back a GPU source when needed.
    Cpu,
    /// Execute on the source's native storage API without host readback.
    SourceNative,
}

/// Concrete executor selected after resolving the capture source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenPublicationExecutor {
    /// CPU reduction over a host-addressable source or exact platform readback.
    Cpu,
    /// Native platform reduction retaining GPU ownership.
    SourceNative(PlatformGpuApi),
}

impl Ord for ScreenPublicationExecutor {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Cpu, Self::Cpu) => Ordering::Equal,
            (Self::Cpu, Self::SourceNative(_)) => Ordering::Less,
            (Self::SourceNative(_), Self::Cpu) => Ordering::Greater,
            (Self::SourceNative(left), Self::SourceNative(right)) => {
                platform_gpu_api_cmp(left, right)
            }
        }
    }
}

impl PartialOrd for ScreenPublicationExecutor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Canonical finite scalar used by byte-changing processing controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenProfileScalar(u32);

impl ScreenProfileScalar {
    const ZERO: Self = Self(0.0_f32.to_bits());
    const ONE: Self = Self(1.0_f32.to_bits());

    /// Canonicalize a finite scalar, including negative zero.
    ///
    /// # Errors
    ///
    /// Returns [`ScreenPublicationError::NonFiniteProfileScalar`] for NaN or
    /// either infinity.
    pub fn try_new(value: f32) -> Result<Self, ScreenPublicationError> {
        if !value.is_finite() {
            return Err(ScreenPublicationError::NonFiniteProfileScalar);
        }
        Ok(if value == 0.0 {
            Self::ZERO
        } else {
            Self(value.to_bits())
        })
    }

    /// Recover the finite scalar value.
    #[must_use]
    pub const fn value(self) -> f32 {
        f32::from_bits(self.0)
    }
}

/// Detection and removal policy for bars already present in source content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenContentBarsPolicy {
    /// Keep the full source content.
    #[default]
    Disabled,
    /// Detect and crop bars whose luminance stays below the threshold.
    DetectAndCrop {
        /// Canonical finite luminance threshold.
        luminance_threshold: ScreenProfileScalar,
    },
}

/// Fill treatment after aspect resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenLetterboxFill {
    /// Leave pixels outside the fitted source transparent.
    Transparent,
    /// Fill outside pixels with one exact RGBA color.
    Solid([u8; 4]),
    /// Extend the nearest source edge into outside pixels.
    EdgeExtend,
}

impl Default for ScreenLetterboxFill {
    fn default() -> Self {
        Self::Solid([0, 0, 0, u8::MAX])
    }
}

/// Scene-cut reset rule for temporal smoothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenSceneCutPolicy {
    /// Never reset smoothing based on scene content.
    #[default]
    Disabled,
    /// Reset when mean absolute channel delta reaches the threshold.
    MeanAbsoluteDelta {
        /// Canonical finite scene-change threshold.
        threshold: ScreenProfileScalar,
    },
}

/// Temporal smoothing contract for one publication branch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenSmoothingPolicy {
    /// Publish every processed sample without temporal smoothing.
    #[default]
    Disabled,
    /// Apply exponential smoothing with optional scene-cut resets.
    Exponential {
        /// Time constant controlling the smoothing response.
        time_constant: Duration,
        /// Rule that resets history across discontinuous scenes.
        scene_cut: ScreenSceneCutPolicy,
    },
}

/// Canonical saturation, brightness, and gamma controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenColorTuning {
    saturation: ScreenProfileScalar,
    brightness: ScreenProfileScalar,
    gamma: ScreenProfileScalar,
}

impl ScreenColorTuning {
    /// Construct tuning from finite scalar values.
    ///
    /// # Errors
    ///
    /// Returns [`ScreenPublicationError::NonFiniteProfileScalar`] when any
    /// value is non-finite.
    pub fn try_new(
        saturation: f32,
        brightness: f32,
        gamma: f32,
    ) -> Result<Self, ScreenPublicationError> {
        Ok(Self {
            saturation: ScreenProfileScalar::try_new(saturation)?,
            brightness: ScreenProfileScalar::try_new(brightness)?,
            gamma: ScreenProfileScalar::try_new(gamma)?,
        })
    }

    /// Saturation multiplier.
    #[must_use]
    pub const fn saturation(self) -> f32 {
        self.saturation.value()
    }

    /// Brightness multiplier.
    #[must_use]
    pub const fn brightness(self) -> f32 {
        self.brightness.value()
    }

    /// Gamma exponent.
    #[must_use]
    pub const fn gamma(self) -> f32 {
        self.gamma.value()
    }
}

impl Default for ScreenColorTuning {
    fn default() -> Self {
        Self {
            saturation: ScreenProfileScalar::ONE,
            brightness: ScreenProfileScalar::ONE,
            gamma: ScreenProfileScalar::ONE,
        }
    }
}

/// Cursor composition policy for derived bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenCursorPolicy {
    /// Exclude cursor pixels from analysis.
    #[default]
    Exclude,
    /// Include the resolved cursor in analysis.
    Include,
}

/// Cursor storage modes available from one resolved capture source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenCursorCapabilities {
    clean_surface: bool,
    separate_cursor: bool,
    composed_surface: bool,
}

impl ScreenCursorCapabilities {
    /// Describe independently available clean, separate, and composed pixels.
    #[must_use]
    pub const fn new(clean_surface: bool, separate_cursor: bool, composed_surface: bool) -> Self {
        Self {
            clean_surface,
            separate_cursor,
            composed_surface,
        }
    }

    /// A clean surface with no separately-owned cursor pixels.
    #[must_use]
    pub const fn clean_only() -> Self {
        Self::new(true, false, false)
    }

    /// A clean surface plus separately-owned cursor shapes.
    #[must_use]
    pub const fn clean_with_separate_cursor() -> Self {
        Self::new(true, true, false)
    }

    /// A surface whose visible cursor is irreversibly composed.
    #[must_use]
    pub const fn composed_only() -> Self {
        Self::new(false, false, true)
    }

    /// Whether cursor-free pixels can be published.
    #[must_use]
    pub const fn has_clean_surface(self) -> bool {
        self.clean_surface
    }

    /// Whether cursor pixels can be composed from separate storage.
    #[must_use]
    pub const fn has_separate_cursor(self) -> bool {
        self.separate_cursor
    }

    /// Whether the source can provide pixels with the cursor already composed.
    #[must_use]
    pub const fn has_composed_surface(self) -> bool {
        self.composed_surface
    }

    const fn supports_inclusion(self) -> bool {
        self.composed_surface || (self.clean_surface && self.separate_cursor)
    }
}

impl Default for ScreenCursorCapabilities {
    fn default() -> Self {
        Self::clean_only()
    }
}

/// Grid sampling policy for zone publications.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenGridPolicy {
    /// Weight every covered source pixel by its area.
    #[default]
    AreaWeighted,
    /// Sample one normalized point per zone.
    PointSample,
}

/// Reduction filter used while producing the resolved raster.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenReductionFilter {
    /// Select the nearest source sample.
    Nearest,
    /// Interpolate four neighboring source samples.
    Bilinear,
    /// Integrate source coverage over each destination pixel.
    #[default]
    Area,
}

/// Requested output color contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenTargetColorimetry {
    /// Retain the source encoding and metadata exactly.
    PreserveSource,
    /// Decode and encode into one fully-known target contract.
    ConvertTo(KnownCaptureColorimetry),
}

impl Default for ScreenTargetColorimetry {
    fn default() -> Self {
        Self::ConvertTo(KnownCaptureColorimetry::SRGB)
    }
}

/// Color operations one concrete reducer can execute faithfully.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenColorTransformCapabilities {
    linear_light_sdr_processing: bool,
    linear_relative_color_conversion: bool,
    pq_bt2390_tone_mapping: bool,
    algorithm_revision: Option<NonZeroU32>,
}

impl ScreenColorTransformCapabilities {
    /// No byte-changing color operation is executable.
    pub const NONE: Self = Self {
        linear_light_sdr_processing: false,
        linear_relative_color_conversion: false,
        pq_bt2390_tone_mapping: false,
        algorithm_revision: None,
    };

    /// Declare the exact color operations implemented by one reducer.
    #[must_use]
    pub const fn new(
        linear_light_sdr_processing: bool,
        linear_relative_color_conversion: bool,
        pq_bt2390_tone_mapping: bool,
        algorithm_revision: NonZeroU32,
    ) -> Self {
        Self {
            linear_light_sdr_processing,
            linear_relative_color_conversion,
            pq_bt2390_tone_mapping,
            algorithm_revision: Some(algorithm_revision),
        }
    }

    /// Whether same-encoding SDR samples can be processed in linear light end to end.
    #[must_use]
    pub const fn supports_linear_light_sdr_processing(self) -> bool {
        self.linear_light_sdr_processing
    }

    /// Whether differing known SDR contracts can be converted end to end.
    #[must_use]
    pub const fn supports_linear_relative_color_conversion(self) -> bool {
        self.linear_relative_color_conversion
    }

    /// Whether PQ HDR can be mapped to SDR with BT.2390 end to end.
    #[must_use]
    pub const fn supports_pq_bt2390_tone_mapping(self) -> bool {
        self.pq_bt2390_tone_mapping
    }

    /// Whether this reducer's end-to-end conversion promises cover one gamut policy.
    #[must_use]
    pub const fn supports_gamut_policy(self, policy: ScreenGamutMapPolicy) -> bool {
        match policy {
            ScreenGamutMapPolicy::RelativeColorimetricClip => {
                self.linear_relative_color_conversion || self.pq_bt2390_tone_mapping
            }
        }
    }

    /// Exact processing algorithm revision implemented by this reducer.
    #[must_use]
    pub const fn algorithm_revision(self) -> Option<NonZeroU32> {
        self.algorithm_revision
    }
}

/// Policy for source metadata that is not complete enough to decode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenUnknownColorPolicy {
    /// Reject the branch before backend preparation.
    #[default]
    Reject,
    /// Retain unknown encoded samples only through a byte-preserving surface path.
    PreserveEncodedSamples,
    /// Fill only missing source fields from an explicit known contract.
    Assume(KnownCaptureColorimetry),
}

/// Gamut behavior for known-primary conversions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenGamutMapPolicy {
    /// Apply the relative-colorimetric matrix and clip target-linear channels.
    #[default]
    RelativeColorimetricClip,
}

/// Precisely selected HDR-to-SDR electro-electrical transfer operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenToneMapOperator {
    /// ITU-R BT.2390 electro-electrical transfer function.
    Bt2390Eetf,
}

/// Requested HDR-to-SDR tone-map parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenToneMapPolicy {
    operator: ScreenToneMapOperator,
    target_luminance: CaptureLuminanceContext,
}

impl ScreenToneMapPolicy {
    /// Construct one complete tone-map request.
    #[must_use]
    pub const fn new(
        operator: ScreenToneMapOperator,
        target_luminance: CaptureLuminanceContext,
    ) -> Self {
        Self {
            operator,
            target_luminance,
        }
    }

    /// Exact tone-map operator.
    #[must_use]
    pub const fn operator(self) -> ScreenToneMapOperator {
        self.operator
    }

    /// Absolute target display context.
    #[must_use]
    pub const fn target_luminance(self) -> CaptureLuminanceContext {
        self.target_luminance
    }
}

/// Admission policy for HDR source or target contracts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenHdrPolicy {
    /// Reject HDR before backend preparation.
    #[default]
    Reject,
    /// Resolve an HDR source into an SDR target with exact parameters.
    ToneMap(ScreenToneMapPolicy),
}

/// Fully-resolved tone-map operation participating in physical identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResolvedScreenToneMap {
    operator: ScreenToneMapOperator,
    source_luminance: CaptureLuminanceContext,
    target_luminance: CaptureLuminanceContext,
    gamut: ScreenGamutMapPolicy,
}

impl ResolvedScreenToneMap {
    /// Exact tone-map operator.
    #[must_use]
    pub const fn operator(self) -> ScreenToneMapOperator {
        self.operator
    }

    /// Absolute source luminance context.
    #[must_use]
    pub const fn source_luminance(self) -> CaptureLuminanceContext {
        self.source_luminance
    }

    /// Absolute target luminance context.
    #[must_use]
    pub const fn target_luminance(self) -> CaptureLuminanceContext {
        self.target_luminance
    }

    /// Exact gamut conversion behavior.
    #[must_use]
    pub const fn gamut(self) -> ScreenGamutMapPolicy {
        self.gamut
    }
}

/// Byte-changing color operation selected before backend preparation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolvedScreenColorTransform {
    /// Preserve source encoded samples and metadata.
    PreserveEncodedSamples,
    /// Decode, process, and re-encode without changing the SDR encoding.
    LinearLightSdr,
    /// Decode, convert primaries in linear light, and encode the target.
    LinearRelativeColorimetric { gamut: ScreenGamutMapPolicy },
    /// Decode and tone-map HDR samples into an SDR target.
    ToneMap(ResolvedScreenToneMap),
}

/// Complete color pipeline used as part of physical byte identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResolvedScreenColorPipeline {
    effective_source: Option<KnownCaptureColorimetry>,
    output: CaptureColorimetry,
    transform: ResolvedScreenColorTransform,
}

impl ResolvedScreenColorPipeline {
    /// Effective source contract after explicit assumption resolution.
    #[must_use]
    pub const fn effective_source(self) -> Option<KnownCaptureColorimetry> {
        self.effective_source
    }

    /// Exact metadata describing published channel values.
    #[must_use]
    pub const fn output(self) -> CaptureColorimetry {
        self.output
    }

    /// Exact color operation required before publication.
    #[must_use]
    pub const fn transform(self) -> ResolvedScreenColorTransform {
        self.transform
    }
}

/// Complete immutable byte-changing processing configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenProcessingProfile {
    content_bars: ScreenContentBarsPolicy,
    letterbox_fill: ScreenLetterboxFill,
    smoothing: ScreenSmoothingPolicy,
    tuning: ScreenColorTuning,
    cursor: ScreenCursorPolicy,
    grid: ScreenGridPolicy,
    reduction_filter: ScreenReductionFilter,
    target_pixel_format: CapturePixelFormat,
    target_colorimetry: ScreenTargetColorimetry,
    unknown_color: ScreenUnknownColorPolicy,
    hdr: ScreenHdrPolicy,
    gamut: ScreenGamutMapPolicy,
    algorithm_revision: NonZeroU32,
}

/// Input fields for constructing an immutable processing profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenProcessingProfileConfig {
    /// Detection and crop policy for bars in source content.
    pub content_bars: ScreenContentBarsPolicy,
    /// Fill treatment after geometric fitting.
    pub letterbox_fill: ScreenLetterboxFill,
    /// Temporal smoothing and scene-cut behavior.
    pub smoothing: ScreenSmoothingPolicy,
    /// Saturation, brightness, and gamma controls.
    pub tuning: ScreenColorTuning,
    /// Cursor composition policy.
    pub cursor: ScreenCursorPolicy,
    /// Zone grid sampling policy.
    pub grid: ScreenGridPolicy,
    /// Raster reduction filter.
    pub reduction_filter: ScreenReductionFilter,
    /// Pixel storage format of the derived output.
    pub target_pixel_format: CapturePixelFormat,
    /// Desired output color contract.
    pub target_colorimetry: ScreenTargetColorimetry,
    /// Admission policy for incomplete source metadata.
    pub unknown_color: ScreenUnknownColorPolicy,
    /// Admission policy for HDR sources and targets.
    pub hdr: ScreenHdrPolicy,
    /// Gamut behavior for known-primary conversions.
    pub gamut: ScreenGamutMapPolicy,
    /// Revision of the complete processing algorithm.
    pub algorithm_revision: NonZeroU32,
}

impl ScreenProcessingProfileConfig {
    /// Exact encoded-sample identity without any managed color operation.
    ///
    /// This profile is executable without reducer color capabilities for a
    /// canonical native surface. Fully-known SDR metadata is retained, while
    /// unknown metadata is admitted only through the same byte-identity proof.
    /// Algorithm revision is irrelevant because no processing is performed.
    #[must_use]
    pub fn exact_encoded_identity(pixel_format: CapturePixelFormat) -> Self {
        Self {
            content_bars: ScreenContentBarsPolicy::Disabled,
            smoothing: ScreenSmoothingPolicy::Disabled,
            tuning: ScreenColorTuning::default(),
            cursor: ScreenCursorPolicy::Exclude,
            reduction_filter: ScreenReductionFilter::Nearest,
            target_pixel_format: pixel_format,
            target_colorimetry: ScreenTargetColorimetry::PreserveSource,
            unknown_color: ScreenUnknownColorPolicy::PreserveEncodedSamples,
            ..Self::default()
        }
    }
}

/// Desired high-quality managed output. Resolution remains capability-gated
/// until the reducer advertises executable color transforms.
impl Default for ScreenProcessingProfileConfig {
    fn default() -> Self {
        Self {
            content_bars: ScreenContentBarsPolicy::default(),
            letterbox_fill: ScreenLetterboxFill::default(),
            smoothing: ScreenSmoothingPolicy::default(),
            tuning: ScreenColorTuning::default(),
            cursor: ScreenCursorPolicy::default(),
            grid: ScreenGridPolicy::default(),
            reduction_filter: ScreenReductionFilter::default(),
            target_pixel_format: CapturePixelFormat::Rgba8,
            target_colorimetry: ScreenTargetColorimetry::default(),
            unknown_color: ScreenUnknownColorPolicy::default(),
            hdr: ScreenHdrPolicy::default(),
            gamut: ScreenGamutMapPolicy::default(),
            algorithm_revision: NonZeroU32::MIN,
        }
    }
}

impl ScreenProcessingProfile {
    /// Freeze a complete processing configuration.
    #[must_use]
    pub const fn new(config: ScreenProcessingProfileConfig) -> Self {
        Self {
            content_bars: config.content_bars,
            letterbox_fill: config.letterbox_fill,
            smoothing: config.smoothing,
            tuning: config.tuning,
            cursor: config.cursor,
            grid: config.grid,
            reduction_filter: config.reduction_filter,
            target_pixel_format: config.target_pixel_format,
            target_colorimetry: config.target_colorimetry,
            unknown_color: config.unknown_color,
            hdr: config.hdr,
            gamut: config.gamut,
            algorithm_revision: config.algorithm_revision,
        }
    }

    /// Content-bar detection policy.
    #[must_use]
    pub const fn content_bars(&self) -> ScreenContentBarsPolicy {
        self.content_bars
    }

    /// Post-fit fill treatment.
    #[must_use]
    pub const fn letterbox_fill(&self) -> ScreenLetterboxFill {
        self.letterbox_fill
    }

    /// Temporal smoothing policy.
    #[must_use]
    pub const fn smoothing(&self) -> ScreenSmoothingPolicy {
        self.smoothing
    }

    /// Color tuning controls.
    #[must_use]
    pub const fn tuning(&self) -> ScreenColorTuning {
        self.tuning
    }

    /// Cursor composition policy.
    #[must_use]
    pub const fn cursor(&self) -> ScreenCursorPolicy {
        self.cursor
    }

    /// Zone grid sampling policy.
    #[must_use]
    pub const fn grid(&self) -> ScreenGridPolicy {
        self.grid
    }

    /// Raster reduction filter.
    #[must_use]
    pub const fn reduction_filter(&self) -> ScreenReductionFilter {
        self.reduction_filter
    }

    /// Derived output pixel format.
    #[must_use]
    pub const fn target_pixel_format(&self) -> CapturePixelFormat {
        self.target_pixel_format
    }

    /// Desired output color contract.
    #[must_use]
    pub const fn target_colorimetry(&self) -> ScreenTargetColorimetry {
        self.target_colorimetry
    }

    /// Incomplete-source admission policy.
    #[must_use]
    pub const fn unknown_color(&self) -> ScreenUnknownColorPolicy {
        self.unknown_color
    }

    /// HDR admission policy.
    #[must_use]
    pub const fn hdr(&self) -> ScreenHdrPolicy {
        self.hdr
    }

    /// Gamut conversion behavior.
    #[must_use]
    pub const fn gamut(&self) -> ScreenGamutMapPolicy {
        self.gamut
    }

    /// Complete processing algorithm revision.
    #[must_use]
    pub const fn algorithm_revision(&self) -> NonZeroU32 {
        self.algorithm_revision
    }
}

impl Default for ScreenProcessingProfile {
    fn default() -> Self {
        Self::new(ScreenProcessingProfileConfig::default())
    }
}

impl Ord for ScreenProcessingProfile {
    fn cmp(&self, other: &Self) -> Ordering {
        self.content_bars
            .cmp(&other.content_bars)
            .then_with(|| self.letterbox_fill.cmp(&other.letterbox_fill))
            .then_with(|| self.smoothing.cmp(&other.smoothing))
            .then_with(|| self.tuning.cmp(&other.tuning))
            .then_with(|| self.cursor.cmp(&other.cursor))
            .then_with(|| self.grid.cmp(&other.grid))
            .then_with(|| self.reduction_filter.cmp(&other.reduction_filter))
            .then_with(|| {
                pixel_format_rank(self.target_pixel_format)
                    .cmp(&pixel_format_rank(other.target_pixel_format))
            })
            .then_with(|| self.target_colorimetry.cmp(&other.target_colorimetry))
            .then_with(|| self.unknown_color.cmp(&other.unknown_color))
            .then_with(|| self.hdr.cmp(&other.hdr))
            .then_with(|| self.gamut.cmp(&other.gamut))
            .then_with(|| self.algorithm_revision.cmp(&other.algorithm_revision))
    }
}

impl PartialOrd for ScreenProcessingProfile {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One consumer's unresolved publication request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenPublicationRequest {
    selector: ScreenSourceSelector,
    kind: ScreenPublicationKind,
    executor: ScreenPublicationExecutorRequest,
    extent: ScreenExtentRequest,
    aspect: ScreenAspectPolicy,
    processing_profile: Arc<ScreenProcessingProfile>,
}

impl ScreenPublicationRequest {
    /// Construct one immutable logical publication request.
    #[must_use]
    pub fn new(
        selector: ScreenSourceSelector,
        kind: ScreenPublicationKind,
        executor: ScreenPublicationExecutorRequest,
        extent: ScreenExtentRequest,
        aspect: ScreenAspectPolicy,
        processing_profile: Arc<ScreenProcessingProfile>,
    ) -> Self {
        Self {
            selector,
            kind,
            executor,
            extent,
            aspect,
            processing_profile,
        }
    }

    /// Unresolved source selector.
    #[must_use]
    pub const fn selector(&self) -> &ScreenSourceSelector {
        &self.selector
    }

    /// Requested publication kind.
    #[must_use]
    pub const fn kind(&self) -> ScreenPublicationKind {
        self.kind
    }

    /// Requested execution lane.
    #[must_use]
    pub const fn executor(&self) -> ScreenPublicationExecutorRequest {
        self.executor
    }

    /// Requested extent policy.
    #[must_use]
    pub const fn extent(&self) -> ScreenExtentRequest {
        self.extent
    }

    /// Requested aspect policy.
    #[must_use]
    pub const fn aspect(&self) -> ScreenAspectPolicy {
        self.aspect
    }

    /// Complete processing profile.
    #[must_use]
    pub fn processing_profile(&self) -> &Arc<ScreenProcessingProfile> {
        &self.processing_profile
    }

    /// Resolve this request independently against one exact capture source.
    ///
    /// # Errors
    ///
    /// Rejects a source resolved from another selector and geometry whose
    /// aspect-derived dimensions cannot fit in `u32`.
    pub fn resolve(
        &self,
        source: &ResolvedScreenSource,
    ) -> Result<ResolvedScreenPublicationDescriptor, ScreenPublicationError> {
        self.resolve_with_color_capabilities(source, ScreenColorTransformCapabilities::NONE)
    }

    /// Resolve against one source and the exact color operations implemented
    /// by the selected reducer.
    ///
    /// # Errors
    ///
    /// In addition to ordinary resolution failures, rejects every
    /// byte-changing color operation not declared by `capabilities`.
    pub fn resolve_with_color_capabilities(
        &self,
        source: &ResolvedScreenSource,
        capabilities: ScreenColorTransformCapabilities,
    ) -> Result<ResolvedScreenPublicationDescriptor, ScreenPublicationError> {
        source.validate_selector(&self.selector)?;
        let cursor_capabilities = source.config.cursor_capabilities;
        match self.processing_profile.cursor {
            ScreenCursorPolicy::Exclude if !cursor_capabilities.has_clean_surface() => {
                return Err(ScreenPublicationError::CursorExclusionUnsupported);
            }
            ScreenCursorPolicy::Include if !cursor_capabilities.supports_inclusion() => {
                return Err(ScreenPublicationError::CursorInclusionUnsupported);
            }
            ScreenCursorPolicy::Exclude | ScreenCursorPolicy::Include => {}
        }
        let (executor, residency) =
            resolve_executor(source.config.resources.api(), self.kind, self.executor)?;
        let geometry = resolve_geometry(source.config.logical_extent, self.extent, self.aspect)?;
        let color_pipeline = resolve_color_pipeline(
            &source.config,
            self.kind,
            geometry,
            &self.processing_profile,
            capabilities,
        )?;
        Ok(ResolvedScreenPublicationDescriptor {
            physical: ScreenPhysicalReductionDescriptor {
                source_epoch: source.epoch.clone(),
                source: source.config.clone(),
                executor,
                source_region: geometry.source_region,
                reduction_extent: geometry.output_extent,
                cursor: self.processing_profile.cursor,
                reduction_filter: self.processing_profile.reduction_filter,
                algorithm_revision: self.processing_profile.algorithm_revision,
                target_pixel_format: self.processing_profile.target_pixel_format,
                color_pipeline,
            },
            residency,
            kind: self.kind,
            aspect: self.aspect,
            processing_profile: Arc::clone(&self.processing_profile),
        })
    }
}

/// Source metadata after a control-plane selector resolves.
#[derive(Clone, Debug)]
pub struct ResolvedScreenSource {
    selector: ScreenSourceSelector,
    epoch: CaptureEpoch,
    config: ResolvedScreenSourceConfig,
}

impl ResolvedScreenSource {
    /// Construct an exact resolved source snapshot.
    #[must_use]
    pub const fn new(
        selector: ScreenSourceSelector,
        epoch: CaptureEpoch,
        config: ResolvedScreenSourceConfig,
    ) -> Self {
        Self {
            selector,
            epoch,
            config,
        }
    }

    /// Selector whose resolution produced this snapshot.
    #[must_use]
    pub const fn selector(&self) -> &ScreenSourceSelector {
        &self.selector
    }

    /// Exact source and capture generation identity.
    #[must_use]
    pub const fn epoch(&self) -> &CaptureEpoch {
        &self.epoch
    }

    /// Complete source configuration used by publication identity.
    #[must_use]
    pub const fn config(&self) -> &ResolvedScreenSourceConfig {
        &self.config
    }

    /// Processed native logical extent.
    #[must_use]
    pub const fn logical_extent(&self) -> PixelExtent {
        self.config.logical_extent
    }

    /// Native source pixel format.
    #[must_use]
    pub const fn pixel_format(&self) -> CapturePixelFormat {
        self.config.pixel_format
    }

    /// Native source color metadata.
    #[must_use]
    pub const fn colorimetry(&self) -> CaptureColorimetry {
        self.config.colorimetry
    }

    fn validate_selector(
        &self,
        requested: &ScreenSourceSelector,
    ) -> Result<(), ScreenPublicationError> {
        if requested != &self.selector {
            return Err(ScreenPublicationError::SourceSelectorMismatch);
        }
        if let ScreenSourceSelector::Exact(source_id) = requested
            && source_id != &self.epoch.source_id
        {
            return Err(ScreenPublicationError::SourceSelectorMismatch);
        }
        Ok(())
    }
}

/// Canonical non-negative rational source coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRational {
    numerator: u64,
    denominator: NonZeroU64,
}

impl ScreenRational {
    /// Construct and reduce one non-negative rational coordinate.
    ///
    /// # Errors
    ///
    /// Returns [`ScreenPublicationError::GeometryOverflow`] for a zero
    /// denominator.
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, ScreenPublicationError> {
        let denominator =
            NonZeroU64::new(denominator).ok_or(ScreenPublicationError::GeometryOverflow)?;
        let divisor = greatest_common_divisor(numerator, denominator.get());
        Ok(Self {
            numerator: numerator / divisor,
            denominator: NonZeroU64::new(denominator.get() / divisor)
                .ok_or(ScreenPublicationError::GeometryOverflow)?,
        })
    }

    /// Construct an exact integer coordinate.
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self {
            numerator: value as u64,
            denominator: NonZeroU64::MIN,
        }
    }

    /// Reduced numerator.
    #[must_use]
    pub const fn numerator(self) -> u64 {
        self.numerator
    }

    /// Positive reduced denominator.
    #[must_use]
    pub const fn denominator(self) -> NonZeroU64 {
        self.denominator
    }
}

impl Ord for ScreenRational {
    fn cmp(&self, other: &Self) -> Ordering {
        (u128::from(self.numerator) * u128::from(other.denominator.get()))
            .cmp(&(u128::from(other.numerator) * u128::from(self.denominator.get())))
    }
}

impl PartialOrd for ScreenRational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Exact source-space window before any downstream integer crop policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenSubpixelRect {
    x: ScreenRational,
    y: ScreenRational,
    width: ScreenRational,
    height: ScreenRational,
}

impl ScreenSubpixelRect {
    /// Exact horizontal source origin.
    #[must_use]
    pub const fn x(self) -> ScreenRational {
        self.x
    }

    /// Exact vertical source origin.
    #[must_use]
    pub const fn y(self) -> ScreenRational {
        self.y
    }

    /// Exact source-window width.
    #[must_use]
    pub const fn width(self) -> ScreenRational {
        self.width
    }

    /// Exact source-window height.
    #[must_use]
    pub const fn height(self) -> ScreenRational {
        self.height
    }
}

/// Independently resolved exact source window and output raster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedScreenGeometry {
    source_region: ScreenSubpixelRect,
    output_extent: PixelExtent,
}

impl ResolvedScreenGeometry {
    /// Exact source region consumed by this branch.
    ///
    /// Integer texture crops are a downstream policy choice and must not be
    /// substituted back into this byte-equivalence descriptor.
    #[must_use]
    pub const fn source_region(self) -> ScreenSubpixelRect {
        self.source_region
    }

    /// Exact resolved analysis raster.
    #[must_use]
    pub const fn output_extent(self) -> PixelExtent {
        self.output_extent
    }
}

/// Complete byte-equivalence descriptor for shared physical reduction work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenPhysicalReductionDescriptor {
    source_epoch: CaptureEpoch,
    source: ResolvedScreenSourceConfig,
    executor: ScreenPublicationExecutor,
    source_region: ScreenSubpixelRect,
    reduction_extent: PixelExtent,
    cursor: ScreenCursorPolicy,
    reduction_filter: ScreenReductionFilter,
    algorithm_revision: NonZeroU32,
    target_pixel_format: CapturePixelFormat,
    color_pipeline: ResolvedScreenColorPipeline,
}

/// Canonical sharing key for physical reduction work.
pub type ScreenPhysicalReductionKey = ScreenPhysicalReductionDescriptor;

impl ScreenPhysicalReductionDescriptor {
    /// Exact source epoch fenced by this reduction.
    #[must_use]
    pub const fn source_epoch(&self) -> &CaptureEpoch {
        &self.source_epoch
    }

    /// Complete resolved source configuration.
    #[must_use]
    pub const fn source(&self) -> &ResolvedScreenSourceConfig {
        &self.source
    }

    /// Concrete executor owning this physical reduction.
    #[must_use]
    pub const fn executor(&self) -> &ScreenPublicationExecutor {
        &self.executor
    }

    /// Exact selected source-space region.
    #[must_use]
    pub const fn source_region(&self) -> ScreenSubpixelRect {
        self.source_region
    }

    /// Exact physical reduction raster.
    #[must_use]
    pub const fn reduction_extent(&self) -> PixelExtent {
        self.reduction_extent
    }

    /// Cursor composition policy applied before reduction.
    #[must_use]
    pub const fn cursor(&self) -> ScreenCursorPolicy {
        self.cursor
    }

    /// Physical reduction filter.
    #[must_use]
    pub const fn reduction_filter(&self) -> ScreenReductionFilter {
        self.reduction_filter
    }

    /// Physical reduction algorithm revision.
    #[must_use]
    pub const fn algorithm_revision(&self) -> NonZeroU32 {
        self.algorithm_revision
    }

    /// Target storage pixel format.
    #[must_use]
    pub const fn target_pixel_format(&self) -> CapturePixelFormat {
        self.target_pixel_format
    }

    /// Complete resolved color conversion and output metadata.
    #[must_use]
    pub const fn color_pipeline(&self) -> ResolvedScreenColorPipeline {
        self.color_pipeline
    }
}

impl Ord for ScreenPhysicalReductionDescriptor {
    fn cmp(&self, other: &Self) -> Ordering {
        capture_epoch_cmp(&self.source_epoch, &other.source_epoch)
            .then_with(|| self.source.cmp(&other.source))
            .then_with(|| self.executor.cmp(&other.executor))
            .then_with(|| self.source_region.cmp(&other.source_region))
            .then_with(|| {
                extent_key(self.reduction_extent).cmp(&extent_key(other.reduction_extent))
            })
            .then_with(|| self.cursor.cmp(&other.cursor))
            .then_with(|| self.reduction_filter.cmp(&other.reduction_filter))
            .then_with(|| self.algorithm_revision.cmp(&other.algorithm_revision))
            .then_with(|| {
                pixel_format_rank(self.target_pixel_format)
                    .cmp(&pixel_format_rank(other.target_pixel_format))
            })
            .then_with(|| self.color_pipeline.cmp(&other.color_pipeline))
    }
}

impl PartialOrd for ScreenPhysicalReductionDescriptor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Full byte-equivalence key for one independently resolved publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedScreenPublicationDescriptor {
    physical: ScreenPhysicalReductionDescriptor,
    residency: ScreenPublicationResidency,
    kind: ScreenPublicationKind,
    aspect: ScreenAspectPolicy,
    processing_profile: Arc<ScreenProcessingProfile>,
}

impl ResolvedScreenPublicationDescriptor {
    /// Exact resolved source epoch.
    #[must_use]
    pub const fn source_epoch(&self) -> &CaptureEpoch {
        &self.physical.source_epoch
    }

    /// Structurally complete source configuration participating in identity.
    #[must_use]
    pub const fn source(&self) -> &ResolvedScreenSourceConfig {
        &self.physical.source
    }

    /// Source logical extent participating in byte identity.
    #[must_use]
    pub const fn source_extent(&self) -> PixelExtent {
        self.physical.source.logical_extent
    }

    /// Native source pixel format.
    #[must_use]
    pub const fn source_pixel_format(&self) -> CapturePixelFormat {
        self.physical.source.pixel_format
    }

    /// Native source color metadata.
    #[must_use]
    pub const fn source_colorimetry(&self) -> CaptureColorimetry {
        self.physical.source.colorimetry
    }

    /// Independently resolved crop and output raster.
    #[must_use]
    pub const fn geometry(&self) -> ResolvedScreenGeometry {
        ResolvedScreenGeometry {
            source_region: self.physical.source_region,
            output_extent: self.physical.reduction_extent,
        }
    }

    /// Complete physical-reduction sharing descriptor.
    #[must_use]
    pub const fn physical(&self) -> &ScreenPhysicalReductionDescriptor {
        &self.physical
    }

    /// Concrete executor selected for this resolved branch.
    #[must_use]
    pub const fn executor(&self) -> &ScreenPublicationExecutor {
        self.physical.executor()
    }

    /// Publication kind.
    #[must_use]
    pub const fn kind(&self) -> ScreenPublicationKind {
        self.kind
    }

    /// Concrete storage residency produced by the selected executor.
    #[must_use]
    pub fn required_residency(&self) -> ScreenPublicationResidency {
        self.residency.clone()
    }

    /// Geometric policy retained in the complete output contract.
    #[must_use]
    pub const fn aspect(&self) -> ScreenAspectPolicy {
        self.aspect
    }

    /// Complete processing profile.
    #[must_use]
    pub fn processing_profile(&self) -> &Arc<ScreenProcessingProfile> {
        &self.processing_profile
    }
}

impl Ord for ResolvedScreenPublicationDescriptor {
    fn cmp(&self, other: &Self) -> Ordering {
        self.physical
            .cmp(&other.physical)
            .then_with(|| self.residency.cmp(&other.residency))
            .then_with(|| self.kind.cmp(&other.kind))
            .then_with(|| self.aspect.cmp(&other.aspect))
            .then_with(|| self.processing_profile.cmp(&other.processing_profile))
    }
}

impl PartialOrd for ResolvedScreenPublicationDescriptor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One unresolved consumer request and its independent scheduling cadence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredScreenBranchDemand {
    request: ScreenPublicationRequest,
    requested_hz: NonZeroU32,
}

impl RegisteredScreenBranchDemand {
    /// Register a logical request at a non-zero cadence.
    #[must_use]
    pub const fn new(request: ScreenPublicationRequest, requested_hz: NonZeroU32) -> Self {
        Self {
            request,
            requested_hz,
        }
    }

    /// Logical request retained by the registration.
    #[must_use]
    pub const fn request(&self) -> &ScreenPublicationRequest {
        &self.request
    }

    /// Requested scheduling cadence.
    #[must_use]
    pub const fn requested_hz(&self) -> NonZeroU32 {
        self.requested_hz
    }

    /// Independently resolve this demand against one exact source snapshot.
    ///
    /// # Errors
    ///
    /// Propagates publication resolution failures.
    pub fn resolve(
        &self,
        source: &ResolvedScreenSource,
    ) -> Result<ResolvedScreenBranchDemand, ScreenPublicationError> {
        self.resolve_with_color_capabilities(source, ScreenColorTransformCapabilities::NONE)
    }

    /// Resolve with the exact color operations implemented by the selected reducer.
    ///
    /// # Errors
    ///
    /// Propagates publication resolution and capability-admission failures.
    pub fn resolve_with_color_capabilities(
        &self,
        source: &ResolvedScreenSource,
        capabilities: ScreenColorTransformCapabilities,
    ) -> Result<ResolvedScreenBranchDemand, ScreenPublicationError> {
        Ok(ResolvedScreenBranchDemand {
            descriptor: self
                .request
                .resolve_with_color_capabilities(source, capabilities)?,
            requested_hz: self.requested_hz,
        })
    }
}

/// One independently resolved publication and its scheduling cadence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedScreenBranchDemand {
    descriptor: ResolvedScreenPublicationDescriptor,
    requested_hz: NonZeroU32,
}

impl ResolvedScreenBranchDemand {
    /// Full byte-equivalence descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.descriptor
    }

    /// Requested scheduling cadence.
    #[must_use]
    pub const fn requested_hz(&self) -> NonZeroU32 {
        self.requested_hz
    }

    pub(crate) fn into_parts(self) -> (ResolvedScreenPublicationDescriptor, NonZeroU32) {
        (self.descriptor, self.requested_hz)
    }
}

/// Failure to validate or resolve a publication request.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ScreenPublicationError {
    /// The resolved source was produced for another selector or exact identity.
    #[error("resolved screen source does not match the publication selector")]
    SourceSelectorMismatch,
    /// Zone analysis always produces host-addressable colors.
    #[error("screen zone publications require the CPU executor")]
    SourceNativeZonesUnsupported,
    /// Cursor exclusion was requested from composed-only capture storage.
    #[error("screen source cannot exclude cursor pixels from composed storage")]
    CursorExclusionUnsupported,
    /// Cursor inclusion was requested without composed or separately-owned pixels.
    #[error("screen source cannot provide visible cursor pixels")]
    CursorInclusionUnsupported,
    /// Source metadata cannot support color-managed processing.
    #[error("screen source colorimetry is unknown")]
    UnknownSourceColorimetry,
    /// An explicit assumption contradicted backend-provided metadata.
    #[error("screen color assumption conflicts with known source metadata")]
    ColorAssumptionConflict,
    /// Unknown samples were requested through a byte-changing path.
    #[error("unknown colorimetry can only use a byte-preserving surface publication")]
    UnknownPreservationRequiresByteIdentity,
    /// The default policy rejects HDR work before backend preparation.
    #[error("screen HDR processing is not enabled for this publication")]
    HdrRejected,
    /// The selected HDR policy cannot perform this source/target conversion.
    #[error("screen HDR source and target require an unsupported conversion")]
    UnsupportedHdrConversion,
    /// Current publication storage cannot preserve the requested color depth.
    #[error("screen target pixel format cannot preserve the requested color contract")]
    UnsupportedTargetPixelFormat,
    /// The reducer has not advertised an executable color transform.
    #[error("screen reducer does not support the requested color transform")]
    UnsupportedColorTransform,
    /// HDR input did not include the absolute context required for tone mapping.
    #[error("screen HDR source is missing luminance context")]
    MissingSourceLuminance,
    /// Target metadata and tone-map policy named different display luminance.
    #[error("screen tone-map target luminance conflicts with target colorimetry")]
    ToneMapTargetLuminanceConflict,
    /// A processing scalar was NaN or infinite.
    #[error("screen processing profile scalars must be finite")]
    NonFiniteProfileScalar,
    /// Aspect-derived geometry exceeded the representable pixel extent.
    #[error("resolved screen publication geometry exceeds u32 dimensions")]
    GeometryOverflow,
}

fn resolve_executor(
    source_api: &ScreenResourceApi,
    kind: ScreenPublicationKind,
    requested: ScreenPublicationExecutorRequest,
) -> Result<(ScreenPublicationExecutor, ScreenPublicationResidency), ScreenPublicationError> {
    match requested {
        ScreenPublicationExecutorRequest::Cpu => Ok((
            ScreenPublicationExecutor::Cpu,
            ScreenPublicationResidency::Cpu,
        )),
        ScreenPublicationExecutorRequest::SourceNative => {
            if matches!(kind, ScreenPublicationKind::Zones { .. }) {
                return Err(ScreenPublicationError::SourceNativeZonesUnsupported);
            }
            Ok(match source_api {
                ScreenResourceApi::Cpu => (
                    ScreenPublicationExecutor::Cpu,
                    ScreenPublicationResidency::Cpu,
                ),
                ScreenResourceApi::PlatformGpu(api) => (
                    ScreenPublicationExecutor::SourceNative(api.clone()),
                    ScreenPublicationResidency::PlatformGpu(api.clone()),
                ),
            })
        }
    }
}

fn resolve_color_pipeline(
    source_config: &ResolvedScreenSourceConfig,
    kind: ScreenPublicationKind,
    geometry: ResolvedScreenGeometry,
    profile: &ScreenProcessingProfile,
    capabilities: ScreenColorTransformCapabilities,
) -> Result<ResolvedScreenColorPipeline, ScreenPublicationError> {
    let byte_preserving =
        byte_preserving_surface_processing(source_config, kind, geometry, profile);
    let source = source_config.colorimetry();
    let known_source = source.try_known();
    let effective_source = match known_source {
        Ok(known) => known,
        Err(error) => match profile.unknown_color {
            ScreenUnknownColorPolicy::Reject => match error {
                CaptureColorimetryError::MissingHdrLuminance => {
                    return Err(ScreenPublicationError::MissingSourceLuminance);
                }
                CaptureColorimetryError::NonFinitePositiveScalar
                | CaptureColorimetryError::NonPositiveScalar
                | CaptureColorimetryError::PeakBelowReferenceWhite
                | CaptureColorimetryError::UnknownColorSpace
                | CaptureColorimetryError::UnknownTransferFunction
                | CaptureColorimetryError::UnknownDynamicRange
                | CaptureColorimetryError::TransferDynamicRangeMismatch => {
                    return Err(ScreenPublicationError::UnknownSourceColorimetry);
                }
            },
            ScreenUnknownColorPolicy::PreserveEncodedSamples => {
                if source.dynamic_range() == Some(CaptureDynamicRange::High)
                    || matches!(
                        source.transfer_function(),
                        CaptureTransferFunction::Pq | CaptureTransferFunction::Hlg
                    )
                {
                    return Err(ScreenPublicationError::UnsupportedTargetPixelFormat);
                }
                if profile.target_colorimetry != ScreenTargetColorimetry::PreserveSource
                    || !byte_preserving
                {
                    return Err(ScreenPublicationError::UnknownPreservationRequiresByteIdentity);
                }
                return Ok(ResolvedScreenColorPipeline {
                    effective_source: None,
                    output: source,
                    transform: ResolvedScreenColorTransform::PreserveEncodedSamples,
                });
            }
            ScreenUnknownColorPolicy::Assume(assumption) => {
                merge_color_assumption(source, assumption)?
            }
        },
    };

    resolve_known_color_pipeline(effective_source, byte_preserving, profile, capabilities)
}

fn merge_color_assumption(
    source: CaptureColorimetry,
    assumption: KnownCaptureColorimetry,
) -> Result<KnownCaptureColorimetry, ScreenPublicationError> {
    if source.color_space() != CaptureColorSpace::Unknown
        && source.color_space() != assumption.color_space()
        || source.transfer_function() != CaptureTransferFunction::Unknown
            && source.transfer_function() != assumption.transfer_function()
        || source
            .dynamic_range()
            .is_some_and(|range| range != assumption.dynamic_range())
        || matches!(
            (source.luminance(), assumption.luminance()),
            (Some(source), Some(assumption)) if source != assumption
        )
    {
        return Err(ScreenPublicationError::ColorAssumptionConflict);
    }
    KnownCaptureColorimetry::try_new(
        if source.color_space() == CaptureColorSpace::Unknown {
            assumption.color_space()
        } else {
            source.color_space()
        },
        if source.transfer_function() == CaptureTransferFunction::Unknown {
            assumption.transfer_function()
        } else {
            source.transfer_function()
        },
        source.dynamic_range().unwrap_or(assumption.dynamic_range()),
        source.luminance().or(assumption.luminance()),
    )
    .map_err(|_| ScreenPublicationError::ColorAssumptionConflict)
}

fn resolve_known_color_pipeline(
    source: KnownCaptureColorimetry,
    byte_preserving: bool,
    profile: &ScreenProcessingProfile,
    capabilities: ScreenColorTransformCapabilities,
) -> Result<ResolvedScreenColorPipeline, ScreenPublicationError> {
    let target = match profile.target_colorimetry {
        ScreenTargetColorimetry::PreserveSource => source,
        ScreenTargetColorimetry::ConvertTo(target) => target,
    };
    let source_hdr = source.dynamic_range() == CaptureDynamicRange::High;
    let target_hdr = target.dynamic_range() == CaptureDynamicRange::High;

    if source_hdr || target_hdr {
        return resolve_hdr_color_pipeline(source, target, profile, capabilities);
    }

    let preserve = profile.target_colorimetry == ScreenTargetColorimetry::PreserveSource
        && byte_preserving
        && source.dynamic_range() == CaptureDynamicRange::Standard;
    if preserve {
        return Ok(ResolvedScreenColorPipeline {
            effective_source: Some(source),
            output: CaptureColorimetry::from_known(target),
            transform: ResolvedScreenColorTransform::PreserveEncodedSamples,
        });
    }
    if capabilities.algorithm_revision() != Some(profile.algorithm_revision) {
        return Err(ScreenPublicationError::UnsupportedColorTransform);
    }
    let same_encoding = source.color_space() == target.color_space()
        && source.transfer_function() == target.transfer_function()
        && source.dynamic_range() == target.dynamic_range();
    let supported = if same_encoding {
        capabilities.supports_linear_light_sdr_processing()
    } else {
        capabilities.supports_linear_relative_color_conversion()
    };
    if !supported || !same_encoding && !capabilities.supports_gamut_policy(profile.gamut) {
        return Err(ScreenPublicationError::UnsupportedColorTransform);
    }
    Ok(ResolvedScreenColorPipeline {
        effective_source: Some(source),
        output: CaptureColorimetry::from_known(target),
        transform: if same_encoding {
            ResolvedScreenColorTransform::LinearLightSdr
        } else {
            ResolvedScreenColorTransform::LinearRelativeColorimetric {
                gamut: profile.gamut,
            }
        },
    })
}

fn resolve_hdr_color_pipeline(
    source: KnownCaptureColorimetry,
    target: KnownCaptureColorimetry,
    profile: &ScreenProcessingProfile,
    capabilities: ScreenColorTransformCapabilities,
) -> Result<ResolvedScreenColorPipeline, ScreenPublicationError> {
    match profile.hdr {
        ScreenHdrPolicy::Reject => Err(ScreenPublicationError::HdrRejected),
        ScreenHdrPolicy::ToneMap(policy)
            if source.dynamic_range() == CaptureDynamicRange::High
                && source.transfer_function() == CaptureTransferFunction::Pq
                && target.dynamic_range() == CaptureDynamicRange::Standard =>
        {
            if capabilities.algorithm_revision() != Some(profile.algorithm_revision)
                || !capabilities.supports_pq_bt2390_tone_mapping()
                || !capabilities.supports_gamut_policy(profile.gamut)
            {
                return Err(ScreenPublicationError::UnsupportedColorTransform);
            }
            if target
                .luminance()
                .is_some_and(|luminance| luminance != policy.target_luminance)
            {
                return Err(ScreenPublicationError::ToneMapTargetLuminanceConflict);
            }
            let source_luminance = source
                .luminance()
                .ok_or(ScreenPublicationError::MissingSourceLuminance)?;
            let output = target.with_luminance(policy.target_luminance);
            Ok(ResolvedScreenColorPipeline {
                effective_source: Some(source),
                output: CaptureColorimetry::from_known(output),
                transform: ResolvedScreenColorTransform::ToneMap(ResolvedScreenToneMap {
                    operator: policy.operator,
                    source_luminance,
                    target_luminance: policy.target_luminance,
                    gamut: profile.gamut,
                }),
            })
        }
        ScreenHdrPolicy::ToneMap(_) => Err(ScreenPublicationError::UnsupportedHdrConversion),
    }
}

fn byte_preserving_surface_processing(
    source: &ResolvedScreenSourceConfig,
    kind: ScreenPublicationKind,
    geometry: ResolvedScreenGeometry,
    profile: &ScreenProcessingProfile,
) -> bool {
    let source_geometry = source.geometry();
    let source_extent = source.logical_extent();
    matches!(kind, ScreenPublicationKind::Surface)
        && source_geometry.rotation() == CaptureRotation::Identity
        && source_geometry.crop().is_none()
        && source_geometry.native_extent() == source_geometry.storage_extent()
        && source_geometry.storage_extent() == source_extent
        && source.reflection() == ScreenSourceReflection::None
        && geometry.source_region == full_source_region(source_extent)
        && geometry.output_extent == source_extent
        && profile.content_bars == ScreenContentBarsPolicy::Disabled
        && profile.smoothing == ScreenSmoothingPolicy::Disabled
        && profile.tuning == ScreenColorTuning::default()
        && profile.cursor == ScreenCursorPolicy::Exclude
        && profile.reduction_filter == ScreenReductionFilter::Nearest
        && profile.target_pixel_format == source.pixel_format()
}

fn resolve_geometry(
    source: PixelExtent,
    request: ScreenExtentRequest,
    aspect: ScreenAspectPolicy,
) -> Result<ResolvedScreenGeometry, ScreenPublicationError> {
    let full_source = full_source_region(source);
    let ScreenExtentRequest::Bounded(bounds) = request else {
        return Ok(ResolvedScreenGeometry {
            source_region: full_source,
            output_extent: source,
        });
    };
    let max_width = bounds.max_width;
    let max_height = bounds.max_height;
    let upscale = bounds.upscale;

    let (Some(max_width), Some(max_height)) = (max_width, max_height) else {
        let output_extent = contain_source_aspect(source, max_width, max_height, upscale)?;
        return Ok(ResolvedScreenGeometry {
            source_region: full_source,
            output_extent,
        });
    };

    if aspect == ScreenAspectPolicy::Contain {
        return Ok(ResolvedScreenGeometry {
            source_region: full_source,
            output_extent: contain_source_aspect(
                source,
                Some(max_width),
                Some(max_height),
                upscale,
            )?,
        });
    }

    let requested = extent(max_width.get(), max_height.get())?;
    let output_extent = match upscale {
        ScreenUpscalePolicy::Allow => requested,
        ScreenUpscalePolicy::Never => fit_extent_inside(requested, source)?,
    };
    Ok(ResolvedScreenGeometry {
        source_region: centered_cover_region(source, output_extent)?,
        output_extent,
    })
}

fn contain_source_aspect(
    source: PixelExtent,
    max_width: Option<NonZeroU32>,
    max_height: Option<NonZeroU32>,
    upscale: ScreenUpscalePolicy,
) -> Result<PixelExtent, ScreenPublicationError> {
    match (max_width, max_height) {
        (None, None) => Ok(source),
        (Some(width), None) => {
            let width = match upscale {
                ScreenUpscalePolicy::Never => width.get().min(source.width()),
                ScreenUpscalePolicy::Allow => width.get(),
            };
            let height = scaled_dimension(source.height(), width, source.width())?;
            extent(width, height)
        }
        (None, Some(height)) => {
            let height = match upscale {
                ScreenUpscalePolicy::Never => height.get().min(source.height()),
                ScreenUpscalePolicy::Allow => height.get(),
            };
            let width = scaled_dimension(source.width(), height, source.height())?;
            extent(width, height)
        }
        (Some(width), Some(height)) => {
            let (width, height) = match upscale {
                ScreenUpscalePolicy::Never => (
                    width.get().min(source.width()),
                    height.get().min(source.height()),
                ),
                ScreenUpscalePolicy::Allow => (width.get(), height.get()),
            };
            fit_aspect_inside(source, width, height)
        }
    }
}

fn fit_extent_inside(
    requested: PixelExtent,
    bounds: PixelExtent,
) -> Result<PixelExtent, ScreenPublicationError> {
    if requested.width() <= bounds.width() && requested.height() <= bounds.height() {
        return Ok(requested);
    }
    fit_aspect_inside(requested, bounds.width(), bounds.height())
}

fn fit_aspect_inside(
    aspect: PixelExtent,
    width_bound: u32,
    height_bound: u32,
) -> Result<PixelExtent, ScreenPublicationError> {
    let width_limited = u64::from(width_bound) * u64::from(aspect.height())
        <= u64::from(height_bound) * u64::from(aspect.width());
    if width_limited {
        let height = scaled_dimension(aspect.height(), width_bound, aspect.width())?;
        extent(width_bound, height)
    } else {
        let width = scaled_dimension(aspect.width(), height_bound, aspect.height())?;
        extent(width, height_bound)
    }
}

fn centered_cover_region(
    source: PixelExtent,
    output: PixelExtent,
) -> Result<ScreenSubpixelRect, ScreenPublicationError> {
    let source_is_wider = u64::from(source.width()) * u64::from(output.height())
        > u64::from(source.height()) * u64::from(output.width());
    if source_is_wider {
        let denominator = u64::from(output.height());
        let width_numerator = u64::from(source.height()) * u64::from(output.width());
        let x_numerator = u64::from(source.width()) * denominator - width_numerator;
        Ok(ScreenSubpixelRect {
            x: ScreenRational::new(x_numerator, denominator * 2)?,
            y: ScreenRational::from_u32(0),
            width: ScreenRational::new(width_numerator, denominator)?,
            height: ScreenRational::from_u32(source.height()),
        })
    } else {
        let denominator = u64::from(output.width());
        let height_numerator = u64::from(source.width()) * u64::from(output.height());
        let y_numerator = u64::from(source.height()) * denominator - height_numerator;
        Ok(ScreenSubpixelRect {
            x: ScreenRational::from_u32(0),
            y: ScreenRational::new(y_numerator, denominator * 2)?,
            width: ScreenRational::from_u32(source.width()),
            height: ScreenRational::new(height_numerator, denominator)?,
        })
    }
}

fn scaled_dimension(
    source_axis: u32,
    target_axis: u32,
    source_basis: u32,
) -> Result<u32, ScreenPublicationError> {
    let value = (u64::from(source_axis) * u64::from(target_axis) / u64::from(source_basis)).max(1);
    u32::try_from(value).map_err(|_| ScreenPublicationError::GeometryOverflow)
}

fn extent(width: u32, height: u32) -> Result<PixelExtent, ScreenPublicationError> {
    PixelExtent::new(width, height).map_err(|_| ScreenPublicationError::GeometryOverflow)
}

const fn full_source_region(source: PixelExtent) -> ScreenSubpixelRect {
    ScreenSubpixelRect {
        x: ScreenRational::from_u32(0),
        y: ScreenRational::from_u32(0),
        width: ScreenRational::from_u32(source.width()),
        height: ScreenRational::from_u32(source.height()),
    }
}

const fn extent_key(extent: PixelExtent) -> (u32, u32) {
    (extent.width(), extent.height())
}

const fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn capture_geometry_cmp(left: CaptureGeometry, right: CaptureGeometry) -> Ordering {
    let left_origin = left.origin();
    let right_origin = right.origin();
    (left_origin.x, left_origin.y)
        .cmp(&(right_origin.x, right_origin.y))
        .then_with(|| extent_key(left.native_extent()).cmp(&extent_key(right.native_extent())))
        .then_with(|| extent_key(left.storage_extent()).cmp(&extent_key(right.storage_extent())))
        .then_with(|| {
            capture_rotation_rank(left.rotation()).cmp(&capture_rotation_rank(right.rotation()))
        })
        .then_with(|| pixel_rect_key(left.crop()).cmp(&pixel_rect_key(right.crop())))
        .then_with(|| {
            let left_scale = left.source_scale();
            let right_scale = right.source_scale();
            (left_scale.numerator(), left_scale.denominator())
                .cmp(&(right_scale.numerator(), right_scale.denominator()))
        })
}

fn capture_epoch_cmp(left: &CaptureEpoch, right: &CaptureEpoch) -> Ordering {
    left.source_id
        .as_str()
        .cmp(right.source_id.as_str())
        .then_with(|| left.topology_generation.cmp(&right.topology_generation))
        .then_with(|| left.session_generation.cmp(&right.session_generation))
}

const fn pixel_rect_key(rect: Option<PixelRect>) -> Option<(u32, u32, u32, u32)> {
    match rect {
        Some(rect) => Some((
            rect.x(),
            rect.y(),
            rect.extent().width(),
            rect.extent().height(),
        )),
        None => None,
    }
}

const fn capture_rotation_rank(rotation: CaptureRotation) -> u8 {
    match rotation {
        CaptureRotation::Identity => 0,
        CaptureRotation::Clockwise90 => 1,
        CaptureRotation::Clockwise180 => 2,
        CaptureRotation::Clockwise270 => 3,
    }
}

fn platform_gpu_api_cmp(left: &PlatformGpuApi, right: &PlatformGpuApi) -> Ordering {
    match (left, right) {
        (PlatformGpuApi::Other(left), PlatformGpuApi::Other(right)) => left.cmp(right),
        _ => platform_gpu_api_rank(left).cmp(&platform_gpu_api_rank(right)),
    }
}

const fn platform_gpu_api_rank(api: &PlatformGpuApi) -> u8 {
    match api {
        PlatformGpuApi::Direct3d11 => 0,
        PlatformGpuApi::DmaBuf => 1,
        PlatformGpuApi::Vulkan => 2,
        PlatformGpuApi::Metal => 3,
        PlatformGpuApi::Other(_) => 4,
    }
}

const fn pixel_format_rank(format: CapturePixelFormat) -> u8 {
    match format {
        CapturePixelFormat::Rgba8 => 0,
        CapturePixelFormat::Bgra8 => 1,
    }
}
