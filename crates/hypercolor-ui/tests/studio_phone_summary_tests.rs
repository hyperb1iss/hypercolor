//! Studio's phone summary: hardware counts per zone, and the effects a
//! zone's picker offers.

use std::collections::HashSet;

use hypercolor_types::api::effects::EffectSourceKind;
use hypercolor_types::api::scene::ZoneResource;
use hypercolor_types::effect::EffectCategory;
use hypercolor_ui::api::{EffectCapabilitySet, EffectSummary};
use hypercolor_ui::effect_search::IndexedEffect;
use hypercolor_ui::pages::studio::phone_summary::{
    MemberCounts, member_counts, member_summary, zone_effect_choices,
};

fn zone(members: &[(&str, &str)]) -> ZoneResource {
    serde_json::from_value(serde_json::json!({
        "id": "11111111-1111-4111-8111-111111111111",
        "name": "Desk",
        "role": "custom",
        "enabled": true,
        "brightness": 1.0,
        "members": members
            .iter()
            .map(|(id, device)| serde_json::json!({ "id": id, "device_id": device, "name": id }))
            .collect::<Vec<_>>(),
        "layers": [],
    }))
    .expect("zone fixture parses")
}

fn effect(id: &str, name: &str, category: EffectCategory, runnable: bool) -> IndexedEffect {
    IndexedEffect::new(EffectSummary {
        id: id.to_owned(),
        name: name.to_owned(),
        description: String::new(),
        author: "Hypercolor".to_owned(),
        category,
        source: EffectSourceKind::Native,
        runnable,
        tags: Vec::new(),
        version: "1.0.0".to_owned(),
        audio_reactive: false,
        input_reactive: false,
        capabilities: EffectCapabilitySet::default(),
        cover_image_url: None,
        controls: None,
        presets: None,
    })
}

#[test]
fn counts_outputs_and_the_distinct_devices_behind_them() {
    let counts = member_counts(&zone(&[
        ("strip-a", "usb:one"),
        ("strip-b", "usb:one"),
        ("fan", "usb:two"),
    ]));
    assert_eq!(
        counts,
        MemberCounts {
            outputs: 3,
            devices: 2
        }
    );
    assert_eq!(member_counts(&zone(&[])), MemberCounts::default());
}

#[test]
fn hardware_reads_as_a_sentence() {
    assert_eq!(member_summary(MemberCounts::default()), "No devices yet");
    assert_eq!(
        member_summary(MemberCounts {
            outputs: 1,
            devices: 1
        }),
        "1 output on 1 device"
    );
    assert_eq!(
        member_summary(MemberCounts {
            outputs: 12,
            devices: 4
        }),
        "12 outputs on 4 devices"
    );
}

#[test]
fn the_picker_lists_favorites_first_and_only_runnable_led_effects() {
    let effects = [
        effect("rainbow", "Rainbow", EffectCategory::Ambient, true),
        effect("pulse", "Audio Pulse", EffectCategory::Audio, true),
        effect("clock", "Clock Face", EffectCategory::Display, true),
        effect("broken", "Broken", EffectCategory::Ambient, false),
        effect("wave", "Color Wave", EffectCategory::Ambient, true),
    ];
    let favorites: HashSet<String> = ["wave".to_owned()].into_iter().collect();
    let ids: Vec<String> = zone_effect_choices(&effects, &favorites, "")
        .into_iter()
        .map(|choice| choice.id)
        .collect();
    assert_eq!(ids, ["wave", "rainbow", "pulse"]);
}

#[test]
fn the_picker_search_matches_name_and_category() {
    let effects = [
        effect("rainbow", "Rainbow", EffectCategory::Ambient, true),
        effect("pulse", "Audio Pulse", EffectCategory::Audio, true),
    ];
    let none = HashSet::new();
    let by_name: Vec<String> = zone_effect_choices(&effects, &none, "  PULSE ")
        .into_iter()
        .map(|choice| choice.name)
        .collect();
    assert_eq!(by_name, ["Audio Pulse"]);
    let by_category = zone_effect_choices(&effects, &none, "ambient");
    assert_eq!(by_category.len(), 1);
    assert_eq!(by_category[0].category, "ambient");
    assert!(!by_category[0].favorite);
}
