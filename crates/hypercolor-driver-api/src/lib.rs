//! Driver-facing host boundary for modular Hypercolor drivers.
//!
//! This crate defines the stable capability surface between the daemon-owned
//! runtime and driver implementations. Drivers should depend on these
//! traits and shared request/response types instead of reaching into daemon
//! internals directly.

pub mod backend;
pub mod config;
pub mod control_apply;
pub mod control_surface;
pub mod controls;
pub mod descriptor;
pub mod discovery;
pub mod driver_discovery;
pub mod host;
pub mod module;
pub mod net;
pub mod pairing;
pub mod support;
pub mod validation;

pub use backend::{
    BackendInfo, ConnectExecution, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId,
    DeviceDeliveryObserver, DeviceDeliveryStatus, DeviceDisplaySink, DeviceFrameSink,
    DeviceLifecyclePolicy, DeviceWriteOutcome, HealthStatus, OutputCadence,
};
pub use config::{DriverConfigProvider, DriverConfigView};
pub use controls::{
    BackendRebindActions, ControlApplyTarget, DeviceControlStore, DriverControlHost,
    DriverControlProvider, DriverControlStore, DriverLifecycleActions, ValidatedControlChanges,
};
pub use descriptor::{DRIVER_API_SCHEMA_VERSION, DriverDescriptor};
pub use discovery::{DiscoveredDevice, DiscoveryConnectBehavior, TransportScanner};
pub use driver_discovery::{
    DiscoveryCapability, DiscoveryRequest, DiscoveryResult, DriverDiscoveredDevice,
};
pub use host::{
    DriverCredentialStore, DriverDiscoveryState, DriverHost, DriverRuntimeActions,
    DriverTrackedDevice, TrackedDeviceCtx,
};
pub use module::{
    DriverModule, DriverPresentationProvider, DriverProtocolCatalog, DriverRuntimeCacheProvider,
};
pub use net::{CredentialStore, MdnsBrowser, MdnsService};
pub use pairing::{
    ClearPairingOutcome, DeviceAuthState, DeviceAuthSummary, PairDeviceOutcome, PairDeviceRequest,
    PairDeviceStatus, PairingCapability, PairingDescriptor, PairingFieldDescriptor,
    PairingFlowKind,
};
