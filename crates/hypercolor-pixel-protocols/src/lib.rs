//! Wire encoders for the open pixel streaming protocols.
//!
//! - [`ddp`]: Distributed Display Protocol, an offset-addressed byte stream.
//! - [`e131`]: ANSI E1.31 Streaming ACN (sACN) data and sync packets.
//! - [`artnet`]: Art-Net 4 ArtDmx packets addressed by Port-Address.
//! - [`dmx`]: DMX512 universe math shared by E1.31 and Art-Net.
//!
//! Everything here is pure encoding: no sockets, no async, no device
//! policy. Drivers own transport, pacing, and configuration.

pub mod artnet;
pub mod ddp;
pub mod dmx;
pub mod e131;
