use hypercolor_types::api::system::{
    EffectHealthStatus, GpuPassTimingStatus, GpuPassTimingsStatus, GpuTransferStatus,
};

#[test]
fn gpu_measurements_preserve_units_coverage_and_transfer_payloads_on_the_wire() {
    let health = EffectHealthStatus {
        gpu_pass_timings: GpuPassTimingsStatus {
            instance_id: 42,
            supported: true,
            enabled: true,
            unavailable_reason: None,
            stages: vec![GpuPassTimingStatus {
                stage: "compose".to_owned(),
                attempts: 5,
                completed: 2,
                skipped: 1,
                failed: 1,
                cancelled: 1,
                total_ns: 250_000,
                last_ns: 100_000,
                max_ns: 150_000,
            }],
        },
        gpu_transfers: GpuTransferStatus {
            preview_readback_count: 1,
            preview_readback_bytes: 640 * 480 * 4,
            display_readback_count: 2,
            display_readback_bytes: 400 * 400 * 4 * 2,
            led_readback_count: 3,
            led_readback_bytes: 100 * 4 * 3,
            sampling_canvas_readback_count: 1,
            sampling_canvas_readback_bytes: 640 * 480 * 4,
        },
        ..EffectHealthStatus::default()
    };
    let wire = serde_json::to_value(&health).expect("GPU telemetry should serialize");
    assert_eq!(wire["gpu_pass_timings"]["stages"][0]["total_ns"], 250_000);
    assert_eq!(wire["gpu_pass_timings"]["stages"][0]["skipped"], 1);
    assert_eq!(wire["gpu_transfers"]["led_readback_bytes"], 1200);
    assert_eq!(
        wire["gpu_transfers"]["sampling_canvas_readback_bytes"],
        1_228_800
    );
    let decoded: EffectHealthStatus =
        serde_json::from_value(wire).expect("GPU telemetry should deserialize");
    assert_eq!(decoded, health);
}

#[test]
fn older_effect_health_snapshots_leave_gpu_measurements_unavailable() {
    let mut wire = serde_json::to_value(EffectHealthStatus::default())
        .expect("effect health should serialize");
    let object = wire.as_object_mut().expect("effect health is an object");
    object.remove("gpu_pass_timings");
    object.remove("gpu_transfers");
    let decoded: EffectHealthStatus =
        serde_json::from_value(wire).expect("older daemons do not supply GPU measurements");
    assert!(!decoded.gpu_pass_timings.enabled);
    assert!(decoded.gpu_pass_timings.stages.is_empty());
    assert_eq!(decoded.gpu_transfers, GpuTransferStatus::default());
}
