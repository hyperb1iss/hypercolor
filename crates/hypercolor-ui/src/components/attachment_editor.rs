//! Channel component editor — draft rows for strip/matrix/library components.
//!
//! Draft rows support three kinds of components:
//! - **Strip**: inline definition with editable LED count (no template needed until save)
//! - **Matrix**: inline definition with editable rows × cols
//! - **Component**: reference to a known component definition from the library (TOML)
#![cfg_attr(test, allow(dead_code))]

#[cfg(test)]
use std::collections::HashMap;

use hypercolor_types::attachment::AttachmentSlot;

use crate::api::{AttachmentBindingSummary, TemplateSummary};

/// A single component in the draft editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComponentDraft {
    /// Custom strip — LED count is user-editable, template created at save time.
    Strip { led_count: u32 },
    /// Custom matrix — dimensions are user-editable, template created at save time.
    Matrix { cols: u32, rows: u32 },
    /// Known component from the library (fans, branded strips, etc.).
    Component { template_id: String },
}

/// A row in the channel component editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PersistedAttachmentTarget {
    pub binding_index: usize,
    pub instance: u32,
}

/// A row in the channel component editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DraftRow {
    pub kind: ComponentDraft,
    pub name: String,
    pub persisted_target: Option<PersistedAttachmentTarget>,
}

impl DraftRow {
    pub(crate) fn new_strip(led_count: u32) -> Self {
        Self {
            kind: ComponentDraft::Strip { led_count },
            name: String::new(),
            persisted_target: None,
        }
    }

    pub(crate) fn new_matrix(cols: u32, rows: u32) -> Self {
        Self {
            kind: ComponentDraft::Matrix { cols, rows },
            name: String::new(),
            persisted_target: None,
        }
    }

    pub(crate) fn from_component(template_id: String, name: String) -> Self {
        Self {
            kind: ComponentDraft::Component { template_id },
            name,
            persisted_target: None,
        }
    }

    /// Total LED count for this component.
    pub(crate) fn led_count(&self, templates: &[TemplateSummary]) -> Option<u32> {
        match &self.kind {
            ComponentDraft::Strip { led_count } => Some(*led_count),
            ComponentDraft::Matrix { cols, rows } => Some(cols * rows),
            ComponentDraft::Component { template_id } => templates
                .iter()
                .find(|t| t.id == *template_id)
                .map(|t| t.led_count),
        }
    }

    /// Whether this row needs a template to be created before saving.
    pub(crate) fn needs_template_creation(&self) -> bool {
        matches!(
            self.kind,
            ComponentDraft::Strip { .. } | ComponentDraft::Matrix { .. }
        )
    }

    /// Whether this row is complete (has enough info to save).
    pub(crate) fn is_complete(&self, templates: &[TemplateSummary]) -> bool {
        match &self.kind {
            ComponentDraft::Strip { led_count } => *led_count > 0,
            ComponentDraft::Matrix { cols, rows } => *cols > 0 && *rows > 0,
            ComponentDraft::Component { template_id } => {
                templates.iter().any(|t| t.id == *template_id)
            }
        }
    }
}

/// Summary of a channel's draft state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChannelDraftSummary {
    pub total_leds: u32,
    pub available_leds: u32,
    pub overflow_leds: u32,
    pub has_incomplete_rows: bool,
}

impl ChannelDraftSummary {
    #[must_use]
    pub(crate) fn is_valid(&self) -> bool {
        !self.has_incomplete_rows && self.overflow_leds == 0
    }
}

/// Expand saved bindings into draft rows.
#[must_use]
pub(crate) fn expand_bindings_to_drafts(
    slot_id: &str,
    bindings: &[AttachmentBindingSummary],
    templates: &[TemplateSummary],
) -> Vec<DraftRow> {
    let mut relevant: Vec<_> = bindings
        .iter()
        .enumerate()
        .filter(|(_, binding)| binding.slot_id == slot_id)
        .map(|(binding_index, binding)| (binding_index, binding.clone()))
        .collect();
    relevant.sort_by(|a, b| {
        a.1.led_offset
            .cmp(&b.1.led_offset)
            .then_with(|| a.1.template_name.cmp(&b.1.template_name))
            .then_with(|| a.1.template_id.cmp(&b.1.template_id))
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut rows = Vec::new();
    for (binding_index, binding) in relevant {
        for instance in 0..binding.instances.max(1) {
            let base_name = binding
                .name
                .as_ref()
                .map(|n| {
                    if binding.instances > 1 {
                        format!("{n} {}", instance + 1)
                    } else {
                        n.clone()
                    }
                })
                .unwrap_or_default();

            // Check if this is a user-created strip/matrix template
            let tmpl = templates.iter().find(|t| t.id == binding.template_id);
            let is_user = tmpl
                .and_then(|t| t.origin.as_ref())
                .map(|o| *o == hypercolor_types::attachment::AttachmentOrigin::User)
                .unwrap_or(false);

            if is_user {
                // Reconstruct as inline strip/matrix based on category
                let category = tmpl.map(|t| t.category.as_str()).unwrap_or("strip");
                match category {
                    "matrix" => {
                        // We don't know the exact dims from the summary, approximate
                        let total = binding.effective_led_count;
                        let side = (total as f32).sqrt().ceil() as u32;
                        rows.push(DraftRow {
                            kind: ComponentDraft::Matrix {
                                cols: side,
                                rows: total.div_ceil(side).max(1),
                            },
                            name: base_name,
                            persisted_target: Some(PersistedAttachmentTarget {
                                binding_index,
                                instance,
                            }),
                        });
                    }
                    _ => {
                        rows.push(DraftRow {
                            kind: ComponentDraft::Strip {
                                led_count: binding.effective_led_count,
                            },
                            name: base_name,
                            persisted_target: Some(PersistedAttachmentTarget {
                                binding_index,
                                instance,
                            }),
                        });
                    }
                }
            } else {
                let mut row = DraftRow::from_component(binding.template_id.clone(), base_name);
                row.persisted_target = Some(PersistedAttachmentTarget {
                    binding_index,
                    instance,
                });
                rows.push(row);
            }
        }
    }
    rows
}

/// Summarize channel draft state for validation.
#[must_use]
pub(crate) fn summarize_channel(
    slot: &AttachmentSlot,
    rows: &[DraftRow],
    templates: &[TemplateSummary],
) -> ChannelDraftSummary {
    let mut total_leds = 0_u32;
    let mut has_incomplete = false;

    for row in rows {
        if let Some(count) = row.led_count(templates) {
            total_leds = total_leds.saturating_add(count);
        } else {
            has_incomplete = true;
        }
        if !row.is_complete(templates) {
            has_incomplete = true;
        }
    }

    ChannelDraftSummary {
        total_leds,
        available_leds: slot.led_count,
        overflow_leds: total_leds.saturating_sub(slot.led_count),
        has_incomplete_rows: has_incomplete,
    }
}

// ── Legacy compatibility ──────────────────────────────────────────────────

/// Old draft row type — kept for the transition period.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentDraftRow {
    pub template_id: String,
    pub name: Option<String>,
}

#[cfg(test)]
impl AttachmentDraftRow {
    #[must_use]
    pub(crate) fn empty() -> Self {
        Self {
            template_id: String::new(),
            name: None,
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentRowPlacement {
    pub template_id: String,
    pub template_name: String,
    pub name: Option<String>,
    pub led_offset: u32,
    pub led_count: u32,
    pub led_end: u32,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlotDraftSummary {
    pub total_leds: u32,
    pub available_leds: u32,
    pub overflow_leds: u32,
    pub has_incomplete_rows: bool,
    pub rows: Vec<Option<AttachmentRowPlacement>>,
}

#[cfg(test)]
impl SlotDraftSummary {
    #[must_use]
    pub(crate) fn is_valid(&self) -> bool {
        !self.has_incomplete_rows && self.overflow_leds == 0
    }
}

#[cfg(test)]
#[must_use]
pub(crate) fn expand_slot_bindings(
    slot_id: &str,
    bindings: &[AttachmentBindingSummary],
) -> Vec<AttachmentDraftRow> {
    let mut relevant = bindings
        .iter()
        .filter(|binding| binding.slot_id == slot_id)
        .cloned()
        .collect::<Vec<_>>();
    relevant.sort_by(|left, right| {
        left.led_offset
            .cmp(&right.led_offset)
            .then_with(|| left.template_name.cmp(&right.template_name))
            .then_with(|| left.template_id.cmp(&right.template_id))
    });

    let mut rows = Vec::new();
    for binding in relevant {
        for instance in 0..binding.instances.max(1) {
            let name = binding.name.as_ref().map(|name| {
                if binding.instances > 1 {
                    format!("{name} {}", instance + 1)
                } else {
                    name.clone()
                }
            });
            rows.push(AttachmentDraftRow {
                template_id: binding.template_id.clone(),
                name,
            });
        }
    }

    rows
}

#[cfg(test)]
#[must_use]
pub(crate) fn summarize_slot_rows(
    slot: &AttachmentSlot,
    rows: &[AttachmentDraftRow],
    templates: &[TemplateSummary],
) -> SlotDraftSummary {
    let template_lookup: HashMap<&str, &TemplateSummary> = templates
        .iter()
        .map(|template| (template.id.as_str(), template))
        .collect();

    let mut total_leds = 0_u32;
    let mut has_incomplete_rows = false;
    let mut placements = Vec::with_capacity(rows.len());

    for row in rows {
        let template_id = row.template_id.trim();
        if template_id.is_empty() {
            has_incomplete_rows = true;
            placements.push(None);
            continue;
        }

        let Some(template) = template_lookup.get(template_id).copied() else {
            has_incomplete_rows = true;
            placements.push(None);
            continue;
        };

        let led_offset = total_leds;
        let led_count = template.led_count;
        total_leds = total_leds.saturating_add(led_count);

        placements.push(Some(AttachmentRowPlacement {
            template_id: template.id.clone(),
            template_name: template.name.clone(),
            name: row.name.clone(),
            led_offset,
            led_count,
            led_end: led_offset.saturating_add(led_count),
        }));
    }

    SlotDraftSummary {
        total_leds,
        available_leds: slot.led_count,
        overflow_leds: total_leds.saturating_sub(slot.led_count),
        has_incomplete_rows,
        rows: placements,
    }
}

#[cfg(test)]
pub(crate) fn pack_slot_rows(
    slot: &AttachmentSlot,
    rows: &[AttachmentDraftRow],
    templates: &[TemplateSummary],
) -> Result<Vec<AttachmentRowPlacement>, String> {
    let summary = summarize_slot_rows(slot, rows, templates);
    if summary.has_incomplete_rows {
        return Err("Select a component for every row".to_owned());
    }
    if summary.overflow_leds > 0 {
        return Err(format!(
            "{} needs {} LEDs but only {} are available",
            slot.name, summary.total_leds, slot.led_count
        ));
    }

    Ok(summary.rows.into_iter().flatten().collect())
}
