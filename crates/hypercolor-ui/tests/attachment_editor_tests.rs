#[path = "../src/api/mod.rs"]
mod api;
#[path = "../src/components/attachment_editor.rs"]
mod attachment_editor;

use api::{AttachmentBindingSummary, TemplateSummary};
use attachment_editor::{DraftRow, expand_bindings_to_drafts, summarize_channel};
use hypercolor_types::attachment::{AttachmentCategory, AttachmentSlot};

fn slot() -> AttachmentSlot {
    AttachmentSlot {
        id: "channel-1".to_owned(),
        name: "Channel 1".to_owned(),
        led_start: 0,
        led_count: 48,
        suggested_categories: vec![AttachmentCategory::Fan],
        allowed_templates: Vec::new(),
        allow_custom: true,
    }
}

fn template(id: &str, name: &str, category: AttachmentCategory, led_count: u32) -> TemplateSummary {
    TemplateSummary {
        id: id.to_owned(),
        name: name.to_owned(),
        vendor: "Lian Li".to_owned(),
        category,
        origin: None,
        led_count,
        description: String::new(),
        tags: Vec::new(),
    }
}

fn binding(
    slot_id: &str,
    template_id: &str,
    template_name: &str,
    instances: u32,
    led_offset: u32,
    name: Option<&str>,
) -> AttachmentBindingSummary {
    AttachmentBindingSummary {
        slot_id: slot_id.to_owned(),
        template_id: template_id.to_owned(),
        template_name: template_name.to_owned(),
        name: name.map(ToOwned::to_owned),
        enabled: true,
        instances,
        led_offset,
        effective_led_count: 16_u32.saturating_mul(instances.max(1)),
    }
}

fn pack_rows(rows: &[DraftRow], templates: &[TemplateSummary]) -> Vec<(u32, u32)> {
    let mut offset = 0_u32;
    rows.iter()
        .map(|row| {
            let led_count = row
                .led_count(templates)
                .expect("test rows should resolve to a template");
            let placement = (offset, offset.saturating_add(led_count));
            offset = placement.1;
            placement
        })
        .collect()
}

#[test]
fn expand_slot_bindings_splits_multi_instance_bindings_into_rows() {
    let rows = expand_bindings_to_drafts(
        "channel-1",
        &[binding(
            "channel-1",
            "lian-li-sl-unifan-fan",
            "Lian Li UNIFan SL120 - 16 LED",
            3,
            0,
            Some("Top Fan"),
        )],
        &[],
    );

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].name, "Top Fan 1");
    assert_eq!(rows[2].name, "Top Fan 3");
}

#[test]
fn expand_bindings_to_drafts_tracks_saved_binding_targets_per_instance() {
    let rows = expand_bindings_to_drafts(
        "channel-1",
        &[
            binding("channel-1", "rear-fan", "Rear Fan", 1, 16, Some("Rear")),
            binding("channel-1", "front-fan", "Front Fan", 2, 0, Some("Front")),
        ],
        &[],
    );

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].name, "Front 1");
    assert_eq!(
        rows[0]
            .persisted_target
            .expect("saved rows should carry a persisted target")
            .binding_index,
        1
    );
    assert_eq!(
        rows[1]
            .persisted_target
            .expect("second instance should carry a persisted target")
            .instance,
        1
    );
    assert_eq!(
        rows[2]
            .persisted_target
            .expect("later bindings should preserve their original index")
            .binding_index,
        0
    );
}

#[test]
fn pack_slot_rows_auto_packs_rows_without_manual_offsets() {
    let templates = vec![template(
        "lian-li-sl-unifan-fan",
        "Lian Li UNIFan SL120 - 16 LED",
        AttachmentCategory::Fan,
        16,
    )];
    let rows = vec![
        DraftRow::from_component("lian-li-sl-unifan-fan".to_owned(), String::new()),
        DraftRow::from_component("lian-li-sl-unifan-fan".to_owned(), String::new()),
        DraftRow::from_component("lian-li-sl-unifan-fan".to_owned(), String::new()),
    ];

    let packed = pack_rows(&rows, &templates);

    assert_eq!(packed.len(), 3);
    assert_eq!(packed[0].0, 0);
    assert_eq!(packed[1].0, 16);
    assert_eq!(packed[2].0, 32);
    assert_eq!(packed[2].1, 48);
}

#[test]
fn summarize_slot_rows_flags_overflow_when_rows_exceed_slot_capacity() {
    let slot = slot();
    let templates = vec![
        template(
            "fan",
            "Lian Li UNIFan SL120 - 16 LED",
            AttachmentCategory::Fan,
            16,
        ),
        template(
            "strip",
            "Lian Li O11 Dynamic Evo Front Strip - 47 LED",
            AttachmentCategory::Case,
            47,
        ),
    ];
    let rows = vec![
        DraftRow::from_component("fan".to_owned(), String::new()),
        DraftRow::from_component("strip".to_owned(), String::new()),
    ];

    let summary = summarize_channel(&slot, &rows, &templates);

    assert_eq!(summary.total_leds, 63);
    assert_eq!(summary.overflow_leds, 15);
    assert!(!summary.is_valid());
}
