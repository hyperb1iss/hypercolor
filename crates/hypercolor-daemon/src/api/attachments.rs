//! Attachment template catalog endpoints.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as AxumPath, Query, State};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tokio::sync::RwLockWriteGuard;

use hypercolor_core::attachment::{ComponentRegistry, TemplateFilter};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::spatial::generate_positions;
use hypercolor_types::attachment::{
    ComponentCategory, ComponentOrigin, ComponentTemplate, ComponentTemplateManifest,
};

use crate::api::AppState;
use crate::api::devices::Pagination;
use crate::api::envelope::ApiResponse;
use crate::domain::{DomainError, ResourceKind};

// Wire contracts live in hypercolor-types::api::attachments — shared
// with the web UI and the TUI.
pub use hypercolor_types::api::attachments::{
    ListTemplatesQuery, TemplateDetail, TemplateListResponse, TemplateSummary,
};

// The category and vendor facets and the per-template item routes are not
// in spec 78's Appendix A, so their shapes stay daemon-local rather than
// entering the shared contract on the way to deletion.
#[derive(Debug, Serialize)]
pub struct CategoryListResponse {
    pub items: Vec<CategorySummary>,
}

#[derive(Debug, Serialize)]
pub struct CategorySummary {
    pub category: ComponentCategory,
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
        return DomainError::validation("limit must be between 1 and 200").into_response();
    }
    let offset = query.offset.unwrap_or(0);

    let filter = match build_filter(&query) {
        Ok(filter) => filter,
        Err(error) => return error.into_response(),
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
        return DomainError::not_found(ResourceKind::AttachmentTemplate, &id).into_response();
    };

    ApiResponse::ok(template_detail(template))
}

/// `POST /api/v1/attachments/templates`
pub async fn create_template(
    State(state): State<Arc<AppState>>,
    Json(mut template): Json<ComponentTemplate>,
) -> Response {
    template.origin = ComponentOrigin::User;

    let mut registry = state.attachment_registry.write().await;
    if registry.get(&template.id).is_some() {
        return DomainError::conflict(format!(
            "Attachment template already exists: {}",
            template.id
        ))
        .into_response();
    }

    if let Err(error) = register_and_persist_template(&mut registry, &template) {
        return error.into_response();
    }

    ApiResponse::created(template_detail(&template))
}

/// `PUT /api/v1/attachments/templates/{id}`
pub async fn update_template(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(mut template): Json<ComponentTemplate>,
) -> Response {
    if template.id != id {
        return DomainError::validation("template ID in path must match request body")
            .into_response();
    }
    template.origin = ComponentOrigin::User;

    let mut registry = state.attachment_registry.write().await;
    let Some(existing) = registry.get(&id) else {
        return DomainError::not_found(ResourceKind::AttachmentTemplate, &id).into_response();
    };
    if existing.origin == ComponentOrigin::BuiltIn {
        return DomainError::forbidden(format!("Built-in template cannot be updated: {id}"))
            .into_response();
    }

    if let Err(error) = register_and_persist_template(&mut registry, &template) {
        return error.into_response();
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
            return DomainError::conflict(format!(
                "Attachment template is still bound in a device profile: {id}"
            ))
            .into_response();
        }
    }

    let mut registry = state.attachment_registry.write().await;
    let Some(existing) = registry.get(&id) else {
        return DomainError::not_found(ResourceKind::AttachmentTemplate, &id).into_response();
    };
    if existing.origin == ComponentOrigin::BuiltIn {
        return DomainError::forbidden(format!("Built-in template cannot be deleted: {id}"))
            .into_response();
    }

    let removed = match registry.remove(&id) {
        Ok(template) => template,
        Err(error) => return DomainError::Internal(anyhow::anyhow!("{error}")).into_response(),
    };
    if let Err(error) = delete_user_template_file(&id) {
        return DomainError::Internal(anyhow::anyhow!("{error}")).into_response();
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

fn build_filter(query: &ListTemplatesQuery) -> Result<TemplateFilter, DomainError> {
    let category = query.category.as_deref().map(ComponentCategory::from_raw);
    let origin = match query.origin.as_deref() {
        Some("built_in") => Some(ComponentOrigin::BuiltIn),
        Some("user") => Some(ComponentOrigin::User),
        Some(other) => {
            return Err(DomainError::validation_field(
                "origin",
                format!("invalid origin filter: {other}"),
            ));
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

fn template_summary(template: &ComponentTemplate) -> TemplateSummary {
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

fn template_detail(template: &ComponentTemplate) -> TemplateDetail {
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

fn register_and_persist_template(
    registry: &mut RwLockWriteGuard<'_, ComponentRegistry>,
    template: &ComponentTemplate,
) -> Result<(), DomainError> {
    if let Err(error) = registry.register(template.clone()) {
        return Err(DomainError::validation(error.to_string()));
    }

    let manifest = ComponentTemplateManifest {
        schema_version: 1,
        template: template.clone(),
    };
    let payload = toml::to_string_pretty(&manifest).map_err(|error| {
        DomainError::Internal(anyhow::anyhow!("failed to serialize template: {error}"))
    })?;
    let output_path = user_template_path(&template.id);
    if let Some(parent) = output_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return Err(DomainError::Internal(anyhow::anyhow!(
            "failed to create user attachment directory: {error}"
        )));
    }
    if let Err(error) = std::fs::write(&output_path, payload) {
        return Err(DomainError::Internal(anyhow::anyhow!(
            "failed to persist user template {}: {error}",
            output_path.display()
        )));
    }
    Ok(())
}

fn category_label(category: &ComponentCategory) -> String {
    match category {
        ComponentCategory::Aio => "AIO Coolers".to_owned(),
        ComponentCategory::Fan => "Fans".to_owned(),
        ComponentCategory::Strip => "LED Strips".to_owned(),
        ComponentCategory::Strimer => "Strimers".to_owned(),
        ComponentCategory::Case => "Cases".to_owned(),
        ComponentCategory::Heatsink => "Heatsinks".to_owned(),
        ComponentCategory::Radiator => "Radiators".to_owned(),
        ComponentCategory::Matrix => "Matrices".to_owned(),
        ComponentCategory::Ring => "Rings".to_owned(),
        ComponentCategory::Bulb => "Bulbs".to_owned(),
        ComponentCategory::Other(raw) => titleize(raw),
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
    let manifest: ComponentTemplateManifest =
        toml::from_str(&raw).map_err(|error| error.to_string())?;
    Ok(manifest.template.id == id)
}
