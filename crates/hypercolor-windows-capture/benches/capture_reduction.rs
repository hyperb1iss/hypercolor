use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hypercolor_windows_capture::{
    CaptureExtent, CaptureReductionBenchmark, subsample_stride_within, subsampled_extent,
};

const ANALYSIS_WIDTH: u32 = 1280;
const ANALYSIS_HEIGHT: u32 = 720;

fn analysis_extent() -> CaptureExtent {
    CaptureExtent::try_new(ANALYSIS_WIDTH, ANALYSIS_HEIGHT).expect("analysis extent is non-empty")
}

fn checked_rgba_len(width: u32, height: u32) -> Result<usize, String> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| format!("RGBA8 byte geometry overflows for {width}x{height}"))
}

fn solid_plane(width: u32, height: u32) -> Result<Vec<u8>, String> {
    let len = checked_rgba_len(width, height)?;
    let mut plane = Vec::new();
    plane
        .try_reserve(len)
        .map_err(|error| format!("could not reserve {len} source bytes: {error}"))?;
    plane.resize(len, 0x7F);
    Ok(plane)
}

fn summed_samples(
    harness: &mut CaptureReductionBenchmark,
    iterations: u64,
    select: impl Fn(hypercolor_windows_capture::ReductionBenchmarkSample) -> Duration,
) -> Duration {
    (0..iterations)
        .map(|_| {
            select(
                harness
                    .sample()
                    .expect("D3D11 reduction benchmark succeeds"),
            )
        })
        .sum()
}

fn cpu_box_reduce(
    source: &[u8],
    width: u32,
    height: u32,
    requested_extent: CaptureExtent,
    output: &mut Vec<u8>,
) -> Result<(), String> {
    let source_len = checked_rgba_len(width, height)?;
    if source.len() != source_len {
        return Err(format!(
            "source has {} bytes, expected {source_len}",
            source.len()
        ));
    }
    let stride = subsample_stride_within(width, height, requested_extent);
    let output_width = subsampled_extent(width, stride);
    let output_height = subsampled_extent(height, stride);
    let output_len = checked_rgba_len(output_width, output_height)?;
    if output_len > output.capacity() {
        output
            .try_reserve(output_len.saturating_sub(output.len()))
            .map_err(|error| format!("could not reserve {output_len} output bytes: {error}"))?;
    }
    output.resize(output_len, 0);
    for output_y in 0..output_height {
        let source_y = output_y * stride;
        let end_y = (source_y + stride).min(height);
        for output_x in 0..output_width {
            let source_x = output_x * stride;
            let end_x = (source_x + stride).min(width);
            let mut channels = [0_u64; 3];
            let mut samples = 0_u64;
            for y in source_y..end_y {
                for x in source_x..end_x {
                    let offset = (y as usize * width as usize + x as usize) * 4;
                    channels[0] += u64::from(source[offset]);
                    channels[1] += u64::from(source[offset + 1]);
                    channels[2] += u64::from(source[offset + 2]);
                    samples += 1;
                }
            }
            let target = (output_y as usize * output_width as usize + output_x as usize) * 4;
            output[target] = (channels[0] / samples) as u8;
            output[target + 1] = (channels[1] / samples) as u8;
            output[target + 2] = (channels[2] / samples) as u8;
            output[target + 3] = 0xFF;
        }
    }
    Ok(())
}

fn percentile(samples: &mut [Duration], percentile: f64) -> Duration {
    samples.sort_unstable();
    let index = ((samples.len().saturating_sub(1)) as f64 * percentile).round() as usize;
    samples[index]
}

fn report_deadlines(width: u32, height: u32) {
    let requested_extent = analysis_extent();
    let mut harness = CaptureReductionBenchmark::new(width, height, requested_extent)
        .expect("hardware D3D11 benchmark fixture opens");
    let report = harness
        .run_cadence(120)
        .expect("real-time cadence sample succeeds");
    let mut acquisition = report.acquisition_enqueue;
    let mut analysis = report.analysis_latency;
    let phase_samples = (0..120)
        .map(|_| harness.sample().expect("phase sample succeeds"))
        .collect::<Vec<_>>();
    let mut gpu_enqueue = phase_samples
        .iter()
        .map(|sample| sample.analysis_enqueue)
        .collect::<Vec<_>>();
    let mut gpu_wait = phase_samples
        .iter()
        .map(|sample| sample.wait)
        .collect::<Vec<_>>();
    let mut map_readback = phase_samples
        .iter()
        .map(|sample| sample.map)
        .collect::<Vec<_>>();
    let source = solid_plane(width, height).expect("CPU benchmark source admits");
    let mut output = Vec::new();
    let mut cpu_analysis = (0..120)
        .map(|_| {
            let started = Instant::now();
            cpu_box_reduce(&source, width, height, requested_extent, &mut output)
                .expect("CPU benchmark reduction succeeds");
            started.elapsed()
        })
        .collect::<Vec<_>>();
    eprintln!(
        "{width}x{height}: acquisition_enqueue p50={:?} p95={:?} p99={:?}, \
         analysis_latency p50={:?} p95={:?} p99={:?}, \
         acquisition_missed={}/120, analysis_missed={}/60, ring_busy={}, \
         source_bytes={}, readback_bytes={}",
        percentile(&mut acquisition, 0.50),
        percentile(&mut acquisition, 0.95),
        percentile(&mut acquisition, 0.99),
        percentile(&mut analysis, 0.50),
        percentile(&mut analysis, 0.95),
        percentile(&mut analysis, 0.99),
        report.acquisition_misses,
        report.analysis_misses,
        report.ring_busy,
        report.source_bytes,
        report.readback_bytes,
    );
    eprintln!(
        "{width}x{height}: gpu_enqueue p50={:?} p95={:?} p99={:?}, \
         gpu_wait p50={:?} p95={:?} p99={:?}, \
         map_readback p50={:?} p95={:?} p99={:?}, \
         cpu_analysis p50={:?} p95={:?} p99={:?}, \
         source_bytes_per_frame={}, readback_bytes_per_frame={}",
        percentile(&mut gpu_enqueue, 0.50),
        percentile(&mut gpu_enqueue, 0.95),
        percentile(&mut gpu_enqueue, 0.99),
        percentile(&mut gpu_wait, 0.50),
        percentile(&mut gpu_wait, 0.95),
        percentile(&mut gpu_wait, 0.99),
        percentile(&mut map_readback, 0.50),
        percentile(&mut map_readback, 0.95),
        percentile(&mut map_readback, 0.99),
        percentile(&mut cpu_analysis, 0.50),
        percentile(&mut cpu_analysis, 0.95),
        percentile(&mut cpu_analysis, 0.99),
        phase_samples[0].source_bytes,
        phase_samples[0].readback_bytes,
    );
}

fn capture_reduction(criterion: &mut Criterion) {
    for (width, height) in [
        (1920, 1080),
        (2560, 1440),
        (3840, 2160),
        (7680, 2160),
        (2160, 7680),
    ] {
        let requested_extent = analysis_extent();
        report_deadlines(width, height);
        let id = format!("{width}x{height}");
        let mut gpu = CaptureReductionBenchmark::new(width, height, requested_extent)
            .expect("hardware D3D11 benchmark fixture opens");
        let probe = gpu.sample().expect("benchmark probe succeeds");
        let mut group = criterion.benchmark_group(format!("windows_capture/{id}"));
        group.throughput(Throughput::Bytes(probe.readback_bytes));

        group.bench_function(BenchmarkId::new("acquisition_enqueue", &id), |benchmark| {
            benchmark.iter_custom(|iterations| {
                summed_samples(&mut gpu, iterations, |sample| sample.acquisition_enqueue)
            });
        });
        group.bench_function(BenchmarkId::new("analysis_enqueue", &id), |benchmark| {
            benchmark.iter_custom(|iterations| {
                summed_samples(&mut gpu, iterations, |sample| sample.analysis_enqueue)
            });
        });
        group.bench_function(BenchmarkId::new("wait", &id), |benchmark| {
            benchmark.iter_custom(|iterations| {
                summed_samples(&mut gpu, iterations, |sample| sample.wait)
            });
        });
        group.bench_function(BenchmarkId::new("map", &id), |benchmark| {
            benchmark.iter_custom(|iterations| {
                summed_samples(&mut gpu, iterations, |sample| sample.map)
            });
        });

        let source = solid_plane(width, height).expect("CPU benchmark source admits");
        let mut output = Vec::new();
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_function(BenchmarkId::new("cpu_analysis", &id), |benchmark| {
            benchmark.iter(|| {
                cpu_box_reduce(&source, width, height, requested_extent, &mut output)
                    .expect("CPU benchmark reduction succeeds");
            });
        });
        group.finish();
    }
}

criterion_group!(benches, capture_reduction);
criterion_main!(benches);
