//! [`ServoRenderPlatform`] for Windows: a hidden-window context for CPU
//! readback and, with `servo-context`, an ANGLE context publishing into a
//! D3D11 shared-texture ring for GPU import.

use std::rc::Rc;

use hypercolor_gpu_frame::servo::{
    ServoGpuAdapterIdentity, ServoGpuImportSession, ServoRenderPlatform,
};
use servo::RenderingContext;

use crate::servo_window::hidden_window_rendering_context;

/// Build the Windows Servo platform.
#[must_use]
pub fn servo_render_platform() -> Option<Box<dyn ServoRenderPlatform>> {
    Some(Box::new(WindowsServoPlatform))
}

/// Windows Servo platform.
#[derive(Debug, Default)]
pub struct WindowsServoPlatform;

impl ServoRenderPlatform for WindowsServoPlatform {
    fn name(&self) -> &'static str {
        "Windows"
    }

    fn create_cpu_rendering_context(
        &mut self,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Option<Rc<dyn RenderingContext>>> {
        hidden_window_rendering_context(width, height).map(Some)
    }

    #[cfg(feature = "servo-context")]
    fn create_gpu_import_session(
        &mut self,
        width: u32,
        height: u32,
        adapter: Option<ServoGpuAdapterIdentity>,
    ) -> anyhow::Result<ServoGpuImportSession> {
        gpu_import::create_session(width, height, adapter)
    }

    #[cfg(not(feature = "servo-context"))]
    fn create_gpu_import_session(
        &mut self,
        _width: u32,
        _height: u32,
        _adapter: Option<ServoGpuAdapterIdentity>,
    ) -> anyhow::Result<ServoGpuImportSession> {
        anyhow::bail!("Windows Servo GPU import support is not compiled in")
    }
}

#[cfg(feature = "servo-context")]
mod gpu_import {
    use std::rc::Rc;

    use anyhow::anyhow;
    use hypercolor_gpu_frame::servo::{
        ServoGpuAdapterIdentity, ServoGpuFrameImporter, ServoGpuImportFailure,
        ServoGpuImportSession,
    };
    use servo::RenderingContext;

    use crate::servo_context::{
        WindowsAngleRenderingContext, WindowsDxgiAdapterIdentity, WindowsServoNativeFrame,
    };
    use crate::{
        ImportedEffectFrame, ImportedFrameFormat, WindowsD3d11SharedTextureImportDescriptor,
        WindowsD3d11SharedTextureImporter, WindowsGpuInteropError,
    };

    pub(super) fn create_session(
        width: u32,
        height: u32,
        adapter: Option<ServoGpuAdapterIdentity>,
    ) -> anyhow::Result<ServoGpuImportSession> {
        let adapter_identity = adapter.map(|adapter| WindowsDxgiAdapterIdentity {
            vendor_id: adapter.vendor_id,
            device_id: adapter.device_id,
        });
        let context = Rc::new(WindowsAngleRenderingContext::new(
            width,
            height,
            adapter_identity,
        )?);
        let rendering_context: Rc<dyn RenderingContext> = context.clone();
        Ok(ServoGpuImportSession {
            rendering_context,
            importer: Box::new(WindowsServoFrameImporter {
                context,
                importer: None,
            }),
        })
    }

    /// Imports the ANGLE context's shared-texture frames into wgpu.
    struct WindowsServoFrameImporter {
        context: Rc<WindowsAngleRenderingContext>,
        importer: Option<WindowsD3d11SharedTextureImporter>,
    }

    impl WindowsServoFrameImporter {
        fn native_frame(&self) -> Result<WindowsServoNativeFrame, WindowsGpuInteropError> {
            self.context.native_frame()
        }

        fn ensure_importer(
            &mut self,
            device: &wgpu::Device,
            descriptor: WindowsD3d11SharedTextureImportDescriptor,
        ) -> anyhow::Result<()> {
            let should_recreate = self
                .importer
                .as_ref()
                .is_none_or(|importer| importer.descriptor() != descriptor);
            if !should_recreate {
                return Ok(());
            }
            self.importer = Some(WindowsD3d11SharedTextureImporter::new(device, descriptor)?);
            Ok(())
        }
    }

    impl ServoGpuFrameImporter for WindowsServoFrameImporter {
        fn warm(&mut self, device: &wgpu::Device, width: u32, height: u32) -> anyhow::Result<()> {
            let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
                width,
                height,
                ImportedFrameFormat::Bgra8Unorm,
            )?;
            self.ensure_importer(device, descriptor)
        }

        fn import_frame(
            &mut self,
            device: &wgpu::Device,
            _width: u32,
            _height: u32,
        ) -> Result<ImportedEffectFrame, ServoGpuImportFailure> {
            let native_frame = self.native_frame().map_err(anyhow::Error::from)?;
            let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
                native_frame.width,
                native_frame.height,
                native_frame.format,
            )
            .map_err(anyhow::Error::from)?;
            self.ensure_importer(device, descriptor)?;
            let importer = self
                .importer
                .as_mut()
                .ok_or_else(|| anyhow!("Servo GPU importer was not initialized"))?;
            importer
                .import_servo_native_frame(device, native_frame)
                .map_err(|error| anyhow::Error::from(error).into())
        }

        fn clear(&mut self) {
            self.importer = None;
        }
    }
}
