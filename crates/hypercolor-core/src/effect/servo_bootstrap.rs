//! Servo rendering-context bootstrap.
//!
//! The worker asks one [`ServoRenderPlatform`] (selected by chaining the GPU
//! interop crates' stub-everywhere constructors) for a rendering context and
//! applies the GPU-import policy here: attempt a GPU-importable context when
//! the mode allows it, fail hard when the mode requires it, and otherwise fall
//! back to the platform's CPU context or Servo's portable software context.

use std::rc::Rc;

use anyhow::{Result, anyhow};
use dpi::PhysicalSize;
use hypercolor_gpu_frame::servo::ServoRenderPlatform;
#[cfg(feature = "servo-gpu-import")]
use hypercolor_gpu_frame::servo::{ServoGpuAdapterIdentity, ServoGpuFrameImporter};
use servo::{RenderingContext, SoftwareRenderingContext};
#[cfg(feature = "servo-gpu-import")]
use tracing::warn;

/// A rendering context for one Servo session, plus the importer that reads
/// its frames back into wgpu when the context supports GPU import.
pub(crate) struct ServoRenderingContextHandle {
    pub(crate) rendering_context: Rc<dyn RenderingContext>,
    #[cfg(feature = "servo-gpu-import")]
    pub(crate) gpu_importer: Option<Box<dyn ServoGpuFrameImporter>>,
}

impl ServoRenderingContextHandle {
    fn cpu(rendering_context: Rc<dyn RenderingContext>) -> Self {
        Self {
            rendering_context,
            #[cfg(feature = "servo-gpu-import")]
            gpu_importer: None,
        }
    }
}

/// The platform selected for this worker thread, if any.
///
/// Platforms hold per-thread state (a shared GL device, hidden-window
/// keepalives), so one host lives for the life of the Servo worker.
pub(crate) struct ServoPlatformHost {
    platform: Option<Box<dyn ServoRenderPlatform>>,
}

impl ServoPlatformHost {
    /// Select the platform for the current host by asking each interop crate
    /// in turn; every constructor returns `None` off its operating system.
    pub(crate) fn select() -> Self {
        let platform = None
            .or_else(hypercolor_windows_gpu_interop::servo_render_platform)
            .or_else(select_gpu_import_platform);
        Self { platform }
    }

    /// Create the rendering context for a new session.
    pub(crate) fn create_rendering_context(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<ServoRenderingContextHandle> {
        #[cfg(feature = "servo-gpu-import")]
        if crate::effect::servo_gpu_import_should_attempt()
            && let Some(handle) = self.try_create_gpu_import_context(width, height)?
        {
            return Ok(handle);
        }

        self.create_cpu_rendering_context(width, height)
    }

    fn create_cpu_rendering_context(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<ServoRenderingContextHandle> {
        if let Some(platform) = self.platform.as_mut()
            && let Some(rendering_context) = platform.create_cpu_rendering_context(width, height)?
        {
            return Ok(ServoRenderingContextHandle::cpu(rendering_context));
        }
        bootstrap_software_rendering_context_handle(width, height)
    }

    /// Attempt a GPU-importable context. `Ok(None)` means the platform has
    /// no GPU path here (or it failed in `Auto` mode) and the caller should
    /// fall back to a CPU context.
    #[cfg(feature = "servo-gpu-import")]
    fn try_create_gpu_import_context(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Option<ServoRenderingContextHandle>> {
        let required = matches!(
            crate::effect::servo_gpu_import_mode(),
            hypercolor_types::config::ServoGpuImportMode::On
        );
        let Some(platform) = self.platform.as_mut() else {
            if required {
                return Err(anyhow!(
                    "Servo GPU import is required but this host has no Servo GPU platform"
                ));
            }
            return Ok(None);
        };
        let adapter = crate::effect::servo_gpu_import_adapter_info().map(|adapter_info| {
            ServoGpuAdapterIdentity {
                vendor_id: adapter_info.vendor_id,
                device_id: adapter_info.device_id,
            }
        });
        match platform.create_gpu_import_session(width, height, adapter) {
            Ok(session) => Ok(Some(ServoRenderingContextHandle {
                rendering_context: session.rendering_context,
                gpu_importer: Some(session.importer),
            })),
            Err(error) if required => Err(anyhow!(
                "failed to create required {} Servo GPU import context: {error:#}",
                platform.name()
            )),
            Err(error) => {
                warn!(
                    %error,
                    platform = platform.name(),
                    "Servo GPU import context unavailable; using CPU context"
                );
                Ok(None)
            }
        }
    }
}

#[cfg(feature = "servo-gpu-import")]
fn select_gpu_import_platform() -> Option<Box<dyn ServoRenderPlatform>> {
    None.or_else(hypercolor_linux_gpu_interop::servo_render_platform)
        .or_else(hypercolor_macos_gpu_interop::servo_render_platform)
}

#[cfg(not(feature = "servo-gpu-import"))]
fn select_gpu_import_platform() -> Option<Box<dyn ServoRenderPlatform>> {
    None
}

/// Create a headless Servo software rendering context.
///
/// This is the portable CPU-readback path every platform can fall back to.
///
/// # Errors
///
/// Returns an error if the software OpenGL adapter/context cannot be created.
pub fn bootstrap_software_rendering_context(
    width: u32,
    height: u32,
) -> Result<SoftwareRenderingContext> {
    SoftwareRenderingContext::new(PhysicalSize::new(width, height)).map_err(|error| {
        anyhow!("failed to create Servo SoftwareRenderingContext ({width}x{height}): {error:?}")
    })
}

fn bootstrap_software_rendering_context_handle(
    width: u32,
    height: u32,
) -> Result<ServoRenderingContextHandle> {
    Ok(ServoRenderingContextHandle::cpu(Rc::new(
        bootstrap_software_rendering_context(width, height)?,
    )))
}
