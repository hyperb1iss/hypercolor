//! Network throughput snapshot produced by the net input source and
//! injected into display faces as `engine.net`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Throughput of the busiest non-loopback network interface.
///
/// Refreshed at 1 Hz by the net input source; rates are averaged over the
/// refresh interval.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NetStats {
    /// Receive rate in bytes per second.
    pub rx_bps: u64,
    /// Transmit rate in bytes per second.
    pub tx_bps: u64,
    /// Interface the rates were measured on (e.g. `enp5s0`).
    pub iface: String,
}
