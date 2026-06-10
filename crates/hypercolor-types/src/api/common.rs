//! Cross-domain API primitives.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Pagination envelope attached to every list response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Pagination {
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    pub has_more: bool,
}
