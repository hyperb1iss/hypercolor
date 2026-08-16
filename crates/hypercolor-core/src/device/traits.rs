//! Core device backend traits.
//!
//! Device backends implement [`DeviceBackend`] for communication.

pub use hypercolor_driver_api::{
    BackendInfo, ConnectExecution, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId,
    DeviceDeliveryObserver, DeviceDeliveryStatus, DeviceDisplaySink, DeviceFrameSink,
    DeviceLifecyclePolicy, DeviceWriteOutcome, OutputCadence,
};
