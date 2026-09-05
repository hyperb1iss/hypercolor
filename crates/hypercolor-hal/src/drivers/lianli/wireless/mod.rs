//! Lian Li L-Wireless: the 2.4 GHz fan ecosystem behind a USB controller.
//!
//! The controller (`0x0416:0x8040` TX plus its `0x0416:0x8041` RX sibling)
//! tunnels RF frames over USB bulk. Fan PWM, per-LED RGB, and telemetry ride
//! the radio; the LCD on a wireless LCD fan stays wired and is a separate
//! device with its own protocol. Spec 80 sections 6 and 7 carry the wire facts.

pub mod tinyuz;
