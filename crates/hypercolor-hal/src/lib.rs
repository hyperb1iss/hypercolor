//! Hardware abstraction layer for native USB device drivers.
//!
//! `hypercolor-hal` separates pure protocol encoding from transport I/O and
//! provides a static USB device descriptor database.

pub mod attachment_profile;
pub mod database;
pub mod display;
pub mod drivers;
pub mod protocol;
pub mod protocol_config;
pub mod registry;
pub mod smbus_registry;
pub mod transport;

pub use attachment_profile::{effective_attachment_slots, normalize_attachment_profile_slots};
pub use database::ProtocolDatabase;
pub use display::{
    ChunkCommandPolicy, ChunkContext, DisplayChunkLayout, DisplayEncodeError, LineRepack,
    Packed16Format, PrefixContext, RepackError, WireKeepalive, encode_chunked_display_frame,
    encode_chunked_display_frame_into, encode_prefixed_display_frame,
    encode_prefixed_display_frame_into,
};
pub use protocol::{Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ResponseStatus};
pub use protocol_config::{ProtocolRuntimeConfig, runtime_config_for_attachment_profile};
pub use registry::{
    DeviceDescriptor, ProtocolBinding, ProtocolFactory, SerialQuirk, TransportConnectExecution,
    TransportLifecycleHints, TransportType, UsbTransportBinding, UsbTransportFactory,
    UsbTransportFuture, UsbTransportKind, UsbTransportOpenRequest,
};
pub use smbus_registry::{
    ASUS_AURA_SMBUS_PROTOCOL_ID, SmBusProbe, SmBusProbeError, build_smbus_protocol,
    probe_smbus_devices_in_root, probe_smbus_devices_system,
};
pub use transport::{Transport, TransportError};
