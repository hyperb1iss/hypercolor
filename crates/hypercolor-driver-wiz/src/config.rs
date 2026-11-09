//! WiZ driver configuration (Spec 93 §9.3).

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::protocol::SigningKey;

/// Default frames per second sent to each light: the top of the 10 to
/// 20 Hz range several independent clients report as reliable (Spec 93
/// §7.1). WiZ publishes no limit.
pub const DEFAULT_MAX_FPS: u32 = 20;
/// Highest frame rate a user may configure.
pub const MAX_FPS_LIMIT: u32 = 60;

/// WiZ backend and discovery configuration.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WizConfig {
    /// Addresses probed directly, for networks that filter broadcast.
    #[serde(default)]
    pub device_ips: Vec<IpAddr>,

    /// Send the discovery registration as a UDP broadcast.
    #[serde(default = "default_broadcast_discovery")]
    pub broadcast_discovery: bool,

    /// Frames per second sent to each light.
    #[serde(default = "default_max_fps")]
    pub max_fps: u32,

    /// Home signing key exported from the WiZ app, for homes set to "Only
    /// verified controls". Kept out of control surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_key: Option<String>,
}

/// Debug output never shows the signing key, only whether one is set.
impl std::fmt::Debug for WizConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WizConfig")
            .field("device_ips", &self.device_ips)
            .field("broadcast_discovery", &self.broadcast_discovery)
            .field("max_fps", &self.max_fps)
            .field(
                "signing_key",
                &self.signing_key.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl Default for WizConfig {
    fn default() -> Self {
        Self {
            device_ips: Vec::new(),
            broadcast_discovery: default_broadcast_discovery(),
            max_fps: default_max_fps(),
            signing_key: None,
        }
    }
}

impl WizConfig {
    /// Check value ranges and the signing key. Device IPs are validated by
    /// the module.
    ///
    /// # Errors
    ///
    /// Returns a description of the first invalid field.
    pub fn validate_ranges(&self) -> Result<(), String> {
        if !(1..=MAX_FPS_LIMIT).contains(&self.max_fps) {
            return Err(format!(
                "max_fps must be between 1 and {MAX_FPS_LIMIT}, got {}",
                self.max_fps
            ));
        }
        self.parsed_signing_key().map(|_| ())
    }

    /// The signing key, when one is configured and non-empty.
    ///
    /// # Errors
    ///
    /// Returns a description when the key is not 32 hexadecimal characters.
    pub fn parsed_signing_key(&self) -> Result<Option<SigningKey>, String> {
        match self.signing_key.as_deref().map(str::trim) {
            None | Some("") => Ok(None),
            Some(raw) => SigningKey::parse(raw)
                .map(Some)
                .map_err(|error| error.to_string()),
        }
    }
}

const fn default_broadcast_discovery() -> bool {
    true
}

const fn default_max_fps() -> u32 {
    DEFAULT_MAX_FPS
}
