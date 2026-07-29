//! Atomic committed screen authority and bounded typed publications.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use arc_swap::{ArcSwap, ArcSwapOption};
use thiserror::Error;

use super::plan::{
    InputPublicationDemandRevision, ScreenCapturePlan, ScreenPlanError, ScreenPlanGeneration,
    ScreenResourceLifetime, ScreenRetainedResourceLedger, ScreenWorkerActivationLatch,
    ScreenWorkerBinding,
};
use super::{
    CaptureColorSpace, CaptureColorimetry, CaptureEpoch, CapturePixelFormat,
    CaptureTransferFunction, PixelExtent, PlatformGpuApi, PlatformGpuSurface,
    ResolvedScreenPublicationDescriptor, ScreenPublicationKind, ScreenPublicationResidency,
};

const SURFACE_PIXEL_BYTES: u64 = 4;
const ZONE_COLOR_BYTES: u64 = 3;

/// Fixed publication-version capacity admitted for every committed branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenPublicationSlotPolicy {
    retention_slots: NonZeroU32,
    subscriber_slots: u32,
}

impl ScreenPublicationSlotPolicy {
    /// Construct explicit last-good retention and subscriber-held capacities.
    ///
    /// # Errors
    ///
    /// Rejects a total slot count that overflows `u32`.
    pub fn try_new(
        retention_slots: NonZeroU32,
        subscriber_slots: u32,
    ) -> Result<Self, ScreenPlanError> {
        let total_slots = retention_slots
            .get()
            .checked_add(subscriber_slots)
            .ok_or(ScreenPlanError::PublicationSlotCountOverflow)?;
        if total_slots < 2 {
            return Err(ScreenPlanError::PublicationSlotCountTooSmall);
        }
        Ok(Self {
            retention_slots,
            subscriber_slots,
        })
    }

    /// Slots reserved for the last-good stream history.
    #[must_use]
    pub const fn retention_slots(self) -> NonZeroU32 {
        self.retention_slots
    }

    /// Extra in-flight versions that subscribers may retain concurrently.
    #[must_use]
    pub const fn subscriber_slots(self) -> u32 {
        self.subscriber_slots
    }

    /// Total admitted reusable versions per committed branch.
    #[must_use]
    pub fn total_slots(self) -> u32 {
        self.retention_slots
            .get()
            .checked_add(self.subscriber_slots)
            .expect("validated publication slot policy remains representable")
    }
}

impl Default for ScreenPublicationSlotPolicy {
    fn default() -> Self {
        Self {
            retention_slots: NonZeroU32::MIN,
            subscriber_slots: 2,
        }
    }
}

/// Typed surface publication input borrowed only for the publish call.
#[derive(Clone, Copy, Debug)]
pub struct ScreenSurfacePayload<'a> {
    extent: PixelExtent,
    pixel_format: CapturePixelFormat,
    colorimetry: ScreenPublicationColorimetry,
    pixels: &'a [u8],
}

impl<'a> ScreenSurfacePayload<'a> {
    /// Validate exact tightly packed four-byte target storage.
    ///
    /// # Errors
    ///
    /// Rejects unrepresentable or non-exact byte lengths.
    pub fn try_new(
        extent: PixelExtent,
        pixel_format: CapturePixelFormat,
        colorimetry: ScreenPublicationColorimetry,
        pixels: &'a [u8],
    ) -> Result<Self, ScreenPublicationHubError> {
        let expected = surface_byte_len(extent)?;
        if pixels.len() != expected {
            return Err(ScreenPublicationHubError::SurfaceByteLengthMismatch {
                expected,
                observed: pixels.len(),
            });
        }
        Ok(Self {
            extent,
            pixel_format,
            colorimetry,
            pixels,
        })
    }

    /// Exact logical output extent.
    #[must_use]
    pub const fn extent(self) -> PixelExtent {
        self.extent
    }

    /// Exact target encoding and transfer contract.
    #[must_use]
    pub const fn pixel_format(self) -> CapturePixelFormat {
        self.pixel_format
    }

    /// Exact target primaries and transfer contract.
    #[must_use]
    pub const fn colorimetry(self) -> ScreenPublicationColorimetry {
        self.colorimetry
    }

    /// Tightly packed four-byte pixels in the declared format.
    #[must_use]
    pub const fn pixels(self) -> &'a [u8] {
        self.pixels
    }
}

/// Target primaries and transfer contract shared by surfaces and RGB zones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenPublicationColorimetry {
    colorimetry: CaptureColorimetry,
}

impl ScreenPublicationColorimetry {
    /// Construct exact target colorimetry.
    #[must_use]
    pub const fn new(colorimetry: CaptureColorimetry) -> Self {
        Self { colorimetry }
    }

    /// Complete published color metadata.
    #[must_use]
    pub const fn value(self) -> CaptureColorimetry {
        self.colorimetry
    }

    /// Target channel primaries.
    #[must_use]
    pub const fn color_space(self) -> CaptureColorSpace {
        self.colorimetry.color_space()
    }

    /// Target transfer function.
    #[must_use]
    pub const fn transfer_function(self) -> CaptureTransferFunction {
        self.colorimetry.transfer_function()
    }
}

/// Typed GPU surface publication borrowed only for the publish call.
#[derive(Clone, Copy, Debug)]
pub struct ScreenGpuSurfacePayload<'a> {
    colorimetry: ScreenPublicationColorimetry,
    surface: &'a PlatformGpuSurface,
}

impl<'a> ScreenGpuSurfacePayload<'a> {
    /// Construct a lifetime-retaining opaque GPU publication.
    #[must_use]
    pub const fn new(
        colorimetry: ScreenPublicationColorimetry,
        surface: &'a PlatformGpuSurface,
    ) -> Self {
        Self {
            colorimetry,
            surface,
        }
    }

    /// Exact target primaries and transfer contract.
    #[must_use]
    pub const fn colorimetry(self) -> ScreenPublicationColorimetry {
        self.colorimetry
    }

    /// Opaque GPU surface and its erased lifetime owner.
    #[must_use]
    pub const fn surface(self) -> &'a PlatformGpuSurface {
        self.surface
    }
}

/// Typed zone publication input borrowed only for the publish call.
#[derive(Clone, Copy, Debug)]
pub struct ScreenZonesPayload<'a> {
    columns: NonZeroU32,
    rows: NonZeroU32,
    colorimetry: ScreenPublicationColorimetry,
    colors: &'a [[u8; 3]],
}

impl<'a> ScreenZonesPayload<'a> {
    /// Validate exactly one RGB color per logical zone cell.
    ///
    /// # Errors
    ///
    /// Rejects unrepresentable or non-exact color counts.
    pub fn try_new(
        columns: NonZeroU32,
        rows: NonZeroU32,
        colorimetry: ScreenPublicationColorimetry,
        colors: &'a [[u8; 3]],
    ) -> Result<Self, ScreenPublicationHubError> {
        let expected = zone_count(columns, rows)?;
        if colors.len() != expected {
            return Err(ScreenPublicationHubError::ZoneColorCountMismatch {
                expected,
                observed: colors.len(),
            });
        }
        Ok(Self {
            columns,
            rows,
            colorimetry,
            colors,
        })
    }

    /// Logical grid columns.
    #[must_use]
    pub const fn columns(self) -> NonZeroU32 {
        self.columns
    }

    /// Logical grid rows.
    #[must_use]
    pub const fn rows(self) -> NonZeroU32 {
        self.rows
    }

    /// Exact target encoding and transfer contract.
    #[must_use]
    pub const fn colorimetry(self) -> ScreenPublicationColorimetry {
        self.colorimetry
    }

    /// One RGB value per row-major zone cell.
    #[must_use]
    pub const fn colors(self) -> &'a [[u8; 3]] {
        self.colors
    }
}

/// Descriptor-validated publication payload.
#[derive(Clone, Copy, Debug)]
pub enum ScreenBranchPayload<'a> {
    /// Logical four-channel CPU surface.
    Surface(ScreenSurfacePayload<'a>),
    /// Logical four-channel platform GPU surface.
    GpuSurface(ScreenGpuSurfacePayload<'a>),
    /// Logical RGB zone grid.
    Zones(ScreenZonesPayload<'a>),
}

impl ScreenBranchPayload<'_> {
    /// Payload kind used by diagnostics.
    #[must_use]
    pub const fn kind(self) -> ScreenPayloadKind {
        match self {
            Self::Surface(_) | Self::GpuSurface(_) => ScreenPayloadKind::Surface,
            Self::Zones(_) => ScreenPayloadKind::Zones,
        }
    }

    /// Concrete storage residency of this payload.
    #[must_use]
    pub fn residency(self) -> ScreenPublicationResidency {
        match self {
            Self::Surface(_) | Self::Zones(_) => ScreenPublicationResidency::Cpu,
            Self::GpuSurface(payload) => {
                ScreenPublicationResidency::PlatformGpu(payload.surface().api().clone())
            }
        }
    }
}

/// Stable payload category independent from borrowed storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenPayloadKind {
    /// Four-channel target surface.
    Surface,
    /// RGB zone grid.
    Zones,
}

/// Worker-reported source health carried with each accepted publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenPublicationHealth {
    /// Source and processing path are operating normally.
    Healthy,
    /// Publication is valid but the source reports degraded operation.
    Degraded,
    /// Source is rebuilding while still providing a valid frame.
    Recovering,
    /// Source cannot currently produce a valid replacement publication.
    Failed,
}

impl ScreenPublicationHealth {
    const fn encode(self) -> u8 {
        match self {
            Self::Healthy => 1,
            Self::Degraded => 2,
            Self::Recovering => 3,
            Self::Failed => 4,
        }
    }

    const fn decode(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Healthy),
            2 => Some(Self::Degraded),
            3 => Some(Self::Recovering),
            4 => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Publication freshness evaluated at a caller-selected instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenPublicationFreshness {
    /// The freshness deadline has not elapsed.
    Fresh,
    /// The freshness deadline has elapsed.
    Stale,
}

/// Structural branch delivery lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenBranchDeliveryLifecycle {
    /// No publication has been accepted yet.
    Pending,
    /// A last-good publication remains readable.
    Live,
    /// Branch left committed authority.
    Retired,
}

/// Orthogonal lock-free delivery diagnostics for one exact branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenBranchDeliveryState {
    lifecycle: ScreenBranchDeliveryLifecycle,
    freshness: Option<ScreenPublicationFreshness>,
    source_health: Option<ScreenPublicationHealth>,
    last_publish_was_pressured: bool,
    pressure_events: u64,
}

impl ScreenBranchDeliveryState {
    /// Structural pending/live/retired lifecycle.
    #[must_use]
    pub const fn lifecycle(self) -> ScreenBranchDeliveryLifecycle {
        self.lifecycle
    }

    /// Freshness of last-good when one exists.
    #[must_use]
    pub const fn freshness(self) -> Option<ScreenPublicationFreshness> {
        self.freshness
    }

    /// Source health of last-good when one exists.
    #[must_use]
    pub const fn source_health(self) -> Option<ScreenPublicationHealth> {
        self.source_health
    }

    /// Whether the most recent publish attempt failed because every slot was held.
    ///
    /// This lock-free historical diagnostic clears after the next successful
    /// publication. It does not claim current slot occupancy.
    #[must_use]
    pub const fn last_publish_was_pressured(self) -> bool {
        self.last_publish_was_pressured
    }

    /// Saturating count of bounded-slot pressure events.
    #[must_use]
    pub const fn pressure_events(self) -> u64 {
        self.pressure_events
    }
}

/// Worker-side publication intent with optional completion metadata.
#[derive(Clone, Debug)]
pub struct ScreenPublicationMetadata {
    source_epoch: CaptureEpoch,
    worker_plan_generation: ScreenPlanGeneration,
    native_sequence: NonZeroU64,
    captured_at: Instant,
    freshness_deadline: Instant,
    completion: Option<ScreenPublicationCompletion>,
}

#[derive(Clone, Copy, Debug)]
struct ScreenPublicationCompletion {
    published_at: Instant,
    health: ScreenPublicationHealth,
}

impl ScreenPublicationMetadata {
    /// Construct source intent before publication work begins.
    ///
    /// Writable reservations retain this intent while the reducer fills its
    /// slot. Publication time and health are supplied only after that work
    /// finishes.
    ///
    /// # Errors
    ///
    /// Rejects a freshness deadline earlier than capture time.
    pub fn try_intent(
        source_epoch: CaptureEpoch,
        worker_plan_generation: ScreenPlanGeneration,
        native_sequence: NonZeroU64,
        captured_at: Instant,
        freshness_deadline: Instant,
    ) -> Result<Self, ScreenPublicationHubError> {
        if freshness_deadline < captured_at {
            return Err(ScreenPublicationHubError::InvalidPublicationTimeline);
        }
        Ok(Self {
            source_epoch,
            worker_plan_generation,
            native_sequence,
            captured_at,
            freshness_deadline,
            completion: None,
        })
    }

    /// Construct one exact worker publication report.
    ///
    /// # Errors
    ///
    /// Rejects capture/publish/freshness timestamps that move backwards.
    pub fn try_new(
        source_epoch: CaptureEpoch,
        worker_plan_generation: ScreenPlanGeneration,
        native_sequence: NonZeroU64,
        captured_at: Instant,
        published_at: Instant,
        freshness_deadline: Instant,
        health: ScreenPublicationHealth,
    ) -> Result<Self, ScreenPublicationHubError> {
        if published_at < captured_at || freshness_deadline < published_at {
            return Err(ScreenPublicationHubError::InvalidPublicationTimeline);
        }
        let mut metadata = Self::try_intent(
            source_epoch,
            worker_plan_generation,
            native_sequence,
            captured_at,
            freshness_deadline,
        )?;
        metadata.completion = Some(ScreenPublicationCompletion {
            published_at,
            health,
        });
        Ok(metadata)
    }

    fn complete(
        &mut self,
        published_at: Instant,
        health: ScreenPublicationHealth,
    ) -> Result<(), ScreenPublicationHubError> {
        if self.completion.is_some() {
            return Err(ScreenPublicationHubError::PublicationCompletionAlreadySet);
        }
        if published_at < self.captured_at {
            return Err(ScreenPublicationHubError::InvalidPublicationTimeline);
        }
        if published_at > self.freshness_deadline {
            return Err(ScreenPublicationHubError::PublicationFreshnessExpired);
        }
        self.completion = Some(ScreenPublicationCompletion {
            published_at,
            health,
        });
        Ok(())
    }
}

#[derive(Debug)]
enum ScreenPublicationStorage {
    CpuSurface {
        extent: PixelExtent,
        pixel_format: CapturePixelFormat,
        colorimetry: ScreenPublicationColorimetry,
        pixels: Box<[u8]>,
    },
    GpuSurface {
        api: PlatformGpuApi,
        colorimetry: ScreenPublicationColorimetry,
        surface: Option<PlatformGpuSurface>,
    },
    Zones {
        columns: NonZeroU32,
        rows: NonZeroU32,
        color_count: usize,
        colorimetry: ScreenPublicationColorimetry,
        colors: Box<[[u8; 3]]>,
    },
}

impl ScreenPublicationStorage {
    fn try_new(descriptor: &ResolvedScreenPublicationDescriptor) -> Result<Self, ScreenPlanError> {
        match descriptor.kind() {
            ScreenPublicationKind::Surface
                if matches!(
                    descriptor.required_residency(),
                    ScreenPublicationResidency::Cpu
                ) =>
            {
                let extent = descriptor.geometry().output_extent();
                let len = checked_surface_byte_len(extent)?;
                Ok(Self::CpuSurface {
                    extent,
                    pixel_format: descriptor.physical().target_pixel_format(),
                    colorimetry: descriptor_colorimetry(descriptor),
                    pixels: try_zeroed_slice(len)?,
                })
            }
            ScreenPublicationKind::Surface => {
                let ScreenPublicationResidency::PlatformGpu(api) = descriptor.required_residency()
                else {
                    unreachable!("CPU surface descriptors are handled above");
                };
                Ok(Self::GpuSurface {
                    api,
                    colorimetry: descriptor_colorimetry(descriptor),
                    surface: None,
                })
            }
            ScreenPublicationKind::Zones { columns, rows } => {
                let len = checked_zone_count(columns, rows)?;
                Ok(Self::Zones {
                    columns,
                    rows,
                    color_count: len,
                    colorimetry: descriptor_colorimetry(descriptor),
                    colors: try_zeroed_color_slice(len)?,
                })
            }
        }
    }

    fn copy_from(&mut self, payload: ScreenBranchPayload<'_>) {
        match (self, payload) {
            (Self::CpuSurface { pixels, .. }, ScreenBranchPayload::Surface(payload)) => {
                pixels.copy_from_slice(payload.pixels());
            }
            (Self::GpuSurface { surface, .. }, ScreenBranchPayload::GpuSurface(payload)) => {
                *surface = Some(payload.surface().clone());
            }
            (
                Self::Zones {
                    columns,
                    rows,
                    color_count,
                    colors,
                    ..
                },
                ScreenBranchPayload::Zones(payload),
            ) => {
                *columns = payload.columns();
                *rows = payload.rows();
                *color_count = payload.colors().len();
                colors[..*color_count].copy_from_slice(payload.colors());
            }
            _ => unreachable!("payload is descriptor-validated before slot mutation"),
        }
    }

    fn surface_pixels_mut(&mut self) -> Option<&mut [u8]> {
        match self {
            Self::CpuSurface { pixels, .. } => Some(pixels),
            Self::GpuSurface { .. } | Self::Zones { .. } => None,
        }
    }

    fn zone_colors_mut(&mut self) -> Option<&mut [[u8; 3]]> {
        match self {
            Self::CpuSurface { .. } | Self::GpuSurface { .. } => None,
            Self::Zones { colors, .. } => Some(colors),
        }
    }

    fn set_effective_zone_shape(
        &mut self,
        columns: NonZeroU32,
        rows: NonZeroU32,
    ) -> Result<(), ScreenPublicationHubError> {
        let Self::Zones {
            columns: effective_columns,
            rows: effective_rows,
            color_count,
            colors,
            ..
        } = self
        else {
            return Err(ScreenPublicationHubError::PayloadKindMismatch {
                expected: ScreenPayloadKind::Surface,
                observed: ScreenPayloadKind::Zones,
            });
        };
        let count = zone_count(columns, rows)?;
        if count > colors.len() {
            return Err(
                ScreenPublicationHubError::EffectiveZoneShapeExceedsCapacity {
                    columns,
                    rows,
                    capacity: colors.len(),
                },
            );
        }
        *effective_columns = columns;
        *effective_rows = rows;
        *color_count = count;
        Ok(())
    }

    fn reset_effective_shape(
        &mut self,
        descriptor: &ResolvedScreenPublicationDescriptor,
    ) -> Result<(), ScreenPublicationHubError> {
        if let ScreenPublicationKind::Zones { columns, rows } = descriptor.kind() {
            self.set_effective_zone_shape(columns, rows)?;
        }
        Ok(())
    }

    fn take_gpu_surface(&mut self) -> Option<PlatformGpuSurface> {
        let Self::GpuSurface { surface, .. } = self else {
            return None;
        };
        surface.take()
    }

    fn residency(&self) -> ScreenPublicationResidency {
        match self {
            Self::CpuSurface { .. } | Self::Zones { .. } => ScreenPublicationResidency::Cpu,
            Self::GpuSurface { api, .. } => ScreenPublicationResidency::PlatformGpu(api.clone()),
        }
    }

    fn view(&self) -> ScreenBranchPayload<'_> {
        match self {
            Self::CpuSurface {
                extent,
                pixel_format,
                colorimetry,
                pixels,
            } => ScreenBranchPayload::Surface(ScreenSurfacePayload {
                extent: *extent,
                pixel_format: *pixel_format,
                colorimetry: *colorimetry,
                pixels,
            }),
            Self::GpuSurface {
                colorimetry,
                surface,
                ..
            } => ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload {
                colorimetry: *colorimetry,
                surface: surface
                    .as_ref()
                    .expect("published GPU slots always contain a surface"),
            }),
            Self::Zones {
                columns,
                rows,
                color_count,
                colorimetry,
                colors,
            } => ScreenBranchPayload::Zones(ScreenZonesPayload {
                columns: *columns,
                rows: *rows,
                colorimetry: *colorimetry,
                colors: &colors[..*color_count],
            }),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ScreenRetirementCharge {
    pending_bytes: Arc<AtomicU64>,
    bytes: u64,
    armed: AtomicBool,
}

impl ScreenRetirementCharge {
    pub(crate) fn new(pending_bytes: Arc<AtomicU64>, bytes: u64) -> Self {
        Self {
            pending_bytes,
            bytes,
            armed: AtomicBool::new(false),
        }
    }

    pub(crate) fn arm(&self) {
        let was_armed = self.armed.swap(true, Ordering::AcqRel);
        assert!(!was_armed, "retirement charge is armed exactly once");
    }
}

impl Drop for ScreenRetirementCharge {
    fn drop(&mut self) {
        if !self.armed.load(Ordering::Acquire) {
            return;
        }
        let result =
            self.pending_bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                    pending.checked_sub(self.bytes)
                });
        debug_assert!(result.is_ok(), "retirement accounting cannot underflow");
    }
}

/// One immutable publication accepted into a preallocated branch slot.
#[derive(Debug)]
pub struct ScreenBranchPublication {
    committed_plan_generation: ScreenPlanGeneration,
    worker_plan_generation: ScreenPlanGeneration,
    source_epoch: CaptureEpoch,
    native_sequence: NonZeroU64,
    branch_sequence: NonZeroU64,
    captured_at: Instant,
    published_at: Instant,
    freshness_deadline: Instant,
    health: ScreenPublicationHealth,
    storage: ScreenPublicationStorage,
    _retirement_charge: Arc<ScreenRetirementCharge>,
}

impl ScreenBranchPublication {
    fn try_placeholder(
        descriptor: &ResolvedScreenPublicationDescriptor,
        worker_plan_generation: ScreenPlanGeneration,
        retirement_charge: Arc<ScreenRetirementCharge>,
    ) -> Result<Self, ScreenPlanError> {
        let now = Instant::now();
        Ok(Self {
            committed_plan_generation: worker_plan_generation,
            worker_plan_generation,
            source_epoch: descriptor.source_epoch().clone(),
            native_sequence: NonZeroU64::MIN,
            branch_sequence: NonZeroU64::MIN,
            captured_at: now,
            published_at: now,
            freshness_deadline: now,
            health: ScreenPublicationHealth::Recovering,
            storage: ScreenPublicationStorage::try_new(descriptor)?,
            _retirement_charge: retirement_charge,
        })
    }

    /// Committed plan generation that accepted this publication.
    #[must_use]
    pub const fn plan_generation(&self) -> ScreenPlanGeneration {
        self.committed_plan_generation
    }

    /// Worker binding generation that produced this publication.
    #[must_use]
    pub const fn worker_plan_generation(&self) -> ScreenPlanGeneration {
        self.worker_plan_generation
    }

    /// Exact source epoch fenced by the branch descriptor and binding.
    #[must_use]
    pub const fn source_epoch(&self) -> &CaptureEpoch {
        &self.source_epoch
    }

    /// Backend-native non-zero source sequence.
    #[must_use]
    pub const fn native_sequence(&self) -> NonZeroU64 {
        self.native_sequence
    }

    /// Branch-local non-zero accepted sequence.
    #[must_use]
    pub const fn branch_sequence(&self) -> NonZeroU64 {
        self.branch_sequence
    }

    /// Compatibility alias for the branch-local publication sequence.
    #[must_use]
    pub const fn publication_epoch(&self) -> NonZeroU64 {
        self.branch_sequence
    }

    /// Capture timestamp supplied by the source worker.
    #[must_use]
    pub const fn captured_at(&self) -> Instant {
        self.captured_at
    }

    /// Publication timestamp supplied at the worker boundary.
    #[must_use]
    pub const fn published_at(&self) -> Instant {
        self.published_at
    }

    /// Deadline after which this publication is stale.
    #[must_use]
    pub const fn freshness_deadline(&self) -> Instant {
        self.freshness_deadline
    }

    /// Source-reported health at publication.
    #[must_use]
    pub const fn health(&self) -> ScreenPublicationHealth {
        self.health
    }

    /// Concrete storage residency retained by this publication.
    #[must_use]
    pub fn residency(&self) -> ScreenPublicationResidency {
        self.storage.residency()
    }

    /// Freshness evaluated without mutating the retained publication.
    #[must_use]
    pub fn freshness_at(&self, now: Instant) -> ScreenPublicationFreshness {
        if now <= self.freshness_deadline {
            ScreenPublicationFreshness::Fresh
        } else {
            ScreenPublicationFreshness::Stale
        }
    }

    /// Borrow the descriptor-shaped immutable payload.
    #[must_use]
    pub fn payload(&self) -> ScreenBranchPayload<'_> {
        self.storage.view()
    }
}

#[derive(Debug)]
struct ScreenBranchRuntime {
    slots: Vec<Option<Arc<ScreenBranchPublication>>>,
    last_native_sequence: Option<NonZeroU64>,
    next_branch_sequence: u64,
}

struct ScreenBranchEntry {
    descriptor: ResolvedScreenPublicationDescriptor,
    runtime: Mutex<ScreenBranchRuntime>,
    latest: ArcSwapOption<ScreenBranchPublication>,
    last_publish_was_pressured: AtomicBool,
    pressure_events: AtomicU64,
    delivery_health: AtomicU8,
    retired: AtomicBool,
    allocation_bytes: u64,
    retirement_charge: Arc<ScreenRetirementCharge>,
}

impl ScreenBranchEntry {
    fn try_new(
        descriptor: ResolvedScreenPublicationDescriptor,
        worker_plan_generation: ScreenPlanGeneration,
        slot_policy: ScreenPublicationSlotPolicy,
        pending_retired_bytes: Arc<AtomicU64>,
    ) -> Result<Self, ScreenPlanError> {
        let slot_count = usize::try_from(slot_policy.total_slots())
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        let allocation_bytes = descriptor_payload_bytes(&descriptor)?
            .checked_mul(u64::from(slot_policy.total_slots()))
            .ok_or(ScreenPlanError::PublicationSlotCountOverflow)?;
        let retirement_charge = Arc::new(ScreenRetirementCharge::new(
            pending_retired_bytes,
            allocation_bytes,
        ));
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(slot_count)
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        for _ in 0..slot_count {
            slots.push(Some(Arc::new(ScreenBranchPublication::try_placeholder(
                &descriptor,
                worker_plan_generation,
                Arc::clone(&retirement_charge),
            )?)));
        }
        Ok(Self {
            descriptor,
            runtime: Mutex::new(ScreenBranchRuntime {
                slots,
                last_native_sequence: None,
                next_branch_sequence: 0,
            }),
            latest: ArcSwapOption::empty(),
            last_publish_was_pressured: AtomicBool::new(false),
            pressure_events: AtomicU64::new(0),
            delivery_health: AtomicU8::new(0),
            retired: AtomicBool::new(false),
            allocation_bytes,
            retirement_charge,
        })
    }

    fn lock_runtime(&self) -> MutexGuard<'_, ScreenBranchRuntime> {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        self.latest.store(None);
    }

    fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    fn raw_latest(&self) -> Option<Arc<ScreenBranchPublication>> {
        self.latest.load_full()
    }

    fn read(&self) -> Option<Arc<ScreenBranchPublication>> {
        if self.is_retired() {
            return None;
        }
        let publication = self.raw_latest()?;
        if self.is_retired() {
            return None;
        }
        Some(publication)
    }

    fn delivery_state(&self, now: Instant) -> ScreenBranchDeliveryState {
        if self.is_retired() {
            return ScreenBranchDeliveryState {
                lifecycle: ScreenBranchDeliveryLifecycle::Retired,
                freshness: None,
                source_health: None,
                last_publish_was_pressured: false,
                pressure_events: self.pressure_events.load(Ordering::Acquire),
            };
        }
        let Some(publication) = self.latest.load_full() else {
            return ScreenBranchDeliveryState {
                lifecycle: ScreenBranchDeliveryLifecycle::Pending,
                freshness: None,
                source_health: ScreenPublicationHealth::decode(
                    self.delivery_health.load(Ordering::Acquire),
                ),
                last_publish_was_pressured: self.last_publish_was_pressured.load(Ordering::Acquire),
                pressure_events: self.pressure_events.load(Ordering::Acquire),
            };
        };
        if self.is_retired() {
            return ScreenBranchDeliveryState {
                lifecycle: ScreenBranchDeliveryLifecycle::Retired,
                freshness: None,
                source_health: None,
                last_publish_was_pressured: false,
                pressure_events: self.pressure_events.load(Ordering::Acquire),
            };
        }
        ScreenBranchDeliveryState {
            lifecycle: ScreenBranchDeliveryLifecycle::Live,
            freshness: Some(publication.freshness_at(now)),
            source_health: ScreenPublicationHealth::decode(
                self.delivery_health.load(Ordering::Acquire),
            ),
            last_publish_was_pressured: self.last_publish_was_pressured.load(Ordering::Acquire),
            pressure_events: self.pressure_events.load(Ordering::Acquire),
        }
    }

    fn record_pressure(&self) {
        self.pressure_events
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |events| {
                Some(events.saturating_add(1))
            })
            .expect("saturating pressure diagnostics never reject an update");
        self.last_publish_was_pressured
            .store(true, Ordering::Release);
    }

    fn clear_pressure(&self) {
        self.last_publish_was_pressured
            .store(false, Ordering::Release);
    }

    fn report_health(&self, health: ScreenPublicationHealth) {
        self.delivery_health
            .store(health.encode(), Ordering::Release);
    }

    fn reap_releasable_gpu_payloads(&self) -> usize {
        let mut reaped = 0;
        loop {
            let surface = {
                let mut runtime = self.lock_runtime();
                let latest = self.latest.load();
                runtime.slots.iter_mut().find_map(|slot| {
                    let publication = slot.as_mut()?;
                    if latest
                        .as_ref()
                        .is_some_and(|latest| Arc::ptr_eq(latest, publication))
                    {
                        return None;
                    }
                    Arc::get_mut(publication)
                        .and_then(|publication| publication.storage.take_gpu_surface())
                })
            };
            let Some(surface) = surface else {
                return reaped;
            };
            drop(surface);
            reaped += 1;
        }
    }

    fn publications_are_pool_owned(&self) -> bool {
        let runtime = self.lock_runtime();
        let latest = self.latest.load();
        runtime.slots.iter().all(|slot| {
            let Some(publication) = slot else {
                return false;
            };
            let latest_owns = latest
                .as_ref()
                .is_some_and(|latest| Arc::ptr_eq(latest, publication));
            let expected = 1 + usize::from(latest_owns);
            Arc::strong_count(publication) == expected && Arc::weak_count(publication) == 0
        })
    }
}

impl fmt::Debug for ScreenBranchEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScreenBranchEntry")
            .field("descriptor", &self.descriptor)
            .field("retired", &self.is_retired())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
struct ScreenCommittedBranch {
    entry: Arc<ScreenBranchEntry>,
    binding: ScreenWorkerBinding,
}

/// One immutable, atomically installed screen runtime authority snapshot.
pub struct ScreenCommittedState {
    plan: Arc<ScreenCapturePlan>,
    branches: Arc<Vec<ScreenCommittedBranch>>,
    worker_bindings: Arc<Vec<ScreenWorkerBinding>>,
    retained_resources: ScreenRetainedResourceLedger,
    slot_policy: ScreenPublicationSlotPolicy,
    pending_retired_bytes: Arc<AtomicU64>,
}

impl ScreenCommittedState {
    fn initial(
        slot_policy: ScreenPublicationSlotPolicy,
        pending_retired_bytes: Arc<AtomicU64>,
    ) -> Self {
        Self {
            plan: Arc::new(ScreenCapturePlan::default()),
            branches: Arc::new(Vec::new()),
            worker_bindings: Arc::new(Vec::new()),
            retained_resources: ScreenRetainedResourceLedger::default(),
            slot_policy,
            pending_retired_bytes,
        }
    }

    pub(crate) fn prepare(
        base: &Arc<Self>,
        plan: Arc<ScreenCapturePlan>,
        retained_resources: ScreenRetainedResourceLedger,
        new_bindings: impl IntoIterator<Item = ScreenWorkerBinding>,
        activation: Arc<ScreenWorkerActivationLatch>,
    ) -> Result<(Arc<Self>, ScreenCommitActivation), ScreenPlanError> {
        let mut retained_bindings = Vec::new();
        for binding in new_bindings {
            if retained_bindings.len() == retained_bindings.capacity() {
                retained_bindings
                    .try_reserve(1)
                    .map_err(|_| ScreenPlanError::AllocationFailed)?;
            }
            retained_bindings.push(binding);
        }
        let mut new_bindings = retained_bindings;
        new_bindings.sort_unstable_by(|left, right| {
            left.source_id().as_str().cmp(right.source_id().as_str())
        });

        let mut branches = Vec::new();
        branches
            .try_reserve_exact(plan.branches().len())
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        for demand in plan.branches() {
            let descriptor = demand.descriptor();
            let binding = base
                .branch(descriptor)
                .map(|branch| branch.binding.clone())
                .or_else(|| {
                    new_bindings
                        .binary_search_by(|binding| {
                            binding
                                .source_id()
                                .as_str()
                                .cmp(descriptor.source_epoch().source_id.as_str())
                        })
                        .ok()
                        .and_then(|index| new_bindings.get(index).cloned())
                })
                .ok_or_else(|| ScreenPlanError::MissingWorkerAcknowledgement {
                    source_id: descriptor.source_epoch().source_id.clone(),
                })?;
            let entry = if let Some(branch) = base.branch(descriptor) {
                Arc::clone(&branch.entry)
            } else {
                Arc::new(ScreenBranchEntry::try_new(
                    descriptor.clone(),
                    binding.plan_generation(),
                    base.slot_policy,
                    Arc::clone(&base.pending_retired_bytes),
                )?)
            };
            branches.push(ScreenCommittedBranch { entry, binding });
        }

        let mut worker_bindings: Vec<ScreenWorkerBinding> = Vec::new();
        worker_bindings
            .try_reserve_exact(branches.len())
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        for branch in &branches {
            worker_bindings.push(branch.binding.clone());
        }
        worker_bindings.sort_unstable_by(compare_bindings);
        worker_bindings.dedup_by(|right, left| left.is_same(right));

        let mut retired_entries = Vec::new();
        retired_entries
            .try_reserve_exact(base.branches.len())
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        let mut base_index = 0;
        let mut candidate_index = 0;
        while base_index < base.branches.len() {
            let base_branch = &base.branches[base_index];
            while candidate_index < branches.len()
                && branches[candidate_index].entry.descriptor < base_branch.entry.descriptor
            {
                candidate_index += 1;
            }
            if branches
                .get(candidate_index)
                .is_none_or(|candidate| candidate.entry.descriptor != base_branch.entry.descriptor)
            {
                retired_entries.push(Arc::clone(&base_branch.entry));
            }
            base_index += 1;
        }

        let mut retired_bindings = Vec::new();
        retired_bindings
            .try_reserve_exact(base.worker_bindings.len())
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        for binding in base.worker_bindings.iter() {
            if worker_bindings
                .binary_search_by(|candidate| compare_bindings(candidate, binding))
                .is_err()
            {
                retired_bindings.push(binding.clone());
            }
        }
        for binding in &new_bindings {
            if worker_bindings
                .binary_search_by(|candidate| compare_bindings(candidate, binding))
                .is_err()
            {
                retired_bindings.push(binding.clone());
            }
        }
        let mut barrier_entries = Vec::new();
        let barrier_capacity = base
            .branches
            .len()
            .checked_add(branches.len())
            .ok_or(ScreenPlanError::AllocationFailed)?;
        barrier_entries
            .try_reserve_exact(barrier_capacity)
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        barrier_entries.extend(base.branches.iter().map(|branch| Arc::clone(&branch.entry)));
        barrier_entries.extend(branches.iter().map(|branch| Arc::clone(&branch.entry)));
        barrier_entries.sort_unstable_by(|left, right| left.descriptor.cmp(&right.descriptor));
        barrier_entries.dedup_by(|right, left| Arc::ptr_eq(left, right));
        let mut barrier_bindings = Vec::new();
        let binding_capacity = base
            .worker_bindings
            .len()
            .checked_add(worker_bindings.len())
            .ok_or(ScreenPlanError::AllocationFailed)?;
        barrier_bindings
            .try_reserve_exact(binding_capacity)
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        barrier_bindings.extend(base.worker_bindings.iter().cloned());
        barrier_bindings.extend(worker_bindings.iter().cloned());
        barrier_bindings.sort_unstable_by(compare_bindings);
        barrier_bindings.dedup_by(|right, left| left.is_same(right));
        let retired_publication_bytes =
            retired_entries.iter().try_fold(0_u64, |total, entry| {
                total
                    .checked_add(entry.allocation_bytes)
                    .ok_or(ScreenPlanError::RetirementAccountingOverflow)
            })?;
        let retired_resource_lifetimes = base
            .retained_resources
            .retired_lifetimes(&retained_resources)?;
        let retired_resource_bytes =
            retired_resource_lifetimes
                .iter()
                .try_fold(0_u64, |total, lifetime| {
                    total
                        .checked_add(lifetime.resource().bytes())
                        .ok_or(ScreenPlanError::RetirementAccountingOverflow)
                })?;
        let retired_bytes = retired_publication_bytes
            .checked_add(retired_resource_bytes)
            .ok_or(ScreenPlanError::RetirementAccountingOverflow)?;

        Ok((
            Arc::new(Self {
                plan,
                branches: Arc::new(branches),
                worker_bindings: Arc::new(worker_bindings),
                retained_resources,
                slot_policy: base.slot_policy,
                pending_retired_bytes: Arc::clone(&base.pending_retired_bytes),
            }),
            ScreenCommitActivation {
                activation,
                barrier_bindings,
                barrier_entries,
                retired_entries,
                retired_resource_lifetimes,
                retired_bindings,
                retired_bytes,
            },
        ))
    }

    /// Atomically co-published immutable plan.
    #[must_use]
    pub fn plan(&self) -> &ScreenCapturePlan {
        self.plan.as_ref()
    }

    pub(crate) const fn plan_arc(&self) -> &Arc<ScreenCapturePlan> {
        &self.plan
    }

    /// Exact committed worker allocation bytes.
    #[must_use]
    pub const fn retained_exact_bytes(&self) -> u64 {
        self.retained_resources.total_bytes
    }

    pub(crate) const fn retained_resources(&self) -> &ScreenRetainedResourceLedger {
        &self.retained_resources
    }

    pub(crate) fn pending_retirement_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.pending_retired_bytes)
    }

    /// Fixed slot policy represented by this snapshot.
    #[must_use]
    pub const fn slot_policy(&self) -> ScreenPublicationSlotPolicy {
        self.slot_policy
    }

    /// Every distinct worker binding retained by committed branches.
    #[must_use]
    pub fn worker_bindings(&self) -> &[ScreenWorkerBinding] {
        self.worker_bindings.as_slice()
    }

    /// Number of exact committed publication branches.
    #[must_use]
    pub fn branch_count(&self) -> usize {
        self.branches.len()
    }

    /// Bind a publisher from this exact immutable authority snapshot.
    ///
    /// # Errors
    ///
    /// Rejects branches absent from the snapshot and substituted worker
    /// bindings. The resulting capability remains stale-safe if a later state
    /// replaces this snapshot before publication.
    pub fn publisher(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        binding: &ScreenWorkerBinding,
    ) -> Result<ScreenBranchPublisher, ScreenPublicationHubError> {
        let branch = self.branch(descriptor).cloned().ok_or_else(|| {
            ScreenPublicationHubError::BranchMissing {
                descriptor: Arc::new(descriptor.clone()),
            }
        })?;
        if !branch.binding.is_same(binding) {
            return Err(ScreenPublicationHubError::WorkerBindingMismatch {
                descriptor: Arc::new(descriptor.clone()),
            });
        }
        Ok(ScreenBranchPublisher {
            branch: branch.entry,
            binding: branch.binding,
        })
    }

    fn branch(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
    ) -> Option<&ScreenCommittedBranch> {
        self.branches
            .binary_search_by(|branch| branch.entry.descriptor.cmp(descriptor))
            .ok()
            .and_then(|index| self.branches.get(index))
    }

    fn contains_authority(
        &self,
        entry: &Arc<ScreenBranchEntry>,
        binding: &ScreenWorkerBinding,
    ) -> bool {
        self.branch(&entry.descriptor).is_some_and(|current| {
            Arc::ptr_eq(&current.entry, entry) && current.binding.is_same(binding)
        })
    }

    fn contains_entry(&self, entry: &Arc<ScreenBranchEntry>) -> bool {
        self.branch(&entry.descriptor)
            .is_some_and(|current| Arc::ptr_eq(&current.entry, entry))
    }
}

impl fmt::Debug for ScreenCommittedState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScreenCommittedState")
            .field("plan", &self.plan)
            .field("branch_count", &self.branches.len())
            .field("worker_bindings", &self.worker_bindings)
            .field("retained_resources", &self.retained_resources)
            .field("slot_policy", &self.slot_policy)
            .field(
                "pending_retired_bytes",
                &self.pending_retired_bytes.load(Ordering::Acquire),
            )
            .finish()
    }
}

fn compare_bindings(left: &ScreenWorkerBinding, right: &ScreenWorkerBinding) -> std::cmp::Ordering {
    left.source_id()
        .as_str()
        .cmp(right.source_id().as_str())
        .then_with(|| left.transaction_id().cmp(&right.transaction_id()))
        .then_with(|| left.worker_nonce().cmp(&right.worker_nonce()))
}

/// Publication pools detached by an atomic screen authority commit.
#[must_use = "retired publication pools must be reclaimed outside the coordinator lock"]
pub struct ScreenPublicationRetirement {
    entries: Vec<Arc<ScreenBranchEntry>>,
    resource_lifetimes: Vec<ScreenResourceLifetime>,
    bytes: u64,
}

impl ScreenPublicationRetirement {
    /// Bytes that remain admitted until every external storage owner releases them.
    #[must_use]
    pub const fn pending_bytes(&self) -> u64 {
        self.bytes
    }

    /// Number of retired branch pools still owned by this handle.
    #[must_use]
    pub fn branch_count(&self) -> usize {
        self.entries.len()
    }

    /// Number of retired worker allocations still owned by this handoff.
    #[must_use]
    pub fn resource_count(&self) -> usize {
        self.resource_lifetimes.len()
    }

    /// Reclaim every pool for which this handle is the final owner.
    ///
    /// # Errors
    ///
    /// Returns only the still-blocked pools while a publisher, lease, receipt,
    /// publication, or weak publication reference can still observe them.
    pub fn try_reclaim(mut self) -> Result<(), Self> {
        let mut blocked_bytes = 0_u64;
        self.entries.retain(|entry| {
            entry.reap_releasable_gpu_payloads();
            let ready = Arc::strong_count(entry) == 1
                && Arc::weak_count(entry) == 0
                && entry.publications_are_pool_owned();
            let blocked = !ready;
            if blocked {
                blocked_bytes = blocked_bytes
                    .checked_add(entry.allocation_bytes)
                    .expect("retirement subset cannot exceed its checked total");
            }
            blocked
        });
        self.resource_lifetimes.retain(|lifetime| {
            let blocked = !lifetime.is_final_owner();
            if blocked {
                blocked_bytes = blocked_bytes
                    .checked_add(lifetime.resource().bytes())
                    .expect("retirement subset cannot exceed its checked total");
            }
            blocked
        });
        self.bytes = blocked_bytes;
        if self.entries.is_empty() && self.resource_lifetimes.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

impl fmt::Debug for ScreenPublicationRetirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScreenPublicationRetirement")
            .field("branch_count", &self.entries.len())
            .field("resource_count", &self.resource_lifetimes.len())
            .field("pending_bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

/// Prebuilt work for one atomic state swap and explicit retirement handoff.
#[derive(Debug)]
pub(crate) struct ScreenCommitActivation {
    activation: Arc<ScreenWorkerActivationLatch>,
    barrier_bindings: Vec<ScreenWorkerBinding>,
    barrier_entries: Vec<Arc<ScreenBranchEntry>>,
    retired_entries: Vec<Arc<ScreenBranchEntry>>,
    retired_resource_lifetimes: Vec<ScreenResourceLifetime>,
    retired_bindings: Vec<ScreenWorkerBinding>,
    retired_bytes: u64,
}

impl ScreenCommitActivation {
    pub(crate) fn abort(self) {
        self.activation.abort();
    }
}

/// Coordinator-owned atomic screen runtime authority.
pub struct ScreenPublicationHub {
    state: Arc<ArcSwap<ScreenCommittedState>>,
    pending_retired_bytes: Arc<AtomicU64>,
}

impl ScreenPublicationHub {
    pub(crate) fn new(slot_policy: ScreenPublicationSlotPolicy) -> Self {
        let pending_retired_bytes = Arc::new(AtomicU64::new(0));
        Self {
            state: Arc::new(ArcSwap::from_pointee(ScreenCommittedState::initial(
                slot_policy,
                Arc::clone(&pending_retired_bytes),
            ))),
            pending_retired_bytes,
        }
    }

    /// Current committed authority snapshot.
    #[must_use]
    pub fn committed_state(&self) -> Arc<ScreenCommittedState> {
        self.state.load_full()
    }

    /// Currently committed plan generation.
    #[must_use]
    pub fn generation(&self) -> ScreenPlanGeneration {
        self.state.load().plan.generation()
    }

    /// Observe one matching branch lease and its generation from the same
    /// committed authority snapshot.
    #[must_use]
    pub fn observe_matching_lease(
        &self,
        mut matches: impl FnMut(&ResolvedScreenPublicationDescriptor) -> bool,
    ) -> (ScreenPlanGeneration, Option<ScreenBranchLease>) {
        let state = self.state.load_full();
        let generation = state.plan.generation();
        let lease = state
            .branches
            .iter()
            .find(|branch| matches(&branch.entry.descriptor))
            .map(|branch| ScreenBranchLease {
                branch: Arc::clone(&branch.entry),
                authority: Arc::clone(&self.state),
            });
        (generation, lease)
    }

    /// Currently committed demand revision.
    #[must_use]
    pub fn demand_revision(&self) -> InputPublicationDemandRevision {
        self.state.load().plan.demand_revision()
    }

    /// Retired publication storage still owned by a handoff or external observer.
    #[must_use]
    pub fn pending_retired_bytes(&self) -> u64 {
        self.pending_retired_bytes.load(Ordering::Acquire)
    }

    /// Acquire a typed lease only for an active committed branch.
    ///
    /// # Errors
    ///
    /// Rejects descriptors absent from or retired out of committed authority.
    pub fn lease(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
    ) -> Result<ScreenBranchLease, ScreenPublicationHubError> {
        let state = self.state.load_full();
        let branch = state.branch(descriptor).cloned().ok_or_else(|| {
            ScreenPublicationHubError::BranchMissing {
                descriptor: Arc::new(descriptor.clone()),
            }
        })?;
        Ok(ScreenBranchLease {
            branch: branch.entry,
            authority: Arc::clone(&self.state),
        })
    }

    /// Bind a publisher to the exact committed worker authority.
    ///
    /// # Errors
    ///
    /// Rejects branches absent from committed authority and substituted bindings.
    pub fn publisher(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        binding: &ScreenWorkerBinding,
    ) -> Result<ScreenBranchPublisher, ScreenPublicationHubError> {
        let state = self.state.load_full();
        state.publisher(descriptor, binding)
    }

    /// Reserve one admitted descriptor-shaped slot for direct reducer writes.
    ///
    /// The returned reservation owns the only mutable reference to its slot.
    /// Callers fill it through [`PreparedScreenPublication::surface_pixels_mut`]
    /// or [`PreparedScreenPublication::zone_colors_mut`], then consume it with
    /// [`Self::finalize_writable_publication`]. Dropping the reservation returns
    /// the slot without replacing last-good.
    ///
    /// # Errors
    ///
    /// Rejects stale authority, a requested kind unlike the descriptor, wrong
    /// epoch, or retained and reserved slots that exhaust the admitted pool.
    pub fn prepare_writable_publication(
        &self,
        publisher: &ScreenBranchPublisher,
        expected_kind: ScreenPayloadKind,
        metadata: &ScreenPublicationMetadata,
    ) -> Result<PreparedScreenPublication, ScreenPublicationHubError> {
        validate_payload_kind(&publisher.branch.descriptor, expected_kind)?;
        let residency = publisher.branch.descriptor.required_residency();
        if residency != ScreenPublicationResidency::Cpu {
            return Err(ScreenPublicationHubError::ResidencyMismatch {
                expected: residency,
                observed: ScreenPublicationResidency::Cpu,
            });
        }
        validate_publication_intent(&publisher.branch, &publisher.binding, metadata)?;
        if metadata.completion.is_some() {
            return Err(ScreenPublicationHubError::PublicationCompletionAlreadySet);
        }
        self.reserve_publication(publisher, metadata)
    }

    /// Reserve one admitted slot and copy payload bytes outside authority locks.
    ///
    /// # Errors
    ///
    /// Rejects stale authority, wrong shape/kind/epoch, non-monotonic native
    /// sequences, and retained or reserved slots that exhaust the admitted pool.
    pub fn prepare_publication(
        &self,
        publisher: &ScreenBranchPublisher,
        payload: ScreenBranchPayload<'_>,
        metadata: &ScreenPublicationMetadata,
    ) -> Result<PreparedScreenPublication, ScreenPublicationHubError> {
        validate_payload(&publisher.branch.descriptor, payload)?;
        validate_metadata(&publisher.branch, &publisher.binding, metadata)?;
        let mut prepared = self.reserve_publication(publisher, metadata)?;
        let publication_storage = prepared.publication_mut()?;
        publication_storage.storage.copy_from(payload);
        Ok(prepared)
    }

    fn reserve_publication(
        &self,
        publisher: &ScreenBranchPublisher,
        metadata: &ScreenPublicationMetadata,
    ) -> Result<PreparedScreenPublication, ScreenPublicationHubError> {
        let mut runtime = publisher.branch.lock_runtime();
        let state = self.state.load_full();
        if publisher.branch.is_retired() {
            return Err(ScreenPublicationHubError::BranchRetired {
                descriptor: Arc::new(publisher.branch.descriptor.clone()),
            });
        }
        if !state.contains_authority(&publisher.branch, &publisher.binding) {
            return Err(ScreenPublicationHubError::PublisherStale {
                expected: state.plan.generation(),
                observed: publisher.binding.plan_generation(),
            });
        }
        let mut reserved = None;
        for (slot_index, slot) in runtime.slots.iter_mut().enumerate() {
            let Some(mut publication) = slot.take() else {
                continue;
            };
            if Arc::get_mut(&mut publication).is_some() {
                reserved = Some((slot_index, publication));
                break;
            }
            *slot = Some(publication);
        }
        let Some((slot_index, mut publication)) = reserved else {
            publisher.branch.record_pressure();
            return Err(ScreenPublicationHubError::PublicationPressure {
                admitted_slots: u32::try_from(runtime.slots.len()).unwrap_or(u32::MAX),
            });
        };
        drop(runtime);
        if Arc::get_mut(&mut publication).is_none() {
            let reserved = PreparedScreenPublication {
                branch: Arc::clone(&publisher.branch),
                binding: publisher.binding.clone(),
                slot_index,
                publication: Some(publication),
                metadata: metadata.clone(),
            };
            drop(reserved);
            publisher.branch.record_pressure();
            return Err(ScreenPublicationHubError::PublicationPressure {
                admitted_slots: self.state.load().slot_policy.total_slots(),
            });
        }
        Arc::get_mut(&mut publication)
            .expect("reserved publication has unique ownership")
            .storage
            .reset_effective_shape(&publisher.branch.descriptor)?;
        Ok(PreparedScreenPublication {
            branch: Arc::clone(&publisher.branch),
            binding: publisher.binding.clone(),
            slot_index,
            publication: Some(publication),
            metadata: metadata.clone(),
        })
    }

    /// Finalize a copied slot only if exact committed authority still matches.
    ///
    /// # Errors
    ///
    /// Rejects a stale binding, lost reservation, non-monotonic source sequence,
    /// or exhausted branch-local sequence without changing last-good.
    pub fn finalize_publication(
        &self,
        mut prepared: PreparedScreenPublication,
    ) -> Result<ScreenLiveBranchReceipt, ScreenPublicationHubError> {
        let binding = prepared.binding.clone();
        let _finalization = binding.lock_finalization();
        let state = self.state.load_full();
        preflight_publication(&state, &prepared)?;
        Ok(commit_preflighted_publication(state, &mut prepared))
    }

    pub(crate) fn finalize_writable_publications(
        &self,
        prepared: &mut [PreparedScreenPublication],
        published_at: Instant,
        health: ScreenPublicationHealth,
    ) -> Result<(), ScreenPublicationHubError> {
        for publication in prepared.iter_mut() {
            publication.metadata.complete(published_at, health)?;
        }
        let Some(first) = prepared.first() else {
            return Ok(());
        };
        let binding = first.binding.clone();
        if prepared
            .iter()
            .any(|publication| !binding.is_same(&publication.binding))
        {
            return Err(ScreenPublicationHubError::PublicationBatchBindingMismatch);
        }
        let _finalization = binding.lock_finalization();
        let state = self.state.load_full();
        for publication in prepared.iter() {
            preflight_publication(&state, publication)?;
        }
        for publication in prepared.iter_mut() {
            drop(commit_preflighted_publication(
                Arc::clone(&state),
                publication,
            ));
        }
        Ok(())
    }

    /// Complete and finalize one directly written publication slot.
    ///
    /// Completion time is supplied after reducer work, so published freshness
    /// reflects actual delivery rather than reservation time.
    ///
    /// # Errors
    ///
    /// Rejects completion before capture, completion after the freshness
    /// deadline, repeated completion, stale authority, lost reservations, or
    /// non-monotonic source sequences. Every rejection returns the slot without
    /// replacing last-good.
    pub fn finalize_writable_publication(
        &self,
        mut prepared: PreparedScreenPublication,
        published_at: Instant,
        health: ScreenPublicationHealth,
    ) -> Result<ScreenLiveBranchReceipt, ScreenPublicationHubError> {
        prepared.metadata.complete(published_at, health)?;
        self.finalize_publication(prepared)
    }

    /// Copy and finalize one publication using the two-phase slot protocol.
    ///
    /// # Errors
    ///
    /// Returns the typed preparation or finalization failure.
    pub fn publish(
        &self,
        publisher: &ScreenBranchPublisher,
        payload: ScreenBranchPayload<'_>,
        metadata: &ScreenPublicationMetadata,
    ) -> Result<ScreenLiveBranchReceipt, ScreenPublicationHubError> {
        let prepared = self.prepare_publication(publisher, payload, metadata)?;
        self.finalize_publication(prepared)
    }

    /// Update delivery health without replacing the last-good payload.
    ///
    /// # Errors
    ///
    /// Rejects stale or retired worker authority.
    pub fn report_delivery_health(
        &self,
        publisher: &ScreenBranchPublisher,
        health: ScreenPublicationHealth,
    ) -> Result<(), ScreenPublicationHubError> {
        let runtime = publisher.branch.lock_runtime();
        let state = self.state.load_full();
        if !state.contains_authority(&publisher.branch, &publisher.binding) {
            drop(runtime);
            return Err(ScreenPublicationHubError::PublisherStale {
                expected: state.plan.generation(),
                observed: publisher.binding.plan_generation(),
            });
        }
        if publisher.branch.is_retired() {
            drop(runtime);
            return Err(ScreenPublicationHubError::BranchRetired {
                descriptor: Arc::new(publisher.branch.descriptor.clone()),
            });
        }
        publisher.branch.report_health(health);
        drop(runtime);
        Ok(())
    }

    /// Acquire continuity only from an exactly live committed branch.
    ///
    /// # Errors
    ///
    /// Rejects missing, retired, inactive, or pending branches.
    pub fn continuity_lease(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
    ) -> Result<ScreenContinuityLease, ScreenPublicationHubError> {
        let branch = self.lease(descriptor)?;
        let publication =
            branch
                .read()
                .ok_or_else(|| ScreenPublicationHubError::BranchPending {
                    descriptor: Arc::new(descriptor.clone()),
                })?;
        Ok(ScreenContinuityLease {
            branch,
            publication,
        })
    }

    pub(crate) fn commit(
        &self,
        state: Arc<ScreenCommittedState>,
        activation: Box<ScreenCommitActivation>,
    ) -> Result<ScreenPublicationRetirement, (ScreenPlanError, Box<ScreenCommitActivation>)> {
        let mut binding_guards = Vec::new();
        if binding_guards
            .try_reserve_exact(activation.barrier_bindings.len())
            .is_err()
        {
            return Err((ScreenPlanError::AllocationFailed, activation));
        }
        for binding in &activation.barrier_bindings {
            binding_guards.push(binding.lock_finalization());
        }
        let mut guards = Vec::new();
        if guards
            .try_reserve_exact(activation.barrier_entries.len())
            .is_err()
        {
            drop(binding_guards);
            return Err((ScreenPlanError::AllocationFailed, activation));
        }
        for entry in &activation.barrier_entries {
            guards.push(entry.lock_runtime());
        }
        if self
            .pending_retired_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                pending.checked_add(activation.retired_bytes)
            })
            .is_err()
        {
            drop(guards);
            drop(binding_guards);
            return Err((ScreenPlanError::RetirementAccountingOverflow, activation));
        }
        for entry in &activation.retired_entries {
            entry.retirement_charge.arm();
        }
        for lifetime in &activation.retired_resource_lifetimes {
            lifetime.arm_retirement();
        }
        self.state.store(state);
        for entry in &activation.retired_entries {
            entry.retire();
        }
        for binding in &activation.retired_bindings {
            binding.retire();
        }
        activation.activation.activate();
        drop(guards);
        drop(binding_guards);
        for entry in &activation.retired_entries {
            entry.reap_releasable_gpu_payloads();
        }
        let ScreenCommitActivation {
            activation: _,
            barrier_bindings: _,
            barrier_entries: _,
            retired_entries,
            retired_resource_lifetimes,
            retired_bindings: _,
            retired_bytes,
        } = *activation;
        Ok(ScreenPublicationRetirement {
            entries: retired_entries,
            resource_lifetimes: retired_resource_lifetimes,
            bytes: retired_bytes,
        })
    }
}

fn preflight_publication(
    state: &ScreenCommittedState,
    prepared: &PreparedScreenPublication,
) -> Result<(), ScreenPublicationHubError> {
    let runtime = prepared.branch.lock_runtime();
    if !state.contains_authority(&prepared.branch, &prepared.binding) {
        return Err(ScreenPublicationHubError::PublisherStale {
            expected: state.plan.generation(),
            observed: prepared.binding.plan_generation(),
        });
    }
    if prepared.branch.is_retired() {
        return Err(ScreenPublicationHubError::BranchRetired {
            descriptor: Arc::new(prepared.branch.descriptor.clone()),
        });
    }
    validate_metadata(&prepared.branch, &prepared.binding, &prepared.metadata)?;
    if runtime
        .last_native_sequence
        .is_some_and(|sequence| prepared.metadata.native_sequence <= sequence)
    {
        return Err(ScreenPublicationHubError::NativeSequenceNotMonotonic {
            previous: runtime
                .last_native_sequence
                .expect("checked previous native sequence exists"),
            observed: prepared.metadata.native_sequence,
        });
    }
    if runtime
        .next_branch_sequence
        .checked_add(1)
        .and_then(NonZeroU64::new)
        .is_none()
    {
        return Err(ScreenPublicationHubError::BranchSequenceExhausted);
    }
    if !runtime
        .slots
        .get(prepared.slot_index)
        .is_some_and(Option::is_none)
    {
        return Err(ScreenPublicationHubError::PublicationReservationLost);
    }
    let publication = prepared
        .publication
        .as_ref()
        .ok_or(ScreenPublicationHubError::PublicationReservationLost)?;
    if Arc::strong_count(publication) != 1 || Arc::weak_count(publication) != 0 {
        return Err(ScreenPublicationHubError::PublicationReservationLost);
    }
    Ok(())
}

fn commit_preflighted_publication(
    state: Arc<ScreenCommittedState>,
    prepared: &mut PreparedScreenPublication,
) -> ScreenLiveBranchReceipt {
    let mut runtime = prepared.branch.lock_runtime();
    let next_branch_sequence = runtime
        .next_branch_sequence
        .checked_add(1)
        .and_then(NonZeroU64::new)
        .expect("preflighted publication has branch sequence capacity");
    let mut publication = prepared
        .publication
        .take()
        .expect("preflighted publication retains its reservation");
    let publication_storage =
        Arc::get_mut(&mut publication).expect("preflighted publication has unique ownership");
    let completion = prepared
        .metadata
        .completion
        .expect("preflighted publication has completion metadata");
    publication_storage.committed_plan_generation = state.plan.generation();
    publication_storage.worker_plan_generation = prepared.metadata.worker_plan_generation;
    publication_storage.source_epoch = prepared.metadata.source_epoch.clone();
    publication_storage.native_sequence = prepared.metadata.native_sequence;
    publication_storage.branch_sequence = next_branch_sequence;
    publication_storage.captured_at = prepared.metadata.captured_at;
    publication_storage.published_at = completion.published_at;
    publication_storage.freshness_deadline = prepared.metadata.freshness_deadline;
    publication_storage.health = completion.health;
    runtime.next_branch_sequence = next_branch_sequence.get();
    runtime.last_native_sequence = Some(prepared.metadata.native_sequence);
    runtime.slots[prepared.slot_index] = Some(Arc::clone(&publication));
    prepared.branch.latest.store(Some(Arc::clone(&publication)));
    prepared.branch.report_health(completion.health);
    prepared.branch.clear_pressure();
    drop(runtime);
    ScreenLiveBranchReceipt {
        state,
        branch: Arc::clone(&prepared.branch),
        publication,
    }
}

impl fmt::Debug for ScreenPublicationHub {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.load();
        formatter
            .debug_struct("ScreenPublicationHub")
            .field("generation", &state.plan.generation())
            .field("demand_revision", &state.plan.demand_revision())
            .field("branch_count", &state.branches.len())
            .finish_non_exhaustive()
    }
}

/// Opaque typed read handle for one exact committed branch.
#[derive(Clone, Debug)]
pub struct ScreenBranchLease {
    branch: Arc<ScreenBranchEntry>,
    authority: Arc<ArcSwap<ScreenCommittedState>>,
}

impl ScreenBranchLease {
    /// Exact descriptor this lease can read.
    #[must_use]
    pub fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.branch.descriptor
    }

    /// Latest live publication, or `None` while pending or after retirement.
    #[must_use]
    pub fn read(&self) -> Option<Arc<ScreenBranchPublication>> {
        loop {
            let state = self.authority.load_full();
            if !state.contains_entry(&self.branch) {
                return None;
            }
            let publication = self.branch.read();
            if Arc::ptr_eq(&state, &self.authority.load_full()) {
                return publication;
            }
        }
    }

    /// Lock-free delivery state at one caller-selected observation instant.
    #[must_use]
    pub fn delivery_state(&self, now: Instant) -> ScreenBranchDeliveryState {
        loop {
            let state = self.authority.load_full();
            if !state.contains_entry(&self.branch) {
                return ScreenBranchDeliveryState {
                    lifecycle: ScreenBranchDeliveryLifecycle::Retired,
                    freshness: None,
                    source_health: None,
                    last_publish_was_pressured: false,
                    pressure_events: self.branch.pressure_events.load(Ordering::Acquire),
                };
            }
            let delivery = self.branch.delivery_state(now);
            if Arc::ptr_eq(&state, &self.authority.load_full()) {
                return delivery;
            }
        }
    }
}

/// Opaque worker-binding-bound publication capability.
#[derive(Debug)]
pub struct ScreenBranchPublisher {
    branch: Arc<ScreenBranchEntry>,
    binding: ScreenWorkerBinding,
}

impl ScreenBranchPublisher {
    /// Exact descriptor this capability may publish.
    #[must_use]
    pub fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.branch.descriptor
    }

    /// Worker plan generation that issued this capability.
    #[must_use]
    pub fn plan_generation(&self) -> ScreenPlanGeneration {
        self.binding.plan_generation()
    }

    /// Exact worker authority retained by this publisher.
    #[must_use]
    pub fn binding(&self) -> &ScreenWorkerBinding {
        &self.binding
    }

    /// Release superseded GPU payloads no reader still retains.
    ///
    /// The current last-good publication is never cleared. Reader-held slots
    /// remain untouched until a later pass observes unique pool ownership.
    ///
    /// Returns the number of native surface owners released by this pass.
    pub fn reap_releasable_gpu_payloads(&self) -> usize {
        self.branch.reap_releasable_gpu_payloads()
    }
}

/// Descriptor-copied slot awaiting a short authority-gated finalization.
#[must_use = "prepared publication slots must be finalized or returned to their pool"]
pub struct PreparedScreenPublication {
    branch: Arc<ScreenBranchEntry>,
    binding: ScreenWorkerBinding,
    slot_index: usize,
    publication: Option<Arc<ScreenBranchPublication>>,
    metadata: ScreenPublicationMetadata,
}

impl PreparedScreenPublication {
    fn publication_mut(
        &mut self,
    ) -> Result<&mut ScreenBranchPublication, ScreenPublicationHubError> {
        self.publication
            .as_mut()
            .and_then(Arc::get_mut)
            .ok_or(ScreenPublicationHubError::PublicationReservationLost)
    }

    /// Exact descriptor copied into this reserved slot.
    #[must_use]
    pub fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.branch.descriptor
    }

    /// Worker plan generation whose authority must still match at finalize.
    #[must_use]
    pub fn worker_plan_generation(&self) -> ScreenPlanGeneration {
        self.binding.plan_generation()
    }

    /// Borrow the exact tightly packed mutable surface slot.
    ///
    /// The bytes cannot outlive this reservation and become immutable when the
    /// reservation is consumed by finalization.
    ///
    /// # Errors
    ///
    /// Rejects zone reservations and a reservation that lost unique ownership.
    pub fn surface_pixels_mut(&mut self) -> Result<&mut [u8], ScreenPublicationHubError> {
        let expected = descriptor_payload_kind(&self.branch.descriptor);
        let publication = self.publication_mut()?;
        publication.storage.surface_pixels_mut().ok_or(
            ScreenPublicationHubError::PayloadKindMismatch {
                expected,
                observed: ScreenPayloadKind::Surface,
            },
        )
    }

    /// Borrow the exact row-major mutable RGB zone slot.
    ///
    /// The colors cannot outlive this reservation and become immutable when
    /// the reservation is consumed by finalization.
    ///
    /// # Errors
    ///
    /// Rejects surface reservations and a reservation that lost unique
    /// ownership.
    pub fn zone_colors_mut(&mut self) -> Result<&mut [[u8; 3]], ScreenPublicationHubError> {
        let expected = descriptor_payload_kind(&self.branch.descriptor);
        let publication = self.publication_mut()?;
        publication.storage.zone_colors_mut().ok_or(
            ScreenPublicationHubError::PayloadKindMismatch {
                expected,
                observed: ScreenPayloadKind::Zones,
            },
        )
    }

    /// Select the compact row-major prefix published by a dynamic zone crop.
    ///
    /// The reserved allocation remains descriptor-sized. Only the effective
    /// shape and its exact color prefix become visible after finalization.
    ///
    /// # Errors
    ///
    /// Rejects surface reservations, lost unique ownership, arithmetic
    /// overflow, or a shape larger than the admitted descriptor capacity.
    pub fn set_effective_zone_shape(
        &mut self,
        columns: NonZeroU32,
        rows: NonZeroU32,
    ) -> Result<(), ScreenPublicationHubError> {
        let expected = descriptor_payload_kind(&self.branch.descriptor);
        let (maximum_columns, maximum_rows) = match self.branch.descriptor.kind() {
            ScreenPublicationKind::Zones { columns, rows } => (columns, rows),
            ScreenPublicationKind::Surface => {
                return Err(ScreenPublicationHubError::PayloadKindMismatch {
                    expected,
                    observed: ScreenPayloadKind::Zones,
                });
            }
        };
        if columns > maximum_columns || rows > maximum_rows {
            return Err(
                ScreenPublicationHubError::EffectiveZoneShapeExceedsDescriptor {
                    maximum_columns,
                    maximum_rows,
                    observed_columns: columns,
                    observed_rows: rows,
                },
            );
        }
        self.publication_mut()?
            .storage
            .set_effective_zone_shape(columns, rows)
    }
}

impl fmt::Debug for PreparedScreenPublication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedScreenPublication")
            .field("descriptor", &self.branch.descriptor)
            .field("worker_plan_generation", &self.binding.plan_generation())
            .field("slot_index", &self.slot_index)
            .finish_non_exhaustive()
    }
}

impl Drop for PreparedScreenPublication {
    fn drop(&mut self) {
        let Some(mut publication) = self.publication.take() else {
            return;
        };
        if let Some(publication) = Arc::get_mut(&mut publication) {
            drop(publication.storage.take_gpu_surface());
        }
        let mut runtime = self.branch.lock_runtime();
        if let Some(slot) = runtime.slots.get_mut(self.slot_index)
            && slot.is_none()
        {
            *slot = Some(publication);
        }
    }
}

/// Opaque proof that one committed branch accepted a typed publication.
#[derive(Debug)]
pub struct ScreenLiveBranchReceipt {
    state: Arc<ScreenCommittedState>,
    branch: Arc<ScreenBranchEntry>,
    publication: Arc<ScreenBranchPublication>,
}

impl ScreenLiveBranchReceipt {
    /// Exact live branch descriptor.
    #[must_use]
    pub fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.branch.descriptor
    }

    /// Accepted immutable publication slot.
    #[must_use]
    pub const fn publication(&self) -> &Arc<ScreenBranchPublication> {
        &self.publication
    }
}

/// Live continuity lease retaining actual branch and publication storage.
#[derive(Debug)]
pub struct ScreenContinuityLease {
    branch: ScreenBranchLease,
    publication: Arc<ScreenBranchPublication>,
}

impl ScreenContinuityLease {
    /// Exact live branch descriptor.
    #[must_use]
    pub fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        self.branch.descriptor()
    }

    /// Arc-backed publication retained until transition success or abort.
    #[must_use]
    pub const fn publication(&self) -> &Arc<ScreenBranchPublication> {
        &self.publication
    }

    /// Acquire staged from one committed old-plus-new authority snapshot.
    pub fn stage(
        self,
        staged: &ResolvedScreenPublicationDescriptor,
        hub: &ScreenPublicationHub,
    ) -> Result<ScreenTwoPlanContinuityLease, ScreenContinuityStageFailure> {
        loop {
            let state = hub.state.load_full();
            let validation = match state.branch(self.descriptor()) {
                None => Err(ScreenContinuityError::ActiveBranchMissing),
                Some(active) if !Arc::ptr_eq(&active.entry, &self.branch.branch) => {
                    Err(ScreenContinuityError::ActiveBranchNotLive)
                }
                Some(active) if active.entry.raw_latest().is_none() => {
                    Err(ScreenContinuityError::ActiveBranchNotLive)
                }
                Some(_) => state
                    .branch(staged)
                    .cloned()
                    .ok_or(ScreenContinuityError::StagedBranchMissing),
            };
            if !Arc::ptr_eq(&state, &hub.state.load_full()) {
                continue;
            }
            return match validation {
                Ok(staged_branch) => Ok(ScreenTwoPlanContinuityLease {
                    active: self,
                    staged: ScreenBranchLease {
                        branch: staged_branch.entry,
                        authority: Arc::clone(&hub.state),
                    },
                    overlap_state: state,
                }),
                Err(error) => Err(ScreenContinuityStageFailure {
                    error,
                    lease: Box::new(self),
                }),
            };
        }
    }
}

/// Structural old-plus-staged lease preventing release-before-acquire.
#[derive(Debug)]
pub struct ScreenTwoPlanContinuityLease {
    active: ScreenContinuityLease,
    staged: ScreenBranchLease,
    overlap_state: Arc<ScreenCommittedState>,
}

impl ScreenTwoPlanContinuityLease {
    /// Old live branch retained until activation succeeds.
    #[must_use]
    pub fn active_descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        self.active.descriptor()
    }

    /// Staged replacement branch.
    #[must_use]
    pub fn staged_descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        self.staged.descriptor()
    }

    /// Committed generation containing both branches.
    #[must_use]
    pub fn overlap_generation(&self) -> ScreenPlanGeneration {
        self.overlap_state.plan.generation()
    }

    /// Switch only with a current receipt from this exact overlap snapshot.
    pub fn activate(
        self,
        receipt: &ScreenLiveBranchReceipt,
    ) -> Result<ScreenContinuityLease, ScreenContinuityActivationFailure> {
        loop {
            let state = self.staged.authority.load_full();
            let validation = if !Arc::ptr_eq(&state, &self.overlap_state)
                || !Arc::ptr_eq(&receipt.state, &self.overlap_state)
            {
                Err(ScreenContinuityError::OverlapGenerationMismatch)
            } else if state
                .branch(self.active_descriptor())
                .is_none_or(|active| !Arc::ptr_eq(&active.entry, &self.active.branch.branch))
                || self.active.branch.branch.raw_latest().is_none()
            {
                Err(ScreenContinuityError::ActiveBranchNotLive)
            } else if state
                .branch(self.staged_descriptor())
                .is_none_or(|staged| !Arc::ptr_eq(&staged.entry, &self.staged.branch))
            {
                Err(ScreenContinuityError::StagedBranchNotLive)
            } else if !Arc::ptr_eq(&receipt.branch, &self.staged.branch) {
                Err(ScreenContinuityError::StagedReceiptMismatch)
            } else if self
                .staged
                .branch
                .raw_latest()
                .is_none_or(|latest| !Arc::ptr_eq(&latest, &receipt.publication))
            {
                Err(ScreenContinuityError::StagedBranchNotLive)
            } else {
                Ok(())
            };
            if !Arc::ptr_eq(&state, &self.staged.authority.load_full()) {
                continue;
            }
            if let Err(error) = validation {
                return Err(ScreenContinuityActivationFailure {
                    error,
                    transition: Box::new(self),
                });
            }
            return Ok(ScreenContinuityLease {
                branch: self.staged,
                publication: Arc::clone(&receipt.publication),
            });
        }
    }

    /// Abandon staged while retaining the old live lease and storage.
    #[must_use]
    pub fn abort(self) -> ScreenContinuityLease {
        self.active
    }
}

/// Failed staging that returns the still-live original lease.
#[derive(Debug)]
pub struct ScreenContinuityStageFailure {
    error: ScreenContinuityError,
    lease: Box<ScreenContinuityLease>,
}

impl ScreenContinuityStageFailure {
    /// Exact structural failure.
    #[must_use]
    pub const fn error(&self) -> ScreenContinuityError {
        self.error
    }

    /// Recover the still-live original lease and publication storage.
    #[must_use]
    pub fn into_lease(self) -> ScreenContinuityLease {
        *self.lease
    }
}

/// Failed activation retaining old and staged branch handles.
#[derive(Debug)]
pub struct ScreenContinuityActivationFailure {
    error: ScreenContinuityError,
    transition: Box<ScreenTwoPlanContinuityLease>,
}

impl ScreenContinuityActivationFailure {
    /// Exact activation failure.
    #[must_use]
    pub const fn error(&self) -> ScreenContinuityError {
        self.error
    }

    /// Recover the old-plus-staged transition for retry or abort.
    #[must_use]
    pub fn into_transition(self) -> ScreenTwoPlanContinuityLease {
        *self.transition
    }
}

/// Structural continuity authority failure.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ScreenContinuityError {
    /// Committed overlap no longer retains the active branch.
    #[error("committed overlap does not contain the active continuity branch")]
    ActiveBranchMissing,
    /// Active branch no longer has a deliverable publication.
    #[error("active continuity branch is no longer live")]
    ActiveBranchNotLive,
    /// Committed overlap does not contain the staged branch.
    #[error("committed overlap does not contain the staged continuity branch")]
    StagedBranchMissing,
    /// Receipt came from another committed authority snapshot.
    #[error("live receipt was not issued from the committed overlap snapshot")]
    OverlapGenerationMismatch,
    /// Receipt belongs to another exact staged descriptor.
    #[error("live receipt does not belong to the staged continuity branch")]
    StagedReceiptMismatch,
    /// Staged receipt has been superseded or retired.
    #[error("staged continuity branch is not currently live")]
    StagedBranchNotLive,
}

/// Publication authority, shape, sequencing, or bounded-pressure failure.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ScreenPublicationHubError {
    /// Descriptor is absent from committed authority.
    #[error("screen publication branch is absent from committed authority")]
    BranchMissing {
        /// Exact missing descriptor.
        descriptor: Arc<ResolvedScreenPublicationDescriptor>,
    },
    /// Descriptor remains pending its first publication.
    #[error("screen publication branch is pending its first live publication")]
    BranchPending {
        /// Exact pending descriptor.
        descriptor: Arc<ResolvedScreenPublicationDescriptor>,
    },
    /// Descriptor retired and rejects delivery.
    #[error("screen publication branch has retired")]
    BranchRetired {
        /// Exact retired descriptor.
        descriptor: Arc<ResolvedScreenPublicationDescriptor>,
    },
    /// Publisher no longer names an entry in current authority.
    #[error("screen publisher is stale: expected {expected:?}, observed {observed:?}")]
    PublisherStale {
        /// Current committed plan generation.
        expected: ScreenPlanGeneration,
        /// Publisher worker generation.
        observed: ScreenPlanGeneration,
    },
    /// Caller substituted another opaque worker binding.
    #[error("worker binding does not own the requested publication branch")]
    WorkerBindingMismatch {
        /// Exact requested descriptor.
        descriptor: Arc<ResolvedScreenPublicationDescriptor>,
    },
    /// Payload kind differs from the committed descriptor.
    #[error("publication payload kind mismatch: expected {expected:?}, observed {observed:?}")]
    PayloadKindMismatch {
        /// Descriptor-required kind.
        expected: ScreenPayloadKind,
        /// Submitted kind.
        observed: ScreenPayloadKind,
    },
    /// Submitted storage residency differs from the committed descriptor.
    #[error("publication residency mismatch: expected {expected:?}, observed {observed:?}")]
    ResidencyMismatch {
        /// Residency required by the committed source and branch kind.
        expected: ScreenPublicationResidency,
        /// Residency supplied by the worker.
        observed: ScreenPublicationResidency,
    },
    /// Surface extent differs from the committed descriptor.
    #[error("surface extent mismatch: expected {expected:?}, observed {observed:?}")]
    SurfaceExtentMismatch {
        /// Descriptor output extent.
        expected: PixelExtent,
        /// Submitted extent.
        observed: PixelExtent,
    },
    /// Surface storage length is not exact tightly packed RGBA8.
    #[error("surface byte length mismatch: expected {expected}, observed {observed}")]
    SurfaceByteLengthMismatch {
        /// Required byte length.
        expected: usize,
        /// Submitted byte length.
        observed: usize,
    },
    /// Surface packed pixel order differs from the descriptor target.
    #[error("surface pixel format mismatch: expected {expected:?}, observed {observed:?}")]
    SurfacePixelFormatMismatch {
        /// Descriptor target format.
        expected: CapturePixelFormat,
        /// Submitted format.
        observed: CapturePixelFormat,
    },
    /// Submitted primaries or transfer differ from the descriptor target.
    #[error("publication colorimetry mismatch: expected {expected:?}, observed {observed:?}")]
    ColorimetryMismatch {
        /// Descriptor target colorimetry.
        expected: ScreenPublicationColorimetry,
        /// Submitted colorimetry.
        observed: ScreenPublicationColorimetry,
    },
    /// Zone grid shape differs from the committed descriptor.
    #[error(
        "zone shape mismatch: expected {expected_columns}x{expected_rows}, observed {observed_columns}x{observed_rows}"
    )]
    ZoneShapeMismatch {
        /// Descriptor columns.
        expected_columns: NonZeroU32,
        /// Descriptor rows.
        expected_rows: NonZeroU32,
        /// Submitted columns.
        observed_columns: NonZeroU32,
        /// Submitted rows.
        observed_rows: NonZeroU32,
    },
    /// Dynamic crop shape exceeds the descriptor-sized zone allocation.
    #[error(
        "effective zone shape exceeds descriptor: maximum {maximum_columns}x{maximum_rows}, observed {observed_columns}x{observed_rows}"
    )]
    EffectiveZoneShapeExceedsDescriptor {
        /// Descriptor columns.
        maximum_columns: NonZeroU32,
        /// Descriptor rows.
        maximum_rows: NonZeroU32,
        /// Requested effective columns.
        observed_columns: NonZeroU32,
        /// Requested effective rows.
        observed_rows: NonZeroU32,
    },
    /// Dynamic crop color prefix exceeds the admitted zone allocation.
    #[error("effective zone shape {columns}x{rows} exceeds admitted color capacity {capacity}")]
    EffectiveZoneShapeExceedsCapacity {
        /// Requested effective columns.
        columns: NonZeroU32,
        /// Requested effective rows.
        rows: NonZeroU32,
        /// Number of admitted row-major colors.
        capacity: usize,
    },
    /// Zone color count differs from the submitted shape.
    #[error("zone color count mismatch: expected {expected}, observed {observed}")]
    ZoneColorCountMismatch {
        /// Required color count.
        expected: usize,
        /// Submitted color count.
        observed: usize,
    },
    /// Shape arithmetic cannot be represented by this process.
    #[error("publication payload shape exceeds process addressability")]
    PayloadShapeOverflow,
    /// Submitted source epoch differs from the committed descriptor.
    #[error("publication source epoch does not match the committed descriptor")]
    SourceEpochMismatch,
    /// Submitted worker plan differs from the opaque binding.
    #[error("publication worker plan mismatch: expected {expected:?}, observed {observed:?}")]
    WorkerPlanGenerationMismatch {
        /// Binding plan generation.
        expected: ScreenPlanGeneration,
        /// Submitted worker plan generation.
        observed: ScreenPlanGeneration,
    },
    /// Capture/publish/freshness timestamps move backwards.
    #[error("publication timeline moves backwards")]
    InvalidPublicationTimeline,
    /// Completed metadata was supplied to a writable reservation.
    #[error("writable publication intent already contains completion metadata")]
    PublicationCompletionAlreadySet,
    /// Finalization was attempted before publication completion was supplied.
    #[error("publication completion metadata is missing")]
    PublicationCompletionMissing,
    /// One atomic batch mixed reservations from distinct source workers.
    #[error("publication batch contains more than one worker binding")]
    PublicationBatchBindingMismatch,
    /// Reducer work completed after the source freshness deadline.
    #[error("publication completed after its freshness deadline")]
    PublicationFreshnessExpired,
    /// Native source sequence was duplicated or moved backwards.
    #[error("native source sequence must advance: previous {previous}, observed {observed}")]
    NativeSequenceNotMonotonic {
        /// Last accepted native sequence.
        previous: NonZeroU64,
        /// Rejected sequence.
        observed: NonZeroU64,
    },
    /// Readers, weak references, or prepared reservations retain every slot.
    #[error("all {admitted_slots} admitted publication slots are retained or reserved")]
    PublicationPressure {
        /// Fixed admitted slot count.
        admitted_slots: u32,
    },
    /// Reserved slot no longer occupies its exact pool position.
    #[error("prepared publication slot reservation was lost")]
    PublicationReservationLost,
    /// Branch-local accepted sequence space is exhausted.
    #[error("screen publication branch sequence exhausted")]
    BranchSequenceExhausted,
}

fn validate_payload(
    descriptor: &ResolvedScreenPublicationDescriptor,
    payload: ScreenBranchPayload<'_>,
) -> Result<(), ScreenPublicationHubError> {
    let expected_residency = descriptor.required_residency();
    let observed_residency = payload.residency();
    if expected_residency != observed_residency {
        return Err(ScreenPublicationHubError::ResidencyMismatch {
            expected: expected_residency,
            observed: observed_residency,
        });
    }
    match (descriptor.kind(), payload) {
        (ScreenPublicationKind::Surface, ScreenBranchPayload::Surface(surface)) => {
            let expected = descriptor.geometry().output_extent();
            if surface.extent() != expected {
                return Err(ScreenPublicationHubError::SurfaceExtentMismatch {
                    expected,
                    observed: surface.extent(),
                });
            }
            let expected_format = descriptor.physical().target_pixel_format();
            if surface.pixel_format() != expected_format {
                return Err(ScreenPublicationHubError::SurfacePixelFormatMismatch {
                    expected: expected_format,
                    observed: surface.pixel_format(),
                });
            }
            validate_colorimetry(descriptor, surface.colorimetry())?;
        }
        (ScreenPublicationKind::Surface, ScreenBranchPayload::GpuSurface(surface)) => {
            let expected = descriptor.geometry().output_extent();
            if surface.surface().extent() != expected {
                return Err(ScreenPublicationHubError::SurfaceExtentMismatch {
                    expected,
                    observed: surface.surface().extent(),
                });
            }
            let expected_format = descriptor.physical().target_pixel_format();
            if surface.surface().format() != expected_format {
                return Err(ScreenPublicationHubError::SurfacePixelFormatMismatch {
                    expected: expected_format,
                    observed: surface.surface().format(),
                });
            }
            validate_colorimetry(descriptor, surface.colorimetry())?;
        }
        (ScreenPublicationKind::Zones { columns, rows }, ScreenBranchPayload::Zones(zones)) => {
            if zones.columns() != columns || zones.rows() != rows {
                return Err(ScreenPublicationHubError::ZoneShapeMismatch {
                    expected_columns: columns,
                    expected_rows: rows,
                    observed_columns: zones.columns(),
                    observed_rows: zones.rows(),
                });
            }
            validate_colorimetry(descriptor, zones.colorimetry())?;
        }
        (ScreenPublicationKind::Surface, observed) => {
            return Err(ScreenPublicationHubError::PayloadKindMismatch {
                expected: ScreenPayloadKind::Surface,
                observed: observed.kind(),
            });
        }
        (ScreenPublicationKind::Zones { .. }, observed) => {
            return Err(ScreenPublicationHubError::PayloadKindMismatch {
                expected: ScreenPayloadKind::Zones,
                observed: observed.kind(),
            });
        }
    }
    Ok(())
}

fn validate_payload_kind(
    descriptor: &ResolvedScreenPublicationDescriptor,
    observed: ScreenPayloadKind,
) -> Result<(), ScreenPublicationHubError> {
    let expected = descriptor_payload_kind(descriptor);
    if observed == expected {
        Ok(())
    } else {
        Err(ScreenPublicationHubError::PayloadKindMismatch { expected, observed })
    }
}

fn descriptor_payload_kind(descriptor: &ResolvedScreenPublicationDescriptor) -> ScreenPayloadKind {
    match descriptor.kind() {
        ScreenPublicationKind::Surface => ScreenPayloadKind::Surface,
        ScreenPublicationKind::Zones { .. } => ScreenPayloadKind::Zones,
    }
}

fn validate_metadata(
    branch: &ScreenBranchEntry,
    binding: &ScreenWorkerBinding,
    metadata: &ScreenPublicationMetadata,
) -> Result<(), ScreenPublicationHubError> {
    validate_publication_intent(branch, binding, metadata)?;
    let completion = metadata
        .completion
        .ok_or(ScreenPublicationHubError::PublicationCompletionMissing)?;
    if completion.published_at < metadata.captured_at {
        return Err(ScreenPublicationHubError::InvalidPublicationTimeline);
    }
    if completion.published_at > metadata.freshness_deadline {
        return Err(ScreenPublicationHubError::PublicationFreshnessExpired);
    }
    Ok(())
}

fn validate_publication_intent(
    branch: &ScreenBranchEntry,
    binding: &ScreenWorkerBinding,
    metadata: &ScreenPublicationMetadata,
) -> Result<(), ScreenPublicationHubError> {
    if metadata.source_epoch != *branch.descriptor.source_epoch() {
        return Err(ScreenPublicationHubError::SourceEpochMismatch);
    }
    if metadata.worker_plan_generation != binding.plan_generation() {
        return Err(ScreenPublicationHubError::WorkerPlanGenerationMismatch {
            expected: binding.plan_generation(),
            observed: metadata.worker_plan_generation,
        });
    }
    Ok(())
}

fn validate_colorimetry(
    descriptor: &ResolvedScreenPublicationDescriptor,
    observed: ScreenPublicationColorimetry,
) -> Result<(), ScreenPublicationHubError> {
    let expected = descriptor_colorimetry(descriptor);
    if observed == expected {
        Ok(())
    } else {
        Err(ScreenPublicationHubError::ColorimetryMismatch { expected, observed })
    }
}

fn descriptor_colorimetry(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> ScreenPublicationColorimetry {
    ScreenPublicationColorimetry::new(descriptor.physical().color_pipeline().output())
}

fn surface_byte_len(extent: PixelExtent) -> Result<usize, ScreenPublicationHubError> {
    usize::try_from(extent.width())
        .ok()
        .and_then(|width| {
            usize::try_from(extent.height())
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(SURFACE_PIXEL_BYTES as usize))
        .ok_or(ScreenPublicationHubError::PayloadShapeOverflow)
}

fn zone_count(columns: NonZeroU32, rows: NonZeroU32) -> Result<usize, ScreenPublicationHubError> {
    usize::try_from(columns.get())
        .ok()
        .and_then(|columns| {
            usize::try_from(rows.get())
                .ok()
                .and_then(|rows| columns.checked_mul(rows))
        })
        .ok_or(ScreenPublicationHubError::PayloadShapeOverflow)
}

fn checked_surface_byte_len(extent: PixelExtent) -> Result<usize, ScreenPlanError> {
    surface_byte_len(extent).map_err(|_| ScreenPlanError::AllocationFailed)
}

fn checked_zone_count(columns: NonZeroU32, rows: NonZeroU32) -> Result<usize, ScreenPlanError> {
    zone_count(columns, rows).map_err(|_| ScreenPlanError::AllocationFailed)
}

fn descriptor_payload_bytes(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<u64, ScreenPlanError> {
    let bytes = match descriptor.kind() {
        ScreenPublicationKind::Surface
            if matches!(
                descriptor.required_residency(),
                ScreenPublicationResidency::Cpu
            ) =>
        {
            checked_surface_byte_len(descriptor.geometry().output_extent())?
        }
        ScreenPublicationKind::Surface => 0,
        ScreenPublicationKind::Zones { columns, rows } => checked_zone_count(columns, rows)?
            .checked_mul(ZONE_COLOR_BYTES as usize)
            .ok_or(ScreenPlanError::AllocationFailed)?,
    };
    u64::try_from(bytes).map_err(|_| ScreenPlanError::AllocationFailed)
}

fn try_zeroed_slice(len: usize) -> Result<Box<[u8]>, ScreenPlanError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(len)
        .map_err(|_| ScreenPlanError::AllocationFailed)?;
    values.resize(len, 0);
    Ok(values.into_boxed_slice())
}

fn try_zeroed_color_slice(len: usize) -> Result<Box<[[u8; 3]]>, ScreenPlanError> {
    let byte_len = len
        .checked_mul(ZONE_COLOR_BYTES as usize)
        .ok_or(ScreenPlanError::AllocationFailed)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(len)
        .map_err(|_| ScreenPlanError::AllocationFailed)?;
    values.resize(len, [0; 3]);
    debug_assert_eq!(values.len().saturating_mul(3), byte_len);
    Ok(values.into_boxed_slice())
}
