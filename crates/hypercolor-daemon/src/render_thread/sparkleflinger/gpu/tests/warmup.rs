use std::time::{Duration, Instant};

use super::*;
use crate::render_thread::gpu_device::GpuRenderDevice;

/// The longest SparkleFlinger may spend compiling its pipelines.
///
/// Render warmup compiles every compositor, sampling, and area SAT pipeline
/// before the daemon answers its first request, and the desktop app
/// supervisor gives the whole startup 20 seconds to answer `/health` before
/// killing the daemon. The budget is sized for hosted CI runners, which
/// compile far slower than a desktop and vary between runs: the same
/// pipelines took 3.3 s on one windows-latest run and 9.4 s on the next,
/// against 1.6 s on a desktop. A real compiler stall, like the one that
/// held 0.6.1 startup for 54 s on that desktop, still fails by a wide
/// margin.
const PIPELINE_WARMUP_BUDGET: Duration = Duration::from_secs(30);

#[test]
fn gpu_pipeline_warmup_fits_daemon_startup_budget() {
    let Some(render_device) = required_gpu(GpuRenderDevice::new_required_for_test(
        "SparkleFlinger pipeline warmup budget test",
    )) else {
        return;
    };
    let info = render_device.info();

    let started = Instant::now();
    let compositor = GpuSparkleFlinger::with_render_device(render_device)
        .expect("SparkleFlinger pipelines should compile on the test adapter");
    let elapsed = started.elapsed();
    drop(compositor);

    eprintln!(
        "SparkleFlinger pipeline warmup took {elapsed:?} on {} ({:?})",
        info.adapter_name, info.backend
    );
    assert!(
        elapsed < PIPELINE_WARMUP_BUDGET,
        "SparkleFlinger pipeline warmup took {elapsed:?} on {} ({:?}), over the \
         {PIPELINE_WARMUP_BUDGET:?} budget. On DX12 the usual cause is FXC stalling on one \
         shader; rerun with RUST_LOG=wgpu_core=trace,wgpu_hal=trace and compare each \
         \"Naga generated shader\" line with the create_compute_pipeline that follows it.",
        info.adapter_name,
        info.backend
    );
}
