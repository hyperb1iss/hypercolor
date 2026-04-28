//! Hardware abstraction layer for native USB device drivers.
//!
//! `hypercolor-hal` separates pure protocol encoding from transport I/O and
//! provides a static USB device descriptor database.

pub mod attachment_profile;
pub mod database;
pub mod drivers;
pub mod protocol;
pub mod protocol_config;
pub mod registry;
pub mod smbus_registry;
pub mod transport;

pub use attachment_profile::{effective_attachment_slots, normalize_attachment_profile_slots};
pub use database::ProtocolDatabase;
pub use protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus,
};
pub use protocol_config::{ProtocolRuntimeConfig, runtime_config_for_attachment_profile};
pub use registry::{DeviceDescriptor, ProtocolBinding, ProtocolFactory, TransportType};
pub use smbus_registry::{
    ASUS_AURA_SMBUS_PROTOCOL_ID, SmBusProbe, SmBusProbeError, build_smbus_protocol,
    probe_smbus_devices_in_root,
};
pub use transport::{Transport, TransportError};
