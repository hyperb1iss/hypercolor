use hypercolor_gpu_frame::GpuFrameImportFallbackReason;

#[test]
fn every_fallback_reason_code_round_trips_with_a_unique_label() {
    let mut labels = Vec::new();
    for code in 1..=27 {
        let reason = GpuFrameImportFallbackReason::from_u64(code)
            .unwrap_or_else(|| panic!("fallback reason code {code} should decode"));
        assert_eq!(reason.as_u64(), code);
        labels.push(reason.as_str());
    }
    let total = labels.len();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), total, "fallback reason labels must be unique");
    assert_eq!(GpuFrameImportFallbackReason::from_u64(0), None);
    assert_eq!(GpuFrameImportFallbackReason::from_u64(28), None);
}

#[test]
fn device_uuid_mismatch_has_a_stable_code_and_label() {
    let reason = GpuFrameImportFallbackReason::DeviceUuidMismatch;

    assert_eq!(reason.as_u64(), 27);
    assert_eq!(reason.as_str(), "device_uuid_mismatch");
}
