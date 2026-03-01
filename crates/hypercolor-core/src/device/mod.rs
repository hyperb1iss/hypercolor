//! Device backend system — traits, registry, and discovery orchestration.
//!
//! This module defines the output half of Hypercolor's pipeline: everything
//! needed to discover physical hardware, establish connections, and push
//! LED color data to devices over any transport (USB HID, UDP, TCP, HTTP).

mod discovery;
mod registry;
mod traits;

pub use discovery::{DiscoveredDevice, DiscoveryOrchestrator, DiscoveryReport, TransportScanner};
pub use registry::DeviceRegistry;
pub use traits::{BackendInfo, DeviceBackend, DevicePlugin};
