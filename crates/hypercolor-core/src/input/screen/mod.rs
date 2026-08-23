//! Screen capture input source — ambient lighting driven by screen content.
//!
//! Implements [`InputSource`] for screen capture, producing [`ScreenData`]
//! with per-zone colors extracted from a sector grid overlay. The actual
//! screen capture backend (xcap, `PipeWire`, etc.) is external — this module
//! provides the pure analysis pipeline: sector grid computation, letterbox
//! detection, temporal smoothing, and zone mapping.
//!
//! # Architecture
//!
//! ```text
//! Raw RGBA pixels ──> SectorGrid ──> LetterboxDetect ──> TemporalSmoother ──> ZoneColors
//! ```
//!
//! The capture backend feeds raw pixel buffers. Everything downstream is
//! backend-agnostic and testable with synthetic data.

mod admission;
mod cadence;
mod compute;
mod coordinator;
mod demand;
mod fanout;
mod frame;
mod hub;
mod ledger;
mod macos;
mod materialize;
mod plan;
mod process;
mod publication;
mod reducer;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod retained;
mod sampling;
pub mod sector;
pub mod smooth;
mod tone_map;
pub mod tune;
#[cfg(target_os = "linux")]
pub mod wayland;
#[cfg(target_os = "windows")]
pub mod windows;

pub use admission::{
    ScreenByteAdmissionCoordinator, ScreenByteAdmissionError, ScreenByteAdmissionSnapshot,
    ScreenByteLease, ScreenByteReservation, ScreenCapacityPolicySnapshot,
    ScreenCapacityStatusHandle, ScreenCapacityStatusSnapshot,
};
pub use cadence::{
    CaptureCadence, CaptureCadenceError, CapturePacer, MAX_REPRESENTABLE_CAPTURE_FPS,
};
pub use compute::{
    CpuExactReductionAdmissionError, CpuExactReductionComputeCapacity, CpuExactReductionWorkPlan,
    CpuExactReductionWorkTerms, ScreenAnalysisAdmissionError, ScreenAnalysisComputeCapacity,
    ScreenAnalysisComputeLane, ScreenAnalysisFrameWork, ScreenAnalysisWorkPlan,
    ScreenComputeCapacityPolicy,
};
pub(crate) use coordinator::PendingScreenWorkerPreparation;
pub use coordinator::{
    CommittedScreenPublicationTransition, PreparedScreenPublicationPlan,
    ScreenPublicationPreparation, ScreenPublicationTransitionError,
    ScreenPublicationTransitionFailure, ScreenWorkerPreparation, ScreenWorkerRetirement,
};
pub use demand::{ScreenPublicationDemandError, ScreenPublicationDemandSnapshot};
pub use fanout::{
    CpuPublicationFanoutError, CpuPublicationFanoutReport, PreparedCpuLogicalFanout,
    PreparedCpuLogicalFanoutKind, PreparedCpuPhysicalFanout, PreparedCpuPublicationFanout,
    PreparedCpuPublicationFanoutCandidate,
};
pub use frame::{
    CaptureColorSpace, CaptureColorimetry, CaptureColorimetryError, CaptureCursor,
    CaptureCursorContent, CaptureCursorShape, CaptureCursorShapeFormat, CaptureDamage,
    CaptureDynamicRange, CaptureEpoch, CaptureFrame, CaptureFrameError, CaptureFrameMetadata,
    CaptureGeometry, CaptureLuminanceContext, CapturePixelFormat, CapturePlaneLease,
    CapturePlanePool, CapturePositiveScalar, CaptureRotation, CaptureSourceId, CaptureStageKind,
    CaptureStorage, CaptureTransferFunction, CpuCaptureStorage, GeometryNormalizedCaptureSurface,
    KnownCaptureColorimetry, MoveRegion, PhysicalOrigin, PixelExtent, PixelRect, PlatformGpuApi,
    PlatformGpuSurface, PlatformGpuSurfaceOwner, PlatformGpuSurfaceTimingSink, PooledCapturePlane,
    RawCaptureSurface, SourceScale,
};
pub use hub::{
    PreparedScreenPublication, ScreenBranchDeliveryLifecycle, ScreenBranchDeliveryState,
    ScreenBranchLease, ScreenBranchObserver, ScreenBranchObserverSnapshot, ScreenBranchPayload,
    ScreenBranchPublication, ScreenBranchPublisher, ScreenCommittedState,
    ScreenContinuityActivationFailure, ScreenContinuityError, ScreenContinuityLease,
    ScreenContinuityStageFailure, ScreenGpuSurfacePayload, ScreenLiveBranchReceipt,
    ScreenNativeWorkPayload, ScreenPayloadKind, ScreenPublicationColorimetry,
    ScreenPublicationFreshness, ScreenPublicationHealth, ScreenPublicationHub,
    ScreenPublicationHubError, ScreenPublicationMetadata, ScreenPublicationRetirement,
    ScreenPublicationSlotPolicy, ScreenSurfacePayload, ScreenTwoPlanContinuityLease,
    ScreenZonesPayload,
};
pub use ledger::{
    ScreenWorkerExactLedger, ScreenWorkerExactLedgerBuilder, ScreenWorkerLedgerBuildError,
};
#[cfg(feature = "macos-capture-fixtures")]
pub use macos::MacosScreenCaptureFixture;
pub use macos::{MacosNativeTargetManifest, MacosScreenCaptureInput};
pub use materialize::{
    CpuSurfaceMaterializationError, CpuZoneMaterializationError, PreparedCpuSurfaceMaterializer,
    PreparedCpuZoneMaterializer, StagedCpuZonePublication,
};
pub use plan::{
    ArmedScreenPlan, AwaitingBackendScreenPlan, CommittedScreenPlan,
    InputPublicationDemandRevision, PreparingScreenPlan, ScreenAdmissionCapacity,
    ScreenBranchDemand, ScreenCapturePlan, ScreenCompatibilitySelection,
    ScreenConsumerBranchRevision, ScreenConsumerBranchRoute, ScreenExactResource,
    ScreenExactResourceLedger, ScreenInputGraphGeneration, ScreenPhysicalReductionDemand,
    ScreenPlanAbort, ScreenPlanAdmissionLedger, ScreenPlanArmFailure, ScreenPlanBuilder,
    ScreenPlanCommitFailure, ScreenPlanError, ScreenPlanGeneration, ScreenPlanTransactionId,
    ScreenPreparedWorkerToken, ScreenRequiredResourceMinimum, ScreenResourceKind,
    ScreenResourceLedger, ScreenResourceLifetime, ScreenSourcePlanDelta, ScreenWorkerBinding,
    ScreenWorkerBindingState, ScreenWorkerPreparationTicket,
};
pub use process::CaptureFrameProcessor;
pub use publication::{
    AdmittedScreenNativeTargetPreparation, BoundScreenNativeTargetPreparation,
    RegisteredScreenBranchDemand, ResolvedScreenBranchDemand, ResolvedScreenColorPipeline,
    ResolvedScreenColorTransform, ResolvedScreenGeometry, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ResolvedScreenToneMap, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenBoundedExtent, ScreenCaptureBackend,
    ScreenColorTransformCapabilities, ScreenColorTuning, ScreenConsumerBranchId,
    ScreenContentBarsPolicy, ScreenCursorCapabilities, ScreenCursorPolicy,
    ScreenExecutorColorCapabilities, ScreenExtentRequest, ScreenGamutMapPolicy, ScreenGridPolicy,
    ScreenHdrPolicy, ScreenLetterboxFill, ScreenNativeExecutionTarget,
    ScreenNativeExecutionTargetId, ScreenNativePreparationPayload, ScreenNativeRetentionQuote,
    ScreenNativeTargetAllocation, ScreenNativeTargetBindingError, ScreenNativeTargetPreparation,
    ScreenNativeTargetPreparationError, ScreenNativeTargetPreparer,
    ScreenNativeTargetResourceError, ScreenPhysicalGpuDeviceIdentity,
    ScreenPhysicalReductionDescriptor, ScreenProcessingProfile, ScreenProcessingProfileConfig,
    ScreenProfileScalar, ScreenPublicationError, ScreenPublicationExecutor,
    ScreenPublicationExecutorFallbackReason, ScreenPublicationExecutorRequest,
    ScreenPublicationKind, ScreenPublicationRequest, ScreenPublicationResidency, ScreenRational,
    ScreenReductionFilter, ScreenResourceApi, ScreenSceneCutPolicy, ScreenSmoothingPolicy,
    ScreenSourceReflection, ScreenSourceSelector, ScreenSubpixelRect, ScreenTargetColorimetry,
    ScreenToneMapOperator, ScreenToneMapPolicy, ScreenUnknownColorPolicy, ScreenUpscalePolicy,
};
pub use reducer::{
    CpuFallbackNeed, CpuReductionBatchJob, CpuReductionBatchReport, CpuReductionError,
    CpuReductionExecutor, CpuReductionLayout, CpuReductionRequest, CpuSurfaceReductionJob,
    PreparedCpuMaterializationWorkspace, PreparedCpuReductionBatch,
};
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) use retained::{ExactBoxList, ExactBoxNode};
pub use sampling::{
    CpuMappedSamplingPoint, CpuSamplingError, CpuSamplingPoint, CpuSamplingView,
    CpuScalarSamplingView, CpuScalarSource, CpuStorageCoordinate,
};
pub use sector::{LetterboxBars, SectorGrid, proportional_sector_bounds};
pub use smooth::TemporalSmoother;
pub use tone_map::{
    LED_TONE_MAP_ALGORITHM_REVISION, LED_TONE_MAP_MAX_EXPOSURE_EV, LED_TONE_MAP_MIN_EXPOSURE_EV,
    LED_TONE_MAP_TRANSITION_DURATION, LedToneMapCalibration, LedToneMapCalibrationError,
    LedToneMapConstants, LedToneMapCurveTransition, LedToneMapTransitionSample, PreparedLedToneMap,
    PreparedLedToneMapError,
};
pub use tune::ColorTuning;
#[cfg(target_os = "linux")]
pub use wayland::WaylandScreenCaptureInput;
#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
pub use windows::WindowsScreenCaptureFixture;
#[cfg(target_os = "windows")]
pub use windows::{CaptureSourceSink, ResolvedCaptureSource, WindowsScreenCaptureInput};

use crate::input::traits::{InputData, InputSource, ScreenData, ScreenZoneColors};
use crate::input::{SourceKind, SourceStatusHandle, SourceStatusReporter};
use hypercolor_types::canvas::{
    DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, PublishedSurface, RenderSurfacePool,
    SurfaceDescriptor, SurfaceResourceError, SurfaceResourceOwner,
};
use hypercolor_types::event::ZoneColors;
use std::fmt::Write as _;
use std::mem::size_of;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Acquisition cadence requested from a native screen backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenCaptureCadence {
    /// Use the source's configured acquisition cadence.
    #[default]
    Configured,
    /// Allow the native source to publish at the display's refresh cadence.
    NativeRefresh,
    /// Request an explicit nonzero acquisition rate.
    FramesPerSecond(NonZeroU32),
}

impl ScreenCaptureCadence {
    /// Construct an explicit nonzero acquisition rate.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureCadenceError::Zero`] when `frames_per_second` is zero.
    pub const fn frames_per_second(frames_per_second: u32) -> Result<Self, CaptureCadenceError> {
        let cadence = match CaptureCadence::new(frames_per_second) {
            Ok(cadence) => cadence,
            Err(error) => return Err(error),
        };
        Ok(Self::FramesPerSecond(
            NonZeroU32::new(cadence.frames_per_second())
                .expect("validated capture cadence is nonzero"),
        ))
    }

    /// Resolve a demand override against the configured acquisition cadence.
    #[must_use]
    pub const fn resolve(self, configured: Self) -> Self {
        match self {
            Self::Configured => configured,
            explicit => explicit,
        }
    }

    const fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::NativeRefresh, _) | (_, Self::NativeRefresh) => Self::NativeRefresh,
            (Self::FramesPerSecond(left), Self::FramesPerSecond(right)) => {
                if left.get() >= right.get() {
                    Self::FramesPerSecond(left)
                } else {
                    Self::FramesPerSecond(right)
                }
            }
            (Self::Configured, cadence) | (cadence, Self::Configured) => cadence,
        }
    }
}

/// Requested screen publication state for downstream render consumers.
///
/// The requested extent describes the analyzed surface published by the input
/// source. Native capture geometry remains authoritative in frame metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenCaptureDemand {
    /// No consumer currently needs screen publications.
    #[default]
    Inactive,
    /// Publish screen data fitted within this non-empty extent.
    Active {
        /// Maximum width and height requested by the current consumer union.
        requested_extent: PixelExtent,
        /// Native acquisition cadence requested by the current consumer union.
        cadence: ScreenCaptureCadence,
        /// Cursor composition requested from the native source.
        cursor: ScreenCursorPolicy,
    },
}

impl ScreenCaptureDemand {
    /// Construct active demand from a validated extent.
    #[must_use]
    pub const fn active(requested_extent: PixelExtent) -> Self {
        Self::active_with_policy(
            requested_extent,
            ScreenCaptureCadence::Configured,
            ScreenCursorPolicy::Exclude,
        )
    }

    /// Construct active demand with an explicit acquisition and cursor policy.
    #[must_use]
    pub const fn active_with_policy(
        requested_extent: PixelExtent,
        cadence: ScreenCaptureCadence,
        cursor: ScreenCursorPolicy,
    ) -> Self {
        Self::Active {
            requested_extent,
            cadence,
            cursor,
        }
    }

    /// Construct active demand from checked pixel dimensions.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::EmptyExtent`] when either dimension is zero.
    pub fn try_active(width: u32, height: u32) -> Result<Self, CaptureFrameError> {
        match PixelExtent::new(width, height) {
            Ok(requested_extent) => Ok(Self::active(requested_extent)),
            Err(error) => Err(error),
        }
    }

    /// Whether at least one consumer requests screen publications.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Active { .. })
    }

    /// Requested publication extent, or `None` while inactive.
    #[must_use]
    pub const fn requested_extent(self) -> Option<PixelExtent> {
        match self {
            Self::Inactive => None,
            Self::Active {
                requested_extent, ..
            } => Some(requested_extent),
        }
    }

    /// Requested native acquisition cadence, or `None` while inactive.
    #[must_use]
    pub const fn cadence(self) -> Option<ScreenCaptureCadence> {
        match self {
            Self::Inactive => None,
            Self::Active { cadence, .. } => Some(cadence),
        }
    }

    /// Requested cursor composition policy, or `None` while inactive.
    #[must_use]
    pub const fn cursor(self) -> Option<ScreenCursorPolicy> {
        match self {
            Self::Inactive => None,
            Self::Active { cursor, .. } => Some(cursor),
        }
    }

    /// Union independent consumer demands without imposing a resolution cap.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Inactive, demand) | (demand, Self::Inactive) => demand,
            (
                Self::Active {
                    requested_extent: left,
                    cadence: left_cadence,
                    cursor: left_cursor,
                },
                Self::Active {
                    requested_extent: right,
                    cadence: right_cadence,
                    cursor: right_cursor,
                },
            ) => Self::active_with_policy(
                left.union(right),
                left_cadence.union(right_cadence),
                if matches!(left_cursor, ScreenCursorPolicy::Include)
                    || matches!(right_cursor, ScreenCursorPolicy::Include)
                {
                    ScreenCursorPolicy::Include
                } else {
                    ScreenCursorPolicy::Exclude
                },
            ),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AnalyzedScreenSnapshot {
    geometry_frame: CaptureFrame<GeometryNormalizedCaptureSurface>,
    data: ScreenData,
}

impl AnalyzedScreenSnapshot {
    pub(crate) const fn geometry_frame(&self) -> &CaptureFrame<GeometryNormalizedCaptureSurface> {
        &self.geometry_frame
    }

    pub(crate) const fn data(&self) -> &ScreenData {
        &self.data
    }
}

pub(crate) fn analyze_screen_frame(
    analyzer: &mut ScreenCaptureInput,
    frame: CaptureFrame<RawCaptureSurface>,
) -> anyhow::Result<AnalyzedScreenSnapshot> {
    validate_legacy_screen_colorimetry(frame.metadata().colorimetry)?;
    let captured_at = frame.metadata().captured_at;
    let frame = analyzer.capture_processor.process(frame)?;
    let geometry = &frame.metadata().geometry;
    let CaptureStorage::Cpu(storage) = frame.storage() else {
        anyhow::bail!("legacy screen analysis requires CPU storage");
    };
    let extent = geometry.storage_extent();
    let pixels = storage
        .tightly_packed_rgba8(extent)
        .ok_or_else(|| anyhow::anyhow!("legacy screen analysis requires tightly packed RGBA8"))?;
    if !analyzer.push_frame_at(pixels, extent.width(), extent.height(), captured_at)? {
        anyhow::bail!("screen analysis rejected malformed normalized pixels");
    }
    let InputData::Screen(data) = analyzer.sample()? else {
        anyhow::bail!("legacy screen analysis did not produce a snapshot");
    };
    Ok(AnalyzedScreenSnapshot {
        geometry_frame: frame,
        data,
    })
}

pub(crate) fn validate_legacy_screen_colorimetry(
    colorimetry: CaptureColorimetry,
) -> Result<(), CaptureFrameError> {
    if colorimetry.color_space() == CaptureColorSpace::Srgb
        && colorimetry.transfer_function() == CaptureTransferFunction::Srgb
        && colorimetry.dynamic_range() == Some(CaptureDynamicRange::Standard)
    {
        return Ok(());
    }
    Err(CaptureFrameError::UnsupportedLegacyAnalysisColorimetry { colorimetry })
}

// ── CaptureConfig ─────────────────────────────────────────────────────────

/// Runtime configuration for the screen capture input source.
///
/// The capture source itself is chosen through the desktop portal picker;
/// `restore_token` carries the persisted choice back into the portal.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureConfig {
    /// Target capture frames per second. Default: 30.
    pub target_fps: u32,

    /// Cadence requested from the native acquisition backend.
    pub acquisition_cadence: ScreenCaptureCadence,

    /// Sector grid columns (horizontal divisions). Default: 8.
    pub grid_cols: u32,

    /// Sector grid rows (vertical divisions). Default: 6.
    pub grid_rows: u32,

    /// Maximum retained plus peak transient bytes admitted for grid analysis.
    pub analysis_memory_bytes: u64,

    /// Temporal smoothing factor (0.0 = frozen, 1.0 = raw). Default: 0.3.
    pub smoothing_alpha: f32,

    /// Scene-cut detection threshold for the temporal smoother. Default: 100.0.
    pub scene_cut_threshold: f32,

    /// Luminance threshold for letterbox detection (0.0 - 1.0). Default: 0.02.
    pub letterbox_threshold: f32,

    /// Whether letterbox detection is enabled. Default: false, matching
    /// `config::CaptureConfig` — ambient lighting mirrors desktops far more
    /// often than letterboxed film, and dark desktop content trips the
    /// detector into cropping real picture away.
    pub letterbox_enabled: bool,

    /// Color tuning applied to zone colors after smoothing.
    pub tuning: ColorTuning,

    /// Target LED white-point x coordinate in CIE xy chromaticity space.
    pub target_led_white_x: f32,

    /// Target LED white-point y coordinate in CIE xy chromaticity space.
    pub target_led_white_y: f32,

    /// Target LED reference white in nits for HDR tone mapping.
    pub target_led_reference_white_nits: f32,

    /// Calibrated target LED peak in nits for HDR tone mapping.
    pub target_led_peak_nits: f32,

    /// User exposure adjustment in exposure-value stops.
    pub exposure_ev: f32,

    /// XDG portal restore token from a previous session, if any.
    pub restore_token: Option<String>,

    /// Persisted capture source. Windows accepts `auto`, stable monitor ids,
    /// and legacy numeric indices; the XDG portal owns selection on Linux.
    pub source: String,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            target_fps: 30,
            acquisition_cadence: ScreenCaptureCadence::FramesPerSecond(
                NonZeroU32::new(30).expect("default capture cadence is nonzero"),
            ),
            grid_cols: 8,
            grid_rows: 6,
            analysis_memory_bytes: u64::MAX,
            smoothing_alpha: 0.3,
            scene_cut_threshold: 100.0,
            letterbox_threshold: 0.02,
            letterbox_enabled: false,
            tuning: ColorTuning::default(),
            target_led_white_x: 0.3127,
            target_led_white_y: 0.3290,
            target_led_reference_white_nits: 203.0,
            target_led_peak_nits: 406.0,
            exposure_ev: 0.0,
            restore_token: None,
            source: "auto".to_owned(),
        }
    }
}

/// Checked retained, transient, and work ledger for one analysis grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenAnalysisResourcePlan {
    grid_cells: usize,
    extent_pixels: usize,
    retained_bytes: u64,
    extent_retained_bytes: u64,
    transient_bytes: u64,
    max_zone_id_bytes: usize,
    zone_snapshot_slot_bytes: u64,
}

const ZONE_SNAPSHOT_SLOT_COUNT: usize = 3;

struct PreparedZoneSnapshot {
    zones: Arc<Vec<ZoneColors>>,
    resource_owner: Arc<dyn SurfaceResourceOwner>,
    cols: u32,
    rows: u32,
}

impl ScreenAnalysisResourcePlan {
    /// Build a resource plan and admit it against a byte fence.
    ///
    /// # Errors
    ///
    /// Returns a checked geometry or capacity error without dimensional caps.
    pub fn try_new(
        grid_cols: u32,
        grid_rows: u32,
        target_fps: u32,
        capacity_bytes: u64,
    ) -> Result<Self, SurfaceResourceError> {
        let extent = PixelExtent::new(1, 1).expect("unit analysis extent is non-empty");
        Self::try_new_for_extent(grid_cols, grid_rows, target_fps, extent, capacity_bytes)
    }

    /// Build a complete grid and publication-extent resource plan.
    pub fn try_new_for_extent(
        grid_cols: u32,
        grid_rows: u32,
        _target_fps: u32,
        requested_extent: PixelExtent,
        capacity_bytes: u64,
    ) -> Result<Self, SurfaceResourceError> {
        if grid_cols == 0 || grid_rows == 0 {
            return Err(SurfaceResourceError::EmptyDimensions {
                width: grid_cols,
                height: grid_rows,
            });
        }
        let grid_cells_u64 = u64::from(grid_cols)
            .checked_mul(u64::from(grid_rows))
            .ok_or(SurfaceResourceError::ByteLengthOverflow {
                width: grid_cols,
                height: grid_rows,
            })?;
        let grid_cells = usize::try_from(grid_cells_u64).map_err(|_| {
            SurfaceResourceError::ByteLengthOverflow {
                width: grid_cols,
                height: grid_rows,
            }
        })?;
        let max_zone_id_bytes = "screen:sector_".len()
            + decimal_digits(grid_rows.saturating_sub(1))
            + 1
            + decimal_digits(grid_cols.saturating_sub(1));
        let sector_grid_bytes = size_of::<[u8; 3]>().checked_mul(2).ok_or(
            SurfaceResourceError::ByteLengthOverflow {
                width: grid_cols,
                height: grid_rows,
            },
        )?;
        let snapshot_cell_bytes = size_of::<ZoneColors>()
            .checked_add(max_zone_id_bytes)
            .and_then(|bytes| bytes.checked_add(size_of::<[u8; 3]>()))
            .ok_or(SurfaceResourceError::ByteLengthOverflow {
                width: grid_cols,
                height: grid_rows,
            })?;
        let zone_snapshot_slot_bytes = grid_cells_u64
            .checked_mul(u64::try_from(snapshot_cell_bytes).map_err(|_| {
                SurfaceResourceError::ByteLengthOverflow {
                    width: grid_cols,
                    height: grid_rows,
                }
            })?)
            .ok_or(SurfaceResourceError::ByteLengthOverflow {
                width: grid_cols,
                height: grid_rows,
            })?;
        let retained_per_cell = u64::try_from(
            snapshot_cell_bytes
                .checked_mul(ZONE_SNAPSHOT_SLOT_COUNT)
                .and_then(|bytes| bytes.checked_add(sector_grid_bytes))
                .ok_or(SurfaceResourceError::ByteLengthOverflow {
                    width: grid_cols,
                    height: grid_rows,
                })?,
        )
        .map_err(|_| SurfaceResourceError::ByteLengthOverflow {
            width: grid_cols,
            height: grid_rows,
        })?;
        let grid_retained_bytes = grid_cells_u64.checked_mul(retained_per_cell).ok_or(
            SurfaceResourceError::ByteLengthOverflow {
                width: grid_cols,
                height: grid_rows,
            },
        )?;
        let extent_pixels_u64 = u64::from(requested_extent.width())
            .checked_mul(u64::from(requested_extent.height()))
            .ok_or(SurfaceResourceError::ByteLengthOverflow {
                width: requested_extent.width(),
                height: requested_extent.height(),
            })?;
        let extent_pixels = usize::try_from(extent_pixels_u64).map_err(|_| {
            SurfaceResourceError::ByteLengthOverflow {
                width: requested_extent.width(),
                height: requested_extent.height(),
            }
        })?;
        let extent_retained_bytes =
            extent_pixels_u64
                .checked_mul(39)
                .ok_or(SurfaceResourceError::ByteLengthOverflow {
                    width: requested_extent.width(),
                    height: requested_extent.height(),
                })?;
        let retained_bytes = grid_retained_bytes
            .checked_add(extent_retained_bytes)
            .ok_or(SurfaceResourceError::ByteLengthOverflow {
                width: requested_extent.width(),
                height: requested_extent.height(),
            })?;
        let transient_bytes = 0;
        let requested_bytes = retained_bytes.checked_add(transient_bytes).ok_or(
            SurfaceResourceError::ByteLengthOverflow {
                width: grid_cols,
                height: grid_rows,
            },
        )?;
        if requested_bytes > capacity_bytes {
            return Err(SurfaceResourceError::AnalysisCapacityExceeded {
                width: grid_cols,
                height: grid_rows,
                requested_bytes,
                capacity_bytes,
            });
        }
        Ok(Self {
            grid_cells,
            extent_pixels,
            retained_bytes,
            extent_retained_bytes,
            transient_bytes,
            max_zone_id_bytes,
            zone_snapshot_slot_bytes,
        })
    }

    #[must_use]
    pub const fn grid_cells(self) -> usize {
        self.grid_cells
    }

    #[must_use]
    pub const fn extent_pixels(self) -> usize {
        self.extent_pixels
    }

    #[must_use]
    pub const fn extent_retained_bytes(self) -> u64 {
        self.extent_retained_bytes
    }

    #[must_use]
    pub const fn surface_backing_bytes(self) -> u64 {
        (self.extent_retained_bytes / 39) * 12
    }

    #[must_use]
    pub const fn non_surface_retained_bytes(self) -> u64 {
        self.retained_bytes - self.surface_backing_bytes()
    }

    #[must_use]
    const fn zone_snapshot_slot_bytes(self) -> u64 {
        self.zone_snapshot_slot_bytes
    }

    #[must_use]
    pub const fn retained_bytes(self) -> u64 {
        self.retained_bytes
    }

    #[must_use]
    pub const fn transient_bytes(self) -> u64 {
        self.transient_bytes
    }

    #[must_use]
    pub const fn peak_bytes(self) -> u64 {
        self.retained_bytes + self.transient_bytes
    }
}

fn decimal_digits(value: u32) -> usize {
    if value == 0 {
        1
    } else {
        usize::try_from(value.ilog10()).expect("u32 digit count fits usize") + 1
    }
}

/// Largest size within `max_width` x `max_height` that keeps `width` x
/// `height`'s aspect ratio.
///
/// The screen downscale used to target the canvas bounds directly, which
/// squashed a 16:9 desktop into 4:3 — a 1.33x vertical stretch that no
/// downstream fit mode can undo, because the distortion is already baked
/// into the published surface. Circles came out as ellipses on every
/// screen-mirroring effect.
#[must_use]
pub fn fit_within(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 || max_width == 0 || max_height == 0 {
        return (max_width.max(1), max_height.max(1));
    }

    // Integer comparison of width/height against max_width/max_height,
    // cross-multiplied to avoid floating point entirely.
    let source_is_wider =
        u64::from(width) * u64::from(max_height) >= u64::from(height) * u64::from(max_width);

    if source_is_wider {
        let scaled_height = (u64::from(height) * u64::from(max_width) / u64::from(width))
            .try_into()
            .unwrap_or(max_height);
        (max_width, u32::max(scaled_height, 1))
    } else {
        let scaled_width = (u64::from(width) * u64::from(max_height) / u64::from(height))
            .try_into()
            .unwrap_or(max_width);
        (u32::max(scaled_width, 1), max_height)
    }
}

/// One display output the capture backend can address directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenMonitor {
    /// Legacy zero-based enumeration index for display ordering.
    pub index: usize,
    /// Stable capture source id suitable for persistence.
    pub id: String,
    /// OS device name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Desktop width in pixels.
    pub width: u32,
    /// Desktop height in pixels.
    pub height: u32,
    /// Whether this output anchors the virtual desktop origin.
    pub primary: bool,
}

/// Display outputs the capture backend can address by index.
///
/// Empty where the backend owns source selection instead (the XDG portal
/// on Linux) or no backend exists. Emptiness is the capability signal: a
/// UI with monitors shows a picker, a UI without falls back to whatever
/// selection flow the platform provides.
#[must_use]
pub fn available_monitors() -> Vec<ScreenMonitor> {
    #[cfg(target_os = "windows")]
    {
        hypercolor_windows_capture::list_monitors()
            .into_iter()
            .map(|monitor| ScreenMonitor {
                index: monitor.index,
                id: monitor.id,
                name: monitor.name,
                width: monitor.width,
                height: monitor.height,
                primary: monitor.primary,
            })
            .collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// Parse a configured capture source into a Windows monitor selector.
///
/// `capture.source` is a free-form string shared across backends. The XDG
/// portal picks its own source and leaves the value at "auto", so this only
/// matters on Windows, which addresses display outputs directly. Stable ids
/// survive adapter/output enumeration reorder; numeric values remain accepted
/// for configuration compatibility.
#[must_use]
pub fn monitor_selector_from_source(source: &str) -> hypercolor_windows_capture::MonitorSelector {
    hypercolor_windows_capture::MonitorSelector::parse(source)
}

// ── ScreenCaptureInput ────────────────────────────────────────────────────

/// Screen capture input source implementing [`InputSource`].
///
/// Owns the sector grid configuration, temporal smoother, and latest frame
/// state. The actual pixel data is pushed in via [`push_frame`] — the
/// capture backend lives outside this struct (behind a feature flag).
///
/// # Usage
///
/// ```rust,ignore
/// let mut input = ScreenCaptureInput::new(CaptureConfig::default());
/// input.start()?;
///
/// // Backend captures a frame and pushes raw RGBA pixels:
/// input.push_frame(&rgba_pixels, width, height)?;
///
/// // Render loop samples the latest data:
/// let data = input.sample()?;
/// ```
pub struct ScreenCaptureInput {
    /// Runtime configuration.
    config: CaptureConfig,

    /// Temporal smoother for flicker reduction.
    smoother: TemporalSmoother,

    capture_processor: CaptureFrameProcessor,

    analysis_grid: SectorGrid,

    policy_grid: SectorGrid,

    zone_snapshot_pool: Vec<PreparedZoneSnapshot>,

    latest_zone_colors: Option<ScreenZoneColors>,

    analysis_resource_plan: ScreenAnalysisResourcePlan,

    admission_coordinator: ScreenByteAdmissionCoordinator,

    analysis_lease: ScreenByteLease,

    surface_resource_owner: Arc<dyn SurfaceResourceOwner>,

    /// Effective sector dimensions after letterbox cropping.
    latest_grid_width: u32,
    latest_grid_height: u32,

    /// Latest downscaled capture frame for screen-reactive effects.
    latest_canvas_downscale: Option<PublishedSurface>,

    downscale_pool: RenderSurfacePool,

    requested_extent: PixelExtent,

    policy_pixels: Vec<[u8; 3]>,

    analysis_compute_capacity: Option<ScreenAnalysisComputeCapacity>,

    analysis_work_plan: Option<ScreenAnalysisWorkPlan>,

    /// Whether the source is actively capturing.
    running: bool,

    /// Frame dimensions from the most recent push.
    frame_width: u32,
    frame_height: u32,

    /// Detected letterbox bars from the most recent frame.
    letterbox: LetterboxBars,
    frame_generation: u64,
    status_frame_generation: u64,
    latest_acquired_at: Option<Instant>,
    status: SourceStatusReporter,
}

impl ScreenCaptureInput {
    /// Create a new screen capture input with the given configuration.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        let requested_extent = PixelExtent::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT)
            .expect("default canvas extent is non-empty");
        Self::with_requested_extent(config, requested_extent)
            .expect("default screen publication extent fits available memory")
    }

    /// Create screen analysis with an explicit publication extent.
    ///
    /// # Errors
    ///
    /// Returns an error when storage for the requested extent cannot be
    /// admitted before construction.
    pub fn with_requested_extent(
        config: CaptureConfig,
        requested_extent: PixelExtent,
    ) -> Result<Self, ScreenAnalysisAdmissionError> {
        let capacity = ScreenAdmissionCapacity::new(
            config.analysis_memory_bytes,
            config.analysis_memory_bytes,
        );
        let admission_coordinator = ScreenByteAdmissionCoordinator::new(capacity);
        Self::with_requested_extent_and_optional_compute_capacity(
            config,
            requested_extent,
            admission_coordinator,
            None,
        )
    }

    /// Create screen analysis against a shared source-level byte fence.
    pub fn with_requested_extent_and_admission(
        config: CaptureConfig,
        requested_extent: PixelExtent,
        admission_coordinator: ScreenByteAdmissionCoordinator,
    ) -> Result<Self, ScreenAnalysisAdmissionError> {
        Self::with_requested_extent_and_optional_compute_capacity(
            config,
            requested_extent,
            admission_coordinator,
            None,
        )
    }

    /// Create screen analysis with explicit publication and compute capacity.
    ///
    /// This constructor lets embedders supply capacity calibrated for their
    /// worker environment while preserving the complete requested workload.
    ///
    /// # Errors
    ///
    /// Returns an error when storage for the requested extent cannot be
    /// admitted before construction.
    pub fn with_requested_extent_and_compute_capacity(
        config: CaptureConfig,
        requested_extent: PixelExtent,
        analysis_compute_capacity: ScreenAnalysisComputeCapacity,
    ) -> Result<Self, ScreenAnalysisAdmissionError> {
        let capacity = ScreenAdmissionCapacity::new(
            config.analysis_memory_bytes,
            config.analysis_memory_bytes,
        );
        let admission_coordinator = ScreenByteAdmissionCoordinator::new(capacity);
        Self::with_requested_extent_and_optional_compute_capacity(
            config,
            requested_extent,
            admission_coordinator,
            Some(analysis_compute_capacity),
        )
    }

    /// Create screen analysis against shared memory and calibrated compute fences.
    pub fn with_requested_extent_admission_and_compute_capacity(
        config: CaptureConfig,
        requested_extent: PixelExtent,
        admission_coordinator: ScreenByteAdmissionCoordinator,
        analysis_compute_capacity: ScreenAnalysisComputeCapacity,
    ) -> Result<Self, ScreenAnalysisAdmissionError> {
        Self::with_requested_extent_and_optional_compute_capacity(
            config,
            requested_extent,
            admission_coordinator,
            Some(analysis_compute_capacity),
        )
    }

    fn with_requested_extent_and_optional_compute_capacity(
        config: CaptureConfig,
        requested_extent: PixelExtent,
        admission_coordinator: ScreenByteAdmissionCoordinator,
        analysis_compute_capacity: Option<ScreenAnalysisComputeCapacity>,
    ) -> Result<Self, ScreenAnalysisAdmissionError> {
        let prepared =
            prepare_analysis_resources(&config, requested_extent, &admission_coordinator)?;
        let PreparedAnalysisResources {
            analysis_resource_plan,
            analysis_grid,
            policy_grid,
            zone_snapshot_pool,
            smoother,
            downscale_pool,
            policy_pixels,
            analysis_lease,
            surface_resource_owner,
        } = prepared;

        Ok(Self {
            config,
            smoother,
            capture_processor: CaptureFrameProcessor::default(),
            analysis_grid,
            policy_grid,
            zone_snapshot_pool,
            latest_zone_colors: None,
            analysis_resource_plan,
            admission_coordinator,
            analysis_lease,
            surface_resource_owner,
            latest_grid_width: 0,
            latest_grid_height: 0,
            latest_canvas_downscale: None,
            downscale_pool,
            requested_extent,
            policy_pixels,
            analysis_compute_capacity,
            analysis_work_plan: None,
            running: false,
            frame_width: 0,
            frame_height: 0,
            letterbox: LetterboxBars::default(),
            frame_generation: 0,
            status_frame_generation: 0,
            latest_acquired_at: None,
            status: SourceStatusReporter::new(
                "screen_analysis",
                SourceKind::Screen,
                "in_process",
                true,
                true,
                true,
            ),
        })
    }

    /// Push a raw RGBA8 frame into the pipeline.
    ///
    /// Computes the sector grid, detects letterbox bars, applies temporal
    /// smoothing, and stores the result for the next `sample()` call.
    ///
    /// # Arguments
    ///
    /// * `frame` — Raw RGBA8 pixel data, row-major, 4 bytes per pixel.
    /// * `width` — Frame width in pixels.
    /// * `height` — Frame height in pixels.
    pub fn push_frame(
        &mut self,
        frame: &[u8],
        width: u32,
        height: u32,
    ) -> Result<bool, ScreenAnalysisAdmissionError> {
        self.push_frame_at(frame, width, height, Instant::now())
    }

    fn push_frame_at(
        &mut self,
        frame: &[u8],
        width: u32,
        height: u32,
        acquired_at: Instant,
    ) -> Result<bool, ScreenAnalysisAdmissionError> {
        let Ok(input_extent) = PixelExtent::new(width, height) else {
            return Ok(false);
        };
        let Some(expected_frame_len) = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .and_then(|stride| usize::try_from(height).ok()?.checked_mul(stride))
        else {
            return Ok(false);
        };
        if frame.len() < expected_frame_len {
            return Ok(false);
        }
        if self
            .analysis_work_plan
            .is_none_or(|plan| plan.input_extent() != input_extent)
        {
            self.admit_frame_extent(input_extent)?;
        }
        if !self.analysis_grid.try_update(
            frame,
            width,
            height,
            self.config.grid_cols,
            self.config.grid_rows,
        ) {
            return Ok(false);
        }

        let letterbox = if self.config.letterbox_enabled {
            self.analysis_grid
                .detect_letterbox(self.config.letterbox_threshold)
        } else {
            LetterboxBars::default()
        };

        let effective_cols = self
            .analysis_grid
            .cols()
            .saturating_sub(letterbox.left.saturating_add(letterbox.right));
        let effective_rows = self
            .analysis_grid
            .rows()
            .saturating_sub(letterbox.top.saturating_add(letterbox.bottom));
        let region = if letterbox.has_bars() && effective_cols > 0 && effective_rows > 0 {
            FrameRegion::from_letterbox(
                width,
                height,
                self.analysis_grid.cols(),
                self.analysis_grid.rows(),
                letterbox,
            )
            .unwrap_or_else(|| FrameRegion::full(width, height))
        } else {
            FrameRegion::full(width, height)
        };
        let (effective_cols, effective_rows) = if region.width == width && region.height == height {
            (self.analysis_grid.cols(), self.analysis_grid.rows())
        } else {
            (effective_cols, effective_rows)
        };
        let (downscale_width, downscale_height) = fit_within(
            region.width,
            region.height,
            self.requested_extent.width(),
            self.requested_extent.height(),
        );
        let elapsed = self.latest_acquired_at.map_or(Duration::ZERO, |previous| {
            acquired_at.saturating_duration_since(previous)
        });
        let reset_smoother = (self.latest_grid_width != 0 || self.latest_grid_height != 0)
            && (self.latest_grid_width != effective_cols
                || self.latest_grid_height != effective_rows);
        let Some(canvas_downscale) = downscale_frame(
            frame,
            width,
            height,
            region,
            downscale_width,
            downscale_height,
            &mut self.downscale_pool,
            &mut self.smoother,
            self.config.tuning,
            &mut self.policy_pixels,
            elapsed,
            reset_smoother,
            false,
            &self.surface_resource_owner,
        )?
        else {
            return Ok(false);
        };
        if !self.policy_grid.try_update(
            canvas_downscale.rgba_bytes(),
            canvas_downscale.width(),
            canvas_downscale.height(),
            effective_cols,
            effective_rows,
        ) {
            return Ok(false);
        }
        let Some(zone_colors) = prepare_zone_snapshot(
            &mut self.zone_snapshot_pool,
            self.policy_grid.colors(),
            effective_cols,
            effective_rows,
        ) else {
            return Ok(false);
        };

        self.smoother.commit_staged();
        self.latest_canvas_downscale = Some(canvas_downscale);
        self.latest_grid_width = effective_cols;
        self.latest_grid_height = effective_rows;
        self.latest_zone_colors = Some(zone_colors);
        self.letterbox = letterbox;
        self.frame_width = width;
        self.frame_height = height;
        self.frame_generation = self.frame_generation.wrapping_add(1);
        self.latest_acquired_at = Some(acquired_at);
        Ok(true)
    }

    /// Current configuration.
    #[must_use]
    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }

    pub(crate) fn set_led_tone_map_calibration(&mut self, calibration: LedToneMapCalibration) {
        self.config.target_led_white_x = calibration.target_white_x();
        self.config.target_led_white_y = calibration.target_white_y();
        self.config.target_led_reference_white_nits = calibration.target_reference_white_nits();
        self.config.target_led_peak_nits = calibration.target_peak_nits();
        self.config.exposure_ev = calibration.exposure_ev();
    }

    /// Update the requested publication extent for the next analyzed frame.
    pub fn set_requested_extent(
        &mut self,
        requested_extent: PixelExtent,
    ) -> Result<(), ScreenAnalysisAdmissionError> {
        if self.requested_extent == requested_extent {
            return Ok(());
        }
        let prepared = prepare_analysis_resources(
            &self.config,
            requested_extent,
            &self.admission_coordinator,
        )?;
        let analysis_work_plan = self
            .analysis_work_plan
            .map(|plan| {
                admit_analysis_work(
                    ScreenAnalysisWorkPlan::try_new(
                        plan.input_extent(),
                        requested_extent,
                        &self.config,
                    )?,
                    self.analysis_compute_capacity,
                )
            })
            .transpose()?;
        self.install_prepared_analysis(prepared, requested_extent);
        self.analysis_work_plan = analysis_work_plan;
        Ok(())
    }

    /// Current requested publication extent.
    #[must_use]
    pub const fn requested_extent(&self) -> PixelExtent {
        self.requested_extent
    }

    /// Admit a frame-storage extent before compatibility analysis begins.
    ///
    /// # Errors
    ///
    /// Returns a typed rejection when the complete requested resolution,
    /// output, grid, and cadence exceed caller-calibrated compute throughput.
    pub fn admit_frame_extent(
        &mut self,
        input_extent: PixelExtent,
    ) -> Result<ScreenAnalysisWorkPlan, ScreenAnalysisAdmissionError> {
        let plan = admit_analysis_work(
            ScreenAnalysisWorkPlan::try_new(input_extent, self.requested_extent, &self.config)?,
            self.analysis_compute_capacity,
        )?;
        self.analysis_work_plan = Some(plan);
        Ok(plan)
    }

    /// Currently admitted compatibility-analysis workload.
    #[must_use]
    pub const fn analysis_work_plan(&self) -> Option<ScreenAnalysisWorkPlan> {
        self.analysis_work_plan
    }

    /// Apply new analysis settings to a running pipeline.
    ///
    /// Grid, smoothing, letterbox, and tuning changes take effect on the next
    /// pushed frame. The smoother resets when the grid shape changes so stale
    /// zone state never blends into the new layout.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the active settings when the capture
    /// cadence is not representable by the scheduler clock.
    pub fn apply_settings(
        &mut self,
        config: CaptureConfig,
    ) -> Result<(), ScreenAnalysisAdmissionError> {
        CaptureCadence::new(config.target_fps)?;
        let prepared = if config.grid_cols != self.config.grid_cols
            || config.grid_rows != self.config.grid_rows
            || config.target_fps != self.config.target_fps
            || config.analysis_memory_bytes != self.config.analysis_memory_bytes
        {
            Some(prepare_analysis_resources(
                &config,
                self.requested_extent,
                &self.admission_coordinator,
            )?)
        } else {
            None
        };
        let analysis_work_plan = self
            .analysis_work_plan
            .map(|plan| {
                admit_analysis_work(
                    ScreenAnalysisWorkPlan::try_new(
                        plan.input_extent(),
                        self.requested_extent,
                        &config,
                    )?,
                    self.analysis_compute_capacity,
                )
            })
            .transpose()?;
        if let Some(prepared) = prepared {
            self.install_prepared_analysis(prepared, self.requested_extent);
        }
        self.smoother.set_alpha(config.smoothing_alpha);
        self.smoother
            .set_scene_cut_threshold(config.scene_cut_threshold);
        self.config = config;
        self.analysis_work_plan = analysis_work_plan;
        Ok(())
    }

    /// Most recently detected letterbox bars.
    #[must_use]
    pub fn letterbox_bars(&self) -> &LetterboxBars {
        &self.letterbox
    }

    /// Frame dimensions from the most recent push.
    #[must_use]
    pub fn frame_dimensions(&self) -> (u32, u32) {
        (self.frame_width, self.frame_height)
    }

    #[must_use]
    pub const fn analysis_resource_plan(&self) -> ScreenAnalysisResourcePlan {
        self.analysis_resource_plan
    }

    fn install_prepared_analysis(
        &mut self,
        prepared: PreparedAnalysisResources,
        requested_extent: PixelExtent,
    ) {
        self.analysis_grid = prepared.analysis_grid;
        self.policy_grid = prepared.policy_grid;
        self.zone_snapshot_pool = prepared.zone_snapshot_pool;
        self.smoother = prepared.smoother;
        self.downscale_pool = prepared.downscale_pool;
        self.policy_pixels = prepared.policy_pixels;
        self.analysis_resource_plan = prepared.analysis_resource_plan;
        self.analysis_lease = prepared.analysis_lease;
        self.surface_resource_owner = prepared.surface_resource_owner;
        self.requested_extent = requested_extent;
        self.latest_zone_colors = None;
        self.latest_canvas_downscale = None;
        self.latest_grid_width = 0;
        self.latest_grid_height = 0;
        self.smoother.reset();
    }
}

impl InputSource for ScreenCaptureInput {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        CaptureCadence::new(self.config.target_fps)?;
        self.status.begin_session()?;
        self.running = true;
        self.smoother.reset();
        self.latest_zone_colors = None;
        self.latest_grid_width = 0;
        self.latest_grid_height = 0;
        self.latest_canvas_downscale = None;
        self.latest_acquired_at = None;
        self.status_frame_generation = self.frame_generation;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.latest_zone_colors = None;
        self.latest_grid_width = 0;
        self.latest_grid_height = 0;
        self.latest_canvas_downscale = None;
        self.latest_acquired_at = None;
        self.smoother.reset();
        self.status.stop();
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        let Some(zone_colors) = self.latest_zone_colors.as_ref() else {
            return Ok(InputData::None);
        };

        if self.status_frame_generation != self.frame_generation {
            if let (Some(status), Some(acquired_at)) =
                (self.status.session(), self.latest_acquired_at)
            {
                let cadence = CaptureCadence::new(self.config.target_fps)?;
                status.record_sample(acquired_at, cadence.freshness_deadline(acquired_at)?, 1)?;
            }
            self.status_frame_generation = self.frame_generation;
        }

        Ok(InputData::Screen(ScreenData {
            zone_colors: zone_colors.clone(),
            grid_width: self.latest_grid_width,
            grid_height: self.latest_grid_height,
            canvas_downscale: self.latest_canvas_downscale.clone(),
            source_width: self.frame_width,
            source_height: self.frame_height,
            letterbox: [
                self.letterbox.top,
                self.letterbox.bottom,
                self.letterbox.left,
                self.letterbox.right,
            ],
        }))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }
}

fn admit_analysis_work(
    plan: ScreenAnalysisWorkPlan,
    capacity: Option<ScreenAnalysisComputeCapacity>,
) -> Result<ScreenAnalysisWorkPlan, ScreenAnalysisAdmissionError> {
    match capacity {
        Some(capacity) => plan.admit(capacity),
        None => Ok(plan),
    }
}

#[derive(Clone, Copy)]
struct FrameRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl FrameRegion {
    const fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn from_letterbox(
        width: u32,
        height: u32,
        cols: u32,
        rows: u32,
        bars: LetterboxBars,
    ) -> Option<Self> {
        if width == 0 || height == 0 || cols == 0 || rows == 0 {
            return None;
        }
        let first_col = bars.left;
        let last_col = cols.checked_sub(bars.right)?.checked_sub(1)?;
        let first_row = bars.top;
        let last_row = rows.checked_sub(bars.bottom)?.checked_sub(1)?;
        if first_col > last_col || first_row > last_row {
            return None;
        }
        let x = sector::proportional_bounds(first_col, cols, width).0;
        let right = sector::proportional_bounds(last_col, cols, width).1;
        let y = sector::proportional_bounds(first_row, rows, height).0;
        let bottom = sector::proportional_bounds(last_row, rows, height).1;
        let width = right.checked_sub(x)?;
        let height = bottom.checked_sub(y)?;
        (width > 0 && height > 0).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }
}

struct PreparedAnalysisResources {
    analysis_resource_plan: ScreenAnalysisResourcePlan,
    analysis_grid: SectorGrid,
    policy_grid: SectorGrid,
    zone_snapshot_pool: Vec<PreparedZoneSnapshot>,
    smoother: TemporalSmoother,
    downscale_pool: RenderSurfacePool,
    policy_pixels: Vec<[u8; 3]>,
    analysis_lease: ScreenByteLease,
    surface_resource_owner: Arc<dyn SurfaceResourceOwner>,
}

fn prepare_analysis_resources(
    config: &CaptureConfig,
    requested_extent: PixelExtent,
    coordinator: &ScreenByteAdmissionCoordinator,
) -> Result<PreparedAnalysisResources, SurfaceResourceError> {
    let plan = ScreenAnalysisResourcePlan::try_new_for_extent(
        config.grid_cols,
        config.grid_rows,
        config.target_fps,
        requested_extent,
        config.analysis_memory_bytes,
    )?;
    let mut reservation = coordinator
        .try_acquire(plan.peak_bytes())
        .map_err(|error| {
            let available = match error {
                ScreenByteAdmissionError::CapacityExceeded {
                    available_bytes, ..
                } => available_bytes,
                ScreenByteAdmissionError::CapacityShrinkRejected {
                    requested_capacity, ..
                } => requested_capacity,
                ScreenByteAdmissionError::RevisionExhausted => 0,
            };
            SurfaceResourceError::AnalysisCapacityExceeded {
                width: config.grid_cols,
                height: config.grid_rows,
                requested_bytes: plan.peak_bytes(),
                capacity_bytes: available,
            }
        })?;
    let analysis_grid = prepare_sector_grid(config, plan)?;
    let policy_grid = prepare_sector_grid(config, plan)?;
    let mut zone_snapshot_owners = Vec::new();
    zone_snapshot_owners
        .try_reserve_exact(ZONE_SNAPSHOT_SLOT_COUNT)
        .map_err(|_| analysis_allocation_error(config, plan))?;
    for _ in 0..ZONE_SNAPSHOT_SLOT_COUNT {
        let slot_reservation = reservation
            .split_off(plan.zone_snapshot_slot_bytes())
            .expect("zone snapshot bytes are a checked subset of the aggregate quote");
        let owner: Arc<dyn SurfaceResourceOwner> = Arc::new(slot_reservation.freeze());
        zone_snapshot_owners.push(owner);
    }
    let zone_snapshot_pool = prepare_zone_snapshot_pool(config, plan, zone_snapshot_owners)?;
    let smoother = TemporalSmoother::try_new_for_grid(
        config.smoothing_alpha,
        config.scene_cut_threshold,
        requested_extent.width(),
        requested_extent.height(),
    )?;
    let downscale_pool = prepare_downscale_pool(requested_extent)?;
    let policy_pixels = prepare_policy_pixels(requested_extent)?;
    let surface_reservation = reservation
        .split_off(plan.surface_backing_bytes())
        .expect("surface bytes are a checked subset of the aggregate quote");
    let analysis_lease = reservation.freeze();
    let surface_lease = surface_reservation.freeze();
    let surface_resource_owner: Arc<dyn SurfaceResourceOwner> = Arc::new(surface_lease);
    Ok(PreparedAnalysisResources {
        analysis_resource_plan: plan,
        analysis_grid,
        policy_grid,
        zone_snapshot_pool,
        smoother,
        downscale_pool,
        policy_pixels,
        analysis_lease,
        surface_resource_owner,
    })
}

fn prepare_downscale_pool(extent: PixelExtent) -> Result<RenderSurfacePool, SurfaceResourceError> {
    let descriptor = SurfaceDescriptor::rgba8888(extent.width(), extent.height());
    RenderSurfacePool::try_with_lazy_slot_count_and_cap(
        descriptor,
        ZONE_SNAPSHOT_SLOT_COUNT,
        ZONE_SNAPSHOT_SLOT_COUNT,
    )
}

fn analysis_allocation_error(
    config: &CaptureConfig,
    plan: ScreenAnalysisResourcePlan,
) -> SurfaceResourceError {
    SurfaceResourceError::AllocationFailed {
        width: config.grid_cols,
        height: config.grid_rows,
        byte_len: usize::try_from(plan.retained_bytes).unwrap_or(usize::MAX),
    }
}

fn prepare_sector_grid(
    config: &CaptureConfig,
    plan: ScreenAnalysisResourcePlan,
) -> Result<SectorGrid, SurfaceResourceError> {
    SectorGrid::try_with_capacity(config.grid_cols, config.grid_rows)
        .ok_or_else(|| analysis_allocation_error(config, plan))
}

fn prepare_zone_snapshot_pool(
    config: &CaptureConfig,
    plan: ScreenAnalysisResourcePlan,
    resource_owners: Vec<Arc<dyn SurfaceResourceOwner>>,
) -> Result<Vec<PreparedZoneSnapshot>, SurfaceResourceError> {
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(ZONE_SNAPSHOT_SLOT_COUNT)
        .map_err(|_| analysis_allocation_error(config, plan))?;
    for resource_owner in resource_owners {
        let mut zones = Vec::new();
        zones
            .try_reserve_exact(plan.grid_cells)
            .map_err(|_| analysis_allocation_error(config, plan))?;
        for row in 0..config.grid_rows {
            for col in 0..config.grid_cols {
                let mut zone_id = String::new();
                zone_id
                    .try_reserve_exact(plan.max_zone_id_bytes)
                    .map_err(|_| analysis_allocation_error(config, plan))?;
                write!(zone_id, "screen:sector_{row}_{col}")
                    .map_err(|_| analysis_allocation_error(config, plan))?;
                let mut colors = Vec::new();
                colors
                    .try_reserve_exact(1)
                    .map_err(|_| analysis_allocation_error(config, plan))?;
                colors.push([0, 0, 0]);
                zones.push(ZoneColors { zone_id, colors });
            }
        }
        slots.push(PreparedZoneSnapshot {
            zones: Arc::new(zones),
            resource_owner,
            cols: config.grid_cols,
            rows: config.grid_rows,
        });
    }
    Ok(slots)
}

fn prepare_zone_snapshot(
    slots: &mut [PreparedZoneSnapshot],
    colors: &[[u8; 3]],
    cols: u32,
    rows: u32,
) -> Option<ScreenZoneColors> {
    let slot = slots
        .iter_mut()
        .find(|slot| Arc::strong_count(&slot.zones) == 1)?;
    let zones =
        Arc::get_mut(&mut slot.zones).expect("uniquely owned zone snapshot slot is mutable");
    if zones.len() < colors.len() {
        return None;
    }
    if slot.cols != cols || slot.rows != rows {
        let mut index = 0_usize;
        for row in 0..rows {
            for col in 0..cols {
                let zone = zones.get_mut(index)?;
                zone.zone_id.clear();
                if write!(zone.zone_id, "screen:sector_{row}_{col}").is_err() {
                    return None;
                }
                index += 1;
            }
        }
        if index != colors.len() {
            return None;
        }
        slot.cols = cols;
        slot.rows = rows;
    }
    for (zone, color) in zones.iter_mut().zip(colors) {
        zone.colors[0] = *color;
    }
    ScreenZoneColors::from_prepared(
        Arc::clone(&slot.zones),
        colors.len(),
        Arc::clone(&slot.resource_owner),
    )
}

fn prepare_policy_pixels(extent: PixelExtent) -> Result<Vec<[u8; 3]>, SurfaceResourceError> {
    let pixel_count = usize::try_from(extent.width())
        .ok()
        .and_then(|width| {
            usize::try_from(extent.height())
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(SurfaceResourceError::ByteLengthOverflow {
            width: extent.width(),
            height: extent.height(),
        })?;
    let byte_len = pixel_count
        .checked_mul(3)
        .ok_or(SurfaceResourceError::ByteLengthOverflow {
            width: extent.width(),
            height: extent.height(),
        })?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(pixel_count)
        .map_err(|_| SurfaceResourceError::AllocationFailed {
            width: extent.width(),
            height: extent.height(),
            byte_len,
        })?;
    Ok(pixels)
}

fn downscale_frame(
    frame: &[u8],
    width: u32,
    height: u32,
    region: FrameRegion,
    target_width: u32,
    target_height: u32,
    surface_pool: &mut RenderSurfacePool,
    smoother: &mut TemporalSmoother,
    tuning: ColorTuning,
    policy_pixels: &mut Vec<[u8; 3]>,
    elapsed: Duration,
    reset_smoother: bool,
    suppress_scene_cut_bypass: bool,
    surface_resource_owner: &Arc<dyn SurfaceResourceOwner>,
) -> Result<Option<PublishedSurface>, SurfaceResourceError> {
    if width == 0 || height == 0 || target_width == 0 || target_height == 0 {
        return Ok(None);
    }

    let expected_len = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|pixels| pixels.checked_mul(4));
    let Some(expected_len) = expected_len else {
        return Ok(None);
    };
    if frame.len() < expected_len {
        return Ok(None);
    }
    let required_pixels = usize::try_from(target_width)
        .ok()
        .and_then(|width| {
            usize::try_from(target_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(SurfaceResourceError::ByteLengthOverflow {
            width: target_width,
            height: target_height,
        })?;
    if policy_pixels.capacity() < required_pixels {
        let additional = required_pixels.saturating_sub(policy_pixels.len());
        let byte_len =
            required_pixels
                .checked_mul(3)
                .ok_or(SurfaceResourceError::ByteLengthOverflow {
                    width: target_width,
                    height: target_height,
                })?;
        policy_pixels.try_reserve_exact(additional).map_err(|_| {
            SurfaceResourceError::AllocationFailed {
                width: target_width,
                height: target_height,
                byte_len,
            }
        })?;
    }

    let descriptor = SurfaceDescriptor::rgba8888(target_width, target_height);
    let Some(mut lease) = surface_pool.try_dequeue_bounded_for_descriptor(descriptor)? else {
        return Ok(None);
    };
    if !lease.canvas_mut().has_resource_owner() {
        lease
            .canvas_mut()
            .set_resource_owner(Arc::clone(surface_resource_owner));
    }
    if !copy_downscaled_rgba(
        frame,
        width,
        region,
        target_width,
        target_height,
        lease.canvas_mut().as_rgba_bytes_mut(),
    ) {
        lease.release();
        return Ok(None);
    }
    let bytes = lease.canvas_mut().as_rgba_bytes_mut();
    policy_pixels.clear();
    policy_pixels.extend(
        bytes
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2]]),
    );
    if !smoother.stage_for_elapsed_grid(
        policy_pixels,
        target_width,
        target_height,
        elapsed,
        reset_smoother,
        suppress_scene_cut_bypass,
    ) {
        lease.release();
        return Ok(None);
    }
    tuning.apply(policy_pixels);
    for (pixel, color) in bytes.chunks_exact_mut(4).zip(policy_pixels.iter()) {
        pixel[..3].copy_from_slice(color);
        pixel[3] = 255;
    }

    Ok(Some(lease.submit(0, 0)))
}

fn copy_downscaled_rgba(
    frame: &[u8],
    width: u32,
    region: FrameRegion,
    target_width: u32,
    target_height: u32,
    output: &mut [u8],
) -> bool {
    let Some(src_width) = usize::try_from(width).ok() else {
        return false;
    };
    let Some(target_width_usize) = usize::try_from(target_width).ok() else {
        return false;
    };
    for y in 0..target_height {
        let Some(region_y) =
            u32::try_from(u64::from(y) * u64::from(region.height) / u64::from(target_height)).ok()
        else {
            return false;
        };
        let Some(candidate_y) = region.y.checked_add(region_y) else {
            return false;
        };
        let src_y = candidate_y.min(region.y.saturating_add(region.height).saturating_sub(1));
        let Some(src_row) = usize::try_from(src_y).ok() else {
            return false;
        };
        for x in 0..target_width {
            let Some(region_x) =
                u32::try_from(u64::from(x) * u64::from(region.width) / u64::from(target_width))
                    .ok()
            else {
                return false;
            };
            let Some(candidate_x) = region.x.checked_add(region_x) else {
                return false;
            };
            let src_x = candidate_x.min(region.x.saturating_add(region.width).saturating_sub(1));
            let Some(src_col) = usize::try_from(src_x).ok() else {
                return false;
            };
            let Some(src_idx) = src_row
                .checked_mul(src_width)
                .and_then(|index| index.checked_add(src_col))
                .and_then(|index| index.checked_mul(4))
            else {
                return false;
            };
            let Some(dst_idx) = usize::try_from(y)
                .ok()
                .and_then(|row| row.checked_mul(target_width_usize))
                .and_then(|index| {
                    usize::try_from(x)
                        .ok()
                        .and_then(|column| index.checked_add(column))
                })
                .and_then(|index| index.checked_mul(4))
            else {
                return false;
            };
            let (Some(source), Some(target)) = (
                frame.get(src_idx..src_idx.saturating_add(4)),
                output.get_mut(dst_idx..dst_idx.saturating_add(4)),
            ) else {
                return false;
            };
            target.copy_from_slice(source);
        }
    }
    true
}
