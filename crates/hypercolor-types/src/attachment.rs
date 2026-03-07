//! Attachment metadata for controller ports, layout templates, and user-defined
//! accessories.
//!
//! Attachments bridge the gap between a physical controller channel and a
//! spatial layout shape. A controller exposes one or more attachment slots
//! (ports/channels with LED ranges). A template describes a reusable accessory
//! layout such as a Strimer cable, fan ring, or AIO pump halo. Users can also
//! author their own templates in TOML and bind them to slots.

use serde::{Deserialize, Serialize};

use crate::device::{DeviceInfo, DeviceTopologyHint};
use crate::spatial::LedTopology;

const CURRENT_ATTACHMENT_SCHEMA_VERSION: u32 = 1;

fn current_attachment_schema_version() -> u32 {
    CURRENT_ATTACHMENT_SCHEMA_VERSION
}

fn bool_true() -> bool {
    true
}

/// Template category used for filtering and UI grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentCategory {
    /// Lian Li Strimer-style power cable sleeves.
    Strimer,
    /// Standard fan lighting ring or fan frame.
    Fan,
    /// AIO pump/block accent or halo.
    Aio,
    /// Radiator, case-edge, or other straight accessory strip.
    Radiator,
    /// Generic linear strip.
    Strip,
    /// Generic rectangular matrix or panel.
    Matrix,
    /// Generic ring or halo.
    Ring,
    /// Single-point bulb or indicator.
    Bulb,
    /// Anything that does not fit a standard preset family.
    Other,
}

/// Where an attachment template came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentOrigin {
    /// Shipped by Hypercolor.
    #[default]
    BuiltIn,
    /// Authored by the user or imported from a pack.
    User,
}

/// Default visual footprint for placing an attachment in the layout editor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AttachmentCanvasSize {
    /// Width as a normalized fraction of the canvas.
    pub width: f32,
    /// Height as a normalized fraction of the canvas.
    pub height: f32,
}

impl Default for AttachmentCanvasSize {
    fn default() -> Self {
        Self {
            width: 0.25,
            height: 0.25,
        }
    }
}

/// Controller/slot matcher for a reusable template.
///
/// Empty matcher fields are wildcards. If a template has no compatibility
/// entries at all, it is considered globally compatible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AttachmentCompatibility {
    /// Controller family identifiers, such as `prismrgb`.
    #[serde(default)]
    pub families: Vec<String>,
    /// Optional model identifiers, such as `prism_s`.
    #[serde(default)]
    pub models: Vec<String>,
    /// Optional slot identifiers, such as `gpu`.
    #[serde(default)]
    pub slots: Vec<String>,
}

impl AttachmentCompatibility {
    /// Whether this matcher accepts the given controller/slot tuple.
    #[must_use]
    pub fn matches(
        &self,
        controller_family: &str,
        controller_model: Option<&str>,
        slot_id: &str,
    ) -> bool {
        matches_filter(&self.families, controller_family)
            && matches_optional_filter(&self.models, controller_model)
            && matches_filter(&self.slots, slot_id)
    }
}

/// Reusable attachment layout template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentTemplate {
    /// Stable template identifier.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// UI grouping category.
    pub category: AttachmentCategory,
    /// Built-in or user-authored template.
    #[serde(default)]
    pub origin: AttachmentOrigin,
    /// Optional descriptive text.
    #[serde(default)]
    pub description: String,
    /// Default visual size when dropped into a layout.
    #[serde(default)]
    pub default_size: AttachmentCanvasSize,
    /// Physical LED topology for this attachment.
    pub topology: LedTopology,
    /// Optional controller/slot filters.
    #[serde(default)]
    pub compatible_slots: Vec<AttachmentCompatibility>,
    /// Search/filter tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl AttachmentTemplate {
    /// Number of LEDs required by this template.
    #[must_use]
    pub fn led_count(&self) -> u32 {
        self.topology.led_count()
    }

    /// Whether the template can be bound to the given controller slot.
    #[must_use]
    pub fn supports_slot(
        &self,
        controller_family: &str,
        controller_model: Option<&str>,
        slot_id: &str,
    ) -> bool {
        self.compatible_slots.is_empty()
            || self
                .compatible_slots
                .iter()
                .any(|matcher| matcher.matches(controller_family, controller_model, slot_id))
    }
}

/// TOML-friendly manifest wrapper for one template file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentTemplateManifest {
    /// Schema version for migrations.
    #[serde(default = "current_attachment_schema_version")]
    pub schema_version: u32,
    /// Flattened template body.
    #[serde(flatten)]
    pub template: AttachmentTemplate,
}

impl Default for AttachmentTemplateManifest {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_ATTACHMENT_SCHEMA_VERSION,
            template: AttachmentTemplate {
                id: String::new(),
                name: String::new(),
                category: AttachmentCategory::Other,
                origin: AttachmentOrigin::BuiltIn,
                description: String::new(),
                default_size: AttachmentCanvasSize::default(),
                topology: LedTopology::Point,
                compatible_slots: Vec::new(),
                tags: Vec::new(),
            },
        }
    }
}

/// One physical controller attachment point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentSlot {
    /// Stable slot identifier.
    pub id: String,
    /// User-facing port/channel name.
    pub name: String,
    /// Inclusive LED start index on the physical controller.
    pub led_start: u32,
    /// Number of LEDs available to the slot.
    pub led_count: u32,
    /// Template categories that make sense here.
    #[serde(default)]
    pub suggested_categories: Vec<AttachmentCategory>,
    /// Explicit template IDs that should be offered regardless of category.
    #[serde(default)]
    pub allowed_templates: Vec<String>,
    /// Whether user-authored templates may be bound here.
    #[serde(default = "bool_true")]
    pub allow_custom: bool,
}

impl AttachmentSlot {
    /// Exclusive LED end index on the controller.
    #[must_use]
    pub const fn led_end_exclusive(&self) -> u32 {
        self.led_start.saturating_add(self.led_count)
    }

    /// Whether the slot can host the given template.
    #[must_use]
    pub fn supports_template(&self, template: &AttachmentTemplate) -> bool {
        if template.led_count() > self.led_count {
            return false;
        }

        let explicitly_allowed = self.allowed_templates.iter().any(|id| id == &template.id);
        let category_match = self.suggested_categories.is_empty()
            || self.suggested_categories.contains(&template.category);

        if !(category_match || explicitly_allowed) {
            return false;
        }

        if template.origin == AttachmentOrigin::User && !self.allow_custom && !explicitly_allowed {
            return false;
        }

        true
    }
}

/// Binding from a controller slot to a chosen attachment template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentBinding {
    /// Slot receiving the attachment.
    pub slot_id: String,
    /// Template identifier selected for this slot.
    pub template_id: String,
    /// Optional user-facing override for the attachment name.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether the binding is active.
    #[serde(default = "bool_true")]
    pub enabled: bool,
}

/// Per-controller attachment state persisted in TOML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAttachmentProfile {
    /// Schema version for migrations.
    #[serde(default = "current_attachment_schema_version")]
    pub schema_version: u32,
    /// Attachment points exposed by the controller.
    #[serde(default)]
    pub slots: Vec<AttachmentSlot>,
    /// Current template assignments.
    #[serde(default)]
    pub bindings: Vec<AttachmentBinding>,
}

impl Default for DeviceAttachmentProfile {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_ATTACHMENT_SCHEMA_VERSION,
            slots: Vec::new(),
            bindings: Vec::new(),
        }
    }
}

impl DeviceInfo {
    /// Derive a default attachment profile from the device's discovered zones.
    ///
    /// This gives every zone a stable slot ID and LED range even before the
    /// daemon grows a dedicated attachment registry.
    #[must_use]
    pub fn default_attachment_profile(&self) -> DeviceAttachmentProfile {
        let mut led_start = 0_u32;
        let slots = self
            .zones
            .iter()
            .map(|zone| {
                let slot = AttachmentSlot {
                    id: slugify_slot_id(&zone.name),
                    name: zone.name.clone(),
                    led_start,
                    led_count: zone.led_count,
                    suggested_categories: suggested_categories(&zone.topology),
                    allowed_templates: Vec::new(),
                    allow_custom: true,
                };
                led_start = led_start.saturating_add(zone.led_count);
                slot
            })
            .collect();

        DeviceAttachmentProfile {
            schema_version: CURRENT_ATTACHMENT_SCHEMA_VERSION,
            slots,
            bindings: Vec::new(),
        }
    }
}

fn matches_filter(filters: &[String], value: &str) -> bool {
    filters.is_empty()
        || filters
            .iter()
            .any(|candidate| candidate == "*" || candidate.eq_ignore_ascii_case(value))
}

fn matches_optional_filter(filters: &[String], value: Option<&str>) -> bool {
    if filters.is_empty() {
        return true;
    }

    value.is_some_and(|inner| matches_filter(filters, inner))
}

fn suggested_categories(topology: &DeviceTopologyHint) -> Vec<AttachmentCategory> {
    match topology {
        DeviceTopologyHint::Strip => vec![
            AttachmentCategory::Strip,
            AttachmentCategory::Radiator,
            AttachmentCategory::Other,
        ],
        DeviceTopologyHint::Matrix { .. } => vec![
            AttachmentCategory::Strimer,
            AttachmentCategory::Matrix,
            AttachmentCategory::Other,
        ],
        DeviceTopologyHint::Ring { .. } => vec![
            AttachmentCategory::Fan,
            AttachmentCategory::Aio,
            AttachmentCategory::Ring,
            AttachmentCategory::Other,
        ],
        DeviceTopologyHint::Point => {
            vec![AttachmentCategory::Bulb, AttachmentCategory::Other]
        }
        DeviceTopologyHint::Custom => vec![AttachmentCategory::Other],
    }
}

fn slugify_slot_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut previous_dash = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            previous_dash = false;
            continue;
        }

        if !out.is_empty() && !previous_dash {
            out.push('-');
            previous_dash = true;
        }
    }

    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        return "slot".to_owned();
    }

    trimmed.to_owned()
}
