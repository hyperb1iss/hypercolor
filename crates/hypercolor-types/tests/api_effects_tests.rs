use std::collections::HashMap;

use hypercolor_types::api::Pagination;
use hypercolor_types::api::effects::{
    EffectPresetListResponse, EffectPresetOrigin, EffectPresetSummary,
};
use hypercolor_types::effect::ControlValue;

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
        pagination: Pagination {
            offset: 0,
            limit: 1,
            total: 1,
            has_more: false,
        },
    };

    let json = serde_json::to_value(&response).expect("preset stack should serialize");
    assert_eq!(json["items"][0]["origin"], "saved");
    assert_eq!(json["items"][0]["editable"], true);
    let decoded = serde_json::from_value::<EffectPresetListResponse>(json)
        .expect("preset stack should deserialize");
    assert_eq!(decoded, response);
}
