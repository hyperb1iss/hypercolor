use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hypercolor_macos_capture::{MacosCaptureFrame, MacosCapturePixelFormat, MacosPixelExtent};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CFRetained;
use objc2_core_video::{CVMetalTexture, CVMetalTextureCache, CVPixelBuffer};
use objc2_io_surface::IOSurfaceRef;
use objc2_metal::{MTLPixelFormat, MTLTexture};
use thiserror::Error;

use crate::macos::{
    ImportedEffectFrame, ImportedFrameFormat, MacosGpuInteropError, MacosIosurfaceImportDescriptor,
    MacosIosurfaceImporter, MacosMetalStorageMode, create_core_video_texture_cache,
    import_core_video_metal_texture_plane, import_core_video_pixel_buffer_plane,
    import_iosurface_metal_texture_plane, metal_device_import_contract,
};

const MAX_CAPTURE_DESCRIPTORS: usize = 8;
const MAX_CORE_VIDEO_WRAPPERS: usize = 64;

/// Complete physical identity of one imported capture plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosScreenStorageIdentity {
    /// Capture stream generation that produced the surface.
    pub capture_session_generation: u64,
    /// Core resource generation authorizing this import.
    pub resource_generation: u64,
    /// Process-local IOSurface identity.
    pub iosurface_id: u32,
    /// IOSurface plane index.
    pub plane: u32,
    /// Plane extent.
    pub extent: MacosPixelExtent,
    /// Exact plane stride.
    pub bytes_per_row: usize,
    /// Capture pixel encoding.
    pub pixel_format: MacosCapturePixelFormat,
    /// Exact Core Video pixel-format FourCC delivered by the source.
    pub source_fourcc: u32,
    /// Exact IOSurface allocation size.
    pub allocation_bytes: u64,
    /// Family-selected Metal storage mode.
    pub storage_mode: MacosMetalStorageMode,
    /// Physical Metal device registry identity.
    pub metal_registry_id: u64,
}

/// Imported capture frame retaining its Core Video owner and wgpu wrapper.
#[derive(Debug, Clone)]
pub struct ImportedMacosScreenFrame {
    content_sequence: u64,
    capture: Arc<MacosCaptureFrame>,
    planes: Arc<[ImportedMacosScreenPlane]>,
}

/// One imported IOSurface plane and its exact storage identity.
#[derive(Debug, Clone)]
pub struct ImportedMacosScreenPlane {
    storage_identity: MacosScreenStorageIdentity,
    format: ImportedMacosScreenPlaneFormat,
    storage: ImportedMacosScreenPlaneStorage,
    core_video_wrapper: Option<RetainedCoreVideoTexture>,
}

#[derive(Debug, Clone)]
enum ImportedMacosScreenPlaneStorage {
    Wgpu(ImportedEffectFrame),
    NativeMetal(RetainedMetalTexture),
}

#[derive(Debug, Clone)]
struct RetainedMetalTexture(Retained<ProtocolObject<dyn MTLTexture>>);

// SAFETY: Metal resource objects have immutable identity and allocation
// properties after creation. Hypercolor only retains and borrows this texture;
// all content access is encoded through Metal command queues with resource
// hazard tracking and the capture owner remains alive through GPU completion.
unsafe impl Send for RetainedMetalTexture {}

// SAFETY: shared references expose only immutable resource inspection and
// command encoding. Mutable contents remain synchronized by Metal, not Rust.
unsafe impl Sync for RetainedMetalTexture {}

#[derive(Debug, Clone)]
struct RetainedCoreVideoTexture {
    _wrapper: CFRetained<CVMetalTexture>,
}

// SAFETY: the wrapper is retained only as immutable ownership for its Metal
// texture. Hypercolor never mutates the Core Video wrapper after creation.
unsafe impl Send for RetainedCoreVideoTexture {}

// SAFETY: shared access is limited to retaining and releasing the immutable
// wrapper; pixel contents are synchronized through the retained Metal texture.
unsafe impl Sync for RetainedCoreVideoTexture {}

struct SendableCoreVideoTextureCache(CFRetained<CVMetalTextureCache>);

// SAFETY: every operation on this cache is serialized by its containing
// mutex. Moving the retained Core Foundation reference does not invoke it.
unsafe impl Send for SendableCoreVideoTextureCache {}

impl SendableCoreVideoTextureCache {
    fn cache(&self) -> &CVMetalTextureCache {
        &self.0
    }
}

/// Exact native format retained for one imported capture plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImportedMacosScreenPlaneFormat {
    /// A format represented directly by wgpu.
    Wgpu(ImportedFrameFormat),
    /// ScreenCaptureKit `l10r` represented by Metal BGR10A2 semantics.
    Bgr10A2Unorm,
    /// ScreenCaptureKit `xf44` luma represented by native Metal R16 semantics.
    R16Unorm,
    /// ScreenCaptureKit `xf44` chroma represented by native Metal RG16 semantics.
    Rg16Unorm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CapturePlaneImportDescriptor {
    Wgpu(MacosIosurfaceImportDescriptor),
    Bgr10A2Unorm { width: u32, height: u32 },
    R16Unorm { width: u32, height: u32 },
    Rg16Unorm { width: u32, height: u32 },
}

impl CapturePlaneImportDescriptor {
    fn new(
        width: u32,
        height: u32,
        format: ImportedMacosScreenPlaneFormat,
    ) -> Result<Self, MacosScreenBridgeError> {
        match format {
            ImportedMacosScreenPlaneFormat::Wgpu(format) => Ok(Self::Wgpu(
                MacosIosurfaceImportDescriptor::new(width, height, format)?,
            )),
            ImportedMacosScreenPlaneFormat::Bgr10A2Unorm => {
                if width == 0
                    || height == 0
                    || width > i32::MAX as u32 / 4
                    || height > i32::MAX as u32
                {
                    return Err(MacosGpuInteropError::InvalidDimensions { width, height }.into());
                }
                Ok(Self::Bgr10A2Unorm { width, height })
            }
            ImportedMacosScreenPlaneFormat::R16Unorm => {
                validate_native_dimensions(width, height, 2)?;
                Ok(Self::R16Unorm { width, height })
            }
            ImportedMacosScreenPlaneFormat::Rg16Unorm => {
                validate_native_dimensions(width, height, 4)?;
                Ok(Self::Rg16Unorm { width, height })
            }
        }
    }

    const fn minimum_bytes_per_row(self) -> u32 {
        match self {
            Self::Wgpu(descriptor) => descriptor.width * descriptor.format.bytes_per_texel(),
            Self::Bgr10A2Unorm { width, .. } => width * 4,
            Self::R16Unorm { width, .. } => width * 2,
            Self::Rg16Unorm { width, .. } => width * 4,
        }
    }
}

fn validate_native_dimensions(
    width: u32,
    height: u32,
    bytes_per_texel: u32,
) -> Result<(), MacosScreenBridgeError> {
    if width == 0
        || height == 0
        || width > i32::MAX as u32 / bytes_per_texel
        || height > i32::MAX as u32
    {
        return Err(MacosGpuInteropError::InvalidDimensions { width, height }.into());
    }
    Ok(())
}

impl ImportedMacosScreenFrame {
    /// Complete physical storage identity of the first imported plane.
    ///
    /// Packed RGB frames have exactly one plane. Multi-plane callers should
    /// inspect [`Self::planes`] instead.
    #[must_use]
    pub fn storage_identity(&self) -> MacosScreenStorageIdentity {
        self.first_plane().storage_identity
    }

    /// Monotonic content identity within the capture session.
    #[must_use]
    pub const fn content_sequence(&self) -> u64 {
        self.content_sequence
    }

    /// Retained capture metadata and Core Video owner.
    #[must_use]
    pub fn capture(&self) -> &Arc<MacosCaptureFrame> {
        &self.capture
    }

    /// Every imported IOSurface plane in source order.
    #[must_use]
    pub fn planes(&self) -> &[ImportedMacosScreenPlane] {
        &self.planes
    }

    /// Complete physical storage identities for every imported plane.
    pub fn storage_identities(
        &self,
    ) -> impl ExactSizeIterator<Item = MacosScreenStorageIdentity> + '_ {
        self.planes.iter().map(|plane| plane.storage_identity)
    }

    /// Imported wgpu texture for a packed wgpu-representable frame.
    #[must_use]
    pub fn texture(&self) -> Option<&Arc<wgpu::Texture>> {
        self.first_plane().texture()
    }

    /// Default view over a packed wgpu-representable frame.
    #[must_use]
    pub fn view(&self) -> Option<&Arc<wgpu::TextureView>> {
        self.first_plane().view()
    }

    fn first_plane(&self) -> &ImportedMacosScreenPlane {
        self.planes
            .first()
            .expect("validated capture imports always retain at least one plane")
    }
}

impl ImportedMacosScreenPlane {
    /// Complete physical storage identity used by the wrapper cache.
    #[must_use]
    pub const fn storage_identity(&self) -> MacosScreenStorageIdentity {
        self.storage_identity
    }

    /// Exact wrapped texture format for this plane.
    #[must_use]
    pub const fn format(&self) -> ImportedMacosScreenPlaneFormat {
        self.format
    }

    /// Imported wgpu texture.
    #[must_use]
    pub fn texture(&self) -> Option<&Arc<wgpu::Texture>> {
        match &self.storage {
            ImportedMacosScreenPlaneStorage::Wgpu(imported) => Some(&imported.texture),
            ImportedMacosScreenPlaneStorage::NativeMetal(_) => None,
        }
    }

    /// Default view over the imported plane texture.
    #[must_use]
    pub fn view(&self) -> Option<&Arc<wgpu::TextureView>> {
        match &self.storage {
            ImportedMacosScreenPlaneStorage::Wgpu(imported) => Some(&imported.view),
            ImportedMacosScreenPlaneStorage::NativeMetal(_) => None,
        }
    }

    /// Whether Core Video owns the retained Metal texture wrapper.
    #[must_use]
    pub const fn uses_core_video_texture_cache(&self) -> bool {
        self.core_video_wrapper.is_some()
    }

    /// Borrows the exact native Metal texture for immediate GPU encoding.
    pub fn with_metal_texture<R>(
        &self,
        operation: impl FnOnce(&ProtocolObject<dyn MTLTexture>) -> R,
    ) -> Result<R, MacosScreenBridgeError> {
        match &self.storage {
            ImportedMacosScreenPlaneStorage::Wgpu(imported) => {
                // SAFETY: the guard is borrowed only for this immediate call,
                // and the imported texture is known to use the Metal backend.
                let texture = unsafe { imported.texture.as_hal::<wgpu_hal::api::Metal>() }
                    .ok_or(MacosGpuInteropError::MissingWgpuMetalDevice)?;
                Ok(operation(texture.raw_handle()))
            }
            ImportedMacosScreenPlaneStorage::NativeMetal(texture) => Ok(operation(&texture.0)),
        }
    }
}

/// Native importer candidate used for a retained capture frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosScreenImporterCandidate {
    /// Direct `MTLDevice` IOSurface texture creation.
    DirectIosurface,
    /// Core Video's Metal texture cache.
    CoreVideoTextureCache,
}

/// Errors raised while importing a ScreenCaptureKit frame.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MacosScreenBridgeError {
    /// The frame does not satisfy the native import contract.
    #[error("invalid macOS capture frame: {0}")]
    InvalidFrame(&'static str),
    /// Both zero-copy importer candidates rejected the frame.
    #[error(
        "macOS screen import failed via {first_candidate:?}: {first_error}; then {second_candidate:?}: {second_error}"
    )]
    ImportCandidatesFailed {
        /// Candidate attempted first for the active GPU family.
        first_candidate: MacosScreenImporterCandidate,
        /// Bounded first-candidate result.
        first_error: String,
        /// Candidate attempted second for the active GPU family.
        second_candidate: MacosScreenImporterCandidate,
        /// Bounded second-candidate result.
        second_error: String,
    },
    /// The capture surface could not provide native handles.
    #[error("macOS capture surface handoff failed: {0}")]
    SurfaceHandoff(String),
    /// IOSurface or Metal import failed.
    #[error(transparent)]
    Interop(#[from] MacosGpuInteropError),
}

/// Core-agnostic ScreenCaptureKit IOSurface to wgpu bridge.
pub struct MacosScreenBridge {
    metal_registry_id: u64,
    storage_mode: MacosMetalStorageMode,
    importers: Mutex<HashMap<MacosIosurfaceImportDescriptor, MacosIosurfaceImporter>>,
    core_video_cache: Option<Mutex<SendableCoreVideoTextureCache>>,
    core_video_cache_error: Option<String>,
    native_wrappers: Mutex<HashMap<MacosScreenStorageIdentity, ImportedMacosScreenPlane>>,
}

impl MacosScreenBridge {
    /// Bind a bridge to one Metal-backed wgpu device.
    pub fn new(device: &wgpu::Device) -> Result<Self, MacosScreenBridgeError> {
        let (metal_registry_id, storage_mode) = metal_device_import_contract(device)?;
        let (core_video_cache, core_video_cache_error) =
            match create_core_video_texture_cache(device, storage_mode) {
                Ok(cache) => (Some(Mutex::new(SendableCoreVideoTextureCache(cache))), None),
                Err(error) => (None, Some(bounded_import_error(&error))),
            };
        Ok(Self {
            metal_registry_id,
            storage_mode,
            importers: Mutex::new(HashMap::new()),
            core_video_cache,
            core_video_cache_error,
            native_wrappers: Mutex::new(HashMap::new()),
        })
    }

    /// Physical Metal device registry identity.
    #[must_use]
    pub const fn metal_registry_id(&self) -> u64 {
        self.metal_registry_id
    }

    /// Family-selected storage mode.
    #[must_use]
    pub const fn storage_mode(&self) -> MacosMetalStorageMode {
        self.storage_mode
    }

    /// Import every directly representable plane without a full-frame CPU copy.
    pub fn import_frame(
        &self,
        device: &wgpu::Device,
        resource_generation: u64,
        frame: Arc<MacosCaptureFrame>,
    ) -> Result<ImportedMacosScreenFrame, MacosScreenBridgeError> {
        let device_contract = metal_device_import_contract(device)?;
        validate_import_device_contract(
            self.metal_registry_id,
            self.storage_mode,
            device_contract,
        )?;
        let plane_descriptors = validate_frame(&frame, resource_generation)?;
        let source_pixel_format = frame
            .pixel_format
            .fourcc(frame.color.range)
            .map_err(|_| MacosScreenBridgeError::InvalidFrame("invalid source color range"))?;
        let imported_planes = frame
            .surface
            .with_native_surface(|lease| {
                // SAFETY: the opaque lease was created from this exact
                // retained IOSurface and cannot outlive this closure.
                let iosurface = unsafe { lease.iosurface_ptr().cast::<IOSurfaceRef>().as_ref() };
                // SAFETY: the opaque lease was created from this exact
                // retained pixel buffer and cannot outlive this closure.
                let pixel_buffer =
                    unsafe { lease.pixel_buffer_ptr().cast::<CVPixelBuffer>().as_ref() };
                validate_native_surface(iosurface, &frame)?;
                let candidates = importer_candidate_order(self.storage_mode);
                let first = self.import_candidate(
                    candidates[0],
                    device,
                    iosurface,
                    pixel_buffer,
                    &frame,
                    resource_generation,
                    source_pixel_format,
                    &plane_descriptors,
                );
                let first_error = match first {
                    Ok(planes) => return Ok(planes),
                    Err(error) => bounded_import_error(&error),
                };
                match self.import_candidate(
                    candidates[1],
                    device,
                    iosurface,
                    pixel_buffer,
                    &frame,
                    resource_generation,
                    source_pixel_format,
                    &plane_descriptors,
                ) {
                    Ok(planes) => Ok(planes),
                    Err(error) => Err(MacosScreenBridgeError::ImportCandidatesFailed {
                        first_candidate: candidates[0],
                        first_error,
                        second_candidate: candidates[1],
                        second_error: bounded_import_error(&error),
                    }),
                }
            })
            .map_err(|error| MacosScreenBridgeError::SurfaceHandoff(error.to_string()))??;

        Ok(ImportedMacosScreenFrame {
            content_sequence: frame.sequence,
            capture: frame,
            planes: imported_planes.into(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn import_candidate(
        &self,
        candidate: MacosScreenImporterCandidate,
        device: &wgpu::Device,
        iosurface: &IOSurfaceRef,
        pixel_buffer: &CVPixelBuffer,
        frame: &MacosCaptureFrame,
        resource_generation: u64,
        source_pixel_format: u32,
        descriptors: &[CapturePlaneImportDescriptor],
    ) -> Result<Vec<ImportedMacosScreenPlane>, MacosScreenBridgeError> {
        match candidate {
            MacosScreenImporterCandidate::DirectIosurface => self.import_direct_planes(
                device,
                iosurface,
                frame,
                resource_generation,
                source_pixel_format,
                descriptors,
            ),
            MacosScreenImporterCandidate::CoreVideoTextureCache => self.import_core_video_planes(
                device,
                pixel_buffer,
                frame,
                resource_generation,
                source_pixel_format,
                descriptors,
            ),
        }
    }

    fn import_direct_planes(
        &self,
        device: &wgpu::Device,
        iosurface: &IOSurfaceRef,
        frame: &MacosCaptureFrame,
        resource_generation: u64,
        source_pixel_format: u32,
        descriptors: &[CapturePlaneImportDescriptor],
    ) -> Result<Vec<ImportedMacosScreenPlane>, MacosScreenBridgeError> {
        let mut imported_planes = admitted_plane_vector(descriptors.len())?;
        for (plane, descriptor) in frame.planes.iter().zip(descriptors) {
            let plane_index = usize::try_from(plane.index).map_err(|_| {
                MacosScreenBridgeError::InvalidFrame("capture plane index exceeds usize")
            })?;
            let storage_identity =
                self.storage_identity(frame, plane, resource_generation, source_pixel_format);
            let imported_plane = match descriptor {
                CapturePlaneImportDescriptor::Wgpu(descriptor) => {
                    let mut importers = self
                        .importers
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if !importers.contains_key(descriptor) {
                        if importers.len() >= MAX_CAPTURE_DESCRIPTORS {
                            importers.clear();
                        }
                        importers.insert(
                            *descriptor,
                            MacosIosurfaceImporter::new(device, *descriptor)?,
                        );
                    }
                    let importer = importers.get_mut(descriptor).ok_or(
                        MacosScreenBridgeError::InvalidFrame(
                            "capture importer cache insertion failed",
                        ),
                    )?;
                    let imported = importer.import_iosurface_plane_scoped(
                        device,
                        iosurface,
                        frame.sequence,
                        frame.epoch,
                        resource_generation,
                        plane_index,
                        source_pixel_format,
                    )?;
                    ImportedMacosScreenPlane {
                        storage_identity,
                        format: ImportedMacosScreenPlaneFormat::Wgpu(descriptor.format),
                        storage: ImportedMacosScreenPlaneStorage::Wgpu(imported),
                        core_video_wrapper: None,
                    }
                }
                CapturePlaneImportDescriptor::Bgr10A2Unorm { width, height }
                | CapturePlaneImportDescriptor::R16Unorm { width, height }
                | CapturePlaneImportDescriptor::Rg16Unorm { width, height } => self
                    .native_wrapper_or_insert(storage_identity, || {
                        let (format, metal_format) = native_plane_format(*descriptor);
                        let texture = import_iosurface_metal_texture_plane(
                            device,
                            iosurface,
                            *width,
                            *height,
                            plane_index,
                            source_pixel_format,
                            metal_format,
                            self.storage_mode,
                        )?;
                        Ok(ImportedMacosScreenPlane {
                            storage_identity,
                            format,
                            storage: ImportedMacosScreenPlaneStorage::NativeMetal(
                                RetainedMetalTexture(texture),
                            ),
                            core_video_wrapper: None,
                        })
                    })?,
            };
            imported_planes.push(imported_plane);
        }
        Ok(imported_planes)
    }

    fn import_core_video_planes(
        &self,
        device: &wgpu::Device,
        pixel_buffer: &CVPixelBuffer,
        frame: &MacosCaptureFrame,
        resource_generation: u64,
        source_pixel_format: u32,
        descriptors: &[CapturePlaneImportDescriptor],
    ) -> Result<Vec<ImportedMacosScreenPlane>, MacosScreenBridgeError> {
        let cache = self.core_video_cache.as_ref().ok_or_else(|| {
            MacosScreenBridgeError::SurfaceHandoff(
                self.core_video_cache_error
                    .clone()
                    .unwrap_or_else(|| "Core Video texture cache unavailable".to_owned()),
            )
        })?;
        let cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut wrappers = self
            .native_wrappers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut imported_planes = admitted_plane_vector(descriptors.len())?;
        for (plane, descriptor) in frame.planes.iter().zip(descriptors) {
            let storage_identity =
                self.storage_identity(frame, plane, resource_generation, source_pixel_format);
            if let Some(cached) = wrappers.get(&storage_identity) {
                imported_planes.push(cached.clone());
                continue;
            }
            let plane_index = usize::try_from(plane.index).map_err(|_| {
                MacosScreenBridgeError::InvalidFrame("capture plane index exceeds usize")
            })?;
            let imported_plane = match descriptor {
                CapturePlaneImportDescriptor::Wgpu(descriptor) => {
                    let (imported, wrapper) = import_core_video_pixel_buffer_plane(
                        device,
                        cache.cache(),
                        pixel_buffer,
                        *descriptor,
                        plane_index,
                        frame.surface.iosurface_id,
                        self.storage_mode,
                        frame.sequence,
                    )?;
                    ImportedMacosScreenPlane {
                        storage_identity,
                        format: ImportedMacosScreenPlaneFormat::Wgpu(descriptor.format),
                        storage: ImportedMacosScreenPlaneStorage::Wgpu(imported),
                        core_video_wrapper: Some(RetainedCoreVideoTexture { _wrapper: wrapper }),
                    }
                }
                CapturePlaneImportDescriptor::Bgr10A2Unorm { width, height }
                | CapturePlaneImportDescriptor::R16Unorm { width, height }
                | CapturePlaneImportDescriptor::Rg16Unorm { width, height } => {
                    let (format, metal_format) = native_plane_format(*descriptor);
                    let (texture, wrapper, _) = import_core_video_metal_texture_plane(
                        cache.cache(),
                        pixel_buffer,
                        *width,
                        *height,
                        plane_index,
                        metal_format,
                        frame.surface.iosurface_id,
                        self.storage_mode,
                    )?;
                    ImportedMacosScreenPlane {
                        storage_identity,
                        format,
                        storage: ImportedMacosScreenPlaneStorage::NativeMetal(
                            RetainedMetalTexture(texture),
                        ),
                        core_video_wrapper: Some(RetainedCoreVideoTexture { _wrapper: wrapper }),
                    }
                }
            };
            if wrappers.len() >= MAX_CORE_VIDEO_WRAPPERS {
                wrappers.clear();
                cache.cache().flush(0);
            }
            wrappers.insert(storage_identity, imported_plane.clone());
            imported_planes.push(imported_plane);
        }
        Ok(imported_planes)
    }

    fn storage_identity(
        &self,
        frame: &MacosCaptureFrame,
        plane: &hypercolor_macos_capture::MacosCapturePlane,
        resource_generation: u64,
        source_fourcc: u32,
    ) -> MacosScreenStorageIdentity {
        MacosScreenStorageIdentity {
            capture_session_generation: frame.epoch,
            resource_generation,
            iosurface_id: frame.surface.iosurface_id,
            plane: plane.index,
            extent: plane.extent,
            bytes_per_row: plane.bytes_per_row,
            pixel_format: frame.pixel_format,
            source_fourcc,
            allocation_bytes: frame.surface.allocation_bytes,
            storage_mode: self.storage_mode,
            metal_registry_id: self.metal_registry_id,
        }
    }

    /// Import one retained packed BGRA frame without a full-frame CPU copy.
    pub fn import_bgra_frame(
        &self,
        device: &wgpu::Device,
        resource_generation: u64,
        frame: Arc<MacosCaptureFrame>,
    ) -> Result<ImportedMacosScreenFrame, MacosScreenBridgeError> {
        if frame.pixel_format != MacosCapturePixelFormat::Bgra8 {
            return Err(MacosScreenBridgeError::InvalidFrame(
                "packed BGRA import received another pixel format",
            ));
        }
        self.import_frame(device, resource_generation, frame)
    }

    /// Number of cached physical IOSurface wrappers across live descriptors.
    #[must_use]
    pub fn cached_wrap_count(&self) -> usize {
        let direct = self
            .importers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .fold(0_usize, |total, importer| {
                total.saturating_add(importer.cached_wrap_count())
            });
        direct.saturating_add(
            self.native_wrappers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
        )
    }

    fn native_wrapper_or_insert(
        &self,
        identity: MacosScreenStorageIdentity,
        create: impl FnOnce() -> Result<ImportedMacosScreenPlane, MacosScreenBridgeError>,
    ) -> Result<ImportedMacosScreenPlane, MacosScreenBridgeError> {
        let (plane, flush_core_video) = {
            let mut wrappers = self
                .native_wrappers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(cached) = wrappers.get(&identity) {
                return Ok(cached.clone());
            }
            let plane = create()?;
            let flush_core_video = wrappers.len() >= MAX_CORE_VIDEO_WRAPPERS;
            if flush_core_video {
                wrappers.clear();
            }
            wrappers.insert(identity, plane.clone());
            (plane, flush_core_video)
        };
        if flush_core_video {
            self.flush_core_video_cache();
        }
        Ok(plane)
    }

    fn flush_core_video_cache(&self) {
        if let Some(cache) = &self.core_video_cache {
            cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cache()
                .flush(0);
        }
    }
}

fn validate_import_device_contract(
    expected_registry_id: u64,
    expected_storage_mode: MacosMetalStorageMode,
    actual: (u64, MacosMetalStorageMode),
) -> Result<(), MacosScreenBridgeError> {
    if actual.0 != expected_registry_id {
        return Err(MacosGpuInteropError::MetalRegistryIdMismatch {
            expected: expected_registry_id,
            actual: actual.0,
        }
        .into());
    }
    if actual.1 != expected_storage_mode {
        return Err(MacosGpuInteropError::MetalStorageModeMismatch {
            expected: expected_storage_mode,
            actual: actual.1,
        }
        .into());
    }
    Ok(())
}

fn importer_candidate_order(
    storage_mode: MacosMetalStorageMode,
) -> [MacosScreenImporterCandidate; 2] {
    match storage_mode {
        MacosMetalStorageMode::Shared => [
            MacosScreenImporterCandidate::DirectIosurface,
            MacosScreenImporterCandidate::CoreVideoTextureCache,
        ],
        MacosMetalStorageMode::Managed => [
            MacosScreenImporterCandidate::CoreVideoTextureCache,
            MacosScreenImporterCandidate::DirectIosurface,
        ],
    }
}

fn admitted_plane_vector(
    capacity: usize,
) -> Result<Vec<ImportedMacosScreenPlane>, MacosScreenBridgeError> {
    let mut planes = Vec::new();
    planes.try_reserve_exact(capacity).map_err(|_| {
        MacosScreenBridgeError::InvalidFrame("capture plane metadata allocation failed")
    })?;
    Ok(planes)
}

fn bounded_import_error(error: &impl std::fmt::Display) -> String {
    const MAX_IMPORT_ERROR_CHARS: usize = 512;
    error
        .to_string()
        .chars()
        .take(MAX_IMPORT_ERROR_CHARS)
        .collect()
}

fn validate_frame(
    frame: &MacosCaptureFrame,
    resource_generation: u64,
) -> Result<Vec<CapturePlaneImportDescriptor>, MacosScreenBridgeError> {
    if frame.epoch == 0 || resource_generation == 0 {
        return Err(MacosScreenBridgeError::InvalidFrame(
            "capture and resource generations must be nonzero",
        ));
    }
    let expected_formats = capture_plane_formats(frame.pixel_format)?;
    if frame.planes.len() != expected_formats.len() {
        return Err(MacosScreenBridgeError::InvalidFrame(
            "capture plane count does not match its pixel format",
        ));
    }
    let mut descriptors = Vec::new();
    descriptors
        .try_reserve_exact(frame.planes.len())
        .map_err(|_| {
            MacosScreenBridgeError::InvalidFrame("capture plane descriptor allocation failed")
        })?;
    for (index, (plane, format)) in frame.planes.iter().zip(expected_formats).enumerate() {
        if usize::try_from(plane.index).ok() != Some(index) {
            return Err(MacosScreenBridgeError::InvalidFrame(
                "capture plane indices are not canonical",
            ));
        }
        let expected_extent = capture_plane_extent(frame.pixel_format, frame.storage_extent, index);
        if plane.extent != expected_extent {
            return Err(MacosScreenBridgeError::InvalidFrame(
                "capture plane extent does not match its pixel format",
            ));
        }
        let descriptor =
            CapturePlaneImportDescriptor::new(plane.extent.width, plane.extent.height, *format)?;
        let minimum_stride = usize::try_from(plane.extent.width)
            .ok()
            .and_then(|_| usize::try_from(descriptor.minimum_bytes_per_row()).ok())
            .ok_or(MacosScreenBridgeError::InvalidFrame(
                "capture plane stride overflowed",
            ))?;
        let minimum_length = u64::try_from(plane.bytes_per_row)
            .ok()
            .and_then(|stride| stride.checked_mul(u64::from(plane.extent.height)))
            .ok_or(MacosScreenBridgeError::InvalidFrame(
                "capture plane length overflowed",
            ))?;
        if plane.bytes_per_row < minimum_stride || plane.length_bytes < minimum_length {
            return Err(MacosScreenBridgeError::InvalidFrame(
                "capture plane storage is smaller than its descriptor",
            ));
        }
        descriptors.push(descriptor);
    }
    Ok(descriptors)
}

fn capture_plane_formats(
    pixel_format: MacosCapturePixelFormat,
) -> Result<&'static [ImportedMacosScreenPlaneFormat], MacosScreenBridgeError> {
    const BGRA: &[ImportedMacosScreenPlaneFormat] = &[ImportedMacosScreenPlaneFormat::Wgpu(
        ImportedFrameFormat::Bgra8Unorm,
    )];
    const RGB10: &[ImportedMacosScreenPlaneFormat] =
        &[ImportedMacosScreenPlaneFormat::Bgr10A2Unorm];
    const RGBA16: &[ImportedMacosScreenPlaneFormat] = &[ImportedMacosScreenPlaneFormat::Wgpu(
        ImportedFrameFormat::Rgba16Float,
    )];
    const YUV420: &[ImportedMacosScreenPlaneFormat] = &[
        ImportedMacosScreenPlaneFormat::Wgpu(ImportedFrameFormat::R8Unorm),
        ImportedMacosScreenPlaneFormat::Wgpu(ImportedFrameFormat::Rg8Unorm),
    ];
    const YUV44410: &[ImportedMacosScreenPlaneFormat] = &[
        ImportedMacosScreenPlaneFormat::R16Unorm,
        ImportedMacosScreenPlaneFormat::Rg16Unorm,
    ];

    match pixel_format {
        MacosCapturePixelFormat::Bgra8 => Ok(BGRA),
        MacosCapturePixelFormat::Rgba16Float => Ok(RGBA16),
        MacosCapturePixelFormat::Yuv420VideoRange | MacosCapturePixelFormat::Yuv420FullRange => {
            Ok(YUV420)
        }
        MacosCapturePixelFormat::Yuv44410BiPlanar => Ok(YUV44410),
        MacosCapturePixelFormat::Argb2101010 => Ok(RGB10),
    }
}

fn native_plane_format(
    descriptor: CapturePlaneImportDescriptor,
) -> (ImportedMacosScreenPlaneFormat, MTLPixelFormat) {
    match descriptor {
        CapturePlaneImportDescriptor::Bgr10A2Unorm { .. } => (
            ImportedMacosScreenPlaneFormat::Bgr10A2Unorm,
            MTLPixelFormat::BGR10A2Unorm,
        ),
        CapturePlaneImportDescriptor::R16Unorm { .. } => (
            ImportedMacosScreenPlaneFormat::R16Unorm,
            MTLPixelFormat::R16Unorm,
        ),
        CapturePlaneImportDescriptor::Rg16Unorm { .. } => (
            ImportedMacosScreenPlaneFormat::Rg16Unorm,
            MTLPixelFormat::RG16Unorm,
        ),
        CapturePlaneImportDescriptor::Wgpu(_) => {
            unreachable!("wgpu planes never enter native format selection")
        }
    }
}

const fn capture_plane_extent(
    pixel_format: MacosCapturePixelFormat,
    storage_extent: MacosPixelExtent,
    plane: usize,
) -> MacosPixelExtent {
    if matches!(
        pixel_format,
        MacosCapturePixelFormat::Yuv420VideoRange | MacosCapturePixelFormat::Yuv420FullRange
    ) && plane == 1
    {
        MacosPixelExtent {
            width: storage_extent.width.div_ceil(2),
            height: storage_extent.height.div_ceil(2),
        }
    } else {
        storage_extent
    }
}

fn validate_native_surface(
    iosurface: &IOSurfaceRef,
    frame: &MacosCaptureFrame,
) -> Result<(), MacosScreenBridgeError> {
    let allocation_bytes = u64::try_from(iosurface.alloc_size())
        .map_err(|_| MacosScreenBridgeError::InvalidFrame("IOSurface allocation exceeds u64"))?;
    let source_pixel_format = frame
        .pixel_format
        .fourcc(frame.color.range)
        .map_err(|_| MacosScreenBridgeError::InvalidFrame("invalid source color range"))?;
    if iosurface.id() != frame.surface.iosurface_id
        || iosurface.width() != frame.storage_extent.width as usize
        || iosurface.height() != frame.storage_extent.height as usize
        || iosurface.pixel_format() != source_pixel_format
        || allocation_bytes != frame.surface.allocation_bytes
    {
        return Err(MacosScreenBridgeError::InvalidFrame(
            "IOSurface physical descriptor changed after capture validation",
        ));
    }
    let native_plane_count = iosurface.plane_count();
    if frame.planes.len() == 1 && native_plane_count == 0 {
        if iosurface.bytes_per_row() != frame.planes[0].bytes_per_row {
            return Err(MacosScreenBridgeError::InvalidFrame(
                "IOSurface packed stride changed after capture validation",
            ));
        }
        return Ok(());
    }
    if native_plane_count != frame.planes.len() {
        return Err(MacosScreenBridgeError::InvalidFrame(
            "IOSurface plane count changed after capture validation",
        ));
    }
    for plane in &*frame.planes {
        let index = usize::try_from(plane.index)
            .map_err(|_| MacosScreenBridgeError::InvalidFrame("plane index exceeds usize"))?;
        if iosurface.width_of_plane(index) != plane.extent.width as usize
            || iosurface.height_of_plane(index) != plane.extent.height as usize
            || iosurface.bytes_per_row_of_plane(index) != plane.bytes_per_row
        {
            return Err(MacosScreenBridgeError::InvalidFrame(
                "IOSurface plane descriptor changed after capture validation",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_formats_map_to_exact_direct_plane_formats() {
        assert_eq!(
            capture_plane_formats(MacosCapturePixelFormat::Bgra8)
                .expect("BGRA should import directly"),
            &[ImportedMacosScreenPlaneFormat::Wgpu(
                ImportedFrameFormat::Bgra8Unorm
            )]
        );
        assert_eq!(
            capture_plane_formats(MacosCapturePixelFormat::Rgba16Float)
                .expect("RGBA16Float should import directly"),
            &[ImportedMacosScreenPlaneFormat::Wgpu(
                ImportedFrameFormat::Rgba16Float
            )]
        );
        for format in [
            MacosCapturePixelFormat::Yuv420VideoRange,
            MacosCapturePixelFormat::Yuv420FullRange,
        ] {
            assert_eq!(
                capture_plane_formats(format).expect("8-bit YUV should import directly"),
                &[
                    ImportedMacosScreenPlaneFormat::Wgpu(ImportedFrameFormat::R8Unorm),
                    ImportedMacosScreenPlaneFormat::Wgpu(ImportedFrameFormat::Rg8Unorm)
                ]
            );
        }
        assert_eq!(
            capture_plane_formats(MacosCapturePixelFormat::Yuv44410BiPlanar)
                .expect("10-bit YUV should import directly"),
            &[
                ImportedMacosScreenPlaneFormat::R16Unorm,
                ImportedMacosScreenPlaneFormat::Rg16Unorm
            ]
        );
        assert_eq!(
            capture_plane_formats(MacosCapturePixelFormat::Argb2101010)
                .expect("ARGB2101010 should import as native BGR10A2"),
            &[ImportedMacosScreenPlaneFormat::Bgr10A2Unorm]
        );
    }

    #[test]
    fn yuv420_chroma_extent_uses_ceil_division() {
        let storage = MacosPixelExtent {
            width: 1_919,
            height: 1_079,
        };
        assert_eq!(
            capture_plane_extent(MacosCapturePixelFormat::Yuv420FullRange, storage, 0),
            storage
        );
        assert_eq!(
            capture_plane_extent(MacosCapturePixelFormat::Yuv420FullRange, storage, 1),
            MacosPixelExtent {
                width: 960,
                height: 540
            }
        );
    }

    #[test]
    fn importer_order_is_selected_by_gpu_family_storage_contract() {
        assert_eq!(
            importer_candidate_order(MacosMetalStorageMode::Shared),
            [
                MacosScreenImporterCandidate::DirectIosurface,
                MacosScreenImporterCandidate::CoreVideoTextureCache
            ]
        );
        assert_eq!(
            importer_candidate_order(MacosMetalStorageMode::Managed),
            [
                MacosScreenImporterCandidate::CoreVideoTextureCache,
                MacosScreenImporterCandidate::DirectIosurface
            ]
        );
    }

    #[test]
    fn importer_errors_are_bounded() {
        let oversized = "x".repeat(1_024);
        assert_eq!(bounded_import_error(&oversized).len(), 512);
    }

    #[test]
    fn import_device_contract_requires_registry_and_storage_mode_identity() {
        assert!(
            validate_import_device_contract(
                7,
                MacosMetalStorageMode::Shared,
                (7, MacosMetalStorageMode::Shared)
            )
            .is_ok()
        );
        assert!(matches!(
            validate_import_device_contract(
                7,
                MacosMetalStorageMode::Shared,
                (8, MacosMetalStorageMode::Shared),
            ),
            Err(MacosScreenBridgeError::Interop(
                MacosGpuInteropError::MetalRegistryIdMismatch {
                    expected: 7,
                    actual: 8,
                }
            ))
        ));
        assert!(matches!(
            validate_import_device_contract(
                7,
                MacosMetalStorageMode::Shared,
                (7, MacosMetalStorageMode::Managed),
            ),
            Err(MacosScreenBridgeError::Interop(
                MacosGpuInteropError::MetalStorageModeMismatch {
                    expected: MacosMetalStorageMode::Shared,
                    actual: MacosMetalStorageMode::Managed,
                }
            ))
        ));
    }
}
