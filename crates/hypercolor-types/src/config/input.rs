use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::defaults;

// ─── Input ───────────────────────────────────────────────────────────────────

/// Host keyboard/mouse capture for interactive effects.
///
/// Capture is consent-gated: `enabled` defaults to `false` and nothing opens
/// an input device until the user turns it on. Even when enabled, backends
/// only capture while an active effect declares input reactivity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Capture host keyboard state and events.
    #[serde(default = "defaults::bool_true")]
    pub keyboard: bool,

    /// Capture host pointer state and events.
    #[serde(default = "defaults::bool_true")]
    pub mouse: bool,

    /// Interaction sources routed into authoritative daemon effects.
    #[serde(default = "defaults::daemon_interaction_route")]
    pub daemon_route: InteractionRoutePolicy,

    /// Interaction sources routed into connection-scoped interactive previews.
    #[serde(default = "defaults::preview_interaction_route")]
    pub preview_route: InteractionRoutePolicy,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            keyboard: defaults::bool_true(),
            mouse: defaults::bool_true(),
            daemon_route: defaults::daemon_interaction_route(),
            preview_route: defaults::preview_interaction_route(),
        }
    }
}

/// Which interaction sources one effect consumer receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InteractionRoutePolicy {
    /// Host keyboard and pointer sources only.
    Host,
    /// The consumer's explicitly addressed browser source only.
    Browser,
    /// Host sources plus the consumer's explicitly addressed browser source.
    Merge,
}
