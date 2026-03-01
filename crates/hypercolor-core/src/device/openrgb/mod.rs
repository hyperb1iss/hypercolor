//! `OpenRGB` SDK backend for direct communication with `OpenRGB` servers.
//!
//! This module implements the binary wire protocol (TCP port 6742),
//! controller enumeration, LED color updates, and transport scanning
//! for devices managed by `OpenRGB`.
//!
//! # Architecture
//!
//! ```text
//! OpenRgbScanner (TransportScanner)
//!     |
//!     | TCP probe + enumerate
//!     v
//! OpenRgbClient (SDK wire protocol)
//!     |
//!     | TCP port 6742
//!     v
//! OpenRGB Server
//! ```
//!
//! # Modules
//!
//! - [`proto`] — Binary wire protocol serialization/deserialization
//! - [`client`] — TCP client with handshake, enumeration, and reconnection
//! - [`backend`] — [`DeviceBackend`](super::DeviceBackend) implementation
//! - [`scanner`] — [`TransportScanner`](super::TransportScanner) implementation

pub mod backend;
pub mod client;
pub mod proto;
pub mod scanner;

pub use backend::OpenRgbBackend;
pub use client::{ClientConfig, ConnectionState, OpenRgbClient, ReconnectPolicy};
pub use proto::{
    Command, ControllerData, HEADER_SIZE, MAGIC, PacketHeader, RgbColor, ZoneData, ZoneType,
};
pub use scanner::{OpenRgbScanner, ScannerConfig};
