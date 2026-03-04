//! Hardware abstraction layer for native USB device drivers.
//!
//! `hypercolor-hal` separates pure protocol encoding from transport I/O and
//! provides a static USB device descriptor database.

pub mod database;
pub mod drivers;
pub mod protocol;
pub mod transport;

pub use database::{DeviceDescriptor, ProtocolDatabase, ProtocolParams, TransportType};
pub use protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus,
};
pub use transport::{Transport, TransportError};
