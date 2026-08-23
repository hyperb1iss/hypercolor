//! Driver-facing host boundary for modular Hypercolor drivers.
//!
//! This crate defines the stable capability surface between the daemon-owned
//! runtime and driver implementations. Drivers should depend on these
//! traits and shared request/response types instead of reaching into daemon
//! internals directly.

mod backend;
mod config;
mod controls;
mod descriptor;
mod discovery;
mod driver_discovery;
mod error;
mod host;
mod module;
mod pairing;

pub use backend::{
    BackendInfo, ConnectExecution, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId,
    DeviceDeliveryObserver, DeviceDeliveryStatus, DeviceDisplaySink, DeviceFrameSink,
    DeviceLifecyclePolicy, DeviceWriteOutcome, OutputCadence,
};
pub use config::{DriverConfigProvider, DriverConfigView};
pub use controls::{
    BackendRebindActions, ControlApplyTarget, DeviceControlStore, DriverControlHost,
    DriverControlProvider, DriverControlStore, DriverLifecycleActions, ValidatedControlChanges,
};
pub use descriptor::{DRIVER_API_SCHEMA_VERSION, DriverDescriptor};
pub use discovery::{DiscoveredDevice, DiscoveryConnectBehavior};
pub use driver_discovery::{DiscoveryCapability, DiscoveryRequest};
pub use error::{DriverError, ErrorRecoverability};
pub use host::{
    DriverCredentialStore, DriverDiscoveryState, DriverHost, DriverRuntimeActions,
    DriverTrackedDevice, TrackedDeviceCtx,
};
pub use module::{
    DeviceBackendFactory, DriverModule, DriverPresentationProvider, DriverProtocolCatalog,
    DriverRuntimeCacheProvider, OutputBinding,
};
pub use pairing::PairingCapability;
