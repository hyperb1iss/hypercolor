use super::*;
use crate::render_thread::gpu_device::GpuRenderDevice;
use crate::startup::StartupProgress;

/// Completed steps one compositor build reports: the device probe, the
/// five compositor pipelines (compose, source copy, display finalize RGB
/// and YUV, preview scale), the sample pipeline, the six area SAT
/// pipelines, and the native screen bridge.
const COMPOSITOR_STARTUP_STEPS: u64 = 14;

#[test]
fn every_compositor_pipeline_compile_advances_startup_progress() {
    let Some(render_device) = required_gpu(GpuRenderDevice::new_required_for_test(
        "SparkleFlinger startup progress test",
    )) else {
        return;
    };
    let progress = StartupProgress::default();
    let before = progress.snapshot();

    let compositor =
        GpuSparkleFlinger::with_render_device_reporting(render_device, Some(&progress))
            .expect("SparkleFlinger pipelines should compile on the test adapter");
    drop(compositor);

    let after = progress.snapshot();
    assert_eq!(after.phase, before.phase, "steps never change the phase");
    assert_eq!(
        after.sequence - before.sequence,
        COMPOSITOR_STARTUP_STEPS,
        "each completed compile and build step advances the sequence once"
    );
    assert_eq!(after.detail, None, "no step is left running");
}
