use async_trait::async_trait;

use hypercolor_types::pairing::{
    ClearPairingOutcome, DeviceAuthSummary, PairDeviceOutcome, PairDeviceRequest,
};

use crate::{DriverError, DriverHost, TrackedDeviceCtx};

/// Driver capability for pairing and auth summaries.
#[async_trait]
pub trait PairingCapability: Send + Sync {
    /// Summarize auth state for one tracked device.
    ///
    /// # Errors
    ///
    /// Returns an error if the driver's credential state cannot be read.
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<DeviceAuthSummary>, DriverError>;

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
    ) -> Result<PairDeviceOutcome, DriverError>;

    /// Clear stored credentials for a tracked device.
    ///
    /// # Errors
    ///
    /// Returns an error if the credential clear flow fails.
    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<ClearPairingOutcome, DriverError>;
}
