//! Shared validation helpers for network driver configuration.
//!
//! These helpers let every network driver reject unsafe or non-routable
//! endpoints before they make it into discovery results or pairing flows.

use std::fmt;
use std::net::IpAddr;

/// Errors produced by the shared validation helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// Port `0` is reserved and never valid for a device endpoint.
    PortZero,
    /// Privileged ports below 1024 are not used by RGB devices.
    PrivilegedPort(u16),
    /// The IP address is unspecified, loopback, multicast, broadcast,
    /// or otherwise non-routable for RGB control.
    InvalidIp(IpAddr),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PortZero => f.write_str("port 0 is not valid"),
            Self::PrivilegedPort(port) => {
                write!(f, "privileged port {port} not allowed for RGB devices")
            }
            Self::InvalidIp(ip) => write!(f, "invalid or non-routable IP address {ip}"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Reject ports that are unsafe or uncommon for RGB device control.
///
/// Port 0 is always invalid. Privileged ports below 1024 are also rejected
/// because no RGB device protocol in Hypercolor's supported set uses them,
/// and allowing them would let discovery latch onto arbitrary system services.
///
/// # Errors
///
/// Returns [`ValidationError::PortZero`] or [`ValidationError::PrivilegedPort`]
/// if the port is unsafe.
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
/// Rejects unspecified (`0.0.0.0` / `::`), loopback, multicast, link-local
/// (IPv4), and broadcast addresses. Any IPv4 or IPv6 address outside those
/// ranges is considered routable and returned unchanged.
///
/// # Errors
///
/// Returns [`ValidationError::InvalidIp`] if the address is not routable.
pub fn validate_ip(ip: IpAddr) -> Result<IpAddr, ValidationError> {
    if ip.is_unspecified() || ip.is_multicast() || ip.is_loopback() {
        return Err(ValidationError::InvalidIp(ip));
    }
    match ip {
        IpAddr::V4(v4) if v4.is_link_local() || v4.is_broadcast() => {
            return Err(ValidationError::InvalidIp(ip));
        }
        _ => {}
    }
    Ok(ip)
}
