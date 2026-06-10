use anyhow::Result;
use async_trait::async_trait;

use crate::{DriverHost, TrackedDeviceCtx};

// Pairing data vocabulary lives in hypercolor-types (shared with the
// daemon API contracts and every client); re-exported here so drivers
// and the daemon keep their existing import paths.
pub use hypercolor_types::pairing::{
    ClearPairingOutcome, DeviceAuthState, DeviceAuthSummary, PairDeviceOutcome, PairDeviceRequest,
    PairDeviceStatus, PairingDescriptor, PairingFieldDescriptor, PairingFlowKind,
};

/// Driver capability for pairing and auth summaries.
#[async_trait]
pub trait PairingCapability: Send + Sync {
    /// Summarize auth state for one tracked device.
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary>;

    /// Pair a tracked device.
    ///
    /// # Errors
    ///
    /// Returns an error if the pair flow fails unexpectedly.
    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> Result<PairDeviceOutcome>;

    /// Clear stored credentials for a tracked device.
    ///
    /// # Errors
    ///
    /// Returns an error if the credential clear flow fails.
    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<ClearPairingOutcome>;
}
