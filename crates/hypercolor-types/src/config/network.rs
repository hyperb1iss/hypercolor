use serde::{Deserialize, Serialize};

use super::defaults;

// ─── Network ────────────────────────────────────────────────────────────────

/// Coarse-grained daemon API exposure mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccessMode {
    LocalOnly,
    LanTrusted,
    LanProtected,
    Custom,
}

impl Default for NetworkAccessMode {
    fn default() -> Self {
        defaults::network_access_mode()
    }
}

/// Built-in client-address scopes for network-reachable API listeners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkClientScope {
    LocalSubnets,
    PrivateRanges,
    Custom,
}

impl Default for NetworkClientScope {
    fn default() -> Self {
        defaults::network_client_scope()
    }
}

/// Network discovery and remote access settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    #[serde(default = "defaults::network_access_mode")]
    pub access_mode: NetworkAccessMode,

    #[serde(default = "defaults::network_client_scope")]
    pub client_scope: NetworkClientScope,

    #[serde(default = "defaults::bool_true")]
    pub mdns_publish: bool,

    #[serde(default = "defaults::remote_access")]
    pub remote_access: bool,

    #[serde(default)]
    pub allow_unauthenticated_remote_access: bool,

    #[serde(default)]
    pub allowed_clients: Vec<String>,

    #[serde(default)]
    pub instance_name: Option<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            access_mode: NetworkAccessMode::default(),
            client_scope: NetworkClientScope::default(),
            mdns_publish: defaults::bool_true(),
            remote_access: defaults::remote_access(),
            allow_unauthenticated_remote_access: false,
            allowed_clients: Vec::new(),
            instance_name: None,
        }
    }
}

impl NetworkConfig {
    #[must_use]
    pub const fn remote_access_enabled(&self) -> bool {
        self.remote_access
            || matches!(
                self.access_mode,
                NetworkAccessMode::LanTrusted | NetworkAccessMode::LanProtected
            )
    }

    #[must_use]
    pub const fn unauthenticated_remote_access_allowed(&self) -> bool {
        match self.access_mode {
            NetworkAccessMode::LanTrusted => true,
            NetworkAccessMode::LanProtected => false,
            NetworkAccessMode::LocalOnly | NetworkAccessMode::Custom => {
                self.allow_unauthenticated_remote_access
            }
        }
    }

    #[must_use]
    pub const fn network_bind_requires_auth(&self) -> bool {
        match self.access_mode {
            NetworkAccessMode::LanTrusted => false,
            NetworkAccessMode::LanProtected => true,
            NetworkAccessMode::LocalOnly | NetworkAccessMode::Custom => {
                !self.unauthenticated_remote_access_allowed()
            }
        }
    }
}
