use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use hypercolor_types::attachment::{
    AttachmentCategory, AttachmentOrigin, AttachmentTemplate, AttachmentTemplateManifest,
};

use super::embedded::EMBEDDED_ATTACHMENT_TEMPLATES;

/// Filter set for browsing attachment templates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateFilter {
    pub category: Option<AttachmentCategory>,
    pub vendor: Option<String>,
    pub origin: Option<AttachmentOrigin>,
    pub query: Option<String>,
    pub led_min: Option<u32>,
    pub led_max: Option<u32>,
    pub controller_ids: Vec<String>,
    pub model: Option<String>,
    pub slot_id: Option<String>,
}

/// Registry and index for built-in and user attachment templates.
#[derive(Debug, Default, Clone)]
pub struct AttachmentRegistry {
    templates: HashMap<String, AttachmentTemplate>,
    by_category: HashMap<AttachmentCategory, Vec<String>>,
    by_vendor: HashMap<String, Vec<String>>,
}

/// Attachment template registry/load errors.
#[derive(Debug, Error)]
pub enum AttachmentRegistryError {
    #[error("attachment template already exists: {0}")]
    DuplicateTemplateId(String),
    #[error("cannot remove built-in attachment template: {0}")]
    CannotRemoveBuiltIn(String),
    #[error("attachment template not found: {0}")]
    TemplateNotFound(String),
    #[error("failed to read attachment template directory {path}: {source}")]
    ReadDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read attachment template {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse attachment template {path}: {source}")]
    ParseManifest {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid attachment template '{id}': {reason}")]
    InvalidTemplate { id: String, reason: String },
}

type Result<T> = std::result::Result<T, AttachmentRegistryError>;

fn u32_to_usize(value: u32) -> usize {
    usize::try_from(value).expect("attachment LED counts should fit into usize")
}

impl AttachmentRegistry {
    /// Create an empty attachment registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load built-in templates embedded at compile time.
    pub fn load_builtins(&mut self) -> Result<usize> {
        let mut loaded = 0_usize;

        for (relative_path, raw_toml) in EMBEDDED_ATTACHMENT_TEMPLATES {
            let manifest: AttachmentTemplateManifest =
                toml::from_str(raw_toml).map_err(|source| {
                    AttachmentRegistryError::ParseManifest {
                        path: PathBuf::from(relative_path),
                        source,
                    }
                })?;
            self.register_manifest(manifest, AttachmentOrigin::BuiltIn)?;
            loaded = loaded.saturating_add(1);
        }

        Ok(loaded)
    }

    /// Load user attachment templates from a directory tree.
    pub fn load_user_dir(&mut self, path: &Path) -> Result<usize> {
        let files = collect_toml_files(path)?;
        let mut loaded = 0_usize;

        for file in files {
            let raw =
                fs::read_to_string(&file).map_err(|source| AttachmentRegistryError::ReadFile {
                    path: file.clone(),
                    source,
                })?;
            let manifest: AttachmentTemplateManifest =
                toml::from_str(&raw).map_err(|source| AttachmentRegistryError::ParseManifest {
                    path: file.clone(),
                    source,
                })?;
            self.register_manifest(manifest, AttachmentOrigin::User)?;
            loaded = loaded.saturating_add(1);
        }

        Ok(loaded)
    }

    /// Register or update one template.
    pub fn register(&mut self, template: AttachmentTemplate) -> Result<()> {
        validate_template(&template)?;

        if let Some(existing) = self.templates.get(&template.id) {
            if existing.origin == AttachmentOrigin::BuiltIn {
                return Err(AttachmentRegistryError::DuplicateTemplateId(template.id));
            }
            if template.origin != AttachmentOrigin::User {
                return Err(AttachmentRegistryError::DuplicateTemplateId(template.id));
            }
        }

        self.templates.insert(template.id.clone(), template);
        self.rebuild_indexes();
        Ok(())
    }

    /// Remove one user-defined template.
    pub fn remove(&mut self, id: &str) -> Result<AttachmentTemplate> {
        let Some(template) = self.templates.get(id) else {
            return Err(AttachmentRegistryError::TemplateNotFound(id.to_owned()));
        };
        if template.origin == AttachmentOrigin::BuiltIn {
            return Err(AttachmentRegistryError::CannotRemoveBuiltIn(id.to_owned()));
        }

        let removed = self
            .templates
            .remove(id)
            .expect("template should exist after lookup");
        self.rebuild_indexes();
        Ok(removed)
    }

    /// Get one template by ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&AttachmentTemplate> {
        self.templates.get(id)
    }

    /// Total number of templates currently loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Number of built-in templates.
    #[must_use]
    pub fn builtin_count(&self) -> usize {
        self.templates
            .values()
            .filter(|template| template.origin == AttachmentOrigin::BuiltIn)
            .count()
    }

    /// Number of user templates.
    #[must_use]
    pub fn user_count(&self) -> usize {
        self.templates
            .values()
            .filter(|template| template.origin == AttachmentOrigin::User)
            .count()
    }

    /// Browse templates with optional filtering.
    #[must_use]
    pub fn list(&self, filter: &TemplateFilter) -> Vec<&AttachmentTemplate> {
        let mut templates: Vec<&AttachmentTemplate> = self
            .templates
            .values()
            .filter(|template| template_matches_filter(template, filter))
            .collect();
        templates.sort_by(|left, right| {
            left.vendor
                .to_ascii_lowercase()
                .cmp(&right.vendor.to_ascii_lowercase())
                .then_with(|| {
                    left.name
                        .to_ascii_lowercase()
                        .cmp(&right.name.to_ascii_lowercase())
                })
                .then_with(|| left.id.cmp(&right.id))
        });
        templates
    }

    /// Find templates compatible with a controller slot and LED budget.
    #[must_use]
    pub fn compatible_with(
        &self,
        controller_ids: &[String],
        model: Option<&str>,
        slot_id: &str,
        max_leds: u32,
    ) -> Vec<&AttachmentTemplate> {
        let mut templates: Vec<&AttachmentTemplate> = self
            .templates
            .values()
            .filter(|template| template.led_count() <= max_leds)
            .filter(|template| {
                controller_ids.is_empty()
                    || controller_ids
                        .iter()
                        .any(|controller_id| template.supports_slot(controller_id, model, slot_id))
            })
            .collect();
        templates.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        templates
    }

    /// Category index snapshot for metadata APIs.
    #[must_use]
    pub fn category_counts(&self) -> Vec<(AttachmentCategory, usize)> {
        let mut items: Vec<_> = self
            .by_category
            .iter()
            .map(|(category, ids)| (category.clone(), ids.len()))
            .collect();
        items.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
        items
    }

    /// Vendor index snapshot for metadata APIs.
    #[must_use]
    pub fn vendor_counts(&self) -> Vec<(String, usize)> {
        let mut items: Vec<_> = self
            .by_vendor
            .iter()
            .map(|(vendor, ids)| (vendor.clone(), ids.len()))
            .collect();
        items.sort_by(|left, right| {
            left.0
                .to_ascii_lowercase()
                .cmp(&right.0.to_ascii_lowercase())
        });
        items
    }

    fn register_manifest(
        &mut self,
        mut manifest: AttachmentTemplateManifest,
        origin: AttachmentOrigin,
    ) -> Result<()> {
        manifest.template.origin = origin;
        self.register(manifest.template)
    }

    fn rebuild_indexes(&mut self) {
        self.by_category.clear();
        self.by_vendor.clear();

        for (id, template) in &self.templates {
            self.by_category
                .entry(template.category.clone())
                .or_default()
                .push(id.clone());
            self.by_vendor
                .entry(template.vendor.clone())
                .or_default()
                .push(id.clone());
        }

        for ids in self.by_category.values_mut() {
            ids.sort();
        }
        for ids in self.by_vendor.values_mut() {
            ids.sort();
        }
    }
}

fn collect_toml_files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|source| AttachmentRegistryError::ReadDirectory {
                path: dir.clone(),
                source,
            })?;

        for entry in entries {
            let entry = entry.map_err(|source| AttachmentRegistryError::ReadDirectory {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            let file_type =
                entry
                    .file_type()
                    .map_err(|source| AttachmentRegistryError::ReadDirectory {
                        path: path.clone(),
                        source,
                    })?;

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            let is_toml = path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("toml"));
            if is_toml {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn template_matches_filter(template: &AttachmentTemplate, filter: &TemplateFilter) -> bool {
    if filter
        .category
        .as_ref()
        .is_some_and(|category| !category.matches_category(&template.category))
    {
        return false;
    }

    if filter
        .origin
        .is_some_and(|origin| template.origin != origin)
    {
        return false;
    }

    if filter
        .vendor
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|vendor| !template.vendor.eq_ignore_ascii_case(vendor))
    {
        return false;
    }

    let led_count = template.led_count();
    if filter.led_min.is_some_and(|minimum| led_count < minimum) {
        return false;
    }
    if filter.led_max.is_some_and(|maximum| led_count > maximum) {
        return false;
    }

    if filter.query.as_deref().is_some_and(|query| {
        let needle = query.trim().to_ascii_lowercase();
        !needle.is_empty() && !template_matches_query(template, &needle)
    }) {
        return false;
    }

    matches_compatibility_filter(
        template,
        &filter.controller_ids,
        filter.model.as_deref(),
        filter.slot_id.as_deref(),
    )
}

fn template_matches_query(template: &AttachmentTemplate, needle: &str) -> bool {
    template.name.to_ascii_lowercase().contains(needle)
        || template.id.to_ascii_lowercase().contains(needle)
        || template.vendor.to_ascii_lowercase().contains(needle)
        || template.description.to_ascii_lowercase().contains(needle)
        || template
            .tags
            .iter()
            .any(|tag| tag.to_ascii_lowercase().contains(needle))
}

fn matches_compatibility_filter(
    template: &AttachmentTemplate,
    controller_ids: &[String],
    model: Option<&str>,
    slot_id: Option<&str>,
) -> bool {
    if controller_ids.is_empty() && model.is_none() && slot_id.is_none() {
        return true;
    }

    if template.compatible_slots.is_empty() {
        return true;
    }

    template.compatible_slots.iter().any(|matcher| {
        values_match_filter(&matcher.families, controller_ids)
            && value_matches_filter(&matcher.models, model)
            && value_matches_filter(&matcher.slots, slot_id)
    })
}

fn values_match_filter(filters: &[String], values: &[String]) -> bool {
    filters.is_empty()
        || values.is_empty()
        || values
            .iter()
            .any(|value| filters.iter().any(|filter| filter == value))
}

fn value_matches_filter(filters: &[String], value: Option<&str>) -> bool {
    if filters.is_empty() {
        return true;
    }

    value.is_some_and(|value| {
        filters
            .iter()
            .any(|candidate| candidate == "*" || candidate.eq_ignore_ascii_case(value))
    })
}

fn validate_template(template: &AttachmentTemplate) -> Result<()> {
    if template.id.trim().is_empty() {
        return Err(AttachmentRegistryError::InvalidTemplate {
            id: template.id.clone(),
            reason: "id must not be empty".to_owned(),
        });
    }
    if template.name.trim().is_empty() {
        return Err(AttachmentRegistryError::InvalidTemplate {
            id: template.id.clone(),
            reason: "name must not be empty".to_owned(),
        });
    }
    if template.vendor.trim().is_empty() {
        return Err(AttachmentRegistryError::InvalidTemplate {
            id: template.id.clone(),
            reason: "vendor must not be empty".to_owned(),
        });
    }

    let led_count = template.led_count();
    if led_count == 0 {
        return Err(AttachmentRegistryError::InvalidTemplate {
            id: template.id.clone(),
            reason: "topology must define at least one LED".to_owned(),
        });
    }

    if let Some(led_names) = &template.led_names
        && led_names.len() != u32_to_usize(led_count)
    {
        return Err(AttachmentRegistryError::InvalidTemplate {
            id: template.id.clone(),
            reason: format!(
                "led_names length {} does not match topology LED count {}",
                led_names.len(),
                led_count
            ),
        });
    }

    if let Some(mapping) = &template.led_mapping {
        if mapping.len() != u32_to_usize(led_count) {
            return Err(AttachmentRegistryError::InvalidTemplate {
                id: template.id.clone(),
                reason: format!(
                    "led_mapping length {} does not match topology LED count {}",
                    mapping.len(),
                    led_count
                ),
            });
        }

        let mut seen = HashSet::with_capacity(mapping.len());
        for &index in mapping {
            if index >= led_count {
                return Err(AttachmentRegistryError::InvalidTemplate {
                    id: template.id.clone(),
                    reason: format!("led_mapping index {index} exceeds LED count {led_count}"),
                });
            }
            if !seen.insert(index) {
                return Err(AttachmentRegistryError::InvalidTemplate {
                    id: template.id.clone(),
                    reason: format!("led_mapping index {index} is duplicated"),
                });
            }
        }
    }

    Ok(())
}
