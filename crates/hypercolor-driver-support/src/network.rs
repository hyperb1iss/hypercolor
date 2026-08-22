//! Validation and metadata helpers for native network drivers.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;

/// Errors produced by the shared network validation helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// Port `0` is reserved and never valid for a device endpoint.
    PortZero,
    /// Privileged ports below 1024 are not used by RGB devices.
    PrivilegedPort(u16),
    /// The IP address is not routable for RGB control.
    InvalidIp(IpAddr),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PortZero => formatter.write_str("port 0 is not valid"),
            Self::PrivilegedPort(port) => {
                write!(
                    formatter,
                    "privileged port {port} not allowed for RGB devices"
                )
            }
            Self::InvalidIp(ip) => write!(formatter, "invalid or non-routable IP address {ip}"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Reject ports that are unsafe for RGB device control.
///
/// # Errors
///
/// Returns [`ValidationError::PortZero`] or [`ValidationError::PrivilegedPort`]
/// for reserved and privileged ports.
pub fn validate_port(port: u16) -> Result<u16, ValidationError> {
    if port == 0 {
        return Err(ValidationError::PortZero);
    }
    if port < 1024 {
        return Err(ValidationError::PrivilegedPort(port));
    }
    Ok(port)
}

/// Reject IP addresses that are not routable for RGB control.
///
/// # Errors
///
/// Returns [`ValidationError::InvalidIp`] for unspecified, loopback,
/// multicast, IPv4 link-local, and IPv4 broadcast addresses.
pub fn validate_ip(ip: IpAddr) -> Result<IpAddr, ValidationError> {
    if ip.is_unspecified() || ip.is_multicast() || ip.is_loopback() {
        return Err(ValidationError::InvalidIp(ip));
    }
    match ip {
        IpAddr::V4(ipv4) if ipv4.is_link_local() || ipv4.is_broadcast() => {
            return Err(ValidationError::InvalidIp(ip));
        }
        _ => {}
    }
    Ok(ip)
}

/// Extract a trimmed metadata value if present and non-empty.
#[must_use]
pub fn metadata_value<'a>(
    metadata: Option<&'a HashMap<String, String>>,
    key: &str,
) -> Option<&'a str> {
    metadata
        .and_then(|values| values.get(key))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Parse a routable network IP from the standard `ip` metadata key.
#[must_use]
pub fn network_ip_from_metadata(metadata: Option<&HashMap<String, String>>) -> Option<IpAddr> {
    metadata
        .and_then(|values| values.get("ip"))
        .and_then(|value| value.parse::<IpAddr>().ok())
        .and_then(|ip| validate_ip(ip).ok())
}

/// Parse a validated port from a metadata key.
#[must_use]
pub fn network_port_from_metadata(
    metadata: Option<&HashMap<String, String>>,
    key: &str,
) -> Option<u16> {
    metadata
        .and_then(|values| values.get(key))
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|port| validate_port(port).ok())
}

/// Push a credential lookup key if it is not already present.
pub fn push_lookup_key(keys: &mut Vec<String>, key: String) {
    if !keys.iter().any(|existing| existing == &key) {
        keys.push(key);
    }
}
