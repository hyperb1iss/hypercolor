//! Hardware abstraction layer for native USB device drivers.
//!
//! `hypercolor-hal` separates pure protocol encoding from transport I/O and
//! provides a static USB device descriptor database.

pub mod database;
pub mod drivers;
pub mod protocol;
pub mod registry;
pub mod transport;

pub use database::ProtocolDatabase;
pub use protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus,
};
pub use registry::{DeviceDescriptor, ProtocolBinding, ProtocolFactory, TransportType};
pub use transport::{Transport, TransportError};
