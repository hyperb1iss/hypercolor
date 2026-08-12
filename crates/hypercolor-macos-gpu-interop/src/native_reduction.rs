use std::ptr::NonNull;

use hypercolor_macos_capture::{
    MacosCapturePixelFormat, MacosChromaLocation, MacosColorRange, MacosTransferFunction,
    MacosYuvMatrix,
};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLComputeCommandEncoder, MTLComputePipelineState,
    MTLDevice, MTLLibrary, MTLPixelFormat, MTLSize, MTLStorageMode, MTLTexture,
    MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};
use thiserror::Error;

use crate::{
    ImportedMacosScreenFrame, MacosGpuInteropError, MacosScreenBridgeError,
    metal_device_import_contract,
};

const NATIVE_REDUCTION_SHADER: &str = include_str!("native_reduction.metal");

/// Spatial filter executed by the native Metal reducer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MacosNativeReductionFilter {
    /// Select the nearest complete source sample.
    Nearest,
    /// Interpolate four complete source samples.
    Bilinear,
    /// Integrate every covered complete source sample by area.
    #[default]
    Area,
}

/// Output transfer function applied after linear-light spatial reduction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacosNativeOutputTransfer {
    /// IEC 61966-2-1 sRGB encoding.
    Srgb,
    /// Linear normalized encoding.
    Linear,
    /// ITU-R BT.709 encoding.
    Rec709,
    /// ITU-R BT.2020 encoding.
    Rec2020,
}

/// Bars surrounding a materialized native reduction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MacosNativeLetterboxFill {
    /// Fully transparent black.
    Transparent,
    /// RGBA color expressed as normalized channels.
    Solid([f32; 4]),
}

/// Eight-bit target storage format requested by the resolved descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacosNativeTargetFormat {
    /// Red, green, blue, alpha byte storage.
    Rgba8,
    /// Blue, green, red, alpha byte storage.
    Bgra8,
}

/// Canonical color-transform constants prepared by the shared color pipeline.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MacosNativeColorTransform {
    source_to_target: [[f32; 4]; 3],
    source_luminance_and_exposure: [f32; 4],
    curve: [f32; 4],
}

impl MacosNativeColorTransform {
    /// Copy the canonical 80-byte transform shared with the CPU reducer.
    #[must_use]
    pub const fn new(
        source_to_target: [[f32; 4]; 3],
        source_luminance_and_exposure: [f32; 4],
        curve: [f32; 4],
    ) -> Self {
        Self {
            source_to_target,
            source_luminance_and_exposure,
            curve,
        }
    }
}

/// Complete geometry and color operation for one native reduction dispatch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MacosNativeReductionDescriptor {
    output_extent: [u32; 2],
    content_rect: [u32; 4],
    source_rect: [f32; 4],
    filter: MacosNativeReductionFilter,
    color: Option<(MacosNativeOutputTransfer, MacosNativeColorTransform)>,
}

impl MacosNativeReductionDescriptor {
    /// Construct and validate one reduction into a logical RGBA8 output.
    ///
    /// The source rectangle is expressed as pixel-edge coordinates in the
    /// retained luma or packed-RGB storage plane.
    pub fn new(
        output_extent: [u32; 2],
        content_rect: [u32; 4],
        source_rect: [f32; 4],
        filter: MacosNativeReductionFilter,
        color: Option<(MacosNativeOutputTransfer, MacosNativeColorTransform)>,
    ) -> Result<Self, MacosNativeReductionError> {
        if output_extent.contains(&0) || content_rect[2] == 0 || content_rect[3] == 0 {
            return Err(MacosNativeReductionError::InvalidGeometry);
        }
        let content_right = content_rect[0]
            .checked_add(content_rect[2])
            .ok_or(MacosNativeReductionError::InvalidGeometry)?;
        let content_bottom = content_rect[1]
            .checked_add(content_rect[3])
            .ok_or(MacosNativeReductionError::InvalidGeometry)?;
        if content_right > output_extent[0]
            || content_bottom > output_extent[1]
            || !source_rect.into_iter().all(f32::is_finite)
            || source_rect[0] < 0.0
            || source_rect[1] < 0.0
            || source_rect[2] <= 0.0
            || source_rect[3] <= 0.0
        {
            return Err(MacosNativeReductionError::InvalidGeometry);
        }
        Ok(Self {
            output_extent,
            content_rect,
            source_rect,
            filter,
            color,
        })
    }

    /// Logical output extent produced by this dispatch.
    #[must_use]
    pub const fn output_extent(self) -> [u32; 2] {
        self.output_extent
    }
}

/// Owner-backed RGBA8 texture written by native reduction and sampled by wgpu.
#[derive(Debug, Clone)]
pub struct MacosNativeReductionTarget {
    width: u32,
    height: u32,
    format: MacosNativeTargetFormat,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl MacosNativeReductionTarget {
    /// Output width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Output height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Exact target byte storage format.
    #[must_use]
    pub const fn format(&self) -> MacosNativeTargetFormat {
        self.format
    }

    /// RGBA8 texture on the registered Metal device.
    #[must_use]
    pub const fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Default RGBA8 texture view.
    #[must_use]
    pub const fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}

struct RetainedComputePipeline(Retained<ProtocolObject<dyn MTLComputePipelineState>>);

// SAFETY: Metal compute pipeline states are immutable and documented for
// concurrent command encoding after construction.
unsafe impl Send for RetainedComputePipeline {}

// SAFETY: shared access invokes only immutable pipeline-state methods.
unsafe impl Sync for RetainedComputePipeline {}

/// Metal compute pipeline converting retained capture planes into RGBA8.
pub struct MacosNativeReducer {
    metal_registry_id: u64,
    reduction_pipeline: RetainedComputePipeline,
    materialization_pipeline: RetainedComputePipeline,
}

/// Errors raised while preparing or executing native capture reduction.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MacosNativeReductionError {
    /// The requested source or output geometry is invalid.
    #[error("invalid macOS native reduction geometry")]
    InvalidGeometry,
    /// The imported source planes contradict the capture format.
    #[error("invalid macOS native reduction planes: {0}")]
    InvalidPlanes(&'static str),
    /// The target belongs to a different physical Metal device.
    #[error("macOS native reduction target belongs to a different Metal device")]
    DeviceMismatch,
    /// The MSL library could not compile.
    #[error("failed to compile macOS native reduction shader: {0}")]
    ShaderCompilation(String),
    /// One required MSL entry point was missing.
    #[error("macOS native reduction shader has no {0} entry point")]
    MissingEntryPoint(&'static str),
    /// The Metal compute pipeline could not be created.
    #[error("failed to create macOS native reduction pipeline: {0}")]
    PipelineCreation(String),
    /// The Metal device could not allocate the target texture.
    #[error("failed to allocate macOS native reduction target")]
    TargetAllocation,
    /// The wgpu command encoder did not expose its Metal command buffer.
    #[error("wgpu command encoder did not expose a Metal command buffer")]
    MissingCommandBuffer,
    /// IOSurface or wgpu Metal interop failed.
    #[error(transparent)]
    Interop(#[from] MacosGpuInteropError),
    /// Capture-plane import failed.
    #[error(transparent)]
    ScreenBridge(#[from] MacosScreenBridgeError),
}

impl MacosNativeReducer {
    /// Compile the reducer on the exact Metal device backing `device`.
    pub fn new(device: &wgpu::Device) -> Result<Self, MacosNativeReductionError> {
        let (metal_registry_id, _) = metal_device_import_contract(device)?;
        let hal_device = require_metal_device(device)?;
        let raw_device = hal_device.raw_device();
        let library = raw_device
            .newLibraryWithSource_options_error(&NSString::from_str(NATIVE_REDUCTION_SHADER), None)
            .map_err(|error| {
                MacosNativeReductionError::ShaderCompilation(
                    error.localizedDescription().to_string(),
                )
            })?;
        let reduction_pipeline = create_pipeline(raw_device, &library, "hypercolor_reduce")?;
        let materialization_pipeline =
            create_pipeline(raw_device, &library, "hypercolor_materialize")?;
        Ok(Self {
            metal_registry_id,
            reduction_pipeline,
            materialization_pipeline,
        })
    }

    /// Allocate one owner-backed RGBA8 target on the registered Metal device.
    pub fn create_target(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: MacosNativeTargetFormat,
    ) -> Result<MacosNativeReductionTarget, MacosNativeReductionError> {
        if width == 0 || height == 0 {
            return Err(MacosNativeReductionError::InvalidGeometry);
        }
        let (registry_id, _) = metal_device_import_contract(device)?;
        if registry_id != self.metal_registry_id {
            return Err(MacosNativeReductionError::DeviceMismatch);
        }
        let hal_device = require_metal_device(device)?;
        let descriptor = MTLTextureDescriptor::new();
        descriptor.setTextureType(MTLTextureType::Type2D);
        let (metal_format, wgpu_format) = match format {
            MacosNativeTargetFormat::Rgba8 => {
                (MTLPixelFormat::RGBA8Unorm, wgpu::TextureFormat::Rgba8Unorm)
            }
            MacosNativeTargetFormat::Bgra8 => {
                (MTLPixelFormat::BGRA8Unorm, wgpu::TextureFormat::Bgra8Unorm)
            }
        };
        descriptor.setPixelFormat(metal_format);
        // SAFETY: dimensions are validated as non-zero, the fixed counts are
        // valid for a non-arrayed 2D texture, and no multiplication occurs.
        unsafe {
            descriptor.setWidth(width as usize);
            descriptor.setHeight(height as usize);
            descriptor.setMipmapLevelCount(1);
            descriptor.setArrayLength(1);
            descriptor.setSampleCount(1);
        }
        descriptor.setStorageMode(MTLStorageMode::Private);
        descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite);
        let metal_texture = hal_device
            .raw_device()
            .newTextureWithDescriptor(&descriptor)
            .ok_or(MacosNativeReductionError::TargetAllocation)?;
        let wgpu_descriptor = wgpu::TextureDescriptor {
            label: Some("macOS native screen reduction target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        };
        let copy_size = wgpu_hal::CopyExtent {
            width,
            height,
            depth: 1,
        };
        // SAFETY: the texture was allocated by this wgpu device's raw Metal
        // device and exactly matches the descriptor and copy extent.
        let hal_texture = unsafe {
            wgpu_hal::metal::Device::texture_from_raw(
                metal_texture,
                wgpu_format,
                MTLTextureType::Type2D,
                1,
                1,
                copy_size,
            )
        };
        // SAFETY: the HAL texture belongs to this device and exactly matches
        // the supplied wgpu descriptor.
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu_hal::api::Metal>(hal_texture, &wgpu_descriptor)
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(MacosNativeReductionTarget {
            width,
            height,
            format,
            texture,
            view,
        })
    }

    /// Encode one zero-copy source conversion and spatial reduction.
    pub fn encode(
        &self,
        imported: &ImportedMacosScreenFrame,
        target: &MacosNativeReductionTarget,
        descriptor: MacosNativeReductionDescriptor,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), MacosNativeReductionError> {
        if descriptor.output_extent != [target.width, target.height] {
            return Err(MacosNativeReductionError::InvalidGeometry);
        }
        let source = imported.capture();
        let storage = source.storage_extent;
        if descriptor.source_rect[0] + descriptor.source_rect[2] > storage.width as f32
            || descriptor.source_rect[1] + descriptor.source_rect[3] > storage.height as f32
        {
            return Err(MacosNativeReductionError::InvalidGeometry);
        }
        validate_plane_count(source.pixel_format, imported.planes().len())?;
        let parameters = ReductionParameters::new(imported, descriptor)?;
        let output = raw_metal_texture(&target.texture)?;
        let first = &imported.planes()[0];
        first.with_metal_texture(|plane0| {
            if let Some(second) = imported.planes().get(1) {
                second.with_metal_texture(|plane1| {
                    self.encode_raw(encoder, plane0, plane1, &output, &parameters)
                })?
            } else {
                self.encode_raw(encoder, plane0, plane0, &output, &parameters)
            }
        })??;
        Ok(())
    }

    /// Encode bars and one physical target into an independently resolved output.
    pub fn encode_materialization(
        &self,
        source: &MacosNativeReductionTarget,
        target: &MacosNativeReductionTarget,
        content_rect: [u32; 4],
        fill: MacosNativeLetterboxFill,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), MacosNativeReductionError> {
        if content_rect[2] != source.width || content_rect[3] != source.height {
            return Err(MacosNativeReductionError::InvalidGeometry);
        }
        if source.format != target.format {
            return Err(MacosNativeReductionError::InvalidPlanes(
                "physical and logical target formats differ",
            ));
        }
        let right = content_rect[0]
            .checked_add(content_rect[2])
            .ok_or(MacosNativeReductionError::InvalidGeometry)?;
        let bottom = content_rect[1]
            .checked_add(content_rect[3])
            .ok_or(MacosNativeReductionError::InvalidGeometry)?;
        if right > target.width || bottom > target.height {
            return Err(MacosNativeReductionError::InvalidGeometry);
        }
        let parameters = MaterializationParameters {
            content_rect,
            output_extent: [target.width, target.height, 0, 0],
            fill: match fill {
                MacosNativeLetterboxFill::Transparent => [0.0; 4],
                MacosNativeLetterboxFill::Solid(color) => color,
            },
        };
        let source = raw_metal_texture(&source.texture)?;
        let target = raw_metal_texture(&target.texture)?;
        // SAFETY: the callback borrows the raw encoder only for this encoding
        // operation. The Metal command buffer remains owned and ended by wgpu.
        unsafe {
            encoder.as_hal_mut::<wgpu_hal::api::Metal, _, _>(|hal_encoder| {
                let hal_encoder =
                    hal_encoder.ok_or(MacosNativeReductionError::MissingCommandBuffer)?;
                let command_buffer = hal_encoder
                    .raw_command_buffer()
                    .ok_or(MacosNativeReductionError::MissingCommandBuffer)?;
                let compute = command_buffer
                    .computeCommandEncoder()
                    .ok_or(MacosNativeReductionError::MissingCommandBuffer)?;
                compute.setComputePipelineState(&self.materialization_pipeline.0);
                compute.setTexture_atIndex(Some(source.raw_handle()), 0);
                compute.setTexture_atIndex(Some(target.raw_handle()), 1);
                compute.setBytes_length_atIndex(
                    NonNull::from(&parameters).cast(),
                    size_of::<MaterializationParameters>(),
                    0,
                );
                compute.dispatchThreads_threadsPerThreadgroup(
                    MTLSize {
                        width: parameters.output_extent[0] as usize,
                        height: parameters.output_extent[1] as usize,
                        depth: 1,
                    },
                    MTLSize {
                        width: 8,
                        height: 8,
                        depth: 1,
                    },
                );
                compute.endEncoding();
                Ok(())
            })
        }
    }

    fn encode_raw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        plane0: &ProtocolObject<dyn MTLTexture>,
        plane1: &ProtocolObject<dyn MTLTexture>,
        output: &impl std::ops::Deref<Target = wgpu_hal::metal::Texture>,
        parameters: &ReductionParameters,
    ) -> Result<(), MacosNativeReductionError> {
        // SAFETY: the callback borrows the raw encoder only for this encoding
        // operation. The Metal command buffer remains owned and ended by wgpu.
        unsafe {
            encoder.as_hal_mut::<wgpu_hal::api::Metal, _, _>(|hal_encoder| {
                let hal_encoder =
                    hal_encoder.ok_or(MacosNativeReductionError::MissingCommandBuffer)?;
                let command_buffer = hal_encoder
                    .raw_command_buffer()
                    .ok_or(MacosNativeReductionError::MissingCommandBuffer)?;
                let compute = command_buffer
                    .computeCommandEncoder()
                    .ok_or(MacosNativeReductionError::MissingCommandBuffer)?;
                compute.setComputePipelineState(&self.reduction_pipeline.0);
                compute.setTexture_atIndex(Some(plane0), 0);
                compute.setTexture_atIndex(Some(plane1), 1);
                compute.setTexture_atIndex(Some(output.raw_handle()), 2);
                compute.setBytes_length_atIndex(
                    NonNull::from(parameters).cast(),
                    size_of::<ReductionParameters>(),
                    0,
                );
                compute.dispatchThreads_threadsPerThreadgroup(
                    MTLSize {
                        width: parameters.output_and_format[0] as usize,
                        height: parameters.output_and_format[1] as usize,
                        depth: 1,
                    },
                    MTLSize {
                        width: 8,
                        height: 8,
                        depth: 1,
                    },
                );
                compute.endEncoding();
                Ok(())
            })
        }
    }
}

#[repr(C, align(16))]
struct MaterializationParameters {
    content_rect: [u32; 4],
    output_extent: [u32; 4],
    fill: [f32; 4],
}

#[repr(C, align(16))]
struct ReductionParameters {
    content_rect: [u32; 4],
    output_and_format: [u32; 4],
    source_rect: [f32; 4],
    source_and_chroma_extent: [u32; 4],
    color: [u32; 4],
    operation: [u32; 4],
    transform: MacosNativeColorTransform,
}

impl ReductionParameters {
    fn new(
        imported: &ImportedMacosScreenFrame,
        descriptor: MacosNativeReductionDescriptor,
    ) -> Result<Self, MacosNativeReductionError> {
        let capture = imported.capture();
        capture
            .color
            .validate_for(capture.pixel_format)
            .map_err(|_| {
                MacosNativeReductionError::InvalidPlanes(
                    "capture color metadata does not match the source format",
                )
            })?;
        let format = source_format_code(capture.pixel_format);
        let range = u32::from(capture.color.range == MacosColorRange::Video);
        let matrix = match capture.color.matrix {
            None => 0,
            Some(MacosYuvMatrix::Bt601) => 1,
            Some(MacosYuvMatrix::Bt709) => 2,
            Some(MacosYuvMatrix::Bt2020) => 3,
        };
        let chroma_location = match capture.color.chroma_location {
            None => 0,
            Some(MacosChromaLocation::Center) => 1,
            Some(MacosChromaLocation::Left) => 2,
            Some(MacosChromaLocation::TopLeft) => 3,
        };
        let source_transfer = source_transfer_code(capture.color.transfer);
        let (managed, output_transfer, transform) = descriptor.color.map_or_else(
            || (0, 0, identity_color_transform()),
            |(output, transform)| {
                let output_transfer = match output {
                    MacosNativeOutputTransfer::Srgb => 0,
                    MacosNativeOutputTransfer::Linear => 1,
                    MacosNativeOutputTransfer::Rec709 => 2,
                    MacosNativeOutputTransfer::Rec2020 => 3,
                };
                (1, output_transfer, transform)
            },
        );
        let chroma = imported
            .planes()
            .get(1)
            .map_or(capture.storage_extent, |plane| {
                plane.storage_identity().extent
            });
        Ok(Self {
            content_rect: descriptor.content_rect,
            output_and_format: [
                descriptor.output_extent[0],
                descriptor.output_extent[1],
                format,
                descriptor.filter as u32,
            ],
            source_rect: descriptor.source_rect,
            source_and_chroma_extent: [
                capture.storage_extent.width,
                capture.storage_extent.height,
                chroma.width,
                chroma.height,
            ],
            color: [range, matrix, chroma_location, source_transfer],
            operation: [managed, output_transfer, 0, 0],
            transform,
        })
    }
}

fn identity_color_transform() -> MacosNativeColorTransform {
    MacosNativeColorTransform::new(
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
        [0.212_639, 0.715_168_65, 0.072_192_32, 1.0],
        [1.0, 1.0, 1.0, 1.0],
    )
}

const fn source_format_code(format: MacosCapturePixelFormat) -> u32 {
    match format {
        MacosCapturePixelFormat::Bgra8 => 0,
        MacosCapturePixelFormat::Argb2101010 => 1,
        MacosCapturePixelFormat::Rgba16Float => 2,
        MacosCapturePixelFormat::Yuv420VideoRange | MacosCapturePixelFormat::Yuv420FullRange => 3,
        MacosCapturePixelFormat::Yuv44410BiPlanar => 4,
    }
}

const fn source_transfer_code(transfer: MacosTransferFunction) -> u32 {
    match transfer {
        MacosTransferFunction::Srgb => 0,
        MacosTransferFunction::Rec709 => 1,
        MacosTransferFunction::Rec2020 => 2,
        MacosTransferFunction::Linear => 3,
        MacosTransferFunction::Pq => 4,
        MacosTransferFunction::Hlg => 5,
    }
}

fn validate_plane_count(
    format: MacosCapturePixelFormat,
    plane_count: usize,
) -> Result<(), MacosNativeReductionError> {
    let expected = if matches!(
        format,
        MacosCapturePixelFormat::Yuv420VideoRange
            | MacosCapturePixelFormat::Yuv420FullRange
            | MacosCapturePixelFormat::Yuv44410BiPlanar
    ) {
        2
    } else {
        1
    };
    if plane_count == expected {
        Ok(())
    } else {
        Err(MacosNativeReductionError::InvalidPlanes(
            "plane count does not match the capture format",
        ))
    }
}

fn require_metal_device(
    device: &wgpu::Device,
) -> Result<impl std::ops::Deref<Target = wgpu_hal::metal::Device> + '_, MacosGpuInteropError> {
    // SAFETY: the HAL device is borrowed only for immediate Metal allocation
    // or pipeline construction and never outlives the wgpu device.
    unsafe { device.as_hal::<wgpu_hal::api::Metal>() }
        .ok_or(MacosGpuInteropError::MissingWgpuMetalDevice)
}

fn raw_metal_texture(
    texture: &wgpu::Texture,
) -> Result<impl std::ops::Deref<Target = wgpu_hal::metal::Texture> + '_, MacosGpuInteropError> {
    // SAFETY: the guard is borrowed only for immediate command encoding and
    // the target was constructed on the Metal backend by this module.
    unsafe { texture.as_hal::<wgpu_hal::api::Metal>() }
        .ok_or(MacosGpuInteropError::MissingWgpuMetalDevice)
}

fn create_pipeline(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    entry_point: &'static str,
) -> Result<RetainedComputePipeline, MacosNativeReductionError> {
    let function = library
        .newFunctionWithName(&NSString::from_str(entry_point))
        .ok_or(MacosNativeReductionError::MissingEntryPoint(entry_point))?;
    device
        .newComputePipelineStateWithFunction_error(&function)
        .map(RetainedComputePipeline)
        .map_err(|error| {
            MacosNativeReductionError::PipelineCreation(error.localizedDescription().to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_rejects_content_outside_output() {
        assert!(matches!(
            MacosNativeReductionDescriptor::new(
                [8, 8],
                [4, 4, 5, 4],
                [0.0, 0.0, 8.0, 8.0],
                MacosNativeReductionFilter::Area,
                None,
            ),
            Err(MacosNativeReductionError::InvalidGeometry)
        ));
    }

    #[test]
    fn canonical_transform_stays_gpu_abi_compatible() {
        assert_eq!(size_of::<MacosNativeColorTransform>(), 80);
        assert_eq!(align_of::<MacosNativeColorTransform>(), 16);
        assert_eq!(size_of::<ReductionParameters>(), 176);
        assert_eq!(align_of::<ReductionParameters>(), 16);
        assert_eq!(size_of::<MaterializationParameters>(), 48);
        assert_eq!(align_of::<MaterializationParameters>(), 16);
    }
}
