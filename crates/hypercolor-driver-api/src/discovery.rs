//! Native discovery boundary shared by transport scanners and the orchestrator.

use std::collections::HashMap;

use anyhow::Result;
use hypercolor_types::device::{
    ConnectionType, DeviceFamily, DeviceFingerprint, DeviceInfo, DeviceOrigin,
};

/// A single-transport device scanner.
#[async_trait::async_trait]
pub trait TransportScanner: Send + Sync {
    /// Human-readable scanner name for logging and diagnostics.
    fn name(&self) -> &str;

    /// Run a one-shot scan and return all currently reachable devices.
    ///
    /// # Errors
    ///
    /// Returns an error if the transport is inaccessible or the scan
    /// encounters an unrecoverable failure.
    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>>;
}

/// Whether a discovered device should trigger an immediate lifecycle connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryConnectBehavior {
    /// Safe to auto-connect as soon as discovery sees the device.
    AutoConnect,

    /// Keep the device visible in inventory, but defer auto-connect until a
    /// later discovery pass upgrades it to `AutoConnect`.
    Deferred,
}

impl DiscoveryConnectBehavior {
    /// Whether this behavior should emit a connect action on discovery.
    #[must_use]
    pub const fn should_auto_connect(self) -> bool {
        matches!(self, Self::AutoConnect)
    }

    /// Merge two behaviors, preserving the more capable auto-connect mode.
    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        if matches!(self, Self::AutoConnect) || matches!(other, Self::AutoConnect) {
            Self::AutoConnect
        } else {
            Self::Deferred
        }
    }
}

/// Raw discovery result from a single scanner.
#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    /// How this device connects to the host.
    pub connection_type: ConnectionType,

    /// Driver ownership and output routing metadata.
    pub origin: DeviceOrigin,

    /// Preliminary device name from the transport layer.
    pub name: String,

    /// Device family, if identifiable from the transport layer.
    pub family: DeviceFamily,

    /// Stable identity fingerprint for deduplication.
    pub fingerprint: DeviceFingerprint,

    /// Whether discovery should trigger an immediate lifecycle connect.
    pub connect_behavior: DiscoveryConnectBehavior,

    /// Pre-built device info ready for registry insertion.
    pub info: DeviceInfo,

    /// Additional scanner metadata.
    pub metadata: HashMap<String, String>,
}
