//! Global settings API contracts — `/api/v1/settings/*`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request body for `PUT /api/v1/settings/brightness`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SetBrightnessRequest {
    /// Master brightness percentage, 0-100.
    pub brightness: u8,
}
