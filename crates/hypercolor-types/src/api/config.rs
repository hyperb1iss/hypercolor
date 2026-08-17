//! Config API contracts — `/api/v1/config*`.
//!
//! The config write routes take the value itself as the request body
//! (a section writes as a JSON object, a scalar as a JSON scalar), so
//! the only named request shape here is the apply-mode query.

use serde::{Deserialize, Serialize};

/// Query parameters shared by every config mutation route.
///
/// Live application is the default: a client that wants the value on
/// disk without disturbing the running daemon asks for `?live=false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigApplyQuery {
    #[serde(default = "live_apply_default")]
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
