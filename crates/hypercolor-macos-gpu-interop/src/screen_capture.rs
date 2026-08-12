use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hypercolor_macos_capture::{MacosCaptureFrame, MacosCapturePixelFormat, MacosPixelExtent};
use objc2_io_surface::IOSurfaceRef;
use thiserror::Error;

use crate::macos::{
    ImportedEffectFrame, ImportedFrameFormat, MacosGpuInteropError, MacosIosurfaceImportDescriptor,
    MacosIosurfaceImporter, MacosMetalStorageMode, metal_device_import_contract,
};

const MAX_CAPTURE_DESCRIPTORS: usize = 8;

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
    storage_identity: MacosScreenStorageIdentity,
    content_sequence: u64,
    capture: Arc<MacosCaptureFrame>,
    imported: ImportedEffectFrame,
}

impl ImportedMacosScreenFrame {
    /// Complete physical storage identity used by the wrapper cache.
    #[must_use]
    pub const fn storage_identity(&self) -> MacosScreenStorageIdentity {
        self.storage_identity
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

    /// Imported wgpu texture.
    #[must_use]
    pub fn texture(&self) -> &Arc<wgpu::Texture> {
        &self.imported.texture
    }

    /// Default view over the imported texture.
    #[must_use]
    pub fn view(&self) -> &Arc<wgpu::TextureView> {
        &self.imported.view
    }
}

/// Errors raised while importing a ScreenCaptureKit frame.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MacosScreenBridgeError {
    /// The frame does not satisfy the packed BGRA import contract.
    #[error("invalid macOS capture frame: {0}")]
    InvalidFrame(&'static str),
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
}

impl MacosScreenBridge {
    /// Bind a bridge to one Metal-backed wgpu device.
    pub fn new(device: &wgpu::Device) -> Result<Self, MacosScreenBridgeError> {
        let (metal_registry_id, storage_mode) = metal_device_import_contract(device)?;
        Ok(Self {
            metal_registry_id,
            storage_mode,
            importers: Mutex::new(HashMap::new()),
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

    /// Import one retained packed BGRA frame without a full-frame CPU copy.
    pub fn import_bgra_frame(
        &self,
        device: &wgpu::Device,
        resource_generation: u64,
        frame: Arc<MacosCaptureFrame>,
    ) -> Result<ImportedMacosScreenFrame, MacosScreenBridgeError> {
        validate_bgra_frame(&frame, resource_generation)?;
        let descriptor = MacosIosurfaceImportDescriptor::new(
            frame.storage_extent.width,
            frame.storage_extent.height,
            ImportedFrameFormat::Bgra8Unorm,
        )?;
        let plane = frame
            .planes
            .first()
            .ok_or(MacosScreenBridgeError::InvalidFrame("missing packed plane"))?;
        let storage_identity = MacosScreenStorageIdentity {
            capture_session_generation: frame.epoch,
            resource_generation,
            iosurface_id: frame.surface.iosurface_id,
            plane: plane.index,
            extent: plane.extent,
            bytes_per_row: plane.bytes_per_row,
            pixel_format: frame.pixel_format,
            allocation_bytes: frame.surface.allocation_bytes,
            storage_mode: self.storage_mode,
            metal_registry_id: self.metal_registry_id,
        };
        let imported = frame
            .surface
            .with_native_surface(|lease| {
                // SAFETY: the opaque lease was created from this exact
                // retained IOSurface and cannot outlive this closure.
                let iosurface = unsafe { lease.iosurface_ptr().cast::<IOSurfaceRef>().as_ref() };
                validate_native_surface(iosurface, storage_identity)?;
                let mut importers = self
                    .importers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !importers.contains_key(&descriptor) {
                    if importers.len() >= MAX_CAPTURE_DESCRIPTORS {
                        importers.clear();
                    }
                    importers.insert(descriptor, MacosIosurfaceImporter::new(device, descriptor)?);
                }
                let importer =
                    importers
                        .get_mut(&descriptor)
                        .ok_or(MacosScreenBridgeError::InvalidFrame(
                            "capture importer cache insertion failed",
                        ))?;
                Ok::<ImportedEffectFrame, MacosScreenBridgeError>(
                    importer.import_iosurface_scoped(
                        device,
                        iosurface,
                        frame.sequence,
                        frame.epoch,
                        resource_generation,
                    )?,
                )
            })
            .map_err(|error| MacosScreenBridgeError::SurfaceHandoff(error.to_string()))??;

        Ok(ImportedMacosScreenFrame {
            storage_identity,
            content_sequence: frame.sequence,
            capture: frame,
            imported,
        })
    }

    /// Number of cached physical IOSurface wrappers across live descriptors.
    #[must_use]
    pub fn cached_wrap_count(&self) -> usize {
        self.importers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .fold(0, |total, importer| {
                total.saturating_add(importer.cached_wrap_count())
            })
    }
}

fn validate_bgra_frame(
    frame: &MacosCaptureFrame,
    resource_generation: u64,
) -> Result<(), MacosScreenBridgeError> {
    if frame.epoch == 0 || resource_generation == 0 {
        return Err(MacosScreenBridgeError::InvalidFrame(
            "capture and resource generations must be nonzero",
        ));
    }
    if frame.pixel_format != MacosCapturePixelFormat::Bgra8 {
        return Err(MacosScreenBridgeError::InvalidFrame(
            "packed BGRA import received another pixel format",
        ));
    }
    let [plane] = &*frame.planes else {
        return Err(MacosScreenBridgeError::InvalidFrame(
            "packed BGRA import requires exactly one plane",
        ));
    };
    let minimum_stride = usize::try_from(frame.storage_extent.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or(MacosScreenBridgeError::InvalidFrame(
            "packed BGRA stride overflowed",
        ))?;
    if plane.index != 0
        || plane.extent != frame.storage_extent
        || plane.bytes_per_row < minimum_stride
    {
        return Err(MacosScreenBridgeError::InvalidFrame(
            "packed BGRA plane descriptor is inconsistent",
        ));
    }
    Ok(())
}

fn validate_native_surface(
    iosurface: &IOSurfaceRef,
    expected: MacosScreenStorageIdentity,
) -> Result<(), MacosScreenBridgeError> {
    let allocation_bytes = u64::try_from(iosurface.alloc_size())
        .map_err(|_| MacosScreenBridgeError::InvalidFrame("IOSurface allocation exceeds u64"))?;
    if iosurface.id() != expected.iosurface_id
        || iosurface.width() != expected.extent.width as usize
        || iosurface.height() != expected.extent.height as usize
        || iosurface.bytes_per_row() != expected.bytes_per_row
        || allocation_bytes != expected.allocation_bytes
    {
        return Err(MacosScreenBridgeError::InvalidFrame(
            "IOSurface physical descriptor changed after capture validation",
        ));
    }
    Ok(())
}
