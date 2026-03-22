//! Attachment metadata for controller ports, layout templates, and user-defined
//! accessories.
//!
//! Attachments bridge the gap between a physical controller channel and a
//! spatial layout shape. A controller exposes one or more attachment slots
//! (ports/channels with LED ranges). A template describes a reusable accessory
//! layout such as a Strimer cable, fan ring, or AIO pump halo. Users can also
//! author their own templates in TOML and bind them to slots.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::device::{DeviceFamily, DeviceInfo, DeviceTopologyHint, ZoneInfo};
use crate::spatial::LedTopology;

const CURRENT_ATTACHMENT_SCHEMA_VERSION: u32 = 1;

fn current_attachment_schema_version() -> u32 {
    CURRENT_ATTACHMENT_SCHEMA_VERSION
}

fn bool_true() -> bool {
    true
}

fn default_binding_instances() -> u32 {
    1
}

fn other_attachment_category() -> AttachmentCategory {
    AttachmentCategory::Other("other".to_owned())
}

/// Template category used for filtering and UI grouping.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttachmentCategory {
    /// Standard fan lighting ring or fan frame.
    Fan,
    /// Generic linear strip.
    Strip,
    /// AIO pump/block accent or halo.
    Aio,
    /// Lian Li Strimer-style power cable sleeves.
    Strimer,
    /// Case accent lighting.
    Case,
    /// CPU tower coolers with integrated lighting.
    Heatsink,
    /// Radiator, case-edge, or other straight accessory strip.
    Radiator,
    /// Generic rectangular matrix or panel.
    Matrix,
    /// Generic ring or halo.
    Ring,
    /// Single-point bulb or indicator.
    Bulb,
    /// Anything that does not fit a standard preset family.
    Other(String),
}

impl AttachmentCategory {
    /// Stable string form used in serialized templates and API filters.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Fan => "fan",
            Self::Strip => "strip",
            Self::Aio => "aio",
            Self::Strimer => "strimer",
            Self::Case => "case",
            Self::Heatsink => "heatsink",
            Self::Radiator => "radiator",
            Self::Matrix => "matrix",
            Self::Ring => "ring",
            Self::Bulb => "bulb",
            Self::Other(value) => value.as_str(),
        }
    }

    /// Parse a serialized or user-provided category string.
    #[must_use]
    pub fn from_raw(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "fan" => Self::Fan,
            "strip" => Self::Strip,
            "aio" => Self::Aio,
            "strimer" => Self::Strimer,
            "case" => Self::Case,
            "heatsink" => Self::Heatsink,
            "radiator" => Self::Radiator,
            "matrix" => Self::Matrix,
            "ring" => Self::Ring,
            "bulb" => Self::Bulb,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Whether two categories should be treated as the same UI/filter bucket.
    #[must_use]
    pub fn matches_category(&self, other: &Self) -> bool {
        self == other || matches!((self, other), (Self::Other(_), Self::Other(_)))
    }
}

impl Serialize for AttachmentCategory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AttachmentCategory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_raw(&raw))
    }
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
    /// Accessory vendor or ecosystem (e.g. `Lian Li`, `Corsair`).
    #[serde(default)]
    pub vendor: String,
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
    /// Optional user-facing labels for each LED.
    #[serde(default)]
    pub led_names: Option<Vec<String>>,
    /// Optional spatial-index -> physical-index remapping table.
    #[serde(default)]
    pub led_mapping: Option<Vec<u32>>,
    /// Optional product/marketing image URL.
    #[serde(default)]
    pub image_url: Option<String>,
    /// Optional physical dimensions in millimeters.
    #[serde(default)]
    pub physical_size_mm: Option<(f32, f32)>,
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
                category: other_attachment_category(),
                origin: AttachmentOrigin::BuiltIn,
                description: String::new(),
                vendor: String::new(),
                default_size: AttachmentCanvasSize::default(),
                topology: LedTopology::Point,
                compatible_slots: Vec::new(),
                tags: Vec::new(),
                led_names: None,
                led_mapping: None,
                image_url: None,
                physical_size_mm: None,
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
            || self
                .suggested_categories
                .iter()
                .any(|category| category.matches_category(&template.category));

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
    /// Number of chained template instances bound to the slot.
    #[serde(default = "default_binding_instances")]
    pub instances: u32,
    /// LED offset within the slot where the binding begins.
    #[serde(default)]
    pub led_offset: u32,
}

impl AttachmentBinding {
    /// Effective LED span for this binding given the bound template size.
    #[must_use]
    pub fn effective_led_count(&self, template: &AttachmentTemplate) -> u32 {
        template
            .led_count()
            .saturating_mul(self.instances.max(default_binding_instances()))
    }
}

/// Attachment-derived zone suggestion for layout import and preview flows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentSuggestedZone {
    /// Source slot ID on the physical controller.
    pub slot_id: String,
    /// Bound attachment template identifier.
    pub template_id: String,
    /// Bound attachment template display name.
    pub template_name: String,
    /// Final user-facing zone name for this attachment instance.
    pub name: String,
    /// Zero-based attachment instance index within the binding.
    pub instance: u32,
    /// Inclusive LED start index on the physical controller.
    pub led_start: u32,
    /// Number of LEDs consumed by this instance.
    pub led_count: u32,
    /// Attachment category for UI grouping and shape defaults.
    pub category: AttachmentCategory,
    /// Suggested default layout footprint.
    pub default_size: AttachmentCanvasSize,
    /// Imported topology for spatial sampling.
    pub topology: LedTopology,
    /// Optional spatial-order -> physical-order LED remapping.
    #[serde(default)]
    pub led_mapping: Option<Vec<u32>>,
}

/// Per-controller attachment state persisted in TOML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Attachment-derived zones ready for layout import.
    #[serde(default)]
    pub suggested_zones: Vec<AttachmentSuggestedZone>,
}

impl Default for DeviceAttachmentProfile {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_ATTACHMENT_SCHEMA_VERSION,
            slots: Vec::new(),
            bindings: Vec::new(),
            suggested_zones: Vec::new(),
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
        let mut slot_ids: HashMap<String, u32> = HashMap::new();
        let slots = self
            .zones
            .iter()
            .map(|zone| {
                let slot_id = dedupe_slot_id(&mut slot_ids, &slugify_slot_id(&zone.name));
                let slot = AttachmentSlot {
                    id: slot_id,
                    name: zone.name.clone(),
                    led_start,
                    led_count: zone.led_count,
                    suggested_categories: slot_suggested_categories(self, zone),
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
            suggested_zones: Vec::new(),
        }
    }
}

fn slot_suggested_categories(device: &DeviceInfo, zone: &ZoneInfo) -> Vec<AttachmentCategory> {
    let mut categories = suggested_categories(&zone.topology);

    if is_prismrgb_channel_slot(device, zone) {
        for category in [
            AttachmentCategory::Fan,
            AttachmentCategory::Aio,
            AttachmentCategory::Heatsink,
            AttachmentCategory::Ring,
        ] {
            if !categories.contains(&category) {
                categories.push(category);
            }
        }
    }

    categories
}

fn is_prismrgb_channel_slot(device: &DeviceInfo, zone: &ZoneInfo) -> bool {
    device.family == DeviceFamily::PrismRgb
        && matches!(
            device.model.as_deref(),
            Some("prism_8" | "nollie_8_v2" | "prism_mini")
        )
        && matches!(zone.topology, DeviceTopologyHint::Strip)
        && zone.name.starts_with("Channel ")
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
            AttachmentCategory::Case,
            AttachmentCategory::Radiator,
            other_attachment_category(),
        ],
        DeviceTopologyHint::Matrix { .. } => vec![
            AttachmentCategory::Strimer,
            AttachmentCategory::Matrix,
            other_attachment_category(),
        ],
        DeviceTopologyHint::Ring { .. } => vec![
            AttachmentCategory::Fan,
            AttachmentCategory::Aio,
            AttachmentCategory::Heatsink,
            AttachmentCategory::Ring,
            other_attachment_category(),
        ],
        DeviceTopologyHint::Point => {
            vec![AttachmentCategory::Bulb, other_attachment_category()]
        }
        DeviceTopologyHint::Display { .. } => {
            vec![AttachmentCategory::Matrix, other_attachment_category()]
        }
        DeviceTopologyHint::Custom => vec![other_attachment_category()],
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

fn dedupe_slot_id(slot_ids: &mut HashMap<String, u32>, base_id: &str) -> String {
    let next_index = slot_ids
        .entry(base_id.to_owned())
        .and_modify(|count| *count = count.saturating_add(1))
        .or_insert(1);

    if *next_index == 1 {
        base_id.to_owned()
    } else {
        format!("{base_id}-{next_index}")
    }
}
