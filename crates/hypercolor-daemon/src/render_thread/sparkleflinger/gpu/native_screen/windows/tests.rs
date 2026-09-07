use hypercolor_windows_gpu_interop::D3d11On12ScreenInteropError;

use super::{
    NativeScreenCopyFailurePolicy, native_screen_copy_failure_policy,
    screen_storage_requires_cache_turnover, validate_windows_plan_generation,
};
use crate::render_thread::sparkleflinger::gpu::tests::gpu_test_compositor;

#[test]
fn dx12_compositor_exposes_one_renderer_bound_screen_target() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    if compositor.probe.backend == "dx12" {
        let target = compositor
            .screen_native_execution_target()
            .expect("DX12 compositor should expose a D3D11On12 target");
        assert_eq!(
            target.max_texture_dimension().get(),
            compositor.probe.max_texture_dimension_2d
        );
    } else {
        assert!(compositor.screen_native_execution_target().is_none());
    }
}

#[test]
fn native_screen_copy_failure_policy_separates_pressure_from_stale_structure() {
    use std::num::NonZeroU64;

    use hypercolor_windows_capture::{CaptureError, GpuSurfaceDescriptorId};

    assert_eq!(
        native_screen_copy_failure_policy(&D3d11On12ScreenInteropError::KeyedMutexTimeout),
        NativeScreenCopyFailurePolicy::Retain
    );
    assert_eq!(
        native_screen_copy_failure_policy(&D3d11On12ScreenInteropError::Capture(
            CaptureError::GpuSurfaceUseUnavailable {
                descriptor_id: GpuSurfaceDescriptorId::new(NonZeroU64::MIN),
                source_sequence: 7,
            },
        )),
        NativeScreenCopyFailurePolicy::Retain
    );
    assert_eq!(
        native_screen_copy_failure_policy(&D3d11On12ScreenInteropError::PreparedTargetMismatch {
            field: "plan_generation",
        },),
        NativeScreenCopyFailurePolicy::Reprepare
    );
    assert_eq!(
        native_screen_copy_failure_policy(&D3d11On12ScreenInteropError::TargetContentUncertain {
            operation: "release capture keyed mutex",
            source: Box::new(D3d11On12ScreenInteropError::KeyedMutexTimeout),
        },),
        NativeScreenCopyFailurePolicy::InvalidateFrameAndReprepare
    );
}

#[test]
fn native_screen_storage_turnover_purges_only_changed_targets() {
    assert!(screen_storage_requires_cache_turnover(None, 7));
    assert!(!screen_storage_requires_cache_turnover(Some(7), 7));
    assert!(screen_storage_requires_cache_turnover(Some(7), 8));
}

#[test]
fn native_screen_manifest_generation_is_an_exact_fence() {
    validate_windows_plan_generation(7, 7).expect("matching plan generation is accepted");
    assert!(validate_windows_plan_generation(7, 8).is_err());
}
