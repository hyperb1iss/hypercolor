//! The output resource contracts — `/api/v1/output` (Spec 78 §4).
//!
//! Global output has one home and two knobs. Power and brightness are
//! read together and patched together; there is no separate power
//! route, brightness route, or pause/resume verb.

use serde::{Deserialize, Serialize};

/// Global output power state, both requested and observed.
///
/// A destructive stop and a session sleep both read as `Paused`: the
/// resource says whether output is running, and a stop's extra
/// consequences are observable on the effect surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum OutputPowerMode {
    /// Render and deliver live output.
    Running,
    /// Preserve live state while holding outputs at their off frame.
    Paused,
}

/// The one output resource — `GET /api/v1/output`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct OutputResource {
    pub power: OutputPowerMode,
    /// Global brightness, `0.0..=1.0`.
    pub brightness: f32,
}

/// `PATCH /api/v1/output` — partial: either or both fields.
///
/// The range bound on `brightness` is a domain rule, not a parse rule:
/// the service rejects an out-of-range value as a validation error so
/// the caller gets a named field back instead of a decoder complaint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct OutputPatchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<OutputPowerMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f32>,
}
