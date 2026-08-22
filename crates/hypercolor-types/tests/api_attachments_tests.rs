//! Attachment-template catalog contract tests.
//!
//! These pin the client-tolerance the shared types inherited from the
//! hand-rolled web UI mirrors they replaced. The mirrors declared
//! `origin` as an `Option<ComponentOrigin>`, so an explicit `null`
//! decoded fine; promoting the field to an unconditional
//! `ComponentOrigin` must not make that JSON stop parsing.

use hypercolor_types::api::attachments::{TemplateDetail, TemplateListResponse, TemplateSummary};
use hypercolor_types::attachment::{ComponentCategory, ComponentOrigin};
use serde_json::json;

fn summary_json(origin: Option<serde_json::Value>) -> serde_json::Value {
    let mut value = json!({
        "id": "ll-sl-inf",
        "name": "SL Infinity",
        "vendor": "Lian Li",
        "category": "fan",
        "led_count": 16,
        "description": "120mm fan",
    });
    if let Some(origin) = origin {
        value["origin"] = origin;
    }
    value
}

fn detail_json(origin: Option<serde_json::Value>) -> serde_json::Value {
    let mut value = json!({
        "id": "ll-sl-inf",
        "name": "SL Infinity",
        "vendor": "Lian Li",
        "category": "fan",
        "led_count": 2,
        "description": "120mm fan",
        "default_size": { "width": 0.1, "height": 0.1 },
        "topology": {
            "type": "ring",
            "count": 2,
            "start_angle": 0.0,
            "direction": "clockwise"
        },
    });
    if let Some(origin) = origin {
        value["origin"] = origin;
    }
    value
}

// ── Deserialization tolerance ───────────────────────────────────────────

#[test]
fn template_summary_decodes_an_explicit_null_origin_as_built_in() {
    let summary: TemplateSummary =
        serde_json::from_value(summary_json(Some(serde_json::Value::Null)))
            .expect("an explicit null origin must still decode");

    assert_eq!(summary.origin, ComponentOrigin::BuiltIn);
}

#[test]
fn template_summary_decodes_an_absent_origin_as_built_in() {
    let summary: TemplateSummary =
        serde_json::from_value(summary_json(None)).expect("an absent origin must still decode");

    assert_eq!(summary.origin, ComponentOrigin::BuiltIn);
}

#[test]
fn template_summary_keeps_an_explicit_origin() {
    let summary: TemplateSummary = serde_json::from_value(summary_json(Some(json!("user"))))
        .expect("an explicit origin must decode");

    assert_eq!(summary.origin, ComponentOrigin::User);
}

#[test]
fn template_detail_decodes_an_explicit_null_origin_as_built_in() {
    let detail: TemplateDetail = serde_json::from_value(detail_json(Some(serde_json::Value::Null)))
        .expect("an explicit null origin must still decode");

    assert_eq!(detail.origin, ComponentOrigin::BuiltIn);
}

#[test]
fn template_detail_decodes_an_absent_origin_as_built_in() {
    let detail: TemplateDetail =
        serde_json::from_value(detail_json(None)).expect("an absent origin must still decode");

    assert_eq!(detail.origin, ComponentOrigin::BuiltIn);
}

#[test]
fn template_detail_keeps_an_explicit_origin() {
    let detail: TemplateDetail =
        serde_json::from_value(detail_json(Some(json!("user")))).expect("origin must decode");

    assert_eq!(detail.origin, ComponentOrigin::User);
}

// ── Serialization is unchanged ──────────────────────────────────────────

#[test]
fn template_summary_always_serializes_origin() {
    // The tolerance above is deserialize-only. The daemon still writes
    // the key unconditionally, including for the default variant, so a
    // `skip_serializing_if` creeping in would be a wire change.
    let summary = TemplateSummary {
        id: "ll-sl-inf".to_owned(),
        name: "SL Infinity".to_owned(),
        vendor: "Lian Li".to_owned(),
        category: ComponentCategory::Fan,
        origin: ComponentOrigin::BuiltIn,
        led_count: 16,
        description: "120mm fan".to_owned(),
        image_url: None,
        tags: Vec::new(),
    };

    let value = serde_json::to_value(&summary).expect("summary must serialize");

    assert_eq!(value["origin"], json!("built_in"));
    // `image_url` is likewise emitted as an explicit null rather than
    // dropped, which is what the pre-promotion daemon struct did.
    assert_eq!(value["image_url"], serde_json::Value::Null);
}

#[test]
fn template_summary_round_trips_through_a_null_origin() {
    let decoded: TemplateSummary =
        serde_json::from_value(summary_json(Some(serde_json::Value::Null)))
            .expect("null origin decodes");
    let reencoded = serde_json::to_value(&decoded).expect("summary must serialize");

    // Re-encoding resolves the null to the concrete default, which is
    // the shape the daemon would have sent in the first place.
    assert_eq!(reencoded["origin"], json!("built_in"));

    let again: TemplateSummary = serde_json::from_value(reencoded).expect("re-decode");
    assert_eq!(again, decoded);
}

#[test]
fn complete_template_listing_omits_page_state() {
    let listing: TemplateListResponse =
        serde_json::from_value(json!({ "items": [summary_json(None)], "total": 1 }))
            .expect("a complete listing without page state must decode");

    assert_eq!(listing.items.len(), 1);
    assert_eq!(listing.total, 1);
    assert_eq!(listing.page, None);
}
