//! Ambient lighting-state snapshot assembled by the render thread and
//! injected into display faces as `engine.lighting`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What the rig is currently showing, for faces that mirror the lighting.
///
/// Dominant colors come from the spatial sampler's zone output of the
/// previous frame, quantized so the set stays stable while an effect
/// animates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LightingState {
    /// Name of the active scene, when one is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_name: Option<String>,
    /// Display names of the effects running across active zones.
    pub effect_names: Vec<String>,
    /// Up to three dominant RGB colors across the rig's LED output.
    pub dominant_colors: Vec<[u8; 3]>,
}
