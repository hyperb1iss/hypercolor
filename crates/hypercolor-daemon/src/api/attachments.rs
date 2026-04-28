//! Attachment template catalog endpoints.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as AxumPath, Query, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLockWriteGuard;

use hypercolor_core::attachment::{AttachmentRegistry, TemplateFilter};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::spatial::generate_positions;
use hypercolor_types::attachment::{
    AttachmentCategory, AttachmentOrigin, AttachmentTemplate, AttachmentTemplateManifest,
};

use crate::api::AppState;
use crate::api::devices::Pagination;
use crate::api::envelope::{ApiError, ApiResponse};

#[derive(Debug, Deserialize, Default)]
pub struct ListTemplatesQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub category: Option<String>,
    pub vendor: Option<String>,
    pub origin: Option<String>,
    pub q: Option<String>,
    pub controller_id: Option<String>,
    pub model: Option<String>,
    pub slot_id: Option<String>,
    pub led_min: Option<u32>,
    pub led_max: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct TemplateListResponse {
    pub items: Vec<TemplateSummary>,
    pub pagination: Pagination,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: AttachmentCategory,
    pub origin: AttachmentOrigin,
    pub led_count: u32,
    pub description: String,
    pub image_url: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TemplateDetail {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: AttachmentCategory,
    pub origin: AttachmentOrigin,
    pub led_count: u32,
    pub description: String,
    pub default_size: hypercolor_types::attachment::AttachmentCanvasSize,
    pub topology: hypercolor_types::spatial::LedTopology,
    pub led_positions: Vec<hypercolor_types::spatial::NormalizedPosition>,
    pub compatible_slots: Vec<hypercolor_types::attachment::AttachmentCompatibility>,
    pub tags: Vec<String>,
    pub led_names: Option<Vec<String>>,
    pub led_mapping: Option<Vec<u32>>,
    pub image_url: Option<String>,
    pub physical_size_mm: Option<(f32, f32)>,
}

#[derive(Debug, Serialize)]
pub struct CategoryListResponse {
    pub items: Vec<CategorySummary>,
}

#[derive(Debug, Serialize)]
pub struct CategorySummary {
    pub category: AttachmentCategory,
    pub count: usize,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct VendorListResponse {
    pub items: Vec<VendorSummary>,
}

#[derive(Debug, Serialize)]
pub struct VendorSummary {
    pub vendor: String,
    pub count: usize,
}

/// `GET /api/v1/attachments/templates`
pub async fn list_templates(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListTemplatesQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 200 {
        return ApiError::validation("limit must be between 1 and 200");
    }
    let offset = query.offset.unwrap_or(0);

    let filter = match build_filter(&query) {
        Ok(filter) => filter,
        Err(response) => return response,
    };

    let registry = state.attachment_registry.read().await;
    let templates = registry.list(&filter);
    let total = templates.len();
    let items = templates
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(template_summary)
        .collect::<Vec<_>>();
    let has_more = offset.saturating_add(limit) < total;

    ApiResponse::ok(TemplateListResponse {
        items,
        pagination: Pagination {
            offset,
            limit,
            total,
            has_more,
        },
    })
}

/// `GET /api/v1/attachments/templates/{id}`
pub async fn get_template(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let registry = state.attachment_registry.read().await;
    let Some(template) = registry.get(&id) else {
        return ApiError::not_found(format!("Attachment template not found: {id}"));
    };

    ApiResponse::ok(template_detail(template))
}

/// `POST /api/v1/attachments/templates`
pub async fn create_template(
    State(state): State<Arc<AppState>>,
    Json(mut template): Json<AttachmentTemplate>,
) -> Response {
    template.origin = AttachmentOrigin::User;

    let mut registry = state.attachment_registry.write().await;
    if registry.get(&template.id).is_some() {
        return ApiError::conflict(format!(
            "Attachment template already exists: {}",
            template.id
        ));
    }

    if let Err(response) = register_and_persist_template(&mut registry, &template) {
        return response;
    }

    ApiResponse::created(template_detail(&template))
}

/// `PUT /api/v1/attachments/templates/{id}`
pub async fn update_template(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(mut template): Json<AttachmentTemplate>,
) -> Response {
    if template.id != id {
        return ApiError::validation("template ID in path must match request body");
    }
    template.origin = AttachmentOrigin::User;

    let mut registry = state.attachment_registry.write().await;
    let Some(existing) = registry.get(&id) else {
        return ApiError::not_found(format!("Attachment template not found: {id}"));
    };
    if existing.origin == AttachmentOrigin::BuiltIn {
        return ApiError::forbidden(format!("Built-in template cannot be updated: {id}"));
    }

    if let Err(response) = register_and_persist_template(&mut registry, &template) {
        return response;
    }

    ApiResponse::ok(template_detail(&template))
}

/// `DELETE /api/v1/attachments/templates/{id}`
pub async fn delete_template(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    {
        let profiles = state.attachment_profiles.read().await;
        if profiles.uses_template(&id) {
            return ApiError::conflict(format!(
                "Attachment template is still bound in a device profile: {id}"
            ));
        }
    }

    let mut registry = state.attachment_registry.write().await;
    let Some(existing) = registry.get(&id) else {
        return ApiError::not_found(format!("Attachment template not found: {id}"));
    };
    if existing.origin == AttachmentOrigin::BuiltIn {
        return ApiError::forbidden(format!("Built-in template cannot be deleted: {id}"));
    }

    let removed = match registry.remove(&id) {
        Ok(template) => template,
        Err(error) => return ApiError::internal(error.to_string()),
    };
    if let Err(error) = delete_user_template_file(&id) {
        return ApiError::internal(error);
    }

    ApiResponse::ok(serde_json::json!({
        "id": removed.id,
        "deleted": true,
    }))
}

/// `GET /api/v1/attachments/categories`
pub async fn list_categories(State(state): State<Arc<AppState>>) -> Response {
    let registry = state.attachment_registry.read().await;
    let items = registry
        .category_counts()
        .into_iter()
        .map(|(category, count)| CategorySummary {
            label: category_label(&category),
            category,
            count,
        })
        .collect::<Vec<_>>();

    ApiResponse::ok(CategoryListResponse { items })
}

/// `GET /api/v1/attachments/vendors`
pub async fn list_vendors(State(state): State<Arc<AppState>>) -> Response {
    let registry = state.attachment_registry.read().await;
    let items = registry
        .vendor_counts()
        .into_iter()
        .map(|(vendor, count)| VendorSummary { vendor, count })
        .collect::<Vec<_>>();

    ApiResponse::ok(VendorListResponse { items })
}

#[expect(
    clippy::result_large_err,
    reason = "private handler helper returns a concrete HTTP response on validation failure"
)]
fn build_filter(query: &ListTemplatesQuery) -> Result<TemplateFilter, Response> {
    let category = query.category.as_deref().map(AttachmentCategory::from_raw);
    let origin = match query.origin.as_deref() {
        Some("built_in") => Some(AttachmentOrigin::BuiltIn),
        Some("user") => Some(AttachmentOrigin::User),
        Some(other) => {
            return Err(ApiError::validation(format!(
                "invalid origin filter: {other}"
            )));
        }
        None => None,
    };

    Ok(TemplateFilter {
        category,
        vendor: query.vendor.clone(),
        origin,
        query: query.q.clone(),
        led_min: query.led_min,
        led_max: query.led_max,
        controller_ids: query.controller_id.iter().cloned().collect(),
        model: query.model.clone(),
        slot_id: query.slot_id.clone(),
    })
}

fn template_summary(template: &AttachmentTemplate) -> TemplateSummary {
    TemplateSummary {
        id: template.id.clone(),
        name: template.name.clone(),
        vendor: template.vendor.clone(),
        category: template.category.clone(),
        origin: template.origin,
        led_count: template.led_count(),
        description: template.description.clone(),
        image_url: template.image_url.clone(),
        tags: template.tags.clone(),
    }
}

fn template_detail(template: &AttachmentTemplate) -> TemplateDetail {
    TemplateDetail {
        id: template.id.clone(),
        name: template.name.clone(),
        vendor: template.vendor.clone(),
        category: template.category.clone(),
        origin: template.origin,
        led_count: template.led_count(),
        description: template.description.clone(),
        default_size: template.default_size,
        topology: template.topology.clone(),
        led_positions: generate_positions(&template.topology),
        compatible_slots: template.compatible_slots.clone(),
        tags: template.tags.clone(),
        led_names: template.led_names.clone(),
        led_mapping: template.led_mapping.clone(),
        image_url: template.image_url.clone(),
        physical_size_mm: template.physical_size_mm,
    }
}

#[expect(
    clippy::result_large_err,
    reason = "private handler helper returns a concrete HTTP response on persistence failure"
)]
fn register_and_persist_template(
    registry: &mut RwLockWriteGuard<'_, AttachmentRegistry>,
    template: &AttachmentTemplate,
) -> Result<(), Response> {
    if let Err(error) = registry.register(template.clone()) {
        return Err(ApiError::validation(error.to_string()));
    }

    let manifest = AttachmentTemplateManifest {
        schema_version: 1,
        template: template.clone(),
    };
    let payload = toml::to_string_pretty(&manifest)
        .map_err(|error| ApiError::internal(format!("failed to serialize template: {error}")))?;
    let output_path = user_template_path(&template.id);
    if let Some(parent) = output_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return Err(ApiError::internal(format!(
            "failed to create user attachment directory: {error}"
        )));
    }
    if let Err(error) = std::fs::write(&output_path, payload) {
        return Err(ApiError::internal(format!(
            "failed to persist user template {}: {error}",
            output_path.display()
        )));
    }
    Ok(())
}

fn category_label(category: &AttachmentCategory) -> String {
    match category {
        AttachmentCategory::Aio => "AIO Coolers".to_owned(),
        AttachmentCategory::Fan => "Fans".to_owned(),
        AttachmentCategory::Strip => "LED Strips".to_owned(),
        AttachmentCategory::Strimer => "Strimers".to_owned(),
        AttachmentCategory::Case => "Cases".to_owned(),
        AttachmentCategory::Heatsink => "Heatsinks".to_owned(),
        AttachmentCategory::Radiator => "Radiators".to_owned(),
        AttachmentCategory::Matrix => "Matrices".to_owned(),
        AttachmentCategory::Ring => "Rings".to_owned(),
        AttachmentCategory::Bulb => "Bulbs".to_owned(),
        AttachmentCategory::Other(raw) => titleize(raw),
    }
}

fn titleize(raw: &str) -> String {
    raw.split(['_', '-'])
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn user_templates_root() -> PathBuf {
    ConfigManager::data_dir().join("attachments")
}

fn user_template_path(id: &str) -> PathBuf {
    user_templates_root().join(format!("{id}.toml"))
}

fn delete_user_template_file(id: &str) -> Result<(), String> {
    let root = user_templates_root();
    if !root.exists() {
        return Ok(());
    }

    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| error.to_string())?;

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            let is_toml = path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("toml"));
            if !is_toml {
                continue;
            }

            if matches_template_file(&path, id)? {
                std::fs::remove_file(&path).map_err(|error| error.to_string())?;
                return Ok(());
            }
        }
    }

    Ok(())
}

fn matches_template_file(path: &Path, id: &str) -> Result<bool, String> {
    let raw = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let manifest: AttachmentTemplateManifest =
        toml::from_str(&raw).map_err(|error| error.to_string())?;
    Ok(manifest.template.id == id)
}
