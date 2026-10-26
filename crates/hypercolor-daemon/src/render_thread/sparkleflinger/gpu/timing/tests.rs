use super::*;
use crate::render_thread::gpu_device::GpuRenderDevice;

type OutputResult = std::sync::mpsc::Receiver<Result<(), String>>;

fn dispatch(
    timings: &GpuTimings,
    device: &wgpu::Device,
    verify_output: bool,
) -> (wgpu::CommandEncoder, OutputResult) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("timestamp test shader"),
        source: wgpu::ShaderSource::Wgsl(
            "@group(0) @binding(0) var<storage, read_write> output: array<u32>;
             @compute @workgroup_size(64)
             fn main(@builtin(global_invocation_id) id: vec3<u32>) {
                 output[id.x] = id.x ^ 0xabcdefu;
             }"
            .into(),
        ),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("timestamp test pipeline"),
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("timestamp test output"),
        size: 65_536 * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("timestamp test output readback"),
        size: output.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("timestamp test bind group"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });
    timings.compute(&mut encoder, GpuStage::Compose, "timestamp test", |pass| {
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1024, 1, 1);
    });
    if verify_output {
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output.size());
        encoder.map_buffer_on_submit(&readback.clone(), wgpu::MapMode::Read, .., move |result| {
            let verified = result.map_err(|error| error.to_string()).and_then(|()| {
                let mapped = readback
                    .get_mapped_range(..)
                    .map_err(|error| error.to_string())?;
                let mismatch = (0_u32..)
                    .zip(mapped.as_chunks::<4>().0)
                    .find(|(index, bytes)| u32::from_le_bytes(**bytes) != index ^ 0x00ab_cdef);
                let verified = mismatch.map_or(Ok(()), |(index, bytes)| {
                    Err(format!(
                        "compute output at {index}: got {}, expected {}",
                        u32::from_le_bytes(*bytes),
                        index ^ 0x00ab_cdef
                    ))
                });
                drop(mapped);
                readback.unmap();
                verified
            });
            // A callback can run during queue destruction. Report failures to
            // the test thread instead of panicking inside backend teardown.
            let _ = sender.send(verified);
        });
    }
    (encoder, receiver)
}

fn assert_output(receiver: OutputResult) {
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the compute output callback must finish")
        .expect("the measured compute output must match");
}

#[test]
fn timestamp_queries_complete_asynchronously_and_reuse_slots() {
    let render_device = GpuRenderDevice::new_required_for_test("GPU timestamp tests")
        .expect("a GPU adapter is required for timestamp coverage");
    let device = render_device.device();
    let queue = render_device.queue();
    let timings = GpuTimings::with_enabled(device, queue, true);
    if !timings.snapshot().supported {
        assert!(!timings.snapshot().enabled);
        return;
    }
    assert!(timings.snapshot().enabled);
    let (encoder, output) = dispatch(&timings, device, true);
    assert_eq!(timings.snapshot().stages[0].completed, 0);
    let submission = queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .expect("submitted timestamp map must finish");
    assert_output(output);
    let first = &timings.snapshot().stages[0];
    assert_eq!(first.completed, 1);
    assert_eq!(first.failed, 0);
    assert_eq!(first.cancelled, 0);
    assert_eq!(first.skipped, 0);
    assert!(
        first.total_ns > 0,
        "the GPU dispatch must have measured duration"
    );
    assert_eq!(first.total_ns, first.last_ns);
    assert_eq!(first.max_ns, first.last_ns);

    // Saturation affects observations, never dispatch. Held leases represent
    // encoders or submissions whose callbacks have not yet retired.
    let leases = (0..SLOT_COUNT)
        .map(|_| {
            timings
                .begin(GpuStage::Sample)
                .expect("every retired slot must be reusable")
        })
        .collect::<Vec<_>>();
    assert!(timings.begin(GpuStage::Sample).is_none());
    assert_eq!(timings.snapshot().stages[3].skipped, 1);
    drop(leases);
    assert_eq!(
        timings.snapshot().stages[3].cancelled,
        u64::from(SLOT_COUNT)
    );
    assert!(timings.begin(GpuStage::Sample).is_some());

    // Discarding an encoded command buffer must also return its timing slot.
    let (encoder, _) = dispatch(&timings, device, false);
    drop(encoder);
    assert_eq!(timings.snapshot().stages[0].cancelled, 1);
    let (encoder, output) = dispatch(&timings, device, true);
    let submission = queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .expect("a reused timing slot must complete");
    assert_output(output);
    assert_eq!(timings.snapshot().stages[0].completed, 2);
}

#[test]
fn timestamp_queries_are_optional_on_devices_without_the_feature() {
    let render_device = GpuRenderDevice::new_required_for_test("GPU timestamp fallback")
        .expect("a GPU adapter is required for fallback coverage");
    let (device, queue) = pollster::block_on(
        render_device
            .adapter_for_test()
            .request_device(&wgpu::DeviceDescriptor::default()),
    )
    .expect("a device without optional timestamps must initialize");
    let timings = GpuTimings::with_enabled(&device, &queue, true);
    assert!(!timings.snapshot().supported);
    assert!(!timings.snapshot().enabled);
    let (encoder, output) = dispatch(&timings, &device, true);
    let submission = queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .expect("rendering without timestamps must still complete");
    assert_output(output);
    let snapshot = timings.snapshot();
    assert_eq!(snapshot.stages[0].attempts, 0);
    assert_eq!(snapshot.stages[0].skipped, 0);
    assert_eq!(snapshot.stages[0].completed, 0);
}

#[test]
fn timestamp_conversion_uses_device_period_and_rejects_clock_wraps() {
    assert_eq!(timestamp_duration_ns(10, 18, 2.5), Some(20));
    assert_eq!(timestamp_duration_ns(10, 10, 1.0), Some(0));
    assert_eq!(timestamp_duration_ns(18, 10, 1.0), None);
    assert_eq!(timestamp_duration_ns(10, 18, 0.0), None);
    assert_eq!(timestamp_duration_ns(10, 18, f64::NAN), None);
}

#[test]
fn timestamp_configuration_disabled_records_no_work_or_allocates_queries() {
    let render_device = GpuRenderDevice::new_required_for_test("GPU timing opt-in")
        .expect("GPU timing coverage requires an adapter");
    let device = render_device.device();
    let queue = render_device.queue();
    let timings = GpuTimings::with_enabled(device, queue, false);
    assert!(timings.state.resources.is_none());
    let (encoder, output) = dispatch(&timings, device, true);
    let submission = queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .expect("unprofiled compute must complete");
    assert_output(output);
    let snapshot = timings.snapshot();
    assert!(!snapshot.enabled);
    assert_eq!(
        snapshot.supported,
        device.features().contains(wgpu::Features::TIMESTAMP_QUERY)
    );
    if snapshot.supported {
        assert_eq!(
            snapshot.unavailable_reason.as_deref(),
            Some("disabled_by_configuration")
        );
    }
    for stage in snapshot.stages {
        assert_eq!(stage.attempts, 0);
        assert_eq!(stage.completed, 0);
        assert_eq!(stage.skipped, 0);
    }
}

#[test]
fn timestamp_rejected_submission_cannot_publish_a_reused_slots_old_duration() {
    let render_device = GpuRenderDevice::new_required_for_test("GPU timestamp rejected submission")
        .expect("GPU timing coverage requires an adapter");
    let device = render_device.device();
    let queue = render_device.queue();
    let timings = GpuTimings::with_enabled(device, queue, true);
    if !timings.snapshot().supported {
        return;
    }
    let (encoder, output) = dispatch(&timings, device, true);
    let submission = queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .expect("the first timing must complete");
    assert_output(output);
    let before = timings.snapshot();
    assert_eq!(before.stages[0].completed, 1);
    let commands = dispatch(&timings, device, false).0.finish();
    let doomed = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("intentionally destroyed submission input"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let target = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("invalid submission copy target"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut invalid = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    invalid.copy_buffer_to_buffer(&doomed, 0, &target, 0, 4);
    let invalid = invalid.finish();
    doomed.destroy();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _ = queue.submit([commands, invalid]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .expect("rejected command maps must retire");
    let error =
        pollster::block_on(scope.pop()).expect("the destroyed source must reject the submission");
    assert!(error.to_string().contains("destroyed"));
    let after = timings.snapshot();
    assert_eq!(after.stages[0].completed, before.stages[0].completed);
    assert_eq!(after.stages[0].total_ns, before.stages[0].total_ns);
    assert_eq!(after.stages[0].last_ns, before.stages[0].last_ns);
    assert_eq!(after.stages[0].failed, 1);
    assert_eq!(after.stages[0].cancelled, 0);
}

#[test]
fn timestamp_failed_mapping_retires_its_slot_without_reusing_invalid_map_state() {
    let render_device = GpuRenderDevice::new_required_for_test("GPU timestamp failed mapping")
        .expect("GPU timing coverage requires an adapter");
    let device = render_device.device();
    let queue = render_device.queue();
    let timings = GpuTimings::with_enabled(device, queue, true);
    if !timings.snapshot().supported {
        return;
    }
    let commands = dispatch(&timings, device, false).0.finish();
    let resources = timings
        .state
        .resources
        .as_ref()
        .expect("queries are available");
    resources.slots[0].readback.destroy();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _ = queue.submit([commands]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .expect("failed mappings must retire");
    assert!(pollster::block_on(scope.pop()).is_some());
    assert_eq!(timings.snapshot().stages[0].failed, 1);
    assert!(resources.slots[0].busy.load(Ordering::Relaxed));
    let lease = timings
        .begin(GpuStage::Compose)
        .expect("unaffected slots remain usable");
    assert_eq!(lease.index, 1);
}

#[test]
#[ignore = "GPU throughput experiment; run alone with --ignored --nocapture"]
fn gpu_timestamp_overhead_benchmark() {
    let render_device = GpuRenderDevice::new_required_for_test("GPU timestamp overhead")
        .expect("GPU benchmark requires an adapter");
    let device = render_device.device();
    let queue = render_device.queue();
    let enabled = GpuTimings::with_enabled(device, queue, true);
    assert!(
        enabled.snapshot().enabled,
        "benchmark requires timestamp queries"
    );
    let disabled = GpuTimings::with_enabled(device, queue, false);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("timing overhead shader"),
        source: wgpu::ShaderSource::Wgsl(
            "@group(0) @binding(0) var<storage, read_write> output: array<u32>;
             @compute @workgroup_size(64)
             fn main(@builtin(global_invocation_id) id: vec3<u32>) {
                 output[id.x] = id.x ^ 0xabcdefu;
             }"
            .into(),
        ),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("timing overhead pipeline"),
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("timing overhead output"),
        size: 4096 * 4,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("timing overhead bind group"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });
    let run = |timings: &GpuTimings, batches: usize| {
        let before = timings.snapshot().stages[0].total_ns;
        let start = std::time::Instant::now();
        let mut encode_time = std::time::Duration::ZERO;
        for _ in 0..batches {
            let encode_start = std::time::Instant::now();
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for _ in 0..32 {
                timings.compute(
                    &mut encoder,
                    GpuStage::Compose,
                    "overhead compute",
                    |pass| {
                        pass.set_pipeline(&pipeline);
                        pass.set_bind_group(0, &bind_group, &[]);
                        pass.dispatch_workgroups(64, 1, 1);
                    },
                );
            }
            let commands = encoder.finish();
            encode_time += encode_start.elapsed();
            let submission = queue.submit([commands]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(std::time::Duration::from_secs(10)),
                })
                .expect("benchmark submission must finish");
        }
        (
            encode_time,
            start.elapsed(),
            timings.snapshot().stages[0].total_ns - before,
        )
    };
    let _ = run(&disabled, 20);
    let _ = run(&enabled, 20);
    // Alternating order exposes clock or warmup drift rather than hiding it
    // inside a single before/after pair. Wall time includes GPU completion.
    for round in 0..4 {
        for (label, timings) in if round % 2 == 0 {
            [("disabled", &disabled), ("enabled", &enabled)]
        } else {
            [("enabled", &enabled), ("disabled", &disabled)]
        } {
            let (host_encode, wall, gpu_ns) = run(timings, 100);
            eprintln!(
                "timestamp_overhead round={round} mode={label} passes=3200 host_encode_us={} completion_wall_us={} measured_pass_gpu_ns={gpu_ns}",
                host_encode.as_micros(),
                wall.as_micros()
            );
        }
    }
    assert_eq!(enabled.snapshot().stages[0].skipped, 0);
    assert_eq!(enabled.snapshot().stages[0].failed, 0);
}
