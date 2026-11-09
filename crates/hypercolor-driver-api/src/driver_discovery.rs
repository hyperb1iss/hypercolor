use std::io;
use std::time::Duration;

use async_trait::async_trait;
use hypercolor_types::device::DeviceFingerprint;
use thiserror::Error;

use crate::{DiscoveredDevice, DriverConfigView, DriverError, DriverHost};

/// Discovery request normalized by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRequest {
    pub timeout: Duration,
    pub mdns_enabled: bool,
}

/// One thing a scan learned about one device.
#[derive(Debug, Clone)]
pub enum DiscoveryObservation {
    /// A verified description of a device, applied to the registry.
    Device(DiscoveredDevice),
    /// A device the host already tracks under this fingerprint answered, but
    /// the scan could not refresh its description. Presence is renewed and
    /// everything the host verified earlier stays as it is. A fingerprint
    /// the host does not track is ignored.
    Present(DeviceFingerprint),
}

/// One way a scan may have missed a device it was meant to see.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoverageGap {
    /// A discovery request never left this host.
    #[error("send to {target} failed")]
    SendFailed {
        /// Where the request was going.
        target: String,
    },
    /// The reply stream failed before its collection window ended.
    #[error("receive failed: {reason}")]
    ReceiveFailed {
        /// Failure detail.
        reason: String,
    },
    /// A candidate answered discovery but could not be described, and it
    /// names no tracked device, so it could be any of them.
    #[error("probe of {candidate} failed: {reason}")]
    ProbeFailed {
        /// The candidate the probe targeted.
        candidate: String,
        /// Failure detail.
        reason: String,
    },
    /// A tracked device could not be verified. The host holds this device
    /// as if seen, and the rest of the target can still retire.
    #[error("{fingerprint} could not be verified: {reason}")]
    DeviceUnverified {
        /// The device that stays held.
        fingerprint: DeviceFingerprint,
        /// Failure detail.
        reason: String,
    },
    /// The scan deadline ended probes before they finished.
    #[error("{unfinished} probes did not finish before the scan deadline")]
    ProbesCancelled {
        /// Probes cancelled or never started.
        unfinished: usize,
    },
    /// Admission refused candidates, so they were never probed.
    #[error("admission refused {refused} candidates past its limit")]
    AdmissionFull {
        /// Offers refused this scan.
        refused: usize,
    },
    /// One discovery source of a multi-source scan could not run.
    #[error("{source_name} failed: {reason}")]
    SourceFailed {
        /// The source that failed, such as mDNS or a cloud inventory.
        source_name: String,
        /// Failure detail.
        reason: String,
    },
}

impl CoverageGap {
    /// The one device this gap holds, or `None` when it covers the whole
    /// driver target.
    #[must_use]
    pub const fn device(&self) -> Option<&DeviceFingerprint> {
        match self {
            Self::DeviceUnverified { fingerprint, .. } => Some(fingerprint),
            _ => None,
        }
    }

    /// The gap a failed datagram receive leaves, if any.
    ///
    /// Windows surfaces an ICMP port-unreachable from an earlier send as a
    /// reset on the next receive. That is a peer saying nothing listens
    /// there, the same answer silence gives, not a failing socket.
    #[must_use]
    pub fn from_receive_error(error: &io::Error) -> Option<Self> {
        match error.kind() {
            io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused => None,
            _ => Some(Self::ReceiveFailed {
                reason: error.to_string(),
            }),
        }
    }
}

/// Whether a scan saw everything it was meant to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryCoverage {
    /// A device this scan did not report is gone. An intended collection
    /// window ending is complete, and so is a target that never answered.
    Complete,
    /// The scan may have missed devices. A gap that names a device holds
    /// only that device; any other gap holds the whole target, so nothing
    /// can be inferred from a device's absence.
    Partial(Vec<CoverageGap>),
}

impl DiscoveryCoverage {
    /// Coverage a scan with these gaps earned.
    #[must_use]
    pub fn from_gaps(gaps: Vec<CoverageGap>) -> Self {
        if gaps.is_empty() {
            Self::Complete
        } else {
            Self::Partial(gaps)
        }
    }

    /// The gaps this coverage names.
    #[must_use]
    pub fn gaps(&self) -> &[CoverageGap] {
        match self {
            Self::Complete => &[],
            Self::Partial(gaps) => gaps,
        }
    }
}

/// Everything one scan of a driver target learned.
#[derive(Debug, Clone)]
pub struct DiscoveryBatch {
    /// What the scan saw. These apply whatever the coverage says.
    pub observations: Vec<DiscoveryObservation>,
    /// Whether the host may infer that an unreported device left.
    pub coverage: DiscoveryCoverage,
}

impl DiscoveryBatch {
    /// A batch with the coverage its gaps earned.
    #[must_use]
    pub fn new(observations: Vec<DiscoveryObservation>, gaps: Vec<CoverageGap>) -> Self {
        Self {
            observations,
            coverage: DiscoveryCoverage::from_gaps(gaps),
        }
    }

    /// A complete scan that reports these devices.
    #[must_use]
    pub fn complete(devices: impl IntoIterator<Item = DiscoveredDevice>) -> Self {
        Self {
            observations: devices
                .into_iter()
                .map(DiscoveryObservation::Device)
                .collect(),
            coverage: DiscoveryCoverage::Complete,
        }
    }
}

/// Driver capability for device discovery.
#[async_trait]
pub trait DiscoveryCapability: Send + Sync {
    /// Discover reachable devices for this driver.
    ///
    /// The host applies every observation, and infers that a tracked device
    /// left when no gap covers the target. Report a gap for anything that
    /// could hide a device, naming the device when the scan knows which one;
    /// a driver never needs to invent a record, or fail a scan, to keep the
    /// host from retiring one.
    ///
    /// # Errors
    ///
    /// Returns an error only when discovery could not start at all. A scan
    /// that ran and fell short returns a batch with partial coverage, so
    /// what it did see still applies.
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryBatch, DriverError>;
}
