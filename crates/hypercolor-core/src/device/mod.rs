//! Device backend system — traits, registry, and discovery orchestration.
//!
//! This module defines the output half of Hypercolor's pipeline: everything
//! needed to discover physical hardware, establish connections, and push
//! LED color data to devices over any transport (USB HID, UDP, TCP, HTTP).

mod discovery;
mod lifecycle;
pub mod manager;
pub mod mock;
mod registry;
pub mod smbus_backend;
pub mod smbus_scanner;
mod state_machine;
mod traits;
pub mod usb_backend;
pub mod usb_hotplug;
pub mod usb_scanner;
pub mod wled;

pub use discovery::{
    DiscoveredDevice, DiscoveryConnectBehavior, DiscoveryOrchestrator, DiscoveryProgress,
    DiscoveryReport, ScannerScanReport, TransportScanner,
};
pub use lifecycle::{DeviceLifecycleManager, LifecycleAction};
pub use manager::{AsyncWriteFailure, BackendIo, BackendManager, SegmentRange};
pub use registry::DeviceRegistry;
pub use smbus_backend::SmBusBackend;
pub use smbus_scanner::SmBusScanner;
pub use state_machine::{
    DeviceStateMachine, DeviceStateMachineDebugSnapshot, ReconnectPolicy, ReconnectStatus,
    StateTransitionRecord,
};
pub use traits::{BackendInfo, DeviceBackend, DevicePlugin};
pub use usb_backend::{UsbBackend, UsbProtocolConfigStore};
pub use usb_hotplug::{UsbHotplugEvent, UsbHotplugMonitor};
pub use usb_scanner::UsbScanner;
