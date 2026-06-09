use std::collections::HashMap;

use hypercolor_types::event::{LayerHealth, ZoneChangeKind};
use hypercolor_types::scene::{SceneKind, SceneMutationMode, ZoneRole};
use hypercolor_ui::ws::messages::{
    PerformanceMetrics, extract_effect_error_hint, extract_layer_health, extract_scene_event_hint,
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
fn performance_metrics_deserializes_renderer_diagnostics() {
    let metrics: PerformanceMetrics = serde_json::from_value(serde_json::json!({
        "fps": { "target": 60, "ceiling": 60, "actual": 58.4, "dropped": 1 },
        "frame_time": { "avg_ms": 8.1, "p95_ms": 12.4, "p99_ms": 15.9, "max_ms": 18.2 },
        "stages": {
            "producer_effect_rendering_ms": 2.1,
            "producer_preview_compose_ms": 3.4,
            "composition_ms": 4.2,
            "publish_frame_data_ms": 0.1,
            "publish_group_canvas_ms": 0.2,
            "publish_preview_ms": 0.3,
            "publish_events_ms": 0.4
        },
        "pacing": {
            "gpu_zone_sampling": 114,
            "gpu_sample_cpu_fallback": 2,
            "cpu_sampling_late_readback": 1,
            "led_sampling_readback": 3,
            "gpu_readback_failed_frames": 4,
            "scene_canvas_forced_surface": 5,
            "full_frame_copy_frames": 6
        },
        "effect_health": {
            "servo_render_gpu_frames_total": 120,
            "servo_render_cpu_frames_total": 3,
            "servo_render_cached_frames_total": 9,
            "servo_gpu_import_failures_total": 1,
            "servo_gpu_import_fallbacks_total": 2,
            "servo_gpu_import_fallback_reason": "unsupported format",
            "servo_gpu_import_windows_sync_mode": "gl_finish",
            "servo_gpu_import_stale_frame_total": 3,
            "servo_gpu_import_adapter_mismatch_total": 4,
            "servo_gpu_import_max_ms": 1.7,
            "producer_gpu_frames_total": 130,
            "producer_cpu_frames_total": 5,
            "servo_render_readback_max_ms": 0.0
        },
        "timeline": {
            "frame_token": 42,
            "compositor_backend": "gpu",
            "gpu_zone_sampling": true,
            "gpu_readback_failed": true,
            "budget_ms": 16.67
        },
        "render_surfaces": {
            "slot_count": 6,
            "free_slots": 2,
            "preview_pool_saturation_reallocs": 7,
            "direct_pool_saturation_reallocs": 8,
            "preview_pool_grown_slots": 1,
            "scene_pool_slot_count": 4,
            "scene_pool_shared_published_slots": 2
        },
        "preview": {
            "canvas_receivers": 1,
            "canvas_demand": {
                "subscribers": 1,
                "max_fps": 60,
                "max_width": 1280,
                "max_height": 720,
                "any_rgba": true
            }
        },
        "display_output": {
            "captured_devices": 1,
            "preview_subscribers": 2,
            "write_failures_total": 3,
            "retry_attempts_total": 4,
            "display_lane": {
                "display_frames_total": 100,
                "display_frames_delayed_for_led_total": 6,
                "display_led_priority_wait_max_ms": 0.8
            }
        },
        "copies": {
            "full_frame_count": 2,
            "full_frame_kb": 2400.0,
            "producer_reason": "readback",
            "publication_reason": "canvas"
        },
        "memory": { "daemon_rss_mb": 100.0, "canvas_buffer_kb": 1200 },
        "devices": { "connected": 2, "total_leds": 300, "output_errors": 0 },
        "websocket": { "client_count": 1, "bytes_sent_per_sec": 2048.0 }
    }))
    .expect("metrics payload should include renderer diagnostics");

    assert_eq!(metrics.fps.ceiling, 60);
    assert_eq!(metrics.stages.producer_scene_compose_ms, 3.4);
    assert_eq!(metrics.effect_health.servo_render_gpu_frames_total, 120);
    assert_eq!(
        metrics
            .effect_health
            .servo_gpu_import_fallback_reason
            .as_deref(),
        Some("unsupported format")
    );
    assert_eq!(
        metrics
            .effect_health
            .servo_gpu_import_windows_sync_mode
            .as_deref(),
        Some("gl_finish")
    );
    assert_eq!(metrics.effect_health.servo_gpu_import_stale_frame_total, 3);
    assert_eq!(
        metrics
            .effect_health
            .servo_gpu_import_adapter_mismatch_total,
        4
    );
    assert!(metrics.timeline.gpu_readback_failed);
    assert_eq!(metrics.render_surfaces.scene_pool_saturation_reallocs, 7);
    assert_eq!(metrics.display_output.write_failures_total, 3);
    assert_eq!(metrics.copies.producer_reason.as_deref(), Some("readback"));
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
    assert_eq!(hint.render_group_role, Some(ZoneRole::Display));
    assert_eq!(
        hint.render_group_change_kind,
        Some(ZoneChangeKind::ControlsPatched)
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

    assert_eq!(hint.render_group_role, Some(ZoneRole::Primary));
    assert_eq!(hint.render_group_change_kind, Some(ZoneChangeKind::Updated));
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
    // key that could collide across zones, so the event is rejected.
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
    // A SceneLayerId is unique only within its zone; two groups can
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
