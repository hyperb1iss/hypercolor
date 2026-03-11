//! Shared server identity and discovery types.
//!
//! These types model Hypercolor daemon instances rather than individual RGB
//! devices, so clients can discover and connect to remote servers consistently.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// Stable identity exposed by each Hypercolor daemon instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerIdentity {
    pub instance_id: String,
    pub instance_name: String,
    pub version: String,
}

/// A Hypercolor daemon discovered on the local network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredServer {
    pub identity: ServerIdentity,
    pub host: IpAddr,
    pub port: u16,
    #[serde(default)]
    pub device_count: Option<usize>,
    pub auth_required: bool,
}
