//! Cross-domain API primitives.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Pagination envelope attached to every list response.
// `Default` is the empty page, which lets list responses mark the field
// `#[serde(default)]` and keep parsing a body that omits the envelope.
// Kept out of the doc comment on purpose: utoipa publishes the doc
// comment as the schema description, so editing it moves the generated
// OpenAPI client.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Pagination {
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    pub has_more: bool,
}
