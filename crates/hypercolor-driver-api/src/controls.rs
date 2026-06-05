use anyhow::{Result, bail};
use async_trait::async_trait;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ApplyImpact, ControlActionResult, ControlChange,
    ControlSurfaceDocument, ControlSurfaceEvent, ControlValueMap,
};
use hypercolor_types::device::DeviceId;

use crate::{DriverConfigView, DriverHost, TrackedDeviceCtx};

/// Target for a driver-owned control operation.
#[derive(Debug, Clone, Copy)]
pub enum ControlApplyTarget<'a> {
    /// Driver-module level controls.
    Driver {
        /// Driver module identifier.
        driver_id: &'a str,
        /// Current resolved driver config.
        config: DriverConfigView<'a>,
    },

    /// Controls for one tracked device.
    Device {
        /// Tracked device context.
        device: &'a TrackedDeviceCtx<'a>,
    },
}

/// Driver-normalized control changes that are safe to apply.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedControlChanges {
    /// Changes accepted by validation.
    pub changes: Vec<ControlChange>,

    /// Dynamic impacts validation expects the apply step to perform.
    pub impacts: Vec<ApplyImpact>,
}

impl ValidatedControlChanges {
    /// Construct a validated bundle with no precomputed impacts.
    #[must_use]
    pub fn new(changes: Vec<ControlChange>) -> Self {
        Self {
            changes,
            impacts: Vec::new(),
        }
    }
}

/// Host persistence for driver-scoped control values.
#[async_trait]
pub trait DriverControlStore: Send + Sync {
    /// Load the current typed values for a driver surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be read.
    async fn load_driver_values(&self, driver_id: &str) -> Result<ControlValueMap>;

    /// Persist typed values for a driver surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be written.
    async fn save_driver_values(&self, driver_id: &str, values: ControlValueMap) -> Result<()>;
}

/// Host persistence for device-scoped control values.
#[async_trait]
pub trait DeviceControlStore: Send + Sync {
    /// Load the current typed values for a device surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be read.
    async fn load_device_values(&self, device_id: DeviceId) -> Result<ControlValueMap>;

    /// Persist typed values for a device surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be written.
    async fn save_device_values(&self, device_id: DeviceId, values: ControlValueMap) -> Result<()>;
}

/// Dynamic lifecycle operations available to control apply transactions.
#[async_trait]
pub trait DriverLifecycleActions: Send + Sync {
    /// Reconnect a device after a control change.
    ///
    /// # Errors
    ///
    /// Returns an error if reconnect scheduling fails.
    async fn reconnect_device(&self, device_id: DeviceId, backend_id: &str) -> Result<bool>;

    /// Request a discovery rescan for one driver.
    ///
    /// # Errors
    ///
    /// Returns an error if rescan scheduling fails.
    async fn rescan_driver(&self, driver_id: &str) -> Result<()>;
}

/// Output backend operations available to control apply transactions.
#[async_trait]
pub trait BackendRebindActions: Send + Sync {
    /// Rebind a driver backend after transport-level controls change.
    ///
    /// # Errors
    ///
    /// Returns an error if rebind scheduling fails.
    async fn rebind_backend(&self, driver_id: &str) -> Result<()>;
}

/// Host services available to driver-owned control providers.
pub trait DriverControlHost: Send + Sync {
    /// Driver-scoped typed value store.
    fn driver_config_store(&self) -> &dyn DriverControlStore;

    /// Device-scoped typed value store.
    fn device_config_store(&self) -> &dyn DeviceControlStore;

    /// Lifecycle actions allowed during apply.
    fn lifecycle(&self) -> &dyn DriverLifecycleActions;

    /// Backend rebind actions allowed during apply.
    fn backend_rebind(&self) -> &dyn BackendRebindActions;

    /// Publish a typed control-surface event.
    fn publish_control_event(&self, event: ControlSurfaceEvent);
}

/// Driver capability for typed dynamic control surfaces.
#[async_trait]
pub trait DriverControlProvider: Send + Sync {
    /// Build the optional driver-scoped control surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the surface cannot be built.
    async fn driver_surface(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>>;

    /// Build the optional device-scoped control surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the surface cannot be built.
    async fn device_surface(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>>;

    /// Validate and normalize a batch before mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if validation cannot complete.
    async fn validate_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges>;

    /// Apply a previously validated batch.
    ///
    /// # Errors
    ///
    /// Returns an error if the mutation fails.
    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> Result<ApplyControlChangesResponse>;

    /// Invoke a typed action.
    ///
    /// # Errors
    ///
    /// Returns an error if the action fails.
    async fn invoke_action(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        action_id: &str,
        _input: ControlValueMap,
    ) -> Result<ControlActionResult> {
        bail!("unknown control action: {action_id}")
    }
}
