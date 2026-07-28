//! Immutable screen-publication requests and independently resolved descriptors.

use std::cmp::Ordering;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use super::{
    CaptureColorSpace, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, CaptureTransferFunction, PixelExtent, PixelRect, PlatformGpuApi,
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
    color_space: CaptureColorSpace,
    transfer_function: CaptureTransferFunction,
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
        color_space: CaptureColorSpace,
        transfer_function: CaptureTransferFunction,
        resources: ScreenBackendResourceIdentity,
    ) -> Self {
        Self {
            geometry,
            logical_extent,
            reflection,
            pixel_format,
            color_space,
            transfer_function,
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
        color_space: CaptureColorSpace,
        transfer_function: CaptureTransferFunction,
        cursor_capabilities: ScreenCursorCapabilities,
        resources: ScreenBackendResourceIdentity,
    ) -> Self {
        Self {
            geometry,
            logical_extent,
            reflection,
            pixel_format,
            color_space,
            transfer_function,
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

    /// Native source color space.
    #[must_use]
    pub const fn color_space(&self) -> CaptureColorSpace {
        self.color_space
    }

    /// Native source transfer function.
    #[must_use]
    pub const fn transfer_function(&self) -> CaptureTransferFunction {
        self.transfer_function
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
            .then_with(|| {
                color_space_rank(self.color_space).cmp(&color_space_rank(other.color_space))
            })
            .then_with(|| {
                transfer_function_rank(self.transfer_function)
                    .cmp(&transfer_function_rank(other.transfer_function))
            })
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
    target_color_space: CaptureColorSpace,
    target_transfer_function: CaptureTransferFunction,
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
    /// Color space of the derived output.
    pub target_color_space: CaptureColorSpace,
    /// Transfer function of the derived output.
    pub target_transfer_function: CaptureTransferFunction,
    /// Revision of the complete processing algorithm.
    pub algorithm_revision: NonZeroU32,
}

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
            target_color_space: CaptureColorSpace::Srgb,
            target_transfer_function: CaptureTransferFunction::Srgb,
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
            target_color_space: config.target_color_space,
            target_transfer_function: config.target_transfer_function,
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

    /// Derived output color space.
    #[must_use]
    pub const fn target_color_space(&self) -> CaptureColorSpace {
        self.target_color_space
    }

    /// Derived output transfer function.
    #[must_use]
    pub const fn target_transfer_function(&self) -> CaptureTransferFunction {
        self.target_transfer_function
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
            .then_with(|| {
                color_space_rank(self.target_color_space)
                    .cmp(&color_space_rank(other.target_color_space))
            })
            .then_with(|| {
                transfer_function_rank(self.target_transfer_function)
                    .cmp(&transfer_function_rank(other.target_transfer_function))
            })
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
        extent: ScreenExtentRequest,
        aspect: ScreenAspectPolicy,
        processing_profile: Arc<ScreenProcessingProfile>,
    ) -> Self {
        Self {
            selector,
            kind,
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
        let geometry = resolve_geometry(source.config.logical_extent, self.extent, self.aspect)?;
        Ok(ResolvedScreenPublicationDescriptor {
            physical: ScreenPhysicalReductionDescriptor {
                source_epoch: source.epoch.clone(),
                source: source.config.clone(),
                source_region: geometry.source_region,
                reduction_extent: geometry.output_extent,
                cursor: self.processing_profile.cursor,
                reduction_filter: self.processing_profile.reduction_filter,
                algorithm_revision: self.processing_profile.algorithm_revision,
                target_pixel_format: self.processing_profile.target_pixel_format,
                target_color_space: self.processing_profile.target_color_space,
                target_transfer_function: self.processing_profile.target_transfer_function,
            },
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

    /// Native source color space.
    #[must_use]
    pub const fn color_space(&self) -> CaptureColorSpace {
        self.config.color_space
    }

    /// Native source transfer function.
    #[must_use]
    pub const fn transfer_function(&self) -> CaptureTransferFunction {
        self.config.transfer_function
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
    fn new(numerator: u64, denominator: u64) -> Result<Self, ScreenPublicationError> {
        let denominator =
            NonZeroU64::new(denominator).ok_or(ScreenPublicationError::GeometryOverflow)?;
        let divisor = greatest_common_divisor(numerator, denominator.get());
        Ok(Self {
            numerator: numerator / divisor,
            denominator: NonZeroU64::new(denominator.get() / divisor)
                .ok_or(ScreenPublicationError::GeometryOverflow)?,
        })
    }

    const fn from_u32(value: u32) -> Self {
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
    source_region: ScreenSubpixelRect,
    reduction_extent: PixelExtent,
    cursor: ScreenCursorPolicy,
    reduction_filter: ScreenReductionFilter,
    algorithm_revision: NonZeroU32,
    target_pixel_format: CapturePixelFormat,
    target_color_space: CaptureColorSpace,
    target_transfer_function: CaptureTransferFunction,
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

    /// Target storage color space.
    #[must_use]
    pub const fn target_color_space(&self) -> CaptureColorSpace {
        self.target_color_space
    }

    /// Target storage transfer function.
    #[must_use]
    pub const fn target_transfer_function(&self) -> CaptureTransferFunction {
        self.target_transfer_function
    }
}

impl Ord for ScreenPhysicalReductionDescriptor {
    fn cmp(&self, other: &Self) -> Ordering {
        capture_epoch_cmp(&self.source_epoch, &other.source_epoch)
            .then_with(|| self.source.cmp(&other.source))
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
            .then_with(|| {
                color_space_rank(self.target_color_space)
                    .cmp(&color_space_rank(other.target_color_space))
            })
            .then_with(|| {
                transfer_function_rank(self.target_transfer_function)
                    .cmp(&transfer_function_rank(other.target_transfer_function))
            })
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

    /// Native source color space.
    #[must_use]
    pub const fn source_color_space(&self) -> CaptureColorSpace {
        self.physical.source.color_space
    }

    /// Native source transfer function.
    #[must_use]
    pub const fn source_transfer_function(&self) -> CaptureTransferFunction {
        self.physical.source.transfer_function
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

    /// Publication kind.
    #[must_use]
    pub const fn kind(&self) -> ScreenPublicationKind {
        self.kind
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
        Ok(ResolvedScreenBranchDemand {
            descriptor: self.request.resolve(source)?,
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
    /// Cursor exclusion was requested from composed-only capture storage.
    #[error("screen source cannot exclude cursor pixels from composed storage")]
    CursorExclusionUnsupported,
    /// Cursor inclusion was requested without composed or separately-owned pixels.
    #[error("screen source cannot provide visible cursor pixels")]
    CursorInclusionUnsupported,
    /// A processing scalar was NaN or infinite.
    #[error("screen processing profile scalars must be finite")]
    NonFiniteProfileScalar,
    /// Aspect-derived geometry exceeded the representable pixel extent.
    #[error("resolved screen publication geometry exceeds u32 dimensions")]
    GeometryOverflow,
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

const fn color_space_rank(color_space: CaptureColorSpace) -> u8 {
    match color_space {
        CaptureColorSpace::Srgb => 0,
        CaptureColorSpace::DisplayP3 => 1,
        CaptureColorSpace::Rec2020 => 2,
        CaptureColorSpace::Unknown => 3,
    }
}

const fn transfer_function_rank(transfer_function: CaptureTransferFunction) -> u8 {
    match transfer_function {
        CaptureTransferFunction::Srgb => 0,
        CaptureTransferFunction::Linear => 1,
        CaptureTransferFunction::Pq => 2,
        CaptureTransferFunction::Hlg => 3,
        CaptureTransferFunction::Unknown => 4,
    }
}
