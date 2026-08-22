//! Config API contracts — `/api/v1/config*`.
//!
//! The config write routes take the value itself as the request body
//! (a section writes as a JSON object, a scalar as a JSON scalar), so
//! the only named request shape here is the apply-mode query.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Effective daemon config returned by `GET /api/v1/config`.
///
/// The key registry defines the individual fields. The document stays
/// open-ended here because extensions can add config sections without
/// changing the base daemon schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ConfigDocument {
    #[serde(flatten)]
    pub values: BTreeMap<String, serde_json::Value>,
}

/// Response from `GET /api/v1/config/keys/{key}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ConfigKeyResponse {
    pub key: String,
    pub value: serde_json::Value,
}

/// Outcome of a config write, key reset, or whole-config reset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ConfigMutationResponse {
    /// The mutated key, or null for a whole-config reset.
    pub key: Option<String>,
    /// The effective value after the write, rendered like any read.
    /// Null for a whole-config reset, whose payload spans every key.
    pub value: Option<serde_json::Value>,
    /// Whether the daemon re-applied the change to a running subsystem.
    pub live: bool,
    /// Whether the registry classifies this key as boot-frozen, so the
    /// persisted value only takes effect at the next daemon start.
    pub requires_restart: bool,
    /// Restart-classified roots whose persisted value now differs from
    /// the one the daemon booted with.
    pub pending_restart: Vec<String>,
    /// The config file the write landed in.
    pub path: String,
}

/// Query parameters shared by every config mutation route.
///
/// Live application is the default: a client that wants the value on
/// disk without disturbing the running daemon asks for `?live=false`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, utoipa::IntoParams,
)]
pub struct ConfigApplyQuery {
    #[serde(default = "live_apply_default")]
    #[param(required = false)]
    pub live: bool,
}

impl Default for ConfigApplyQuery {
    fn default() -> Self {
        Self {
            live: live_apply_default(),
        }
    }
}

const fn live_apply_default() -> bool {
    true
}
