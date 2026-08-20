use std::collections::HashMap;

use hypercolor_types::api::effects::{
    EffectCategory, EffectPresetListResponse, EffectPresetOrigin, EffectPresetSummary,
    EffectSourceKind,
};
use hypercolor_types::effect::{ControlValue, EffectSource};

#[test]
fn effect_preset_stack_round_trips_origin_and_editability() {
    let response = EffectPresetListResponse {
        items: vec![EffectPresetSummary {
            id: "0198-preset".to_owned(),
            name: "Deep Ocean".to_owned(),
            description: Some("Cool and calm".to_owned()),
            effect_id: "aurora".to_owned(),
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.4))]),
            tags: vec!["calm".to_owned()],
            origin: EffectPresetOrigin::Saved,
            editable: true,
        }],
        total: 1,
        page: None,
    };

    let json = serde_json::to_value(&response).expect("preset stack should serialize");
    assert_eq!(json["items"][0]["origin"], "saved");
    assert_eq!(json["items"][0]["editable"], true);
    let decoded = serde_json::from_value::<EffectPresetListResponse>(json)
        .expect("preset stack should deserialize");
    assert_eq!(decoded, response);
}

#[test]
fn effect_vocabularies_use_their_canonical_wire_spellings() {
    assert_eq!(
        serde_json::to_value(EffectCategory::Generative).expect("category should serialize"),
        "generative"
    );
    assert_eq!(
        serde_json::to_value(EffectSourceKind::Html).expect("source kind should serialize"),
        "html"
    );
    assert_eq!(EffectCategory::Display.as_str(), "display");
    assert_eq!(EffectSourceKind::Shader.as_str(), "shader");
}

#[test]
fn effect_source_kind_projects_internal_paths_out_of_the_wire() {
    let source = EffectSource::Html {
        path: "/private/effects/aurora.html".into(),
    };

    let kind = EffectSourceKind::from(&source);
    let json = serde_json::to_value(kind).expect("source kind should serialize");

    assert_eq!(json, "html");
    assert!(!json.to_string().contains("private"));
}
