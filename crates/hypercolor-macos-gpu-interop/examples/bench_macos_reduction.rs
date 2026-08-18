use std::fmt;

const DEFAULT_SOURCE: Extent = Extent {
    width: 1920,
    height: 1080,
};
const DEFAULT_OUTPUT: Extent = Extent {
    width: 320,
    height: 180,
};
const DEFAULT_ITERATIONS: usize = 100;
const DEFAULT_WARMUP: usize = 10;
const MIN_ITERATIONS: usize = 100;
const MIN_WARMUP: usize = 10;
const MAX_DIMENSION: u32 = 8_192;
const MAX_PIXELS: u64 = 67_108_864;
const MAX_ITERATIONS: usize = 10_000;
const MAX_WARMUP: usize = 1_000;
const MAX_OPTION_PAIRS: usize = 5;
const BYTES_PER_PIXEL: u64 = 4;
const GPU_REDUCTION_METRIC: &str = "gpu_reduction_time";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Extent {
    width: u32,
    height: u32,
}

impl Extent {
    fn parse(value: &str, name: &str) -> Result<Self, String> {
        let (width, height) = value
            .split_once('x')
            .ok_or_else(|| format!("{name} must use WIDTHxHEIGHT"))?;
        let extent = Self {
            width: parse_u32(width, name)?,
            height: parse_u32(height, name)?,
        };
        extent.byte_len()?;
        Ok(extent)
    }

    fn pixels(self) -> Result<u64, String> {
        if self.width == 0
            || self.height == 0
            || self.width > MAX_DIMENSION
            || self.height > MAX_DIMENSION
        {
            return Err(format!(
                "extent must be between 1x1 and {MAX_DIMENSION}x{MAX_DIMENSION}"
            ));
        }
        let pixels = u64::from(self.width) * u64::from(self.height);
        if pixels > MAX_PIXELS {
            return Err(format!(
                "extent exceeds the {MAX_PIXELS}-pixel allocation bound"
            ));
        }
        Ok(pixels)
    }

    fn byte_len(self) -> Result<usize, String> {
        let bytes = self
            .pixels()?
            .checked_mul(BYTES_PER_PIXEL)
            .ok_or_else(|| "pixel byte count overflowed".to_owned())?;
        usize::try_from(bytes).map_err(|_| "pixel byte count does not fit usize".to_owned())
    }
}

impl fmt::Display for Extent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}x{}", self.width, self.height)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Filter {
    Nearest,
    Bilinear,
    #[default]
    Area,
}

impl Filter {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "nearest" => Ok(Self::Nearest),
            "bilinear" => Ok(Self::Bilinear),
            "area" => Ok(Self::Area),
            _ => Err("filter must be nearest, bilinear, or area".to_owned()),
        }
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nearest => formatter.write_str("nearest"),
            Self::Bilinear => formatter.write_str("bilinear"),
            Self::Area => formatter.write_str("area"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Args {
    source: Extent,
    output: Extent,
    filter: Filter,
    iterations: usize,
    warmup: usize,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            source: DEFAULT_SOURCE,
            output: DEFAULT_OUTPUT,
            filter: Filter::Area,
            iterations: DEFAULT_ITERATIONS,
            warmup: DEFAULT_WARMUP,
        }
    }
}

impl Args {
    fn parse_from(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut arguments = arguments.into_iter();
        let _program = arguments.next();
        let mut option_pairs = 0;
        while let Some(argument) = arguments.next() {
            option_pairs += 1;
            if option_pairs > MAX_OPTION_PAIRS {
                return Err(format!(
                    "at most {MAX_OPTION_PAIRS} option pairs are accepted"
                ));
            }
            let value = arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))?;
            match argument.as_str() {
                "--source" => parsed.source = Extent::parse(&value, "source")?,
                "--output" => parsed.output = Extent::parse(&value, "output")?,
                "--filter" => parsed.filter = Filter::parse(&value)?,
                "--iterations" => {
                    parsed.iterations =
                        parse_bounded_usize(&value, "iterations", MIN_ITERATIONS, MAX_ITERATIONS)?;
                }
                "--warmup" => {
                    parsed.warmup = parse_bounded_usize(&value, "warmup", MIN_WARMUP, MAX_WARMUP)?;
                }
                _ => return Err(format!("unknown argument {argument}")),
            }
        }
        parsed.source.byte_len()?;
        parsed.output.byte_len()?;
        Ok(parsed)
    }
}

fn parse_u32(value: &str, name: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("{name} contains an invalid integer"))
}

fn parse_bounded_usize(
    value: &str,
    name: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{name} must be an integer"))?;
    if (minimum..=maximum).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(format!("{name} must be between {minimum} and {maximum}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Percentiles {
    p50_ns: u128,
    p95_ns: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Metal4Decision {
    NotQualified {
        missing_facilities: [Option<&'static str>; 8],
    },
    NotMeasured(Option<Metal4Failure>),
    Adopt,
    Reject(Metal4Rejection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Metal4Rejection {
    OutputMismatch,
    BaselineMetricUnavailable,
    InsufficientP95Improvement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Metal4Evidence {
    output_parity: bool,
    wgpu_gpu_p95_ns: u128,
    metal4_gpu_p95_ns: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Metal4Failure {
    QualifiedSetup,
    Warmup,
    BaselineMeasurement,
    WgpuGpuIntervalUnavailable,
    BaselineReadback,
    BaselineParity,
    BaselinePercentile,
    TargetCreation,
    ShaderCompilation,
    ShaderEntryPoint,
    PipelineCreation,
    MetalDeviceAccess,
    TargetTextureAccess,
    SourcePlaneAccess,
    CommandSetup,
    DispatchOrCompletion,
    CommitFeedback,
    OutputReadback,
    Metal4Percentile,
}

impl Metal4Failure {
    const fn code(self) -> &'static str {
        match self {
            Self::QualifiedSetup => "qualified_setup_failed",
            Self::Warmup => "warmup_failed",
            Self::BaselineMeasurement => "baseline_measurement_failed",
            Self::WgpuGpuIntervalUnavailable => "wgpu_gpu_interval_unavailable",
            Self::BaselineReadback => "baseline_readback_failed",
            Self::BaselineParity => "baseline_parity_failed",
            Self::BaselinePercentile => "baseline_percentile_failed",
            Self::TargetCreation => "target_creation_failed",
            Self::ShaderCompilation => "shader_compilation_failed",
            Self::ShaderEntryPoint => "shader_entry_point_missing",
            Self::PipelineCreation => "pipeline_creation_failed",
            Self::MetalDeviceAccess => "metal_device_access_failed",
            Self::TargetTextureAccess => "target_texture_access_failed",
            Self::SourcePlaneAccess => "source_plane_access_failed",
            Self::CommandSetup => "command_setup_failed",
            Self::DispatchOrCompletion => "dispatch_or_completion_failed",
            Self::CommitFeedback => "commit_feedback_failed",
            Self::OutputReadback => "output_readback_failed",
            Self::Metal4Percentile => "metal4_percentile_failed",
        }
    }

    const fn hardware_run(self) -> bool {
        matches!(
            self,
            Self::DispatchOrCompletion
                | Self::CommitFeedback
                | Self::OutputReadback
                | Self::Metal4Percentile
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Metal4Evaluation {
    NotRun,
    Failed(Metal4Failure),
    Measured(Metal4Evidence),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Metal4Artifact {
    hardware_attempted: bool,
    hardware_run: bool,
    status: &'static str,
    decision: &'static str,
    reason: &'static str,
    failure: Option<&'static str>,
    missing_facilities: [Option<&'static str>; 8],
}

fn metal4_decision(
    probe: hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe,
    evaluation: Metal4Evaluation,
) -> Metal4Decision {
    if !probe.all_required_facilities() {
        return Metal4Decision::NotQualified {
            missing_facilities: probe.missing_facilities(),
        };
    }
    let evidence = match evaluation {
        Metal4Evaluation::NotRun => return Metal4Decision::NotMeasured(None),
        Metal4Evaluation::Failed(failure) => {
            return Metal4Decision::NotMeasured(Some(failure));
        }
        Metal4Evaluation::Measured(evidence) => evidence,
    };
    if !evidence.output_parity {
        return Metal4Decision::Reject(Metal4Rejection::OutputMismatch);
    }
    if evidence.wgpu_gpu_p95_ns == 0 {
        return Metal4Decision::Reject(Metal4Rejection::BaselineMetricUnavailable);
    }
    let adoption_ceiling = evidence
        .wgpu_gpu_p95_ns
        .saturating_sub(evidence.wgpu_gpu_p95_ns.div_ceil(10));
    if evidence.metal4_gpu_p95_ns <= adoption_ceiling {
        Metal4Decision::Adopt
    } else {
        Metal4Decision::Reject(Metal4Rejection::InsufficientP95Improvement)
    }
}

fn metal4_artifact(
    probe: hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe,
    evaluation: Metal4Evaluation,
) -> Metal4Artifact {
    let hardware_run = match evaluation {
        Metal4Evaluation::NotRun => false,
        Metal4Evaluation::Failed(failure) => failure.hardware_run(),
        Metal4Evaluation::Measured(_) => true,
    };
    let hardware_attempted = matches!(
        evaluation,
        Metal4Evaluation::Failed(_) | Metal4Evaluation::Measured(_)
    );
    match metal4_decision(probe, evaluation) {
        Metal4Decision::NotQualified { missing_facilities } => Metal4Artifact {
            hardware_attempted,
            hardware_run,
            status: "not_qualified",
            decision: "not_evaluated",
            reason: "required_facilities_unavailable",
            failure: None,
            missing_facilities,
        },
        Metal4Decision::NotMeasured(failure) => Metal4Artifact {
            hardware_attempted,
            hardware_run,
            status: "not_measured",
            decision: "not_evaluated",
            reason: failure.map_or("qualified_hardware_run_missing", Metal4Failure::code),
            failure: failure.map(Metal4Failure::code),
            missing_facilities: [None; 8],
        },
        Metal4Decision::Adopt => Metal4Artifact {
            hardware_attempted,
            hardware_run,
            status: "measured",
            decision: "adopt",
            reason: "exact_parity_and_p95_improvement_at_least_10_percent",
            failure: None,
            missing_facilities: [None; 8],
        },
        Metal4Decision::Reject(reason) => Metal4Artifact {
            hardware_attempted,
            hardware_run,
            status: "measured",
            decision: "reject",
            reason: match reason {
                Metal4Rejection::OutputMismatch => "output_parity_mismatch",
                Metal4Rejection::BaselineMetricUnavailable => "wgpu_p95_metric_unavailable",
                Metal4Rejection::InsufficientP95Improvement => "p95_improvement_below_10_percent",
            },
            failure: None,
            missing_facilities: [None; 8],
        },
    }
}

#[cfg(any(target_os = "macos", test))]
fn write_metal4_artifact(
    output: &mut impl std::io::Write,
    probe: hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe,
    evaluation: Metal4Evaluation,
) -> std::io::Result<()> {
    writeln!(output, "metal4_registry_id={}", probe.metal_registry_id)?;
    writeln!(output, "metal4_family={}", probe.metal4_family)?;
    writeln!(
        output,
        "metal4_command_allocator={}",
        probe.command_allocator
    )?;
    writeln!(output, "metal4_command_queue={}", probe.command_queue)?;
    writeln!(output, "metal4_command_buffer={}", probe.command_buffer)?;
    writeln!(output, "metal4_argument_table={}", probe.argument_table)?;
    writeln!(output, "metal4_residency_set={}", probe.residency_set)?;
    writeln!(output, "metal4_shared_event={}", probe.shared_event)?;
    writeln!(output, "metal4_commit_feedback={}", probe.commit_feedback)?;
    writeln!(output, "metal4_artifact_schema=spec76-v1")?;
    let artifact = metal4_artifact(probe, evaluation);
    writeln!(
        output,
        "metal4_hardware_attempted={}",
        artifact.hardware_attempted
    )?;
    writeln!(output, "metal4_hardware_run={}", artifact.hardware_run)?;
    writeln!(output, "metal4_status={}", artifact.status)?;
    writeln!(output, "metal4_decision={}", artifact.decision)?;
    writeln!(output, "metal4_decision_reason={}", artifact.reason)?;
    if let Some(failure) = artifact.failure {
        writeln!(output, "metal4_failure={failure}")?;
    }
    let mut missing = artifact.missing_facilities.into_iter().flatten().peekable();
    if missing.peek().is_some() {
        write!(output, "metal4_missing_facilities=")?;
        for (index, facility) in missing.enumerate() {
            if index > 0 {
                write!(output, ",")?;
            }
            write!(output, "{facility}")?;
        }
        writeln!(output)?;
    }
    Ok(())
}

fn percentiles(samples: &[u128]) -> Result<Percentiles, String> {
    if samples.is_empty() {
        return Err("at least one timing sample is required".to_owned());
    }
    let mut sorted = Vec::new();
    sorted
        .try_reserve_exact(samples.len())
        .map_err(|_| "timing sample allocation failed".to_owned())?;
    sorted.extend_from_slice(samples);
    sorted.sort_unstable();
    Ok(Percentiles {
        p50_ns: sorted[percentile_index(sorted.len(), 50)],
        p95_ns: sorted[percentile_index(sorted.len(), 95)],
    })
}

const fn percentile_index(sample_count: usize, percentile: usize) -> usize {
    (sample_count * percentile).div_ceil(100).saturating_sub(1)
}

fn p95_improvement_basis_points(baseline_ns: u128, candidate_ns: u128) -> Option<i128> {
    if baseline_ns == 0 {
        return None;
    }
    let baseline = i128::try_from(baseline_ns).unwrap_or(i128::MAX);
    let candidate = i128::try_from(candidate_ns).unwrap_or(i128::MAX);
    Some(baseline.saturating_sub(candidate).saturating_mul(10_000) / baseline)
}

fn gpu_interval_nanoseconds(started: f64, completed: f64) -> Result<u128, String> {
    let seconds = completed - started;
    let nanoseconds = seconds * 1_000_000_000.0;
    if started <= 0.0 || !started.is_finite() || !completed.is_finite() || seconds <= 0.0 {
        return Err("GPU interval feedback is invalid".to_owned());
    }
    if !nanoseconds.is_finite() || nanoseconds <= 0.0 || nanoseconds > u128::MAX as f64 {
        return Err("GPU duration is not representable in nanoseconds".to_owned());
    }
    Ok(nanoseconds.round() as u128)
}

#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = macos::run() {
        eprintln!("bench_macos_reduction: {error}");
        std::process::exit(2);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("bench_macos_reduction requires macOS and a Metal-backed wgpu device");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_int, c_ulong, c_void};
    use std::io::{self, Write};
    use std::mem::{align_of, size_of};
    use std::ptr::NonNull;
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::time::{Duration, Instant};

    use hypercolor_macos_capture::{
        MacosCaptureColorimetry, MacosCaptureFrame, MacosCaptureGeometry, MacosCapturePixelFormat,
        MacosCaptureSurface, MacosColorPrimaries, MacosColorRange, MacosPixelExtent,
        MacosPixelRect, MacosPointRect, MacosScale, MacosTransferFunction,
    };
    use hypercolor_macos_gpu_interop::{
        MacosMetal4CapabilityProbe, MacosNativeReducer, MacosNativeReductionDescriptor,
        MacosNativeReductionFilter, MacosNativeTargetFormat, MacosScreenBridge,
        probe_macos_metal4_capabilities,
    };
    use objc2::{
        msg_send,
        rc::Retained,
        runtime::{AnyObject, ProtocolObject},
    };
    use objc2_foundation::NSString;
    use objc2_metal::{
        MTL4ArgumentTable, MTL4ArgumentTableDescriptor, MTL4CommandAllocator, MTL4CommandBuffer,
        MTL4CommandEncoder, MTL4CommandQueue, MTL4CommitOptions, MTL4ComputeCommandEncoder,
        MTLBuffer, MTLCommandBuffer, MTLComputePipelineState, MTLDevice, MTLLibrary,
        MTLResidencySet, MTLResidencySetDescriptor, MTLResourceOptions, MTLSharedEvent, MTLSize,
        MTLTexture,
    };

    use super::{
        Args, BYTES_PER_PIXEL, Extent, Filter, Metal4Evaluation, Metal4Evidence, Metal4Failure,
        Percentiles, gpu_interval_nanoseconds, percentiles, write_metal4_artifact,
    };

    const METAL4_COMPLETION_TIMEOUT_MS: u64 = 10_000;
    const NATIVE_REDUCTION_SHADER: &str = include_str!("../src/native_reduction.metal");

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Metal4Measurement {
        timings: Percentiles,
        output_parity: bool,
    }

    #[derive(Clone, Copy)]
    struct Metal4Report {
        probe: MacosMetal4CapabilityProbe,
        evaluation: Metal4Evaluation,
        measurement: Option<Metal4Measurement>,
    }

    struct QualifiedBenchmarkReport {
        frame: Arc<MacosCaptureFrame>,
        cpu: Percentiles,
        wgpu: WgpuMeasurement,
        metal4: Metal4Report,
        terminal_error: Option<&'static str>,
    }

    struct Metal4RunError {
        failure: Metal4Failure,
        detail: String,
    }

    impl Metal4RunError {
        fn new(failure: Metal4Failure, detail: impl Into<String>) -> Self {
            Self {
                failure,
                detail: detail.into(),
            }
        }
    }

    pub fn run() -> Result<(), String> {
        let args = Args::parse_from(std::env::args())?;
        let wgpu = WgpuFixture::new()?;
        let metal4 =
            probe_macos_metal4_capabilities(&wgpu.device).map_err(|error| error.to_string())?;
        if !metal4.all_required_facilities() {
            print_metal4_artifact(metal4, Metal4Evaluation::NotRun);
            flush_stdout();
            return Ok(());
        }
        match run_qualified(args, &wgpu, metal4) {
            Ok(report) => {
                print_report(
                    args,
                    &report.frame,
                    &wgpu.adapter_info,
                    report.cpu,
                    &report.wgpu,
                    report.metal4,
                );
                flush_stdout();
                report
                    .terminal_error
                    .map_or(Ok(()), |error| Err(error.to_owned()))
            }
            Err(error) => {
                print_metal4_artifact(metal4, Metal4Evaluation::Failed(error.failure));
                flush_stdout();
                Err(error.detail)
            }
        }
    }

    fn run_qualified(
        args: Args,
        wgpu: &WgpuFixture,
        metal4: MacosMetal4CapabilityProbe,
    ) -> Result<QualifiedBenchmarkReport, Metal4RunError> {
        let setup_error = |error: String| Metal4RunError::new(Metal4Failure::QualifiedSetup, error);
        let source_pixels = synthetic_bgra(args.source).map_err(&setup_error)?;
        let frame = Arc::new(capture_frame(args.source, &source_pixels).map_err(&setup_error)?);
        let bridge =
            MacosScreenBridge::new(&wgpu.device).map_err(|error| setup_error(error.to_string()))?;
        let imported = bridge
            .import_frame(&wgpu.device, 1, Arc::clone(&frame))
            .map_err(|error| setup_error(error.to_string()))?;
        let reducer = MacosNativeReducer::new(&wgpu.device)
            .map_err(|error| setup_error(error.to_string()))?;
        let target = reducer
            .create_target(
                &wgpu.device,
                args.output.width,
                args.output.height,
                MacosNativeTargetFormat::Rgba8,
            )
            .map_err(|error| setup_error(error.to_string()))?;
        let descriptor = reduction_descriptor(args).map_err(&setup_error)?;
        let output_bytes = args.output.byte_len().map_err(&setup_error)?;
        let mut cpu_output = allocate_bytes(output_bytes, "CPU output").map_err(&setup_error)?;

        for _ in 0..args.warmup {
            reduce_scalar(&frame, args.output, args.filter, &mut cpu_output)
                .map_err(|error| Metal4RunError::new(Metal4Failure::Warmup, error))?;
            reduce_wgpu(
                &wgpu.device,
                &wgpu.queue,
                &reducer,
                &imported,
                &target,
                descriptor,
            )
            .map_err(|error| Metal4RunError::new(Metal4Failure::Warmup, error))?;
        }

        let cpu_times = measure(args.iterations, || {
            reduce_scalar(&frame, args.output, args.filter, &mut cpu_output)
        })
        .map_err(|error| Metal4RunError::new(Metal4Failure::BaselineMeasurement, error))?;
        let wgpu_measurement = measure_wgpu(
            wgpu,
            &reducer,
            &imported,
            &target,
            descriptor,
            args.iterations,
        )?;
        let gpu_output =
            read_texture_pixels(&wgpu.device, &wgpu.queue, target.texture(), args.output)
                .map_err(|error| Metal4RunError::new(Metal4Failure::BaselineReadback, error))?;
        if cpu_output != gpu_output {
            let mismatch = cpu_output
                .iter()
                .zip(&gpu_output)
                .position(|(cpu, gpu)| cpu != gpu)
                .unwrap_or(cpu_output.len());
            return Err(Metal4RunError::new(
                Metal4Failure::BaselineParity,
                format!(
                    "exact output parity failed at byte {mismatch}: CPU={:?}, wgpu={:?}",
                    cpu_output.get(mismatch),
                    gpu_output.get(mismatch)
                ),
            ));
        }

        let cpu = percentiles(&cpu_times)
            .map_err(|error| Metal4RunError::new(Metal4Failure::BaselinePercentile, error))?;
        let metal4_measurement = evaluate_metal4(wgpu, &reducer, &imported, args, &cpu_output)?;
        let terminal_error =
            (!metal4_measurement.output_parity).then_some("Metal 4 exact output parity failed");
        let wgpu_gpu_p95_ns = wgpu_measurement.gpu.p95_ns;
        Ok(QualifiedBenchmarkReport {
            frame,
            cpu,
            wgpu: wgpu_measurement,
            metal4: Metal4Report {
                probe: metal4,
                evaluation: Metal4Evaluation::Measured(Metal4Evidence {
                    output_parity: metal4_measurement.output_parity,
                    wgpu_gpu_p95_ns,
                    metal4_gpu_p95_ns: metal4_measurement.timings.p95_ns,
                }),
                measurement: Some(metal4_measurement),
            },
            terminal_error,
        })
    }

    fn evaluate_metal4(
        wgpu: &WgpuFixture,
        reducer: &MacosNativeReducer,
        imported: &hypercolor_macos_gpu_interop::ImportedMacosScreenFrame,
        args: Args,
        cpu_output: &[u8],
    ) -> Result<Metal4Measurement, Metal4RunError> {
        let target = reducer
            .create_target(
                &wgpu.device,
                args.output.width,
                args.output.height,
                MacosNativeTargetFormat::Rgba8,
            )
            .map_err(|error| {
                Metal4RunError::new(Metal4Failure::TargetCreation, error.to_string())
            })?;
        let timings = measure_metal4(&wgpu.device, imported, &target, args)?;
        let output = read_texture_pixels(&wgpu.device, &wgpu.queue, target.texture(), args.output)
            .map_err(|error| Metal4RunError::new(Metal4Failure::OutputReadback, error))?;
        Ok(Metal4Measurement {
            timings,
            output_parity: output == cpu_output,
        })
    }

    fn allocate_bytes(length: usize, name: &str) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| format!("{name} allocation of {length} bytes failed"))?;
        bytes.resize(length, 0);
        Ok(bytes)
    }

    fn synthetic_bgra(extent: Extent) -> Result<Vec<u8>, String> {
        let mut pixels = allocate_bytes(extent.byte_len()?, "source fixture")?;
        let width = usize::try_from(extent.width).map_err(|error| error.to_string())?;
        for (index, pixel) in pixels
            .chunks_exact_mut(BYTES_PER_PIXEL as usize)
            .enumerate()
        {
            let x = index % width;
            let y = index / width;
            pixel.copy_from_slice(&[
                ((x * 17 + y * 29) & 0xff) as u8,
                ((x * 31 + y * 7) & 0xff) as u8,
                ((x * 11 + y * 43) & 0xff) as u8,
                255,
            ]);
        }
        Ok(pixels)
    }

    fn capture_frame(extent: Extent, pixels: &[u8]) -> Result<MacosCaptureFrame, String> {
        let extent = MacosPixelExtent::new(extent.width, extent.height)
            .map_err(|error| error.to_string())?;
        let (surface, plane) = MacosCaptureSurface::new_native_bgra_fixture(extent, pixels)
            .map_err(|error| error.to_string())?;
        Ok(MacosCaptureFrame {
            epoch: 1,
            sequence: 1,
            display_time: 1,
            storage_extent: extent,
            planes: Arc::from([plane]),
            pixel_format: MacosCapturePixelFormat::Bgra8,
            color: MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Srgb,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            geometry: MacosCaptureGeometry {
                display_scale_factor: MacosScale::display(1.0)
                    .map_err(|error| error.to_string())?,
                content_scale: MacosScale::new(1.0).map_err(|error| error.to_string())?,
                content_rect_points: MacosPointRect::new(
                    0.0,
                    0.0,
                    extent.width.into(),
                    extent.height.into(),
                )
                .map_err(|error| error.to_string())?,
                content_rect_pixels: MacosPixelRect::new(0, 0, extent.width, extent.height)
                    .map_err(|error| error.to_string())?,
                screen_rect_points: None,
                bounding_rect_points: None,
                bounding_rect_pixels: None,
            },
            damage: Arc::from([]),
            cursor_composed: false,
            surface,
        })
    }

    fn reduction_descriptor(args: Args) -> Result<MacosNativeReductionDescriptor, String> {
        MacosNativeReductionDescriptor::new(
            [args.output.width, args.output.height],
            [0, 0, args.output.width, args.output.height],
            [
                0.0,
                0.0,
                args.source.width as f32,
                args.source.height as f32,
            ],
            match args.filter {
                Filter::Nearest => MacosNativeReductionFilter::Nearest,
                Filter::Bilinear => MacosNativeReductionFilter::Bilinear,
                Filter::Area => MacosNativeReductionFilter::Area,
            },
            None,
        )
        .map_err(|error| error.to_string())
    }

    fn measure(
        iterations: usize,
        mut operation: impl FnMut() -> Result<(), String>,
    ) -> Result<Vec<u128>, String> {
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(iterations)
            .map_err(|_| "timing sample allocation failed".to_owned())?;
        for _ in 0..iterations {
            let started = Instant::now();
            operation()?;
            samples.push(started.elapsed().as_nanos());
        }
        Ok(samples)
    }

    fn reduce_wgpu(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        reducer: &MacosNativeReducer,
        imported: &hypercolor_macos_gpu_interop::ImportedMacosScreenFrame,
        target: &hypercolor_macos_gpu_interop::MacosNativeReductionTarget,
        descriptor: MacosNativeReductionDescriptor,
    ) -> Result<(), String> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_macos_reduction wgpu iteration"),
        });
        reducer
            .encode(imported, target, descriptor, &mut encoder)
            .map_err(|error| error.to_string())?;
        let submission = queue.submit(Some(encoder.finish()));
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| format!("wgpu reduction wait failed: {error:?}"))?;
        Ok(())
    }

    fn reduce_wgpu_measured(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        reducer: &MacosNativeReducer,
        imported: &hypercolor_macos_gpu_interop::ImportedMacosScreenFrame,
        target: &hypercolor_macos_gpu_interop::MacosNativeReductionTarget,
        descriptor: MacosNativeReductionDescriptor,
    ) -> Result<(u128, u128), Metal4RunError> {
        let started = Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_macos_reduction measured wgpu iteration"),
        });
        reducer
            .encode(imported, target, descriptor, &mut encoder)
            .map_err(|error| {
                Metal4RunError::new(Metal4Failure::BaselineMeasurement, error.to_string())
            })?;
        // SAFETY: the raw command buffer is retained only for post-completion
        // timing queries. wgpu still owns encoding, submission, and teardown.
        let command_buffer = unsafe {
            encoder.as_hal_mut::<wgpu_hal::api::Metal, _, _>(|hal_encoder| {
                let raw = hal_encoder?.raw_command_buffer()?;
                Retained::retain(std::ptr::from_ref(raw).cast_mut())
            })
        }
        .ok_or_else(|| {
            Metal4RunError::new(
                Metal4Failure::WgpuGpuIntervalUnavailable,
                "wgpu reduction has no retained Metal command buffer",
            )
        })?;
        let submission = queue.submit(Some(encoder.finish()));
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| {
                Metal4RunError::new(
                    Metal4Failure::BaselineMeasurement,
                    format!("wgpu reduction wait failed: {error:?}"),
                )
            })?;
        let (gpu_started, gpu_completed) = metal_command_buffer_gpu_interval(&command_buffer);
        Ok((
            started.elapsed().as_nanos(),
            gpu_interval_nanoseconds(gpu_started, gpu_completed).map_err(|error| {
                Metal4RunError::new(Metal4Failure::WgpuGpuIntervalUnavailable, error)
            })?,
        ))
    }

    fn metal_command_buffer_gpu_interval(
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    ) -> (f64, f64) {
        // SAFETY: these selectors are scalar properties of MTLCommandBuffer, and
        // the retained command buffer has completed before this function runs.
        unsafe {
            (
                msg_send![command_buffer, GPUStartTime],
                msg_send![command_buffer, GPUEndTime],
            )
        }
    }

    struct WgpuMeasurement {
        completion: Percentiles,
        gpu: Percentiles,
    }

    fn measure_wgpu(
        wgpu: &WgpuFixture,
        reducer: &MacosNativeReducer,
        imported: &hypercolor_macos_gpu_interop::ImportedMacosScreenFrame,
        target: &hypercolor_macos_gpu_interop::MacosNativeReductionTarget,
        descriptor: MacosNativeReductionDescriptor,
        iterations: usize,
    ) -> Result<WgpuMeasurement, Metal4RunError> {
        let mut completion_samples = Vec::new();
        completion_samples
            .try_reserve_exact(iterations)
            .map_err(|_| {
                Metal4RunError::new(
                    Metal4Failure::BaselineMeasurement,
                    "wgpu completion sample allocation failed",
                )
            })?;
        let mut gpu_samples = Vec::new();
        gpu_samples.try_reserve_exact(iterations).map_err(|_| {
            Metal4RunError::new(
                Metal4Failure::BaselineMeasurement,
                "wgpu GPU sample allocation failed",
            )
        })?;
        for _ in 0..iterations {
            let (completion_ns, gpu_ns) = reduce_wgpu_measured(
                &wgpu.device,
                &wgpu.queue,
                reducer,
                imported,
                target,
                descriptor,
            )?;
            completion_samples.push(completion_ns);
            gpu_samples.push(gpu_ns);
        }
        Ok(WgpuMeasurement {
            completion: percentiles(&completion_samples)
                .map_err(|error| Metal4RunError::new(Metal4Failure::BaselinePercentile, error))?,
            gpu: percentiles(&gpu_samples)
                .map_err(|error| Metal4RunError::new(Metal4Failure::BaselinePercentile, error))?,
        })
    }

    #[repr(C, align(16))]
    #[derive(Clone, Copy)]
    struct Metal4ColorTransform {
        source_to_target: [[f32; 4]; 3],
        source_luminance_and_exposure: [f32; 4],
        curve: [f32; 4],
    }

    #[repr(C, align(16))]
    #[derive(Clone, Copy)]
    struct Metal4ReductionParameters {
        content_rect: [u32; 4],
        output_and_format: [u32; 4],
        source_rect: [f32; 4],
        source_and_chroma_extent: [u32; 4],
        color: [u32; 4],
        operation: [u32; 4],
        transform: Metal4ColorTransform,
    }

    const _: () = {
        assert!(size_of::<Metal4ReductionParameters>() == 176);
        assert!(align_of::<Metal4ReductionParameters>() == 16);
    };

    impl Metal4ReductionParameters {
        fn new(args: Args) -> Self {
            Self {
                content_rect: [0, 0, args.output.width, args.output.height],
                output_and_format: [
                    args.output.width,
                    args.output.height,
                    0,
                    match args.filter {
                        Filter::Nearest => 0,
                        Filter::Bilinear => 1,
                        Filter::Area => 2,
                    },
                ],
                source_rect: [
                    0.0,
                    0.0,
                    args.source.width as f32,
                    args.source.height as f32,
                ],
                source_and_chroma_extent: [
                    args.source.width,
                    args.source.height,
                    args.source.width,
                    args.source.height,
                ],
                color: [0; 4],
                operation: [0; 4],
                transform: Metal4ColorTransform {
                    source_to_target: [
                        [1.0, 0.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                    ],
                    source_luminance_and_exposure: [0.212_639, 0.715_168_65, 0.072_192_32, 1.0],
                    curve: [1.0; 4],
                },
            }
        }
    }

    #[derive(Clone, Copy)]
    struct Metal4CommitFeedbackSample {
        gpu_started: f64,
        gpu_completed: f64,
        runtime_error: bool,
    }

    #[repr(C)]
    struct Metal4FeedbackBlockDescriptor {
        reserved: c_ulong,
        size: c_ulong,
    }

    #[repr(C)]
    struct Metal4FeedbackBlock {
        isa: *const c_void,
        flags: c_int,
        reserved: c_int,
        invoke: unsafe extern "C-unwind" fn(*mut Self, *mut AnyObject),
        descriptor: *const Metal4FeedbackBlockDescriptor,
    }

    // SAFETY: the block and descriptor are immutable static ABI records.
    unsafe impl Sync for Metal4FeedbackBlock {}

    unsafe extern "C-unwind" {
        static _NSConcreteGlobalBlock: c_void;
    }

    static METAL4_FEEDBACK_DESCRIPTOR: Metal4FeedbackBlockDescriptor =
        Metal4FeedbackBlockDescriptor {
            reserved: 0,
            size: size_of::<Metal4FeedbackBlock>() as c_ulong,
        };
    static METAL4_FEEDBACK_SAMPLE: Mutex<Option<Metal4CommitFeedbackSample>> = Mutex::new(None);
    static METAL4_FEEDBACK_READY: Condvar = Condvar::new();
    static METAL4_FEEDBACK_SERIALIZER: Mutex<()> = Mutex::new(());
    static METAL4_FEEDBACK_BLOCK: Metal4FeedbackBlock = Metal4FeedbackBlock {
        // SAFETY: the symbol is the Blocks runtime class for immutable global blocks.
        isa: &raw const _NSConcreteGlobalBlock,
        flags: (1 << 28) | (1 << 29),
        reserved: 0,
        invoke: receive_metal4_commit_feedback,
        descriptor: &raw const METAL4_FEEDBACK_DESCRIPTOR,
    };

    unsafe extern "C-unwind" fn receive_metal4_commit_feedback(
        _block: *mut Metal4FeedbackBlock,
        feedback: *mut AnyObject,
    ) {
        // SAFETY: Metal invokes this block with a live MTL4CommitFeedback object.
        let Some(feedback) = (unsafe { feedback.as_ref() }) else {
            return;
        };
        // SAFETY: Metal supplies an object conforming to MTL4CommitFeedback.
        let gpu_started = unsafe { msg_send![feedback, GPUStartTime] };
        // SAFETY: Metal supplies an object conforming to MTL4CommitFeedback.
        let gpu_completed = unsafe { msg_send![feedback, GPUEndTime] };
        // SAFETY: Metal supplies an object conforming to MTL4CommitFeedback.
        let error: Option<Retained<AnyObject>> = unsafe { msg_send![feedback, error] };
        if let Ok(mut sample) = METAL4_FEEDBACK_SAMPLE.lock() {
            *sample = Some(Metal4CommitFeedbackSample {
                gpu_started,
                gpu_completed,
                runtime_error: error.is_some(),
            });
            METAL4_FEEDBACK_READY.notify_one();
        }
    }

    fn prepare_metal4_commit_feedback() -> Result<std::sync::MutexGuard<'static, ()>, Metal4RunError>
    {
        let serialization = METAL4_FEEDBACK_SERIALIZER.lock().map_err(|_| {
            Metal4RunError::new(
                Metal4Failure::CommandSetup,
                "Metal 4 feedback serialization lock is poisoned",
            )
        })?;
        let mut sample = METAL4_FEEDBACK_SAMPLE.lock().map_err(|_| {
            Metal4RunError::new(
                Metal4Failure::CommandSetup,
                "Metal 4 feedback sample lock is poisoned",
            )
        })?;
        *sample = None;
        drop(sample);
        Ok(serialization)
    }

    fn wait_for_metal4_commit_feedback() -> Result<Metal4CommitFeedbackSample, Metal4RunError> {
        let sample = METAL4_FEEDBACK_SAMPLE.lock().map_err(|_| {
            Metal4RunError::new(
                Metal4Failure::CommitFeedback,
                "Metal 4 feedback sample lock is poisoned",
            )
        })?;
        let (mut sample, timeout) = METAL4_FEEDBACK_READY
            .wait_timeout_while(
                sample,
                Duration::from_millis(METAL4_COMPLETION_TIMEOUT_MS),
                |sample| sample.is_none(),
            )
            .map_err(|_| {
                Metal4RunError::new(
                    Metal4Failure::CommitFeedback,
                    "Metal 4 feedback wait lock is poisoned",
                )
            })?;
        if timeout.timed_out() && sample.is_none() {
            return Err(Metal4RunError::new(
                Metal4Failure::CommitFeedback,
                "Metal 4 commit feedback timed out after 10 seconds",
            ));
        }
        sample.take().ok_or_else(|| {
            Metal4RunError::new(
                Metal4Failure::CommitFeedback,
                "Metal 4 commit feedback returned no sample",
            )
        })
    }

    struct Metal4CommandContext {
        allocator: Retained<ProtocolObject<dyn MTL4CommandAllocator>>,
        queue: Retained<ProtocolObject<dyn MTL4CommandQueue>>,
        command_buffer: Retained<ProtocolObject<dyn MTL4CommandBuffer>>,
        completion_event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
        pipeline: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
        argument_table: Retained<ProtocolObject<dyn MTL4ArgumentTable>>,
        residency_set: Retained<ProtocolObject<dyn MTLResidencySet>>,
        _parameters: Retained<ProtocolObject<dyn MTLBuffer>>,
        signal_value: u64,
        submitted: bool,
    }

    impl Metal4CommandContext {
        fn new(
            device: &ProtocolObject<dyn MTLDevice>,
            source: &ProtocolObject<dyn MTLTexture>,
            output: &ProtocolObject<dyn MTLTexture>,
            pipeline: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
            parameters: &Metal4ReductionParameters,
        ) -> Result<Self, Metal4RunError> {
            let allocator = device.newCommandAllocator().ok_or_else(|| {
                Metal4RunError::new(
                    Metal4Failure::CommandSetup,
                    "Metal 4 command allocator creation failed",
                )
            })?;
            let queue = device.newMTL4CommandQueue().ok_or_else(|| {
                Metal4RunError::new(
                    Metal4Failure::CommandSetup,
                    "Metal 4 command queue creation failed",
                )
            })?;
            let command_buffer = device.newCommandBuffer().ok_or_else(|| {
                Metal4RunError::new(
                    Metal4Failure::CommandSetup,
                    "Metal 4 command buffer creation failed",
                )
            })?;
            let completion_event = device.newSharedEvent().ok_or_else(|| {
                Metal4RunError::new(
                    Metal4Failure::CommandSetup,
                    "Metal 4 completion event creation failed",
                )
            })?;

            let argument_descriptor = MTL4ArgumentTableDescriptor::new();
            argument_descriptor.setMaxBufferBindCount(1);
            argument_descriptor.setMaxTextureBindCount(3);
            argument_descriptor.setInitializeBindings(true);
            let argument_table = device
                .newArgumentTableWithDescriptor_error(&argument_descriptor)
                .map_err(|error| {
                    Metal4RunError::new(
                        Metal4Failure::CommandSetup,
                        format!(
                            "Metal 4 argument table creation failed: {}",
                            error.localizedDescription()
                        ),
                    )
                })?;

            // SAFETY: Metal copies exactly one fully initialized parameter
            // structure into a shared buffer during this call.
            let parameter_buffer = unsafe {
                device.newBufferWithBytes_length_options(
                    NonNull::from(parameters).cast::<c_void>(),
                    size_of::<Metal4ReductionParameters>(),
                    MTLResourceOptions::StorageModeShared,
                )
            };
            let parameter_buffer = parameter_buffer.ok_or_else(|| {
                Metal4RunError::new(
                    Metal4Failure::CommandSetup,
                    "Metal 4 parameter buffer creation failed",
                )
            })?;
            // SAFETY: each binding index falls within the descriptor bounds,
            // and all resource IDs belong to the same Metal device.
            unsafe {
                argument_table.setAddress_atIndex(parameter_buffer.gpuAddress(), 0);
                argument_table.setTexture_atIndex(source.gpuResourceID(), 0);
                argument_table.setTexture_atIndex(source.gpuResourceID(), 1);
                argument_table.setTexture_atIndex(output.gpuResourceID(), 2);
            }

            let residency_descriptor = MTLResidencySetDescriptor::new();
            // SAFETY: three is the exact number of retained allocations.
            unsafe {
                residency_descriptor.setInitialCapacity(3);
            }
            let residency_set = device
                .newResidencySetWithDescriptor_error(&residency_descriptor)
                .map_err(|error| {
                    Metal4RunError::new(
                        Metal4Failure::CommandSetup,
                        format!(
                            "Metal 4 residency set creation failed: {}",
                            error.localizedDescription()
                        ),
                    )
                })?;
            residency_set.addAllocation(ProtocolObject::from_ref(source));
            residency_set.addAllocation(ProtocolObject::from_ref(output));
            residency_set.addAllocation(ProtocolObject::from_ref(&*parameter_buffer));
            residency_set.commit();

            Ok(Self {
                allocator,
                queue,
                command_buffer,
                completion_event,
                pipeline,
                argument_table,
                residency_set,
                _parameters: parameter_buffer,
                signal_value: 0,
                submitted: false,
            })
        }

        fn dispatch(&mut self, output: Extent) -> Result<u128, Metal4RunError> {
            if self.submitted {
                self.allocator.reset();
            }
            self.command_buffer
                .beginCommandBufferWithAllocator(&self.allocator);
            self.command_buffer.useResidencySet(&self.residency_set);
            let encoder = self.command_buffer.computeCommandEncoder().ok_or_else(|| {
                Metal4RunError::new(
                    Metal4Failure::CommandSetup,
                    "Metal 4 compute encoder creation failed",
                )
            })?;
            encoder.setComputePipelineState(&self.pipeline);
            encoder.setArgumentTable(Some(&self.argument_table));
            encoder.dispatchThreads_threadsPerThreadgroup(
                MTLSize {
                    width: output.width as usize,
                    height: output.height as usize,
                    depth: 1,
                },
                MTLSize {
                    width: 8,
                    height: 8,
                    depth: 1,
                },
            );
            encoder.endEncoding();
            self.command_buffer.endCommandBuffer();

            let feedback_serialization = prepare_metal4_commit_feedback()?;
            let commit_options = MTL4CommitOptions::new();
            let feedback_block = std::ptr::from_ref(&METAL4_FEEDBACK_BLOCK)
                .cast_mut()
                .cast::<c_void>();
            // SAFETY: the pointer names an immutable global Objective-C block
            // whose callback ABI accepts one MTL4CommitFeedback object.
            unsafe {
                let _: () = msg_send![&*commit_options, addFeedbackHandler: feedback_block];
            }
            let mut command_buffers = [NonNull::from(&*self.command_buffer)];
            let command_buffer_count = command_buffers.len();
            // SAFETY: the array contains exactly one retained Metal 4 command
            // buffer and remains alive for the synchronous commit call.
            unsafe {
                self.queue.commit_count_options(
                    NonNull::from(&mut command_buffers[0]),
                    command_buffer_count,
                    &commit_options,
                );
            }
            self.signal_value = self.signal_value.checked_add(1).ok_or_else(|| {
                Metal4RunError::new(
                    Metal4Failure::DispatchOrCompletion,
                    "Metal 4 completion event sequence exhausted",
                )
            })?;
            self.queue.signalEvent_value(
                ProtocolObject::from_ref(&*self.completion_event),
                self.signal_value,
            );
            if !self
                .completion_event
                .waitUntilSignaledValue_timeoutMS(self.signal_value, METAL4_COMPLETION_TIMEOUT_MS)
            {
                return Err(Metal4RunError::new(
                    Metal4Failure::DispatchOrCompletion,
                    "Metal 4 completion event timed out after 10 seconds",
                ));
            }
            self.submitted = true;
            let feedback = wait_for_metal4_commit_feedback()?;
            drop(feedback_serialization);
            if feedback.runtime_error {
                return Err(Metal4RunError::new(
                    Metal4Failure::DispatchOrCompletion,
                    "Metal 4 commit feedback reported a GPU runtime error",
                ));
            }
            gpu_interval_nanoseconds(feedback.gpu_started, feedback.gpu_completed)
                .map_err(|error| Metal4RunError::new(Metal4Failure::CommitFeedback, error))
        }
    }

    fn measure_metal4(
        device: &wgpu::Device,
        imported: &hypercolor_macos_gpu_interop::ImportedMacosScreenFrame,
        target: &hypercolor_macos_gpu_interop::MacosNativeReductionTarget,
        args: Args,
    ) -> Result<Percentiles, Metal4RunError> {
        // SAFETY: the HAL device is borrowed only for immediate Metal object
        // creation and remains bounded by the owning wgpu device.
        let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Metal>() }.ok_or_else(|| {
            Metal4RunError::new(
                Metal4Failure::MetalDeviceAccess,
                "Metal 4 prototype has no Metal HAL device",
            )
        })?;
        let raw_device = hal_device.raw_device();
        let library = raw_device
            .newLibraryWithSource_options_error(&NSString::from_str(NATIVE_REDUCTION_SHADER), None)
            .map_err(|error| {
                Metal4RunError::new(
                    Metal4Failure::ShaderCompilation,
                    format!(
                        "Metal 4 reduction shader compilation failed: {}",
                        error.localizedDescription()
                    ),
                )
            })?;
        let function = library
            .newFunctionWithName(&NSString::from_str("hypercolor_reduce"))
            .ok_or_else(|| {
                Metal4RunError::new(
                    Metal4Failure::ShaderEntryPoint,
                    "Metal 4 reduction shader has no hypercolor_reduce entry point",
                )
            })?;
        let pipeline = raw_device
            .newComputePipelineStateWithFunction_error(&function)
            .map_err(|error| {
                Metal4RunError::new(
                    Metal4Failure::PipelineCreation,
                    format!(
                        "Metal 4 reduction pipeline creation failed: {}",
                        error.localizedDescription()
                    ),
                )
            })?;
        // SAFETY: the target was allocated by this exact Metal-backed wgpu
        // device and is borrowed only while commands are encoded and completed.
        let target_texture = unsafe { target.texture().as_hal::<wgpu_hal::api::Metal>() }
            .ok_or_else(|| {
                Metal4RunError::new(
                    Metal4Failure::TargetTextureAccess,
                    "Metal 4 target has no Metal texture",
                )
            })?;
        let parameters = Metal4ReductionParameters::new(args);
        let source = imported.planes().first().ok_or_else(|| {
            Metal4RunError::new(
                Metal4Failure::SourcePlaneAccess,
                "Metal 4 fixture has no imported source plane",
            )
        })?;
        source
            .with_metal_texture(|source_texture| {
                let mut context = Metal4CommandContext::new(
                    raw_device,
                    source_texture,
                    target_texture.raw_handle(),
                    pipeline,
                    &parameters,
                )?;
                let mut samples = Vec::new();
                samples.try_reserve_exact(args.iterations).map_err(|_| {
                    Metal4RunError::new(
                        Metal4Failure::CommandSetup,
                        "Metal 4 timing sample allocation failed",
                    )
                })?;
                for _ in 0..args.warmup {
                    let _ = context.dispatch(args.output)?;
                }
                for _ in 0..args.iterations {
                    samples.push(context.dispatch(args.output)?);
                }
                percentiles(&samples)
                    .map_err(|error| Metal4RunError::new(Metal4Failure::Metal4Percentile, error))
            })
            .map_err(|error| {
                Metal4RunError::new(Metal4Failure::SourcePlaneAccess, error.to_string())
            })?
    }

    fn reduce_scalar(
        frame: &MacosCaptureFrame,
        output_extent: Extent,
        filter: Filter,
        output: &mut [u8],
    ) -> Result<(), String> {
        if output.len() != output_extent.byte_len()? {
            return Err("CPU output allocation does not match the requested extent".to_owned());
        }
        frame
            .with_cpu_source(|source| {
                let scale_x = source.extent().width as f32 / output_extent.width as f32;
                let scale_y = source.extent().height as f32 / output_extent.height as f32;
                for y in 0..output_extent.height {
                    for x in 0..output_extent.width {
                        let start = [x as f32 * scale_x, y as f32 * scale_y];
                        let end = [start[0] + scale_x, start[1] + scale_y];
                        let sample = match filter {
                            Filter::Nearest => sample_nearest(source, start, end)?,
                            Filter::Bilinear => sample_bilinear(source, start, end)?,
                            Filter::Area => sample_area(source, start, end)?,
                        };
                        let offset = ((y as usize * output_extent.width as usize) + x as usize) * 4;
                        output[offset..offset + 4].copy_from_slice(
                            &sample.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8),
                        );
                    }
                }
                Ok::<(), String>(())
            })
            .map_err(|error| error.to_string())?
    }

    fn sample_nearest(
        source: hypercolor_macos_capture::MacosCpuSourceView<'_>,
        start: [f32; 2],
        end: [f32; 2],
    ) -> Result<[f32; 4], String> {
        let center = [(start[0] + end[0]) * 0.5, (start[1] + end[1]) * 0.5];
        load_clamped(source, center[0].floor() as i32, center[1].floor() as i32)
    }

    fn sample_bilinear(
        source: hypercolor_macos_capture::MacosCpuSourceView<'_>,
        start: [f32; 2],
        end: [f32; 2],
    ) -> Result<[f32; 4], String> {
        let centered = [
            (start[0] + end[0]) * 0.5 - 0.5,
            (start[1] + end[1]) * 0.5 - 0.5,
        ];
        let lower = [centered[0].floor() as i32, centered[1].floor() as i32];
        let fraction = [
            centered[0] - centered[0].floor(),
            centered[1] - centered[1].floor(),
        ];
        let top = mix(
            load_clamped(source, lower[0], lower[1])?,
            load_clamped(source, lower[0] + 1, lower[1])?,
            fraction[0],
        );
        let bottom = mix(
            load_clamped(source, lower[0], lower[1] + 1)?,
            load_clamped(source, lower[0] + 1, lower[1] + 1)?,
            fraction[0],
        );
        Ok(mix(top, bottom, fraction[1]))
    }

    fn sample_area(
        source: hypercolor_macos_capture::MacosCpuSourceView<'_>,
        start: [f32; 2],
        end: [f32; 2],
    ) -> Result<[f32; 4], String> {
        let first = [start[0].floor() as i32, start[1].floor() as i32];
        let last = [end[0].ceil() as i32, end[1].ceil() as i32];
        let mut total = [0.0_f32; 4];
        let mut total_weight = 0.0_f32;
        for y in first[1]..last[1] {
            let height = (end[1].min((y + 1) as f32) - start[1].max(y as f32)).max(0.0);
            for x in first[0]..last[0] {
                let width = (end[0].min((x + 1) as f32) - start[0].max(x as f32)).max(0.0);
                let weight = width * height;
                let sample = load_clamped(source, x, y)?;
                for channel in 0..4 {
                    total[channel] += sample[channel] * weight;
                }
                total_weight += weight;
            }
        }
        Ok(total.map(|channel| channel / total_weight.max(f32::EPSILON)))
    }

    fn load_clamped(
        source: hypercolor_macos_capture::MacosCpuSourceView<'_>,
        x: i32,
        y: i32,
    ) -> Result<[f32; 4], String> {
        let maximum_x = source.extent().width.saturating_sub(1);
        let maximum_y = source.extent().height.saturating_sub(1);
        source
            .sample_rgba32f(
                x.clamp(0, maximum_x as i32) as u32,
                y.clamp(0, maximum_y as i32) as u32,
            )
            .map_err(|error| error.to_string())
    }

    fn mix(left: [f32; 4], right: [f32; 4], weight: f32) -> [f32; 4] {
        std::array::from_fn(|channel| left[channel] + (right[channel] - left[channel]) * weight)
    }

    fn read_texture_pixels(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        extent: Extent,
    ) -> Result<Vec<u8>, String> {
        let unpadded = extent
            .width
            .checked_mul(BYTES_PER_PIXEL as u32)
            .ok_or_else(|| "readback row byte count overflowed".to_owned())?;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer_size = u64::from(padded)
            .checked_mul(u64::from(extent.height))
            .ok_or_else(|| "readback allocation overflowed".to_owned())?;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bench_macos_reduction readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_macos_reduction readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(extent.height),
                },
            },
            wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
        );
        let submission = queue.submit(Some(encoder.finish()));
        let slice = buffer.slice(..buffer_size);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| format!("readback wait failed: {error:?}"))?;
        receiver
            .recv()
            .map_err(|error| format!("readback callback failed: {error}"))?
            .map_err(|error| format!("readback mapping failed: {error}"))?;
        let mapped = slice.get_mapped_range();
        let mut pixels = allocate_bytes(extent.byte_len()?, "readback output")?;
        for (source, target) in mapped
            .chunks_exact(padded as usize)
            .zip(pixels.chunks_exact_mut(unpadded as usize))
        {
            target.copy_from_slice(&source[..unpadded as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(pixels)
    }

    fn print_report(
        args: Args,
        frame: &MacosCaptureFrame,
        adapter: &wgpu::AdapterInfo,
        cpu: Percentiles,
        wgpu: &WgpuMeasurement,
        metal4: Metal4Report,
    ) {
        println!("benchmark=bench_macos_reduction");
        println!("fixture=synthetic_iosurface");
        println!("source_pixels={}", args.source);
        println!("output_pixels={}", args.output);
        println!("source_bytes={}", args.source.byte_len().unwrap_or(0));
        println!(
            "source_iosurface_allocation_bytes={}",
            frame.surface.allocation_bytes
        );
        println!("output_bytes={}", args.output.byte_len().unwrap_or(0));
        println!("source_pixel_format=bgra8_unorm");
        println!("output_pixel_format=rgba8_unorm");
        println!("dynamic_range=sdr");
        println!("filter={}", args.filter);
        println!("iterations={}", args.iterations);
        println!("warmup={}", args.warmup);
        println!("device_name={}", adapter.name);
        println!("backend={:?}", adapter.backend);
        println!("driver={}", adapter.driver);
        println!("driver_info={}", adapter.driver_info);
        println!("cpu_metric=scalar_reduction_wall_time");
        println!("cpu_p50_ns={}", cpu.p50_ns);
        println!("cpu_p95_ns={}", cpu.p95_ns);
        println!("wgpu_completion_metric=reduction_completion_wall_time_diagnostic_only");
        println!("wgpu_completion_p50_ns={}", wgpu.completion.p50_ns);
        println!("wgpu_completion_p95_ns={}", wgpu.completion.p95_ns);
        println!("wgpu_adoption_metric={}", super::GPU_REDUCTION_METRIC);
        println!("wgpu_gpu_interval=command_buffer_gpu_start_to_end");
        println!("wgpu_gpu_p50_ns={}", wgpu.gpu.p50_ns);
        println!("wgpu_gpu_p95_ns={}", wgpu.gpu.p95_ns);
        println!("output_parity=exact");
        if let Some(measurement) = metal4.measurement {
            println!("metal4_adoption_metric={}", super::GPU_REDUCTION_METRIC);
            println!("metal4_gpu_interval=commit_feedback_gpu_start_to_end");
            println!("metal4_gpu_p50_ns={}", measurement.timings.p50_ns);
            println!("metal4_gpu_p95_ns={}", measurement.timings.p95_ns);
            println!(
                "metal4_output_parity={}",
                if measurement.output_parity {
                    "exact"
                } else {
                    "mismatch"
                }
            );
            if let Some(improvement) =
                super::p95_improvement_basis_points(wgpu.gpu.p95_ns, measurement.timings.p95_ns)
            {
                println!("metal4_p95_improvement_basis_points={improvement}");
            }
        }
        print_metal4_artifact(metal4.probe, metal4.evaluation);
    }

    fn print_metal4_artifact(probe: MacosMetal4CapabilityProbe, evaluation: Metal4Evaluation) {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        if let Err(error) =
            write_metal4_artifact(&mut output, probe, evaluation).and_then(|()| output.flush())
        {
            eprintln!("bench_macos_reduction: could not emit decision artifact: {error}");
        }
    }

    fn flush_stdout() {
        if let Err(error) = io::stdout().flush() {
            eprintln!("bench_macos_reduction: could not flush decision artifact: {error}");
        }
    }

    struct WgpuFixture {
        _instance: wgpu::Instance,
        adapter_info: wgpu::AdapterInfo,
        device: wgpu::Device,
        queue: wgpu::Queue,
    }

    impl WgpuFixture {
        fn new() -> Result<Self, String> {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                }))
                .map_err(|error| format!("could not create wgpu adapter: {error}"))?;
            let adapter_info = adapter.get_info();
            if adapter_info.backend != wgpu::Backend::Metal {
                return Err(format!(
                    "requires Metal wgpu backend, got {:?}",
                    adapter_info.backend
                ));
            }
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("bench_macos_reduction"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                }))
                .map_err(|error| format!("could not create wgpu device: {error}"))?;
            Ok(Self {
                _instance: instance,
                adapter_info,
                device,
                queue,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Args, String> {
        Args::parse_from(arguments.iter().map(ToString::to_string))
    }

    fn qualified_probe() -> hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe {
        hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe {
            metal_registry_id: 9,
            metal4_family: true,
            command_allocator: true,
            command_queue: true,
            command_buffer: true,
            argument_table: true,
            residency_set: true,
            shared_event: true,
            commit_feedback: true,
        }
    }

    fn rendered_artifact(
        probe: hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe,
        evaluation: Metal4Evaluation,
    ) -> String {
        let mut output = Vec::new();
        write_metal4_artifact(&mut output, probe, evaluation)
            .expect("in-memory artifact output succeeds");
        String::from_utf8(output).expect("artifact output is UTF-8")
    }

    #[test]
    fn parser_accepts_every_bounded_option() {
        assert_eq!(
            parse(&[
                "bench",
                "--source",
                "3840x2160",
                "--output",
                "640x480",
                "--filter",
                "bilinear",
                "--iterations",
                "100",
                "--warmup",
                "10",
            ]),
            Ok(Args {
                source: Extent {
                    width: 3840,
                    height: 2160,
                },
                output: Extent {
                    width: 640,
                    height: 480,
                },
                filter: Filter::Bilinear,
                iterations: 100,
                warmup: 10,
            })
        );
    }

    #[test]
    fn parser_rejects_unbounded_or_incomplete_inputs() {
        assert!(parse(&["bench", "--source", "16384x16384"]).is_err());
        assert!(parse(&["bench", "--source", "8193x1"]).is_err());
        assert!(parse(&["bench", "--iterations", "10001"]).is_err());
        assert!(parse(&["bench", "--iterations", "99"]).is_err());
        assert!(parse(&["bench", "--warmup", "1001"]).is_err());
        assert!(parse(&["bench", "--warmup", "9"]).is_err());
        assert!(parse(&["bench", "--filter", "magic"]).is_err());
        assert!(parse(&["bench", "--output"]).is_err());
        assert!(parse(&["bench", "--mystery", "1"]).is_err());
        assert!(
            parse(&[
                "bench",
                "--source",
                "1x1",
                "--output",
                "1x1",
                "--filter",
                "area",
                "--iterations",
                "100",
                "--warmup",
                "10",
                "--source",
                "1x1",
            ])
            .is_err()
        );
    }

    #[test]
    fn percentile_ranks_are_nearest_rank_and_deterministic() {
        let samples = [100, 20, 80, 40, 60, 10, 30, 50, 70, 90];
        assert_eq!(
            percentiles(&samples),
            Ok(Percentiles {
                p50_ns: 50,
                p95_ns: 100,
            })
        );
        assert!(percentiles(&[]).is_err());
        assert_eq!(p95_improvement_basis_points(1_000, 900), Some(1_000));
        assert_eq!(p95_improvement_basis_points(1_000, 1_100), Some(-1_000));
        assert_eq!(p95_improvement_basis_points(0, 0), None);
    }

    #[test]
    fn gpu_interval_conversion_rejects_unavailable_or_invalid_feedback() {
        assert_eq!(gpu_interval_nanoseconds(1.0, 1.000_001), Ok(1_000));
        assert!(gpu_interval_nanoseconds(0.0, 1.0).is_err());
        assert!(gpu_interval_nanoseconds(2.0, 1.0).is_err());
        assert!(gpu_interval_nanoseconds(f64::NAN, 1.0).is_err());
        assert!(gpu_interval_nanoseconds(1.0, f64::INFINITY).is_err());
    }

    #[test]
    fn metal4_decision_requires_qualification_measurement_and_exact_parity() {
        let qualified = qualified_probe();
        assert_eq!(
            metal4_decision(qualified, Metal4Evaluation::NotRun),
            Metal4Decision::NotMeasured(None)
        );
        assert_eq!(
            metal4_decision(
                qualified,
                Metal4Evaluation::Measured(Metal4Evidence {
                    output_parity: false,
                    wgpu_gpu_p95_ns: 1_000,
                    metal4_gpu_p95_ns: 800,
                })
            ),
            Metal4Decision::Reject(Metal4Rejection::OutputMismatch)
        );
        assert_eq!(
            metal4_decision(
                hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe {
                    command_queue: false,
                    ..qualified
                },
                Metal4Evaluation::NotRun,
            ),
            Metal4Decision::NotQualified {
                missing_facilities: [
                    None,
                    None,
                    Some("command_queue"),
                    None,
                    None,
                    None,
                    None,
                    None
                ]
            }
        );
    }

    #[test]
    fn metal4_decision_enforces_the_ten_percent_p95_gate() {
        let qualified = qualified_probe();
        let evidence = |wgpu_gpu_p95_ns, metal4_gpu_p95_ns| {
            Metal4Evaluation::Measured(Metal4Evidence {
                output_parity: true,
                wgpu_gpu_p95_ns,
                metal4_gpu_p95_ns,
            })
        };
        assert_eq!(
            metal4_decision(qualified, evidence(1_000, 900)),
            Metal4Decision::Adopt
        );
        assert_eq!(
            metal4_decision(qualified, evidence(1_000, 901)),
            Metal4Decision::Reject(Metal4Rejection::InsufficientP95Improvement)
        );
        assert_eq!(
            metal4_decision(qualified, evidence(0, 0)),
            Metal4Decision::Reject(Metal4Rejection::BaselineMetricUnavailable)
        );
        let maximum = u128::MAX;
        let adoption_ceiling = maximum - maximum.div_ceil(10);
        assert_eq!(
            metal4_decision(qualified, evidence(maximum, adoption_ceiling)),
            Metal4Decision::Adopt
        );
        assert_eq!(
            metal4_decision(qualified, evidence(maximum, adoption_ceiling + 1)),
            Metal4Decision::Reject(Metal4Rejection::InsufficientP95Improvement)
        );
    }

    #[test]
    fn metal4_artifact_distinguishes_unmeasured_hardware_from_measured_rejection() {
        let qualified = qualified_probe();
        assert_eq!(
            metal4_artifact(qualified, Metal4Evaluation::NotRun),
            Metal4Artifact {
                hardware_attempted: false,
                hardware_run: false,
                status: "not_measured",
                decision: "not_evaluated",
                reason: "qualified_hardware_run_missing",
                failure: None,
                missing_facilities: [None; 8],
            }
        );
        assert_eq!(
            metal4_artifact(
                qualified,
                Metal4Evaluation::Measured(Metal4Evidence {
                    output_parity: false,
                    wgpu_gpu_p95_ns: 1_000,
                    metal4_gpu_p95_ns: 800,
                }),
            ),
            Metal4Artifact {
                hardware_attempted: true,
                hardware_run: true,
                status: "measured",
                decision: "reject",
                reason: "output_parity_mismatch",
                failure: None,
                missing_facilities: [None; 8],
            }
        );
        assert_eq!(
            metal4_artifact(
                qualified,
                Metal4Evaluation::Failed(Metal4Failure::ShaderCompilation),
            ),
            Metal4Artifact {
                hardware_attempted: true,
                hardware_run: false,
                status: "not_measured",
                decision: "not_evaluated",
                reason: "shader_compilation_failed",
                failure: Some("shader_compilation_failed"),
                missing_facilities: [None; 8],
            }
        );
        assert_eq!(
            metal4_artifact(
                qualified,
                Metal4Evaluation::Failed(Metal4Failure::OutputReadback),
            )
            .hardware_run,
            true
        );
        assert!(
            !metal4_artifact(
                qualified,
                Metal4Evaluation::Failed(Metal4Failure::CommandSetup),
            )
            .hardware_run
        );
    }

    #[test]
    fn every_qualified_failure_phase_emits_a_typed_bounded_artifact() {
        let qualified = qualified_probe();
        let before_metal4_dispatch = [
            Metal4Failure::QualifiedSetup,
            Metal4Failure::Warmup,
            Metal4Failure::BaselineMeasurement,
            Metal4Failure::WgpuGpuIntervalUnavailable,
            Metal4Failure::BaselineReadback,
            Metal4Failure::BaselineParity,
            Metal4Failure::BaselinePercentile,
            Metal4Failure::TargetCreation,
            Metal4Failure::ShaderCompilation,
            Metal4Failure::ShaderEntryPoint,
            Metal4Failure::PipelineCreation,
            Metal4Failure::MetalDeviceAccess,
            Metal4Failure::TargetTextureAccess,
            Metal4Failure::SourcePlaneAccess,
            Metal4Failure::CommandSetup,
        ];
        let after_metal4_dispatch = [
            Metal4Failure::DispatchOrCompletion,
            Metal4Failure::CommitFeedback,
            Metal4Failure::OutputReadback,
            Metal4Failure::Metal4Percentile,
        ];

        for failure in before_metal4_dispatch {
            let artifact = metal4_artifact(qualified, Metal4Evaluation::Failed(failure));
            assert!(artifact.hardware_attempted, "{}", failure.code());
            assert!(!artifact.hardware_run, "{}", failure.code());
            assert_eq!(artifact.status, "not_measured");
            assert_eq!(artifact.decision, "not_evaluated");
            assert_eq!(artifact.reason, failure.code());
            assert_eq!(artifact.failure, Some(failure.code()));
            assert_eq!(artifact.missing_facilities, [None; 8]);
            let output = rendered_artifact(qualified, Metal4Evaluation::Failed(failure));
            assert!(output.lines().count() <= 16, "{}", failure.code());
            assert!(output.contains("metal4_hardware_attempted=true\n"));
            assert!(output.contains("metal4_hardware_run=false\n"));
            assert!(output.contains("metal4_status=not_measured\n"));
            assert!(output.contains("metal4_decision=not_evaluated\n"));
            assert!(output.contains(&format!("metal4_failure={}\n", failure.code())));
        }
        for failure in after_metal4_dispatch {
            let artifact = metal4_artifact(qualified, Metal4Evaluation::Failed(failure));
            assert!(artifact.hardware_attempted, "{}", failure.code());
            assert!(artifact.hardware_run, "{}", failure.code());
            assert_eq!(artifact.status, "not_measured");
            assert_eq!(artifact.decision, "not_evaluated");
            assert_eq!(artifact.reason, failure.code());
            assert_eq!(artifact.failure, Some(failure.code()));
            assert_eq!(artifact.missing_facilities, [None; 8]);
            let output = rendered_artifact(qualified, Metal4Evaluation::Failed(failure));
            assert!(output.lines().count() <= 16, "{}", failure.code());
            assert!(output.contains("metal4_hardware_attempted=true\n"));
            assert!(output.contains("metal4_hardware_run=true\n"));
            assert!(output.contains("metal4_status=not_measured\n"));
            assert!(output.contains("metal4_decision=not_evaluated\n"));
            assert!(output.contains(&format!("metal4_failure={}\n", failure.code())));
        }
    }
}
