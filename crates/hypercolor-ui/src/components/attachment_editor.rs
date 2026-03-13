//! Attachment editor helpers for expanding saved bindings into UI rows and
//! packing compact rows back into sequential controller spans.

use std::collections::HashMap;

use hypercolor_types::attachment::AttachmentSlot;

use crate::api::{AttachmentBindingSummary, TemplateSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentDraftRow {
    pub template_id: String,
    pub name: Option<String>,
}

impl AttachmentDraftRow {
    #[must_use]
    pub(crate) fn empty() -> Self {
        Self {
            template_id: String::new(),
            name: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentRowPlacement {
    pub template_id: String,
    pub template_name: String,
    pub name: Option<String>,
    pub led_offset: u32,
    pub led_count: u32,
    pub led_end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlotDraftSummary {
    pub total_leds: u32,
    pub available_leds: u32,
    pub overflow_leds: u32,
    pub has_incomplete_rows: bool,
    pub rows: Vec<Option<AttachmentRowPlacement>>,
}

impl SlotDraftSummary {
    #[must_use]
    pub(crate) fn is_valid(&self) -> bool {
        !self.has_incomplete_rows && self.overflow_leds == 0
    }
}

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

#[must_use]
pub(crate) fn summarize_slot_rows(
    slot: &AttachmentSlot,
    rows: &[AttachmentDraftRow],
    templates: &[TemplateSummary],
) -> SlotDraftSummary {
    let template_lookup = templates
        .iter()
        .map(|template| (template.id.as_str(), template))
        .collect::<HashMap<_, _>>();

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

pub(crate) fn pack_slot_rows(
    slot: &AttachmentSlot,
    rows: &[AttachmentDraftRow],
    templates: &[TemplateSummary],
) -> Result<Vec<AttachmentRowPlacement>, String> {
    let summary = summarize_slot_rows(slot, rows, templates);
    if summary.has_incomplete_rows {
        return Err("Select a template for every attachment row".to_owned());
    }
    if summary.overflow_leds > 0 {
        return Err(format!(
            "{} needs {} LEDs but only {} are available",
            slot.name, summary.total_leds, slot.led_count
        ));
    }

    Ok(summary.rows.into_iter().flatten().collect())
}
