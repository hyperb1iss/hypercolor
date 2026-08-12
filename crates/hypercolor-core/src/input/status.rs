//! Non-blocking latest-value reads for source health and sample freshness.

use arc_swap::{ArcSwap, ArcSwapOption};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::watch;

const SAMPLE_SLOT_COUNT: usize = 3;
// Token layout: [61-bit generation | observed-stale | 2-bit slot index].
const SAMPLE_TOKEN_INDEX_BITS: u32 = 2;
const SAMPLE_TOKEN_INDEX_MASK: u64 = (1 << SAMPLE_TOKEN_INDEX_BITS) - 1;
const SAMPLE_TOKEN_STALE_BIT: u64 = 1 << SAMPLE_TOKEN_INDEX_BITS;
const SAMPLE_TOKEN_GENERATION_SHIFT: u32 = SAMPLE_TOKEN_INDEX_BITS + 1;

/// Broad input-source category used by diagnostics consumers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceKind {
    /// Audio capture or analysis.
    Audio,
    /// Screen or window capture.
    Screen,
    /// Keyboard, pointer, or other interaction capture.
    Interaction,
    /// Media metadata or artwork.
    Media,
    /// Network-delivered input.
    Network,
}

/// Current lifecycle health of an input source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceState {
    /// The source is intentionally inactive.
    Stopped,
    /// A new source session is being established.
    Starting,
    /// The source is healthy and producing data.
    Live,
    /// The source is usable with reduced fidelity.
    Degraded,
    /// The source cannot currently be used.
    Unavailable,
    /// The source session failed terminally.
    Failed,
}

/// Freshness of the latest source data, independent of lifecycle health.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceFreshness {
    /// The source does not publish sampled data with freshness deadlines.
    NotApplicable,
    /// A sampled source session has not produced its first sample yet.
    AwaitingSample,
    /// The latest sample remains inside its freshness deadline.
    Fresh,
    /// The latest sample reached or exceeded its freshness deadline.
    Stale,
}

/// Timestamp field rejected by the atomic sample-plane encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceTimestampField {
    /// Sample capture time.
    SampledAt,
    /// Sample freshness deadline.
    FreshnessDeadline,
}

/// Structured input-source problem details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIssue {
    /// Stable machine-readable issue code.
    pub code: Arc<str>,
    /// Human-readable problem description.
    pub message: Arc<str>,
    /// Optional suggested remediation.
    pub remediation: Option<Arc<str>>,
    /// Whether retrying without a configuration change may recover the source.
    pub retryable: bool,
}

impl SourceIssue {
    /// Create issue details without a remediation hint.
    #[must_use]
    pub fn new(code: impl Into<Arc<str>>, message: impl Into<Arc<str>>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            remediation: None,
            retryable,
        }
    }

    /// Attach a remediation hint.
    #[must_use]
    pub fn with_remediation(mut self, remediation: impl Into<Arc<str>>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }
}

/// Precise lifecycle state for one protected macOS capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacosProtectedSourceState {
    /// Configuration disables the capability.
    Disabled,
    /// The next transition requires an explicit local authorization action.
    NeedsUserAction,
    /// The user denied the requested authorization.
    PermissionDenied,
    /// Authorization is present but the owning process must restart.
    NeedsProcessRestart,
    /// Screen capture requires a source choice from Apple's system picker.
    NeedsSelection,
    /// The capability is authorized and selected but has no active demand.
    ReadyIdle,
    /// Native resources are being established.
    Starting,
    /// The capability is active and producing data.
    Live,
    /// Native delivery stopped transiently and recovery is pending.
    Interrupted,
    /// A previously usable authorization was revoked.
    Revoked,
    /// The current configuration failed terminally.
    Failed,
}

/// TCC authorization evidence for one macOS protected resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacosAuthorizationState {
    /// The adapter has not queried authorization yet.
    Unknown,
    /// No positive grant or explicit denial has been observed.
    NotDetermined,
    /// The user explicitly denied the request.
    Denied,
    /// The current capability owner has positive grant evidence.
    Authorized,
}

/// Process topology that owns a protected macOS capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacosCapabilityOwner {
    /// Daemon process embedded as an app sidecar.
    AppSidecar,
    /// Main application process.
    App,
    /// Direct user launchd service.
    LaunchdService,
    /// Homebrew-managed user service.
    HomebrewService,
    /// Authenticated app broker.
    Broker,
    /// Terminal-launched daemon.
    Standalone,
}

/// Bounded record of two macOS daemon topologies contending for ownership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacosDaemonOwnerConflict {
    /// Topology currently holding the process guard.
    pub active: MacosCapabilityOwner,
    /// Topology that attempted to start.
    pub contender: MacosCapabilityOwner,
    /// Unix timestamp of the observed conflict.
    pub observed_at_ms: u64,
}

/// Native architecture of the active macOS host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacosArchitecture {
    /// Apple Silicon host.
    AppleSilicon,
    /// Intel host.
    Intel,
}

/// Runtime Tahoe feature probes stable for one process and Metal device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacosTahoeCapabilities {
    /// Native host architecture, independent of the running executable slice.
    pub host_architecture: MacosArchitecture,
    /// Whether this process runs under Rosetta translation.
    pub translated_process: bool,
    /// Whether the Tahoe Core Graphics tone-mapping API is callable.
    pub content_tone_mapping_info: bool,
    /// Whether the active Metal device exposes every required Metal 4 facility.
    pub metal4: bool,
}

/// Tahoe capabilities resolved for one selected capture incarnation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacosTahoeSelectionCapabilities {
    /// Stable selected source identity.
    pub source_id: Arc<str>,
    /// Capture session generation that proved these capabilities.
    pub capture_session_generation: u64,
    /// Whether the selected stream delivered canonical HDR.
    pub hdr_capture: bool,
    /// Whether paired SDR and HDR diagnostic screenshots are available.
    pub dual_range_screenshots: bool,
}

/// Persistability and content style of the current macOS screen selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MacosSelectionState {
    /// No source is currently selected.
    None,
    /// A stable display source is selected.
    Display {
        /// Canonical display UUID source identity.
        source_id: Arc<str>,
    },
    /// A window, application, or multi-window choice valid for this process.
    SessionScoped {
        /// Redacted content style suitable for diagnostics.
        content_style: Arc<str>,
    },
}

/// Platform detail for the macOS host-input adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacosInputPlatformStatus {
    /// Keyboard capture lifecycle.
    pub keyboard: MacosProtectedSourceState,
    /// Pointer capture lifecycle.
    pub pointer: MacosProtectedSourceState,
    /// Input Monitoring authorization evidence.
    pub keyboard_tcc: MacosAuthorizationState,
    /// Process topology owning keyboard capture.
    pub keyboard_owner: MacosCapabilityOwner,
    /// Process topology owning pointer capture.
    pub pointer_owner: MacosCapabilityOwner,
    /// Latest daemon-owner conflict, when one exists.
    pub owner_conflict: Option<Arc<MacosDaemonOwnerConflict>>,
}

/// Platform detail for the macOS screen-capture adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacosScreenPlatformStatus {
    /// Screen capture lifecycle.
    pub state: MacosProtectedSourceState,
    /// Screen Recording authorization evidence.
    pub tcc: MacosAuthorizationState,
    /// Process topology owning ScreenCaptureKit.
    pub owner: MacosCapabilityOwner,
    /// Current system-picker selection.
    pub selection: MacosSelectionState,
    /// Process-stable Tahoe host and active Metal-device capabilities.
    pub tahoe: MacosTahoeCapabilities,
    /// Tahoe capabilities for the active selected stream.
    pub tahoe_selection: Option<MacosTahoeSelectionCapabilities>,
    /// Latest daemon-owner conflict, when one exists.
    pub owner_conflict: Option<Arc<MacosDaemonOwnerConflict>>,
}

/// Platform-specific detail attached to a generic input-source status.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SourcePlatformStatus {
    /// macOS host-input state.
    MacosInput(MacosInputPlatformStatus),
    /// macOS screen-capture state.
    MacosScreen(MacosScreenPlatformStatus),
}

/// Screen-capture reduction implementation reported by source diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenCaptureReductionPath {
    /// GPU-native reduction and reduced-surface readback.
    Gpu,
    /// Full-quality CPU reduction after native readback.
    CpuFallback,
}

/// Latest-value screen-capture reduction counters for one source session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenCaptureDiagnostics {
    /// Active reduction implementation.
    pub reduction_path: ScreenCaptureReductionPath,
    /// GPU reductions submitted to the native context.
    pub gpu_submitted: u64,
    /// GPU reductions delivered to the CPU.
    pub gpu_completed: u64,
    /// Frames delivered through CPU fallback.
    pub cpu_completed: u64,
    /// Submissions coalesced while every readback slot was occupied.
    pub ring_busy: u64,
    /// Bytes copied from reduced GPU staging surfaces.
    pub readback_bytes: u64,
    /// GPU initialization or execution failures observed by this session.
    pub gpu_failures: u64,
}

/// Backend-specific latest-value diagnostics attached to a source session.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SourceDiagnostics {
    /// Screen-capture reduction health and throughput.
    ScreenCapture(ScreenCaptureDiagnostics),
}

/// Platform-neutral health result for a backend resource scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceResourceScanHealth {
    /// At least one eligible resource is open and usable.
    Live {
        /// Number of open resources.
        resource_count: usize,
        /// Number of eligible resources rejected for access denial.
        denied_resource_count: usize,
    },
    /// Some eligible resources are usable while others failed to open.
    Degraded {
        /// Number of open resources that remain usable.
        resource_count: usize,
        /// Number of eligible resources rejected for access denial.
        denied_resource_count: usize,
        /// Structured details for the partial failure.
        issue: SourceIssue,
    },
    /// No eligible resource can currently produce input.
    Unavailable {
        /// Number of open resources, normally zero for unavailable health.
        resource_count: usize,
        /// Number of eligible resources rejected for access denial.
        denied_resource_count: usize,
        /// Structured details for the unavailable state.
        issue: SourceIssue,
    },
}

/// Classify backend resource counts without depending on platform APIs.
#[must_use]
pub fn classify_source_resource_scan(
    opened: usize,
    access_denied: usize,
    transient_failures: usize,
) -> SourceResourceScanHealth {
    if opened > 0 && access_denied > 0 {
        let failure_suffix = if transient_failures == 0 {
            String::new()
        } else {
            format!("; {transient_failures} failed to open")
        };
        return SourceResourceScanHealth::Degraded {
            resource_count: opened,
            denied_resource_count: access_denied,
            issue: SourceIssue::new(
                "access_denied",
                format!(
                    "{opened} input resource(s) are usable; {access_denied} are unreadable{failure_suffix}"
                ),
                true,
            )
            .with_remediation("run `just udev-install` and replug or re-login"),
        };
    }
    if opened > 0 && transient_failures > 0 {
        return SourceResourceScanHealth::Degraded {
            resource_count: opened,
            denied_resource_count: 0,
            issue: SourceIssue::new(
                "input_resource_scan_partial_failure",
                format!(
                    "{opened} input resource(s) are usable; {transient_failures} failed to open"
                ),
                true,
            ),
        };
    }
    if opened > 0 {
        return SourceResourceScanHealth::Live {
            resource_count: opened,
            denied_resource_count: 0,
        };
    }
    if access_denied > 0 {
        let failure_suffix = if transient_failures == 0 {
            String::new()
        } else {
            format!("; {transient_failures} failed to open")
        };
        return SourceResourceScanHealth::Unavailable {
            resource_count: 0,
            denied_resource_count: access_denied,
            issue: SourceIssue::new(
                "access_denied",
                format!("{access_denied} input resource(s) are unreadable{failure_suffix}"),
                true,
            )
            .with_remediation("run `just udev-install` and replug or re-login"),
        };
    }
    if transient_failures > 0 {
        return SourceResourceScanHealth::Unavailable {
            resource_count: 0,
            denied_resource_count: 0,
            issue: SourceIssue::new(
                "input_resource_scan_failed",
                format!("{transient_failures} input resource(s) failed to open"),
                true,
            ),
        };
    }
    SourceResourceScanHealth::Unavailable {
        resource_count: 0,
        denied_resource_count: 0,
        issue: SourceIssue::new(
            "input_resources_unavailable",
            "no eligible input resources are available",
            true,
        ),
    }
}

/// One-shot guard for terminal worker failure probes.
#[derive(Default)]
pub struct TerminalFailureLatch {
    latched: bool,
}

impl TerminalFailureLatch {
    /// Probe until the first terminal failure, then skip every later probe.
    pub fn take<T>(&mut self, probe: impl FnOnce() -> Option<T>) -> Option<T> {
        if self.latched {
            return None;
        }
        let failure = probe()?;
        self.latched = true;
        Some(failure)
    }

    /// Re-arm the latch for a newly created worker session.
    pub fn reset(&mut self) {
        self.latched = false;
    }

    /// Whether a terminal failure has already been observed for this session.
    #[must_use]
    pub const fn is_latched(&self) -> bool {
        self.latched
    }
}

/// Immutable latest-value health snapshot for one input source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceStatus {
    /// Stable source identity within the input graph.
    pub source_id: Arc<str>,
    /// Broad source category.
    pub kind: SourceKind,
    /// Active or selected backend name.
    pub backend: Arc<str>,
    /// Whether configuration enables the source.
    pub configured: bool,
    /// Whether the user has consented to capture.
    pub consented: bool,
    /// Whether the current render graph demands source data.
    pub demanded: bool,
    /// Number of committed consumers currently reading this source domain.
    pub active_consumer_count: usize,
    /// Lifecycle health, independent of sample freshness.
    pub state: SourceState,
    /// Freshness of the latest sampled data.
    pub freshness: SourceFreshness,
    /// Generation of the input graph that owns this source session.
    pub source_graph_generation: u64,
    /// Generation of the most recently started source session.
    pub session_generation: u64,
    /// Monotonic time of the last accepted sample.
    pub last_sample_at: Option<Instant>,
    /// Monotonic deadline after which the latest sample becomes stale.
    pub freshness_deadline: Option<Instant>,
    /// Number of live backend resources owned by the source.
    pub resource_count: usize,
    /// Number of eligible backend resources rejected for access denial.
    pub denied_resource_count: usize,
    /// Structured lifecycle-health problem details.
    pub issue: Option<SourceIssue>,
    /// Structured freshness problem details.
    pub freshness_issue: Option<SourceIssue>,
    /// Platform-specific state clients must not infer from generic health.
    pub platform: Option<Arc<SourcePlatformStatus>>,
    /// Whether the source was permanently removed from its owning graph.
    pub retired: bool,
}

impl SourceStatus {
    fn stopped(
        source_id: Arc<str>,
        kind: SourceKind,
        backend: Arc<str>,
        configured: bool,
        consented: bool,
        demanded: bool,
    ) -> Self {
        Self {
            source_id,
            kind,
            backend,
            configured,
            consented,
            demanded,
            active_consumer_count: 0,
            state: SourceState::Stopped,
            freshness: SourceFreshness::NotApplicable,
            source_graph_generation: 0,
            session_generation: 0,
            last_sample_at: None,
            freshness_deadline: None,
            resource_count: 0,
            denied_resource_count: 0,
            issue: None,
            freshness_issue: None,
            platform: None,
            retired: false,
        }
    }

    fn eligible(&self) -> bool {
        self.configured && self.consented && self.demanded
    }
}

pub(crate) struct SourceStatusPolicy {
    configured: bool,
    consented: bool,
    demanded: bool,
}

impl SourceStatusPolicy {
    pub(crate) const fn new(configured: bool, consented: bool, demanded: bool) -> Self {
        Self {
            configured,
            consented,
            demanded,
        }
    }
}

/// Rejected source-status control transition.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum SourceStatusError {
    /// The source was removed from its owning graph.
    #[error("source status publisher is retired")]
    Retired,
    /// The source is not configured, consented, and demanded.
    #[error(
        "source policy is ineligible (configured={configured}, consented={consented}, demanded={demanded})"
    )]
    Ineligible {
        /// Whether configuration enables the source.
        configured: bool,
        /// Whether the user consented to capture.
        consented: bool,
        /// Whether the render graph demands the source.
        demanded: bool,
    },
    /// A graph generation was not strictly newer than the published generation.
    #[error("source graph generation did not advance from {current} to {requested}")]
    GraphGenerationNotAdvanced {
        /// Currently published graph generation.
        current: u64,
        /// Rejected graph generation.
        requested: u64,
    },
    /// A sample deadline was not later than its capture time.
    #[error("source freshness deadline must be later than the sample time")]
    InvalidFreshnessDeadline {
        /// Monotonic sample capture time.
        sampled_at: Instant,
        /// Rejected monotonic freshness deadline.
        freshness_deadline: Instant,
    },
    /// A timestamp falls before the source epoch or outside its encoded range.
    #[error("source {field:?} timestamp is outside the encodable monotonic range")]
    TimestampOutOfRange {
        /// Rejected timestamp field.
        field: SourceTimestampField,
        /// Rejected monotonic timestamp.
        timestamp: Instant,
    },
}

struct SourceStatusShared {
    latest: ArcSwap<SourceStatus>,
    control: Mutex<SourceStatusControl>,
    structural: watch::Sender<Arc<SourceStatus>>,
    sample_signals: watch::Sender<u64>,
    samples: SampleBuffer,
    diagnostics: ArcSwapOption<SourceDiagnostics>,
}

struct SourceStatusControl {
    active_session: Option<u64>,
    next_session: u64,
    sample_signal_generation: u64,
}

struct SampleSlot {
    session_generation: AtomicU64,
    sampled_tick: AtomicU64,
    deadline_tick: AtomicU64,
    resource_count: AtomicUsize,
}

impl SampleSlot {
    fn new() -> Self {
        Self {
            session_generation: AtomicU64::new(0),
            sampled_tick: AtomicU64::new(0),
            deadline_tick: AtomicU64::new(0),
            resource_count: AtomicUsize::new(0),
        }
    }
}

struct SampleBuffer {
    epoch: Instant,
    active_token: AtomicU64,
    slots: [SampleSlot; SAMPLE_SLOT_COUNT],
}

#[derive(Clone, Copy)]
struct SamplePublication {
    token: u64,
    session_generation: u64,
    sampled_at: Option<Instant>,
    freshness_deadline: Option<Instant>,
    resource_count: usize,
}

impl SampleBuffer {
    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            active_token: AtomicU64::new(0),
            slots: std::array::from_fn(|_| SampleSlot::new()),
        }
    }

    fn publish(
        &self,
        session_generation: u64,
        sampled_tick: Option<u64>,
        deadline_tick: Option<u64>,
        resource_count: usize,
    ) -> u64 {
        let active = self.active_token.load(Ordering::Relaxed);
        let active_index = token_index(active);
        let next_index = (active_index + 1) % SAMPLE_SLOT_COUNT;
        let next_generation = token_generation(active)
            .checked_add(1)
            .filter(|generation| *generation <= (u64::MAX >> SAMPLE_TOKEN_GENERATION_SHIFT))
            .expect("sample slot generation exhausted");
        let next_token = (next_generation << SAMPLE_TOKEN_GENERATION_SHIFT)
            | u64::try_from(next_index).expect("sample slot index fits u64");
        let slot = &self.slots[next_index];
        slot.session_generation
            .store(session_generation, Ordering::Relaxed);
        slot.sampled_tick
            .store(sampled_tick.unwrap_or(0), Ordering::Relaxed);
        slot.deadline_tick
            .store(deadline_tick.unwrap_or(0), Ordering::Relaxed);
        slot.resource_count.store(resource_count, Ordering::Relaxed);
        self.active_token.swap(next_token, Ordering::AcqRel)
    }

    fn tombstone(&self) {
        self.publish(0, None, None, 0);
    }

    fn load(&self) -> SamplePublication {
        loop {
            let before = self.active_token.load(Ordering::Acquire);
            let slot = &self.slots[token_index(before)];
            let session_generation = slot.session_generation.load(Ordering::Relaxed);
            let sampled_tick = slot.sampled_tick.load(Ordering::Relaxed);
            let deadline_tick = slot.deadline_tick.load(Ordering::Relaxed);
            let resource_count = slot.resource_count.load(Ordering::Relaxed);
            // The acquire fence orders the relaxed slot reads before the final
            // token validation on weakly ordered CPUs.
            fence(Ordering::Acquire);
            let after = self.active_token.load(Ordering::Relaxed);
            if token_identity(before) == token_identity(after) {
                return SamplePublication {
                    token: after,
                    session_generation,
                    sampled_at: self.decode(sampled_tick),
                    freshness_deadline: self.decode(deadline_tick),
                    resource_count,
                };
            }
        }
    }

    fn mark_stale_observed(&self, sample_token: u64) -> bool {
        let sample_identity = token_identity(sample_token);
        loop {
            let active = self.active_token.load(Ordering::Acquire);
            if token_identity(active) != sample_identity {
                return false;
            }
            if token_observed_stale(active) {
                return true;
            }
            if self
                .active_token
                .compare_exchange_weak(
                    active,
                    active | SAMPLE_TOKEN_STALE_BIT,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    fn encode(
        &self,
        instant: Instant,
        field: SourceTimestampField,
    ) -> Result<u64, SourceStatusError> {
        let Some(elapsed) = instant.checked_duration_since(self.epoch) else {
            return Err(SourceStatusError::TimestampOutOfRange {
                field,
                timestamp: instant,
            });
        };
        let Some(tick) = u64::try_from(elapsed.as_nanos())
            .ok()
            .and_then(|nanos| nanos.checked_add(1))
        else {
            return Err(SourceStatusError::TimestampOutOfRange {
                field,
                timestamp: instant,
            });
        };
        Ok(tick)
    }

    fn decode(&self, tick: u64) -> Option<Instant> {
        (tick != 0)
            .then(|| self.epoch.checked_add(Duration::from_nanos(tick - 1)))
            .flatten()
    }
}

fn token_index(token: u64) -> usize {
    usize::try_from(token & SAMPLE_TOKEN_INDEX_MASK).expect("sample token index fits usize")
}

fn token_generation(token: u64) -> u64 {
    token >> SAMPLE_TOKEN_GENERATION_SHIFT
}

fn token_identity(token: u64) -> u64 {
    token & !SAMPLE_TOKEN_STALE_BIT
}

fn token_observed_stale(token: u64) -> bool {
    token & SAMPLE_TOKEN_STALE_BIT != 0
}

/// Cloneable, read-only access to one source's latest health state.
#[derive(Clone)]
pub struct SourceStatusHandle {
    shared: Arc<SourceStatusShared>,
}

/// Allocation-free source fields used by render-plane availability checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceStatusAvailability {
    /// Broad source category.
    pub kind: SourceKind,
    /// Whether configuration enables the source.
    pub configured: bool,
    /// Whether the user has consented to capture.
    pub consented: bool,
    /// Whether the current render graph demands source data.
    pub demanded: bool,
    /// Current lifecycle health.
    pub state: SourceState,
    /// Effective freshness at the requested time.
    pub freshness: SourceFreshness,
    /// Whether the source was permanently removed from its owning graph.
    pub retired: bool,
}

impl SourceStatusHandle {
    /// Read the effective status at the current monotonic time.
    #[must_use]
    pub fn snapshot(&self) -> Arc<SourceStatus> {
        self.snapshot_at(Instant::now())
    }

    /// Read backend diagnostics for the active source session.
    #[must_use]
    pub fn diagnostics_snapshot(&self) -> Option<Arc<SourceDiagnostics>> {
        let status = self.shared.latest.load_full();
        if !matches!(
            status.state,
            SourceState::Starting | SourceState::Live | SourceState::Degraded
        ) {
            return None;
        }
        self.shared.diagnostics.load_full()
    }

    /// Read the effective status at `now` without taking the control mutex.
    ///
    /// Freshness is projected independently, so expiry never overwrites the
    /// source's lifecycle health or lifecycle issue.
    #[must_use]
    pub fn snapshot_at(&self, now: Instant) -> Arc<SourceStatus> {
        loop {
            let structural = self.shared.latest.load_full();
            let status = self.project(Arc::clone(&structural), now);
            let latest = self.shared.latest.load_full();
            if Arc::ptr_eq(&structural, &latest)
                && let Some(status) = status
            {
                return status;
            }
        }
    }

    /// Read the fields needed for availability aggregation without allocating.
    #[must_use]
    pub fn availability_at(&self, now: Instant) -> SourceStatusAvailability {
        loop {
            let structural = self.shared.latest.load_full();
            let mut freshness = structural.freshness;
            if freshness == SourceFreshness::Fresh {
                let sample = self.shared.samples.load();
                if sample.session_generation != structural.session_generation {
                    continue;
                }
                let Some(freshness_deadline) = sample.freshness_deadline else {
                    continue;
                };
                if now >= freshness_deadline {
                    if !self.shared.samples.mark_stale_observed(sample.token) {
                        continue;
                    }
                    freshness = SourceFreshness::Stale;
                }
            }

            let latest = self.shared.latest.load_full();
            if Arc::ptr_eq(&structural, &latest) {
                return SourceStatusAvailability {
                    kind: structural.kind,
                    configured: structural.configured,
                    consented: structural.consented,
                    demanded: structural.demanded,
                    state: structural.state,
                    freshness,
                    retired: structural.retired,
                };
            }
        }
    }

    /// Subscribe to latest structural publications and sample transitions.
    #[must_use]
    pub fn subscribe(&self) -> SourceStatusSubscription {
        SourceStatusSubscription::new(self.clone())
    }

    fn project(&self, structural: Arc<SourceStatus>, now: Instant) -> Option<Arc<SourceStatus>> {
        if structural.freshness != SourceFreshness::Fresh {
            return Some(structural);
        }

        loop {
            let sample = self.shared.samples.load();
            if sample.session_generation != structural.session_generation {
                return None;
            }
            let (Some(sampled_at), Some(freshness_deadline)) =
                (sample.sampled_at, sample.freshness_deadline)
            else {
                return None;
            };
            let mut effective = (*structural).clone();
            effective.last_sample_at = Some(sampled_at);
            effective.freshness_deadline = Some(freshness_deadline);
            if matches!(structural.state, SourceState::Live | SourceState::Degraded) {
                effective.resource_count = sample.resource_count;
            }
            let stale = now >= freshness_deadline;
            if stale {
                if !self.shared.samples.mark_stale_observed(sample.token) {
                    continue;
                }
                effective.freshness = SourceFreshness::Stale;
                effective.freshness_issue = Some(stale_data_issue());
            } else {
                effective.freshness_issue = None;
            }
            return Some(Arc::new(effective));
        }
    }
}

/// Cloneable lock-free view of every status-aware source in an input graph.
///
/// The registry owns only read-only status handles. It never retains source
/// objects, captured samples, backend resources, or the manager itself.
#[derive(Clone)]
pub struct SourceStatusRegistry {
    latest: Arc<ArcSwap<SourceStatusRegistrySnapshot>>,
}

impl SourceStatusRegistry {
    /// Create an empty generation-zero registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            latest: Arc::new(ArcSwap::from_pointee(SourceStatusRegistrySnapshot {
                source_graph_generation: 0,
                handles: Vec::new(),
            })),
        }
    }

    /// Load the latest immutable registry snapshot without locking the manager.
    #[must_use]
    pub fn snapshot(&self) -> Arc<SourceStatusRegistrySnapshot> {
        self.latest.load_full()
    }

    pub(crate) fn publish(&self, source_graph_generation: u64, handles: Vec<SourceStatusHandle>) {
        self.latest.store(Arc::new(SourceStatusRegistrySnapshot {
            source_graph_generation,
            handles,
        }));
    }
}

impl Default for SourceStatusRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable source-status registry generation.
pub struct SourceStatusRegistrySnapshot {
    source_graph_generation: u64,
    handles: Vec<SourceStatusHandle>,
}

impl SourceStatusRegistrySnapshot {
    /// Canonical manager-owned generation represented by this snapshot.
    #[must_use]
    pub fn source_graph_generation(&self) -> u64 {
        self.source_graph_generation
    }

    /// Read-only source handles in manager registration order.
    #[must_use]
    pub fn handles(&self) -> &[SourceStatusHandle] {
        &self.handles
    }

    /// Resolve every handle into its current effective status.
    #[must_use]
    pub fn statuses(&self) -> Vec<Arc<SourceStatus>> {
        self.handles
            .iter()
            .map(SourceStatusHandle::snapshot)
            .collect()
    }
}

/// Control-plane publisher for one source's status.
///
/// The constructor always creates the canonical stopped, generation-zero
/// status. A source constructor keeps this writer and exposes only the paired
/// [`SourceStatusHandle`] to consumers.
#[derive(Clone)]
pub struct SourceStatusWriter {
    shared: Arc<SourceStatusShared>,
}

impl SourceStatusWriter {
    /// Create a source-health publisher and its read-only handle.
    #[must_use]
    pub fn new(
        source_id: impl Into<Arc<str>>,
        kind: SourceKind,
        backend: impl Into<Arc<str>>,
        configured: bool,
        consented: bool,
        demanded: bool,
    ) -> (Self, SourceStatusHandle) {
        let initial = Arc::new(SourceStatus::stopped(
            source_id.into(),
            kind,
            backend.into(),
            configured,
            consented,
            demanded,
        ));
        let (structural, _) = watch::channel(Arc::clone(&initial));
        let (sample_signals, _) = watch::channel(0);
        let shared = Arc::new(SourceStatusShared {
            latest: ArcSwap::from(initial),
            control: Mutex::new(SourceStatusControl {
                active_session: None,
                next_session: 0,
                sample_signal_generation: 0,
            }),
            structural,
            sample_signals,
            samples: SampleBuffer::new(),
            diagnostics: ArcSwapOption::empty(),
        });
        (
            Self {
                shared: Arc::clone(&shared),
            },
            SourceStatusHandle { shared },
        )
    }

    /// Update configuration, consent, and demand policy.
    ///
    /// Revoking any eligibility flag invalidates the active session and clears
    /// all runtime freshness, resource, and issue state.
    ///
    /// # Errors
    ///
    /// Returns [`SourceStatusError::Retired`] after source removal.
    pub fn set_policy(
        &self,
        configured: bool,
        consented: bool,
        demanded: bool,
    ) -> Result<(), SourceStatusError> {
        let mut control = lock_control(&self.shared);
        let current = self.shared.latest.load_full();
        if current.retired {
            return Err(SourceStatusError::Retired);
        }
        if current.configured == configured
            && current.consented == consented
            && current.demanded == demanded
        {
            return Ok(());
        }

        let mut status = (*current).clone();
        status.configured = configured;
        status.consented = consented;
        status.demanded = demanded;
        if !status.eligible() {
            control.active_session = None;
            clear_stopped_state(&mut status);
        }
        if !(configured && consented && demanded) {
            self.shared.diagnostics.store(None);
            self.shared.samples.tombstone();
        }
        publish_structural(&self.shared, status);
        Ok(())
    }

    /// Publish the committed backend identity without disturbing lifecycle state.
    pub fn set_backend(&self, backend: impl Into<Arc<str>>) -> Result<(), SourceStatusError> {
        let backend = backend.into();
        let _control = lock_control(&self.shared);
        let current = self.shared.latest.load_full();
        if current.retired {
            return Err(SourceStatusError::Retired);
        }
        if current.backend == backend {
            return Ok(());
        }
        let mut status = (*current).clone();
        status.backend = backend;
        publish_structural(&self.shared, status);
        Ok(())
    }

    /// Publish the committed consumer count without disturbing lifecycle state.
    pub fn set_active_consumer_count(
        &self,
        active_consumer_count: usize,
    ) -> Result<(), SourceStatusError> {
        let _control = lock_control(&self.shared);
        let current = self.shared.latest.load_full();
        if current.retired {
            return Err(SourceStatusError::Retired);
        }
        if current.active_consumer_count == active_consumer_count {
            return Ok(());
        }
        let mut status = (*current).clone();
        status.active_consumer_count = active_consumer_count;
        publish_structural(&self.shared, status);
        Ok(())
    }

    /// Publish platform-specific state without disturbing generic lifecycle.
    ///
    /// # Errors
    ///
    /// Returns [`SourceStatusError::Retired`] after source removal.
    pub fn set_platform(
        &self,
        platform: Option<SourcePlatformStatus>,
    ) -> Result<(), SourceStatusError> {
        let platform = platform.map(Arc::new);
        let _control = lock_control(&self.shared);
        let current = self.shared.latest.load_full();
        if current.retired {
            return Err(SourceStatusError::Retired);
        }
        if current.platform == platform {
            return Ok(());
        }
        let mut status = (*current).clone();
        status.platform = platform;
        publish_structural(&self.shared, status);
        Ok(())
    }

    /// Begin a source session with a strictly newer graph generation.
    ///
    /// # Errors
    ///
    /// Returns a typed error when policy is ineligible, the publisher was
    /// retired, or `source_graph_generation` does not strictly advance.
    pub fn begin_session(
        &self,
        source_graph_generation: u64,
    ) -> Result<SourceSessionWriter, SourceStatusError> {
        let mut control = lock_control(&self.shared);
        let current = self.shared.latest.load_full();
        if current.retired {
            return Err(SourceStatusError::Retired);
        }
        if !current.eligible() {
            return Err(SourceStatusError::Ineligible {
                configured: current.configured,
                consented: current.consented,
                demanded: current.demanded,
            });
        }
        require_newer_graph(source_graph_generation, current.source_graph_generation)?;

        control.next_session = control
            .next_session
            .checked_add(1)
            .expect("source session generation exhausted");
        let session_generation = control.next_session;
        control.active_session = Some(session_generation);

        let mut status = (*current).clone();
        status.state = SourceState::Starting;
        status.freshness = SourceFreshness::AwaitingSample;
        status.source_graph_generation = source_graph_generation;
        status.session_generation = session_generation;
        status.last_sample_at = None;
        status.freshness_deadline = None;
        status.resource_count = 0;
        status.denied_resource_count = 0;
        status.issue = None;
        status.freshness_issue = None;
        self.shared.diagnostics.store(None);
        publish_structural(&self.shared, status);
        self.shared.samples.tombstone();

        Ok(SourceSessionWriter {
            shared: Arc::clone(&self.shared),
            session_generation,
        })
    }

    /// Invalidate the active session and publish an intentional stop.
    pub fn stop(&self) {
        let mut control = lock_control(&self.shared);
        let current = self.shared.latest.load_full();
        if current.retired {
            return;
        }
        if control.active_session.is_none() && is_canonical_stopped(&current) {
            return;
        }
        control.active_session = None;
        let mut status = (*current).clone();
        clear_stopped_state(&mut status);
        publish_structural(&self.shared, status);
        self.shared.samples.tombstone();
        self.shared.diagnostics.store(None);
    }

    /// Permanently remove the source at a strictly newer graph generation.
    ///
    /// Subscribers receive the final publication once and then
    /// [`SourceStatusSubscription::changed`] returns `None`.
    ///
    /// # Errors
    ///
    /// Returns a typed error after retirement or when
    /// `removal_graph_generation` does not strictly advance.
    pub fn retire(&self, removal_graph_generation: u64) -> Result<(), SourceStatusError> {
        let mut control = lock_control(&self.shared);
        let current = self.shared.latest.load_full();
        if current.retired {
            return Err(SourceStatusError::Retired);
        }
        require_newer_graph(removal_graph_generation, current.source_graph_generation)?;
        control.active_session = None;
        let mut status = (*current).clone();
        clear_stopped_state(&mut status);
        status.active_consumer_count = 0;
        status.source_graph_generation = removal_graph_generation;
        status.retired = true;
        publish_structural(&self.shared, status);
        self.shared.samples.tombstone();
        self.shared.diagnostics.store(None);
        Ok(())
    }
}

/// Source-owned lifecycle seam connecting a production input to its status.
///
/// The manager supplies the canonical graph generation. Worker and control
/// paths may clone the current [`SourceSessionWriter`] to publish outcomes
/// without borrowing the `Send`-only source object or the manager.
pub struct SourceStatusReporter {
    writer: SourceStatusWriter,
    handle: SourceStatusHandle,
    source_graph_generation: u64,
    session: Option<SourceSessionWriter>,
}

/// Lock-free handoff of the active fenced status writer to a long-lived worker.
///
/// Screen workers survive demand-off periods, so capturing one session at
/// thread creation would strand the successor after off -> on. Workers load
/// this slot only when publishing status; capture data never flows through it.
#[derive(Clone, Default)]
pub struct SourceSessionSlot {
    latest: Arc<ArcSwapOption<SourceSessionWriter>>,
}

impl SourceSessionSlot {
    /// Create an empty session handoff slot.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Install the current manager-bound source session.
    pub fn store(&self, session: SourceSessionWriter) {
        self.latest.store(Some(Arc::new(session)));
    }

    /// Clear the slot before fencing or stopping capture.
    pub fn clear(&self) {
        self.latest.store(None);
    }

    /// Clone the latest session writer without locking the worker or manager.
    #[must_use]
    pub fn load(&self) -> Option<SourceSessionWriter> {
        self.latest.load_full().as_deref().cloned()
    }
}

impl SourceStatusReporter {
    /// Create a source-owned reporter and its independent read handle.
    #[must_use]
    pub fn new(
        source_id: impl Into<Arc<str>>,
        kind: SourceKind,
        backend: impl Into<Arc<str>>,
        configured: bool,
        consented: bool,
        demanded: bool,
    ) -> Self {
        let (writer, handle) =
            SourceStatusWriter::new(source_id, kind, backend, configured, consented, demanded);
        Self {
            writer,
            handle,
            source_graph_generation: 0,
            session: None,
        }
    }

    /// Clone the source's lock-free read handle.
    #[must_use]
    pub fn handle(&self) -> SourceStatusHandle {
        self.handle.clone()
    }

    /// Bind the next source session to a canonical manager-owned generation.
    pub fn set_source_graph_generation(&mut self, source_graph_generation: u64) {
        assert!(
            source_graph_generation >= self.source_graph_generation,
            "source graph generation must never regress"
        );
        self.source_graph_generation = source_graph_generation;
    }

    pub(crate) fn reconfigure(
        &mut self,
        backend: impl Into<Arc<str>>,
        policy: SourceStatusPolicy,
        start_session: bool,
    ) -> Result<Option<SourceSessionWriter>, SourceStatusError> {
        let backend = backend.into();
        let mut control = lock_control(&self.writer.shared);
        let current = self.handle.snapshot();
        if current.retired {
            return Err(SourceStatusError::Retired);
        }
        let eligible = policy.configured && policy.consented && policy.demanded;
        if start_session && !eligible {
            return Err(SourceStatusError::Ineligible {
                configured: policy.configured,
                consented: policy.consented,
                demanded: policy.demanded,
            });
        }
        if start_session && self.source_graph_generation != 0 {
            require_newer_graph(
                self.source_graph_generation,
                current.source_graph_generation,
            )?;
        }

        let mut status = (*current).clone();
        status.backend = backend;
        status.configured = policy.configured;
        status.consented = policy.consented;
        status.demanded = policy.demanded;
        let session = if start_session && self.source_graph_generation != 0 {
            control.next_session = control
                .next_session
                .checked_add(1)
                .expect("source session generation exhausted");
            let session_generation = control.next_session;
            control.active_session = Some(session_generation);
            status.state = SourceState::Starting;
            status.freshness = SourceFreshness::AwaitingSample;
            status.source_graph_generation = self.source_graph_generation;
            status.session_generation = session_generation;
            status.last_sample_at = None;
            status.freshness_deadline = None;
            status.resource_count = 0;
            status.denied_resource_count = 0;
            status.issue = None;
            status.freshness_issue = None;
            Some(SourceSessionWriter {
                shared: Arc::clone(&self.writer.shared),
                session_generation,
            })
        } else {
            control.active_session = None;
            clear_stopped_state(&mut status);
            None
        };
        self.writer.shared.diagnostics.store(None);
        publish_structural(&self.writer.shared, status);
        self.writer.shared.samples.tombstone();
        self.session.clone_from(&session);
        Ok(session)
    }

    /// Begin a generation-fenced session when the source belongs to a manager.
    ///
    /// Standalone sources remain backwards compatible: without a manager-bound
    /// generation their capture behavior is unchanged and status stays stopped.
    pub fn begin_session(&mut self) -> Result<Option<SourceSessionWriter>, SourceStatusError> {
        if self.source_graph_generation == 0 {
            return Ok(None);
        }
        let session = self.writer.begin_session(self.source_graph_generation)?;
        self.session = Some(session.clone());
        Ok(Some(session))
    }

    /// Clone the active fenced writer for a worker or safe sample path.
    #[must_use]
    pub fn session(&self) -> Option<SourceSessionWriter> {
        self.session.clone()
    }

    /// Update source eligibility and fence the session on revocation.
    pub fn set_policy(
        &mut self,
        configured: bool,
        consented: bool,
        demanded: bool,
    ) -> Result<(), SourceStatusError> {
        self.writer.set_policy(configured, consented, demanded)?;
        if !(configured && consented && demanded) {
            self.session = None;
        }
        Ok(())
    }

    /// Publish the backend selected by a successfully committed configuration.
    pub fn set_backend(&mut self, backend: impl Into<Arc<str>>) -> Result<(), SourceStatusError> {
        self.writer.set_backend(backend)
    }

    /// Publish the committed consumer count without disturbing lifecycle state.
    pub fn set_active_consumer_count(
        &mut self,
        active_consumer_count: usize,
    ) -> Result<(), SourceStatusError> {
        self.writer.set_active_consumer_count(active_consumer_count)
    }

    /// Publish platform-specific state without disturbing generic lifecycle.
    pub fn set_platform(
        &mut self,
        platform: Option<SourcePlatformStatus>,
    ) -> Result<(), SourceStatusError> {
        self.writer.set_platform(platform)
    }

    /// Stop and fence the current source session.
    pub fn stop(&mut self) {
        self.session = None;
        self.writer.stop();
    }

    /// Permanently retire the source at the manager's removal generation.
    pub fn retire(&mut self, source_graph_generation: u64) -> Result<(), SourceStatusError> {
        self.writer.retire(source_graph_generation)?;
        self.source_graph_generation = source_graph_generation;
        self.session = None;
        Ok(())
    }
}

/// Generation-fenced publisher for one active source session.
#[derive(Clone)]
pub struct SourceSessionWriter {
    shared: Arc<SourceStatusShared>,
    session_generation: u64,
}

impl SourceSessionWriter {
    /// Generation assigned when this session started.
    #[must_use]
    pub fn session_generation(&self) -> u64 {
        self.session_generation
    }

    /// Earliest monotonic timestamp accepted by this source's sample plane.
    #[must_use]
    pub fn earliest_encodable_timestamp(&self) -> Instant {
        self.shared.samples.epoch
    }

    /// Publish backend diagnostics on a non-subscribed latest-value plane.
    pub fn publish_diagnostics(&self, diagnostics: SourceDiagnostics) -> bool {
        let control = lock_control(&self.shared);
        if control.active_session != Some(self.session_generation) {
            return false;
        }
        self.shared.diagnostics.store(Some(Arc::new(diagnostics)));
        true
    }

    /// Publish a sampled value with its mandatory freshness deadline.
    ///
    /// Once structurally live, steady samples only publish an allocation-free
    /// atomic slot. They do not publish through ArcSwap or wake subscribers.
    /// This method takes the blocking control mutex and must not run inside an
    /// audio, graphics, or other real-time callback.
    ///
    /// # Errors
    ///
    /// A fenced session returns `Ok(false)` before timestamp validation. An
    /// active session returns [`SourceStatusError::InvalidFreshnessDeadline`]
    /// unless `freshness_deadline` is strictly later than `sampled_at`.
    pub fn record_sample(
        &self,
        sampled_at: Instant,
        freshness_deadline: Instant,
        resource_count: usize,
    ) -> Result<bool, SourceStatusError> {
        self.record_sample_with_health(
            sampled_at,
            freshness_deadline,
            resource_count,
            SourceState::Live,
            None,
        )
    }

    /// Publish a fresh sample while preserving recoverable degraded health.
    ///
    /// Steady degraded samples update the allocation-free sample plane without
    /// republishing an identical structural transition.
    ///
    /// # Errors
    ///
    /// Error behavior matches [`Self::record_sample`].
    pub fn record_degraded_sample(
        &self,
        sampled_at: Instant,
        freshness_deadline: Instant,
        resource_count: usize,
        issue: SourceIssue,
    ) -> Result<bool, SourceStatusError> {
        self.record_sample_with_health(
            sampled_at,
            freshness_deadline,
            resource_count,
            SourceState::Degraded,
            Some(issue),
        )
    }

    fn record_sample_with_health(
        &self,
        sampled_at: Instant,
        freshness_deadline: Instant,
        resource_count: usize,
        state: SourceState,
        issue: Option<SourceIssue>,
    ) -> Result<bool, SourceStatusError> {
        let mut control = lock_control(&self.shared);
        if control.active_session != Some(self.session_generation) {
            return Ok(false);
        }
        if freshness_deadline <= sampled_at {
            return Err(SourceStatusError::InvalidFreshnessDeadline {
                sampled_at,
                freshness_deadline,
            });
        }
        let sampled_tick = self
            .shared
            .samples
            .encode(sampled_at, SourceTimestampField::SampledAt)?;
        let deadline_tick = self
            .shared
            .samples
            .encode(freshness_deadline, SourceTimestampField::FreshnessDeadline)?;

        let previous = self.shared.samples.load();
        let prior_token = self.shared.samples.publish(
            self.session_generation,
            Some(sampled_tick),
            Some(deadline_tick),
            resource_count,
        );
        let observed_stale = token_identity(prior_token) == token_identity(previous.token)
            && token_observed_stale(prior_token);
        let current = self.shared.latest.load_full();
        if current.state != state
            || current.freshness != SourceFreshness::Fresh
            || current.denied_resource_count != 0
            || current.issue != issue
        {
            let mut status = (*current).clone();
            status.state = state;
            status.freshness = SourceFreshness::Fresh;
            status.last_sample_at = Some(sampled_at);
            status.freshness_deadline = Some(freshness_deadline);
            status.resource_count = resource_count;
            status.denied_resource_count = 0;
            status.issue = issue;
            status.freshness_issue = None;
            publish_structural(&self.shared, status);
            return Ok(true);
        }

        let deadline_shortened = previous
            .freshness_deadline
            .is_some_and(|deadline| freshness_deadline < deadline);
        let resources_changed = previous.resource_count != resource_count;
        if deadline_shortened || resources_changed || observed_stale {
            publish_sample_signal(&self.shared, &mut control);
        }
        Ok(true)
    }

    /// Mark a genuinely event-driven source live without a freshness deadline.
    ///
    /// Sampled audio, screen, media, and network backends must use
    /// [`Self::record_sample`] instead. This operation is only for sources whose
    /// healthy silence carries no missing-data meaning.
    pub fn mark_event_driven_live_without_deadline(&self, resource_count: usize) -> bool {
        let control = lock_control(&self.shared);
        if control.active_session != Some(self.session_generation) {
            return false;
        }

        let current = self.shared.latest.load_full();
        if current.state == SourceState::Live
            && current.freshness == SourceFreshness::NotApplicable
            && current.resource_count == resource_count
            && current.denied_resource_count == 0
            && current.issue.is_none()
        {
            return true;
        }

        let mut status = (*current).clone();
        status.state = SourceState::Live;
        status.freshness = SourceFreshness::NotApplicable;
        status.last_sample_at = None;
        status.freshness_deadline = None;
        status.resource_count = resource_count;
        status.denied_resource_count = 0;
        status.issue = None;
        status.freshness_issue = None;
        publish_structural(&self.shared, status);
        self.shared.samples.tombstone();
        true
    }

    /// Publish one resource scan as a single coherent structural transition.
    pub fn publish_resource_scan_health(&self, health: SourceResourceScanHealth) -> bool {
        let control = lock_control(&self.shared);
        if control.active_session != Some(self.session_generation) {
            return false;
        }

        let current = self.shared.latest.load_full();
        let mut status = (*current).clone();
        status.freshness = SourceFreshness::NotApplicable;
        status.last_sample_at = None;
        status.freshness_deadline = None;
        status.freshness_issue = None;
        match health {
            SourceResourceScanHealth::Live {
                resource_count,
                denied_resource_count,
            } => {
                status.state = SourceState::Live;
                status.resource_count = resource_count;
                status.denied_resource_count = denied_resource_count;
                status.issue = None;
            }
            SourceResourceScanHealth::Degraded {
                resource_count,
                denied_resource_count,
                issue,
            } => {
                status.state = SourceState::Degraded;
                status.resource_count = resource_count;
                status.denied_resource_count = denied_resource_count;
                status.issue = Some(issue);
            }
            SourceResourceScanHealth::Unavailable {
                resource_count,
                denied_resource_count,
                issue,
            } => {
                status.state = SourceState::Unavailable;
                status.resource_count = resource_count;
                status.denied_resource_count = denied_resource_count;
                status.issue = Some(issue);
            }
        }
        if status == *current {
            return true;
        }
        publish_structural(&self.shared, status);
        self.shared.samples.tombstone();
        true
    }

    /// Publish recoverable degraded health if this session is still active.
    pub fn degraded(&self, issue: SourceIssue) -> bool {
        self.publish(|status| {
            status.state = SourceState::Degraded;
            status.denied_resource_count = 0;
            status.issue = Some(issue);
        })
    }

    /// Publish partial backend health with its usable resource count atomically.
    pub fn degraded_with_resources(&self, issue: SourceIssue, resource_count: usize) -> bool {
        self.publish(|status| {
            status.state = SourceState::Degraded;
            status.resource_count = resource_count;
            status.denied_resource_count = 0;
            status.issue = Some(issue);
        })
    }

    /// Publish unavailable health if this session is still active.
    pub fn unavailable(&self, issue: SourceIssue) -> bool {
        self.publish(|status| {
            status.state = SourceState::Unavailable;
            status.resource_count = 0;
            status.denied_resource_count = 0;
            status.issue = Some(issue);
            clear_unproduced_freshness(status);
        })
    }

    /// Publish terminal failed health and invalidate this session.
    pub fn failed(&self, issue: SourceIssue) -> bool {
        let mut control = lock_control(&self.shared);
        if control.active_session != Some(self.session_generation) {
            return false;
        }
        control.active_session = None;
        let mut status = (*self.shared.latest.load_full()).clone();
        status.state = SourceState::Failed;
        status.resource_count = 0;
        status.denied_resource_count = 0;
        status.issue = Some(issue);
        clear_unproduced_freshness(&mut status);
        publish_structural(&self.shared, status);
        true
    }

    fn publish(&self, update: impl FnOnce(&mut SourceStatus)) -> bool {
        let control = lock_control(&self.shared);
        if control.active_session != Some(self.session_generation) {
            return false;
        }

        let current = self.shared.latest.load_full();
        let mut status = (*current).clone();
        update(&mut status);
        if status == *current {
            return true;
        }
        publish_structural(&self.shared, status);
        true
    }
}

/// Awaitable latest-value source-status transitions.
///
/// Structural changes originate from the immutable watch payload. Fresh sample
/// projection may return a derived Arc with atomic timing fields. A separate
/// sample signal only wakes on deadline shortening, stale recovery, or resource
/// count changes.
pub struct SourceStatusSubscription {
    handle: SourceStatusHandle,
    structural: watch::Receiver<Arc<SourceStatus>>,
    sample_signals: watch::Receiver<u64>,
    current_structural: Arc<SourceStatus>,
    observed_freshness: SourceFreshness,
    observed_resource_count: usize,
    observed_expiry: Option<Instant>,
    terminated: bool,
}

impl SourceStatusSubscription {
    fn new(handle: SourceStatusHandle) -> Self {
        let mut structural = handle.shared.structural.subscribe();
        let current_structural = Arc::clone(&structural.borrow_and_update());
        let mut sample_signals = handle.shared.sample_signals.subscribe();
        sample_signals.borrow_and_update();
        let now = tokio::time::Instant::now().into_std();
        let projected = handle
            .project(Arc::clone(&current_structural), now)
            .unwrap_or_else(|| Arc::clone(&current_structural));
        Self {
            handle,
            structural,
            sample_signals,
            current_structural,
            observed_freshness: projected.freshness,
            observed_resource_count: projected.resource_count,
            observed_expiry: expired_deadline(&projected),
            terminated: projected.retired,
        }
    }

    /// Read the current effective status directly from the latest-value handle.
    #[must_use]
    pub fn snapshot(&self) -> Arc<SourceStatus> {
        self.handle
            .snapshot_at(tokio::time::Instant::now().into_std())
    }

    /// Wait for the next externally visible structural or sample transition.
    ///
    /// The final retirement publication is returned once. Later calls return
    /// `None` immediately. Intermediate structural or sample values may be
    /// coalesced according to Tokio watch latest-value semantics.
    pub async fn changed(&mut self) -> Option<Arc<SourceStatus>> {
        if self.terminated {
            return None;
        }

        loop {
            if self.structural.has_changed().unwrap_or(false) {
                return Some(self.take_structural());
            }

            let now = tokio::time::Instant::now().into_std();
            let Some(status) = self
                .handle
                .project(Arc::clone(&self.current_structural), now)
            else {
                if self.structural.changed().await.is_err() {
                    self.terminated = true;
                    return None;
                }
                return Some(self.take_structural());
            };
            if self.take_sample_transition(&status) {
                return Some(status);
            }

            if let Some(deadline) = fresh_deadline(&status) {
                tokio::select! {
                    biased;
                    structural = self.structural.changed() => {
                        if structural.is_err() {
                            self.terminated = true;
                            return None;
                        }
                        return Some(self.take_structural());
                    }
                    sample = self.sample_signals.changed() => {
                        if sample.is_err() {
                            self.terminated = true;
                            return None;
                        }
                        self.sample_signals.borrow_and_update();
                    }
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {}
                }
            } else {
                tokio::select! {
                    biased;
                    structural = self.structural.changed() => {
                        if structural.is_err() {
                            self.terminated = true;
                            return None;
                        }
                        return Some(self.take_structural());
                    }
                    sample = self.sample_signals.changed() => {
                        if sample.is_err() {
                            self.terminated = true;
                            return None;
                        }
                        self.sample_signals.borrow_and_update();
                    }
                }
            }
        }
    }

    fn take_structural(&mut self) -> Arc<SourceStatus> {
        let structural = Arc::clone(&self.structural.borrow_and_update());
        self.current_structural = Arc::clone(&structural);
        let now = tokio::time::Instant::now().into_std();
        let status = self
            .handle
            .project(structural, now)
            .unwrap_or_else(|| Arc::clone(&self.current_structural));
        self.observe(&status);
        self.terminated = status.retired;
        status
    }

    fn take_sample_transition(&mut self, status: &SourceStatus) -> bool {
        let changed = status.freshness != self.observed_freshness
            || status.resource_count != self.observed_resource_count
            || (status.freshness == SourceFreshness::Stale
                && expired_deadline(status) != self.observed_expiry);
        if changed {
            self.observe(status);
        } else if status.freshness == SourceFreshness::Fresh {
            self.observed_expiry = None;
        }
        changed
    }

    fn observe(&mut self, status: &SourceStatus) {
        self.observed_freshness = status.freshness;
        self.observed_resource_count = status.resource_count;
        self.observed_expiry = expired_deadline(status);
    }
}

fn fresh_deadline(status: &SourceStatus) -> Option<Instant> {
    (status.freshness == SourceFreshness::Fresh)
        .then_some(status.freshness_deadline)
        .flatten()
}

fn expired_deadline(status: &SourceStatus) -> Option<Instant> {
    (status.freshness == SourceFreshness::Stale)
        .then_some(status.freshness_deadline)
        .flatten()
}

fn lock_control(shared: &SourceStatusShared) -> MutexGuard<'_, SourceStatusControl> {
    match shared.control.lock() {
        Ok(control) => control,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn publish_structural(shared: &SourceStatusShared, status: SourceStatus) {
    let status = Arc::new(status);
    shared.latest.store(Arc::clone(&status));
    shared.structural.send_replace(status);
}

fn publish_sample_signal(shared: &SourceStatusShared, control: &mut SourceStatusControl) {
    control.sample_signal_generation = control
        .sample_signal_generation
        .checked_add(1)
        .expect("sample signal generation exhausted");
    shared
        .sample_signals
        .send_replace(control.sample_signal_generation);
}

fn require_newer_graph(requested: u64, current: u64) -> Result<(), SourceStatusError> {
    if requested <= current {
        return Err(SourceStatusError::GraphGenerationNotAdvanced { current, requested });
    }
    Ok(())
}

fn clear_stopped_state(status: &mut SourceStatus) {
    status.state = SourceState::Stopped;
    status.freshness = SourceFreshness::NotApplicable;
    status.last_sample_at = None;
    status.freshness_deadline = None;
    status.resource_count = 0;
    status.denied_resource_count = 0;
    status.issue = None;
    status.freshness_issue = None;
}

fn clear_unproduced_freshness(status: &mut SourceStatus) {
    if status.freshness == SourceFreshness::AwaitingSample {
        status.freshness = SourceFreshness::NotApplicable;
        status.last_sample_at = None;
        status.freshness_deadline = None;
        status.freshness_issue = None;
    }
}

fn is_canonical_stopped(status: &SourceStatus) -> bool {
    status.state == SourceState::Stopped
        && status.freshness == SourceFreshness::NotApplicable
        && status.last_sample_at.is_none()
        && status.freshness_deadline.is_none()
        && status.resource_count == 0
        && status.denied_resource_count == 0
        && status.issue.is_none()
        && status.freshness_issue.is_none()
}

fn stale_data_issue() -> SourceIssue {
    static ISSUE: LazyLock<SourceIssue> = LazyLock::new(|| {
        SourceIssue::new(
            "stale_data",
            "source data exceeded its freshness deadline",
            true,
        )
        .with_remediation("check capture demand and the source backend")
    });
    ISSUE.clone()
}
