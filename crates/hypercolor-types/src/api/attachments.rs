//! Component-template catalog API contracts — `/api/v1/attachments/*`.

use serde::{Deserialize, Serialize};

use crate::api::envelope::ListResponse;
use crate::attachment::{
    ComponentCanvasSize, ComponentCategory, ComponentCompatibility, ComponentOrigin,
};
use crate::spatial::{LedTopology, NormalizedPosition};

/// Accept an absent `origin`, an explicit `null`, or a real value.
///
/// The daemon always sends a value, so this only widens what clients
/// tolerate. It exists because the hand-rolled web UI mirrors these
/// types replaced declared `origin` as an `Option<ComponentOrigin>` and
/// so decoded an explicit `null` happily. `#[serde(default)]` alone
/// covers the absent key but not a present null, which would make the
/// shared type stricter than the mirror it replaced.
fn origin_tolerating_null<'de, D>(deserializer: D) -> Result<ComponentOrigin, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<ComponentOrigin>::deserialize(deserializer)?.unwrap_or_default())
}

/// Query parameters for `GET /api/v1/attachments/templates`.
///
/// Every field narrows the catalog; an empty query lists everything the
/// registry knows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema, utoipa::IntoParams))]
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
// `Eq` is load-bearing beyond equality: f32 is not `Eq`, so deriving it
// proves transitively that nothing in this response is a float. A float
// here would be a wire hazard, because the shapes these types replace
// were built with `json!`, which widens f32 to f64 and reprints it.
pub type TemplateListResponse = ListResponse<TemplateSummary>;

/// One template in the catalog listing.
///
/// `led_count` is the template's resolved LED total, derived from its
/// topology rather than stored, so it is always present even for
/// templates whose topology is generated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: ComponentCategory,
    #[serde(default, deserialize_with = "origin_tolerating_null")]
    pub origin: ComponentOrigin,
    pub led_count: u32,
    pub description: String,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Response for `POST /api/v1/attachments/templates`, the created
/// template.
///
/// The summary's fields plus everything needed to place the attachment:
/// `led_positions` is expanded from the topology at request time, so it
/// is present here but never in the listing.
///
/// The item routes that also return this body today (`GET` and `PUT` on
/// `/attachments/templates/{id}`) are deleted in wave 78.5, which leaves
/// creation as the only caller. The type is chartered on creation for
/// that reason, and the collection listing keeps its own summary shape.
// Unlike its sibling responses this one cannot carry the `Eq` float
// fence: `physical_size_mm` is a genuine `(f32, f32)`. That is safe
// here because the shape was already a struct on the daemon side before
// the promotion, so it never passed through `serde_json::json!` and was
// never exposed to the f32-to-f64 reprint the fence guards against. Its
// serialization path is unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TemplateDetail {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: ComponentCategory,
    #[serde(default, deserialize_with = "origin_tolerating_null")]
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
    #[cfg_attr(feature = "schema", schema(value_type = Option<Vec<f32>>, min_items = 2, max_items = 2))]
    pub physical_size_mm: Option<(f32, f32)>,
}
