//! WLED device backend — DDP and E1.31 protocol support for ESP32/ESP8266 controllers.
//!
//! This module provides everything needed to discover, connect to, and stream
//! pixel data to [WLED](https://kno.wled.ge/) devices over the network.
//!
//! Two streaming protocols are supported:
//!
//! - **DDP** (Distributed Display Protocol) — preferred, smaller header, no universe limits
//! - **E1.31/sACN** (Streaming ACN) — fallback for older firmware or DMX interop

pub mod backend;
mod ddp;
mod e131;
mod scanner;

pub use backend::{
    WledBackend, WledColorFormat, WledDevice, WledDeviceInfo, WledProtocol, WledSegmentInfo,
};
pub use ddp::{DdpPacket, DdpSequence, build_ddp_frame};
pub use e131::{E131Packet, E131SequenceTracker, universes_needed};
pub use scanner::WledScanner;
