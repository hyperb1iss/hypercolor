//! Device backend system — traits, registry, and discovery orchestration.
//!
//! This module defines the output half of Hypercolor's pipeline: everything
//! needed to discover physical hardware, establish connections, and push
//! LED color data to devices over any transport (USB HID, UDP, TCP, HTTP).

#[cfg(unix)]
pub mod blocks;
mod discovery;
mod discovery_server;
mod lifecycle;
pub mod manager;
pub mod mock;
pub mod net;
mod registry;
pub mod smbus_backend;
pub mod smbus_scanner;
mod state_machine;
mod traits;
pub mod usb_backend;
pub mod usb_hotplug;
pub mod usb_scanner;

#[cfg(unix)]
pub use blocks::{BlocksBackend, BlocksScanner};
pub use discovery::{DiscoveryOrchestrator, DiscoveryProgress, DiscoveryReport, ScannerScanReport};
pub use discovery_server::discover_servers;
pub use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior, TransportScanner};
pub use lifecycle::{DeviceLifecycleManager, LifecycleAction};
pub use manager::{
    AsyncWriteFailure, BackendIo, BackendManager, DeviceOutputStatistics, SegmentRange,
};
pub use registry::DeviceRegistry;
pub use smbus_backend::SmBusBackend;
pub use smbus_scanner::SmBusScanner;
pub use state_machine::{
    DeviceStateMachine, DeviceStateMachineDebugSnapshot, ReconnectPolicy, ReconnectStatus,
    StateTransitionRecord,
};
pub use traits::{
    BackendInfo, DeviceBackend, DeviceDisplaySink, DeviceFrameSink, DevicePlugin, HealthStatus,
};
pub use usb_backend::{
    UsbActorMetricsSnapshot, UsbBackend, UsbProtocolConfigStore, usb_actor_metrics_snapshot,
};
pub use usb_hotplug::{UsbHotplugEvent, UsbHotplugMonitor};
pub use usb_scanner::UsbScanner;
