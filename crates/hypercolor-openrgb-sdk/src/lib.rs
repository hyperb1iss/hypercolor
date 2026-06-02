//! Clean OpenRGB SDK protocol support.
//!
//! This crate implements the documented OpenRGB SDK wire format without
//! depending on Hypercolor or OpenRGB implementation code.

pub mod error;
pub mod packet;
pub mod parser;
pub mod types;

pub use error::{OpenRgbError, Result};
pub use packet::{
    CLIENT_MAX_PROTOCOL_VERSION, HEADER_LEN, MAGIC, MAX_PACKET_PAYLOAD_SIZE, MIN_PROTOCOL_VERSION,
    Packet, PacketDecoder, PacketHeader, PacketId, encode_client_packet,
};
pub use parser::parse_controller_data;
pub use types::{
    ColorMode, ControllerData, ControllerMode, ControllerZone, DeviceType, LedData, MatrixMap,
    ModeFlag, ModeFlagPolicy, RgbColor, SegmentData, ZoneType,
};
