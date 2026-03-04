use std::collections::HashMap;
use std::str::FromStr;

use hypercolor_types::effect::{ControlValue, EffectId};
use hypercolor_types::library::{
    EffectPlaylist, EffectPreset, FavoriteEffect, PlaylistId, PlaylistItem, PlaylistItemId,
    PlaylistItemTarget, PresetId,
};
use uuid::Uuid;

#[test]
fn preset_id_round_trips_from_string() {
    let id = PresetId::new();
    let parsed = PresetId::from_str(&id.to_string()).expect("preset id should parse");
    assert_eq!(id, parsed);
}

#[test]
fn playlist_id_round_trips_from_string() {
    let id = PlaylistId::new();
    let parsed = PlaylistId::from_str(&id.to_string()).expect("playlist id should parse");
    assert_eq!(id, parsed);
}

#[test]
fn playlist_item_id_round_trips_from_string() {
    let id = PlaylistItemId::new();
    let parsed = PlaylistItemId::from_str(&id.to_string()).expect("playlist item id should parse");
    assert_eq!(id, parsed);
}

#[test]
fn favorite_effect_serde_roundtrip() {
    let favorite = FavoriteEffect {
        effect_id: EffectId::new(Uuid::now_v7()),
        added_at_ms: 123_456,
    };

    let json = serde_json::to_string(&favorite).expect("serialize favorite");
    let decoded: FavoriteEffect = serde_json::from_str(&json).expect("deserialize favorite");
    assert_eq!(favorite, decoded);
}

#[test]
fn effect_preset_serde_roundtrip() {
    let mut controls = HashMap::new();
    controls.insert("speed".to_owned(), ControlValue::Float(0.75));
    controls.insert("enabled".to_owned(), ControlValue::Boolean(true));

    let preset = EffectPreset {
        id: PresetId::new(),
        name: "Warm Pulse".to_owned(),
        description: Some("A cozy pulse profile".to_owned()),
        effect_id: EffectId::new(Uuid::now_v7()),
        controls,
        tags: vec!["ambient".to_owned(), "favorite".to_owned()],
        created_at_ms: 11,
        updated_at_ms: 22,
    };

    let json = serde_json::to_string(&preset).expect("serialize preset");
    let decoded: EffectPreset = serde_json::from_str(&json).expect("deserialize preset");
    assert_eq!(preset, decoded);
}

#[test]
fn playlist_serde_roundtrip() {
    let effect_id = EffectId::new(Uuid::now_v7());
    let preset_id = PresetId::new();
    let playlist = EffectPlaylist {
        id: PlaylistId::new(),
        name: "Late Night".to_owned(),
        description: Some("Slow cycle".to_owned()),
        items: vec![
            PlaylistItem {
                id: PlaylistItemId::new(),
                target: PlaylistItemTarget::Effect { effect_id },
                duration_ms: Some(8_000),
                transition_ms: Some(300),
            },
            PlaylistItem {
                id: PlaylistItemId::new(),
                target: PlaylistItemTarget::Preset { preset_id },
                duration_ms: None,
                transition_ms: None,
            },
        ],
        loop_enabled: true,
        created_at_ms: 1,
        updated_at_ms: 2,
    };

    let json = serde_json::to_string(&playlist).expect("serialize playlist");
    let decoded: EffectPlaylist = serde_json::from_str(&json).expect("deserialize playlist");
    assert_eq!(playlist, decoded);
}
