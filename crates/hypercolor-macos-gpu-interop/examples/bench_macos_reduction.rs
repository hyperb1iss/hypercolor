use std::fmt;

const DEFAULT_SOURCE: Extent = Extent {
    width: 1920,
    height: 1080,
};
const DEFAULT_OUTPUT: Extent = Extent {
    width: 320,
    height: 180,
};
const DEFAULT_ITERATIONS: usize = 20;
const DEFAULT_WARMUP: usize = 3;
const MAX_DIMENSION: u32 = 8_192;
const MAX_PIXELS: u64 = 67_108_864;
const MAX_ITERATIONS: usize = 10_000;
const MAX_WARMUP: usize = 1_000;
const MAX_OPTION_PAIRS: usize = 5;
const BYTES_PER_PIXEL: u64 = 4;

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
                        parse_bounded_usize(&value, "iterations", 1, MAX_ITERATIONS)?;
                }
                "--warmup" => {
                    parsed.warmup = parse_bounded_usize(&value, "warmup", 0, MAX_WARMUP)?;
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
        missing_facilities: [Option<&'static str>; 5],
    },
    NotImplemented,
}

fn metal4_decision(
    probe: hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe,
) -> Metal4Decision {
    if probe.all_required_facilities() {
        Metal4Decision::NotImplemented
    } else {
        Metal4Decision::NotQualified {
            missing_facilities: probe.missing_facilities(),
        }
    }
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
    use std::sync::{Arc, mpsc};
    use std::time::Instant;

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

    use super::{Args, BYTES_PER_PIXEL, Extent, Filter, Percentiles, percentiles};

    pub fn run() -> Result<(), String> {
        let args = Args::parse_from(std::env::args())?;
        let source_pixels = synthetic_bgra(args.source)?;
        let frame = Arc::new(capture_frame(args.source, &source_pixels)?);
        let wgpu = WgpuFixture::new()?;
        let bridge = MacosScreenBridge::new(&wgpu.device).map_err(|error| error.to_string())?;
        let imported = bridge
            .import_frame(&wgpu.device, 1, Arc::clone(&frame))
            .map_err(|error| error.to_string())?;
        let reducer = MacosNativeReducer::new(&wgpu.device).map_err(|error| error.to_string())?;
        let target = reducer
            .create_target(
                &wgpu.device,
                args.output.width,
                args.output.height,
                MacosNativeTargetFormat::Rgba8,
            )
            .map_err(|error| error.to_string())?;
        let descriptor = reduction_descriptor(args)?;
        let mut cpu_output = allocate_bytes(args.output.byte_len()?, "CPU output")?;

        for _ in 0..args.warmup {
            reduce_scalar(&frame, args.output, args.filter, &mut cpu_output)?;
            reduce_wgpu(
                &wgpu.device,
                &wgpu.queue,
                &reducer,
                &imported,
                &target,
                descriptor,
            )?;
        }

        let cpu_times = measure(args.iterations, || {
            reduce_scalar(&frame, args.output, args.filter, &mut cpu_output)
        })?;
        let gpu_times = measure(args.iterations, || {
            reduce_wgpu(
                &wgpu.device,
                &wgpu.queue,
                &reducer,
                &imported,
                &target,
                descriptor,
            )
        })?;
        let gpu_output =
            read_texture_pixels(&wgpu.device, &wgpu.queue, target.texture(), args.output)?;
        if cpu_output != gpu_output {
            let mismatch = cpu_output
                .iter()
                .zip(&gpu_output)
                .position(|(cpu, gpu)| cpu != gpu)
                .unwrap_or(cpu_output.len());
            return Err(format!(
                "exact output parity failed at byte {mismatch}: CPU={:?}, wgpu={:?}",
                cpu_output.get(mismatch),
                gpu_output.get(mismatch)
            ));
        }

        let cpu = percentiles(&cpu_times)?;
        let gpu = percentiles(&gpu_times)?;
        let metal4 =
            probe_macos_metal4_capabilities(&wgpu.device).map_err(|error| error.to_string())?;
        print_report(args, &frame, &wgpu.adapter_info, cpu, gpu, metal4);
        Ok(())
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
        gpu: Percentiles,
        metal4: MacosMetal4CapabilityProbe,
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
        println!("wgpu_metric=wgpu_encode_submit_to_completion_wall_time");
        println!("wgpu_p50_ns={}", gpu.p50_ns);
        println!("wgpu_p95_ns={}", gpu.p95_ns);
        println!("output_parity=exact");
        println!("metal4_registry_id={}", metal4.metal_registry_id);
        println!("metal4_family={}", metal4.metal4_family);
        println!("metal4_command_allocator={}", metal4.command_allocator);
        println!("metal4_command_queue={}", metal4.command_queue);
        println!("metal4_command_buffer={}", metal4.command_buffer);
        println!("metal4_residency_set={}", metal4.residency_set);
        match super::metal4_decision(metal4) {
            super::Metal4Decision::NotQualified { missing_facilities } => {
                let missing = missing_facilities
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(",");
                println!("metal4_status=not_qualified");
                println!("metal4_missing_facilities={missing}");
            }
            super::Metal4Decision::NotImplemented => {
                println!("metal4_status=not_implemented");
                println!(
                    "metal4_reason=direct_command_allocator_and_residency_set_prototype_not_implemented"
                );
            }
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
                "41",
                "--warmup",
                "7",
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
                iterations: 41,
                warmup: 7,
            })
        );
    }

    #[test]
    fn parser_rejects_unbounded_or_incomplete_inputs() {
        assert!(parse(&["bench", "--source", "16384x16384"]).is_err());
        assert!(parse(&["bench", "--source", "8193x1"]).is_err());
        assert!(parse(&["bench", "--iterations", "10001"]).is_err());
        assert!(parse(&["bench", "--warmup", "1001"]).is_err());
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
                "1",
                "--warmup",
                "0",
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
    }

    #[test]
    fn metal4_decision_never_calls_ordinary_metal_a_comparison() {
        let qualified = hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe {
            metal_registry_id: 9,
            metal4_family: true,
            command_allocator: true,
            command_queue: true,
            command_buffer: true,
            residency_set: true,
        };
        assert_eq!(metal4_decision(qualified), Metal4Decision::NotImplemented);
        assert_eq!(
            metal4_decision(hypercolor_macos_gpu_interop::MacosMetal4CapabilityProbe {
                command_queue: false,
                ..qualified
            }),
            Metal4Decision::NotQualified {
                missing_facilities: [None, None, Some("command_queue"), None, None]
            }
        );
    }
}
