#![allow(dead_code, unused_imports)]

#[path = "../src/ws/messages.rs"]
mod messages;

use hypercolor_types::event::RenderGroupChangeKind;
use hypercolor_types::scene::{RenderGroupRole, SceneKind, SceneMutationMode};
use messages::{extract_scene_event_hint, scene_event_affects_active_effect};

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
