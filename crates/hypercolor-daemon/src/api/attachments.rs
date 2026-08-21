//! Attachment template catalog endpoints.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use tokio::sync::RwLockWriteGuard;

use hypercolor_core::attachment::{ComponentRegistry, TemplateFilter};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::spatial::generate_positions;
use hypercolor_types::attachment::{
    ComponentCategory, ComponentOrigin, ComponentTemplate, ComponentTemplateManifest,
};

use crate::api::envelope;
use crate::app_state::AppState;
use crate::domain::DomainError;

// Wire contracts live in hypercolor-types::api::attachments — shared
// with the web UI and the TUI.
pub use hypercolor_types::api::attachments::{
    ListTemplatesQuery, TemplateDetail, TemplateListResponse, TemplateSummary,
};

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

    envelope::ok(TemplateListResponse {
        items,
        total: u64::try_from(total).expect("attachment template count fits in u64"),
        page: Some(hypercolor_types::api::PageInfo {
            offset: u64::try_from(offset).expect("attachment template offset fits in u64"),
            limit: u64::try_from(limit).expect("attachment template limit fits in u64"),
            has_more,
        }),
    })
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

    envelope::created(template_detail(&template))
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

fn user_templates_root() -> PathBuf {
    ConfigManager::data_dir().join("attachments")
}

fn user_template_path(id: &str) -> PathBuf {
    user_templates_root().join(format!("{id}.toml"))
}
