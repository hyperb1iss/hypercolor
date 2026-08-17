//! Component-template catalog API contracts — `/api/v1/attachments/*`.

use serde::{Deserialize, Serialize};

use crate::api::common::Pagination;
use crate::attachment::{
    ComponentCanvasSize, ComponentCategory, ComponentCompatibility, ComponentOrigin,
};
use crate::spatial::{LedTopology, NormalizedPosition};

/// Query parameters for `GET /api/v1/attachments/templates`.
///
/// Every field narrows the catalog; an empty query lists everything the
/// registry knows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTemplatesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Free-text filter over template name and description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Restrict to templates a given controller can host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_max: Option<u32>,
}

/// Response for `GET /api/v1/attachments/templates`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateListResponse {
    #[serde(default)]
    pub items: Vec<TemplateSummary>,
    pub pagination: Pagination,
}

/// One template in the catalog listing.
///
/// `led_count` is the template's resolved LED total, derived from its
/// topology rather than stored, so it is always present even for
/// templates whose topology is generated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: ComponentCategory,
    #[serde(default)]
    pub origin: ComponentOrigin,
    pub led_count: u32,
    pub description: String,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Response for `GET`, `POST`, and `PUT` on a single template.
///
/// The summary's fields plus everything needed to place the attachment:
/// `led_positions` is expanded from the topology at request time, so it
/// is present here but never in the listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateDetail {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: ComponentCategory,
    #[serde(default)]
    pub origin: ComponentOrigin,
    pub led_count: u32,
    pub description: String,
    pub default_size: ComponentCanvasSize,
    pub topology: LedTopology,
    #[serde(default)]
    pub led_positions: Vec<NormalizedPosition>,
    #[serde(default)]
    pub compatible_slots: Vec<ComponentCompatibility>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub led_names: Option<Vec<String>>,
    #[serde(default)]
    pub led_mapping: Option<Vec<u32>>,
    #[serde(default)]
    pub image_url: Option<String>,
    /// Physical footprint in millimeters, as `[width, height]`.
    #[serde(default)]
    pub physical_size_mm: Option<(f32, f32)>,
}

/// Response for `DELETE /api/v1/attachments/templates/{id}`.
///
/// Built-in templates cannot be deleted, so a success here always means
/// a user-authored template was removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteTemplateResponse {
    pub id: String,
    pub deleted: bool,
}

/// Response for `GET /api/v1/attachments/categories`.
///
/// Unpaginated: the category set is bounded by the catalog's own
/// vocabulary rather than by how many templates are installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryListResponse {
    #[serde(default)]
    pub items: Vec<CategorySummary>,
}

/// One category and how many templates carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategorySummary {
    pub category: ComponentCategory,
    pub count: usize,
    /// Display-ready category name, titleized for unknown categories.
    pub label: String,
}

/// Response for `GET /api/v1/attachments/vendors`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorListResponse {
    #[serde(default)]
    pub items: Vec<VendorSummary>,
}

/// One vendor and how many templates it has in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorSummary {
    pub vendor: String,
    pub count: usize,
}
