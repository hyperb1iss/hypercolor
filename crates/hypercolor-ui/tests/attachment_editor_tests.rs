#[path = "../src/api/mod.rs"]
mod api;
#[path = "../src/components/attachment_editor.rs"]
mod attachment_editor;

use api::{AttachmentBindingSummary, TemplateSummary};
use attachment_editor::{
    AttachmentDraftRow, expand_slot_bindings, pack_slot_rows, summarize_slot_rows,
};
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

#[test]
fn expand_slot_bindings_splits_multi_instance_bindings_into_rows() {
    let rows = expand_slot_bindings(
        "channel-1",
        &[binding(
            "channel-1",
            "lian-li-sl-unifan-fan",
            "Lian Li UNIFan SL120 - 16 LED",
            3,
            0,
            Some("Top Fan"),
        )],
    );

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].template_id, "lian-li-sl-unifan-fan");
    assert_eq!(rows[0].name.as_deref(), Some("Top Fan 1"));
    assert_eq!(rows[2].name.as_deref(), Some("Top Fan 3"));
}

#[test]
fn pack_slot_rows_auto_packs_rows_without_manual_offsets() {
    let slot = slot();
    let templates = vec![template(
        "lian-li-sl-unifan-fan",
        "Lian Li UNIFan SL120 - 16 LED",
        AttachmentCategory::Fan,
        16,
    )];
    let rows = vec![
        AttachmentDraftRow {
            template_id: "lian-li-sl-unifan-fan".to_owned(),
            name: None,
        },
        AttachmentDraftRow {
            template_id: "lian-li-sl-unifan-fan".to_owned(),
            name: None,
        },
        AttachmentDraftRow {
            template_id: "lian-li-sl-unifan-fan".to_owned(),
            name: None,
        },
    ];

    let packed = pack_slot_rows(&slot, &rows, &templates).expect("rows should pack cleanly");

    assert_eq!(packed.len(), 3);
    assert_eq!(packed[0].led_offset, 0);
    assert_eq!(packed[1].led_offset, 16);
    assert_eq!(packed[2].led_offset, 32);
    assert_eq!(packed[2].led_end, 48);
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
        AttachmentDraftRow {
            template_id: "fan".to_owned(),
            name: None,
        },
        AttachmentDraftRow {
            template_id: "strip".to_owned(),
            name: None,
        },
    ];

    let summary = summarize_slot_rows(&slot, &rows, &templates);

    assert_eq!(summary.total_leds, 63);
    assert_eq!(summary.overflow_leds, 15);
    assert!(!summary.is_valid());
}
