#![allow(dead_code, unused_imports)]

#[path = "../src/api/mod.rs"]
mod api;

#[path = "../src/ws/messages.rs"]
mod messages;

use std::collections::HashMap;

use hypercolor_types::event::{LayerHealth, RenderGroupChangeKind};
use hypercolor_types::scene::{RenderGroupRole, SceneKind, SceneMutationMode};
use messages::{
    extract_effect_error_hint, extract_layer_health, extract_scene_event_hint,
    group_has_degraded_layer, layer_health_key, scene_event_affects_active_effect,
};

#[test]
fn extract_scene_event_hint_parses_active_scene_payload() {
    let hint = extract_scene_event_hint(
        "active_scene_changed",
        &serde_json::json!({
            "current": "scene-1",
            "current_name": "Named Scene",
            "current_kind": "named",
            "current_mutation_mode": "snapshot",
            "current_snapshot_locked": true,
        }),
    );

    assert_eq!(hint.event_type, "active_scene_changed");
    assert_eq!(hint.scene_id.as_deref(), Some("scene-1"));
    assert_eq!(hint.scene_name.as_deref(), Some("Named Scene"));
    assert_eq!(hint.scene_kind, Some(SceneKind::Named));
    assert_eq!(hint.scene_mutation_mode, Some(SceneMutationMode::Snapshot));
    assert_eq!(hint.scene_snapshot_locked, Some(true));
    assert_eq!(hint.render_group_role, None);
    assert_eq!(hint.render_group_change_kind, None);
    assert!(scene_event_affects_active_effect(&hint));
}

#[test]
fn extract_scene_event_hint_parses_display_render_group_metadata() {
    let hint = extract_scene_event_hint(
        "render_group_changed",
        &serde_json::json!({
            "scene_id": "scene-1",
            "group_id": "group-1",
            "role": "display",
            "kind": "controls_patched",
        }),
    );

    assert_eq!(hint.event_type, "render_group_changed");
    assert_eq!(hint.scene_id.as_deref(), Some("scene-1"));
    assert_eq!(hint.render_group_role, Some(RenderGroupRole::Display));
    assert_eq!(
        hint.render_group_change_kind,
        Some(RenderGroupChangeKind::ControlsPatched)
    );
}

#[test]
fn scene_event_affects_active_effect_ignores_display_render_group_changes() {
    let hint = extract_scene_event_hint(
        "render_group_changed",
        &serde_json::json!({
            "scene_id": "scene-1",
            "group_id": "group-1",
            "role": "display",
            "kind": "updated",
        }),
    );

    assert!(!scene_event_affects_active_effect(&hint));
}

#[test]
fn scene_event_affects_active_effect_keeps_primary_render_group_changes() {
    let hint = extract_scene_event_hint(
        "render_group_changed",
        &serde_json::json!({
            "scene_id": "scene-1",
            "group_id": "group-1",
            "role": "primary",
            "kind": "updated",
        }),
    );

    assert_eq!(hint.render_group_role, Some(RenderGroupRole::Primary));
    assert_eq!(
        hint.render_group_change_kind,
        Some(RenderGroupChangeKind::Updated)
    );
    assert!(scene_event_affects_active_effect(&hint));
}

#[test]
fn extract_effect_error_hint_parses_fallback_payload() {
    let hint = extract_effect_error_hint(
        "effect_error",
        &serde_json::json!({
            "effect_id": "effect-1",
            "error": "render exploded",
            "fallback": "clear_groups",
        }),
    )
    .expect("effect error hint");

    assert_eq!(hint.event_type, "effect_error");
    assert_eq!(hint.effect_id, "effect-1");
    assert_eq!(hint.error, "render exploded");
    assert_eq!(hint.fallback.as_deref(), Some("clear_groups"));
}

#[test]
fn extract_layer_health_keys_by_scene_group_and_layer() {
    let (key, health) = extract_layer_health(&serde_json::json!({
        "scene_id": "scene-1",
        "group_id": "group-1",
        "layer_id": "layer-7",
        "health": "stalled",
    }))
    .expect("layer health hint");

    assert_eq!(key, layer_health_key("scene-1", "group-1", "layer-7"));
    assert_eq!(health, LayerHealth::Stalled);
}

#[test]
fn extract_layer_health_parses_a_failure_reason() {
    let (_, health) = extract_layer_health(&serde_json::json!({
        "scene_id": "scene-1",
        "group_id": "group-1",
        "layer_id": "layer-7",
        "health": { "failed": { "reason": "decode error" } },
    }))
    .expect("layer health hint");

    assert_eq!(
        health,
        LayerHealth::Failed {
            reason: "decode error".to_owned(),
        }
    );
}

#[test]
fn extract_layer_health_rejects_a_payload_missing_an_identity_field() {
    // All three identity fields are mandatory. Dropping any one leaves a
    // key that could collide across render groups, so the event is rejected.
    assert!(extract_layer_health(&serde_json::json!({ "health": "active" })).is_none());
    assert!(
        extract_layer_health(&serde_json::json!({
            "group_id": "group-1",
            "layer_id": "layer-7",
            "health": "active",
        }))
        .is_none()
    );
    assert!(
        extract_layer_health(&serde_json::json!({
            "scene_id": "scene-1",
            "layer_id": "layer-7",
            "health": "active",
        }))
        .is_none()
    );
}

#[test]
fn layer_health_key_separates_groups_that_share_a_layer_id() {
    // A SceneLayerId is unique only within its render group; two groups can
    // hold the same id, so the composite key must keep their rows distinct.
    let shared_layer = "layer-7";
    let group_a = layer_health_key("scene-1", "group-a", shared_layer);
    let group_b = layer_health_key("scene-1", "group-b", shared_layer);
    assert_ne!(group_a, group_b);
}

/// The group's live layer-id set, as `group_has_degraded_layer` expects it.
fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn group_has_degraded_layer_flags_only_the_owning_group() {
    let mut map = HashMap::new();
    map.insert(
        layer_health_key("scene-1", "group-a", "layer-1"),
        LayerHealth::Failed {
            reason: "boom".to_owned(),
        },
    );
    map.insert(
        layer_health_key("scene-1", "group-b", "layer-2"),
        LayerHealth::Active,
    );

    // group-a owns the failed layer; group-b and an unrelated scene do not.
    assert!(group_has_degraded_layer(
        &map,
        "scene-1",
        "group-a",
        &ids(&["layer-1"]),
    ));
    assert!(!group_has_degraded_layer(
        &map,
        "scene-1",
        "group-b",
        &ids(&["layer-2"]),
    ));
    assert!(!group_has_degraded_layer(
        &map,
        "scene-2",
        "group-a",
        &ids(&["layer-1"]),
    ));
}

#[test]
fn group_has_degraded_layer_ignores_transient_states() {
    let mut map = HashMap::new();
    map.insert(
        layer_health_key("scene-1", "group-a", "layer-1"),
        LayerHealth::Stalled,
    );
    map.insert(
        layer_health_key("scene-1", "group-a", "layer-2"),
        LayerHealth::Loading,
    );
    // Loading and Stalled are transient — not a degraded surface.
    assert!(!group_has_degraded_layer(
        &map,
        "scene-1",
        "group-a",
        &ids(&["layer-1", "layer-2"]),
    ));

    map.insert(
        layer_health_key("scene-1", "group-a", "layer-3"),
        LayerHealth::AssetMissing,
    );
    // A missing asset does count as degraded.
    assert!(group_has_degraded_layer(
        &map,
        "scene-1",
        "group-a",
        &ids(&["layer-1", "layer-2", "layer-3"]),
    ));
}

#[test]
fn group_has_degraded_layer_ignores_stale_removed_layers() {
    let mut map = HashMap::new();
    map.insert(
        layer_health_key("scene-1", "group-a", "layer-gone"),
        LayerHealth::Failed {
            reason: "boom".to_owned(),
        },
    );

    // The failed layer was removed from the stack. The daemon drops its
    // health silently, leaving a stale map entry — but with the layer no
    // longer in the live set, the surface must not read as degraded.
    assert!(!group_has_degraded_layer(&map, "scene-1", "group-a", &[]));
    assert!(!group_has_degraded_layer(
        &map,
        "scene-1",
        "group-a",
        &ids(&["layer-still-here"]),
    ));

    // It does flag while the failed layer is still in the stack.
    assert!(group_has_degraded_layer(
        &map,
        "scene-1",
        "group-a",
        &ids(&["layer-gone"]),
    ));
}
