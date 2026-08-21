use hypercolor_ui::api::{
    GpuCompositorProbeStatus, RenderAccelerationStatus, RenderLoopStatus, SystemStatus,
};

#[test]
fn system_status_deserializes_renderer_acceleration() {
    let wire = serde_json::to_value(SystemStatus {
        compositor_acceleration: RenderAccelerationStatus {
            requested_mode: "auto".to_owned(),
            effective_mode: "gpu".to_owned(),
            servo_gpu_import_mode: "auto".to_owned(),
            servo_gpu_import_attempting: true,
            gpu_probe: Some(GpuCompositorProbeStatus {
                adapter_name: "AMD Radeon".to_owned(),
                backend: "vulkan".to_owned(),
                texture_format: "rgba8unorm".to_owned(),
                max_texture_dimension_2d: 16_384,
                max_storage_textures_per_shader_stage: 8,
                servo_gpu_import_backend_compatible: true,
                linux_servo_gpu_import_backend_compatible: true,
                ..GpuCompositorProbeStatus::default()
            }),
            ..RenderAccelerationStatus::default()
        },
        render_loop: RenderLoopStatus {
            target_fps: 60,
            total_frames: 99,
            ..RenderLoopStatus::default()
        },
        ..SystemStatus::default()
    })
    .expect("status should serialize");
    let status: SystemStatus =
        serde_json::from_value(wire).expect("status payload should include acceleration details");

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
