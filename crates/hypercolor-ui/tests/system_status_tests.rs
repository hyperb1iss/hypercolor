#![allow(dead_code)]

#[path = "../src/api/mod.rs"]
mod api;

use api::SystemStatus;

#[test]
fn system_status_deserializes_renderer_acceleration() {
    let status: SystemStatus = serde_json::from_value(serde_json::json!({
        "running": true,
        "version": "0.1.0",
        "config_path": "/tmp/hypercolor.toml",
        "uptime_seconds": 12,
        "device_count": 2,
        "effect_count": 42,
        "active_effect": "Screen Cast",
        "active_scene": "Desk",
        "active_scene_snapshot_locked": false,
        "global_brightness": 80,
        "compositor_acceleration": {
            "requested_mode": "auto",
            "effective_mode": "gpu",
            "fallback_reason": null,
            "servo_gpu_import_mode": "auto",
            "servo_gpu_import_attempting": true,
            "gpu_probe": {
                "adapter_name": "AMD Radeon",
                "backend": "vulkan",
                "texture_format": "rgba8unorm",
                "max_texture_dimension_2d": 16384,
                "max_storage_textures_per_shader_stage": 8,
                "servo_gpu_import_backend_compatible": true,
                "servo_gpu_import_backend_reason": null,
                "linux_servo_gpu_import_backend_compatible": true,
                "linux_servo_gpu_import_backend_reason": null
            }
        },
        "render_loop": {
            "state": "running",
            "fps_tier": "60fps",
            "target_fps": 60,
            "ceiling_fps": 60,
            "actual_fps": 58.7,
            "consecutive_misses": 1,
            "total_frames": 99
        },
        "capabilities": ["multi-zone-sampling"]
    }))
    .expect("status payload should include acceleration details");

    assert_eq!(status.compositor_acceleration.effective_mode, "gpu");
    assert_eq!(
        status
            .compositor_acceleration
            .gpu_probe
            .as_ref()
            .map(|probe| probe.backend.as_str()),
        Some("vulkan")
    );
    assert_eq!(status.render_loop.target_fps, 60);
    assert_eq!(status.render_loop.total_frames, 99);
    assert!(
        status
            .compositor_acceleration
            .gpu_probe
            .as_ref()
            .is_some_and(|probe| probe.servo_gpu_import_backend_compatible)
    );
}
