//! [`ServoRenderPlatform`] for macOS: a hardware OpenGL context rendering
//! into an IOSurface ring that Metal wraps without a CPU readback.

use std::rc::Rc;

use anyhow::anyhow;
use hypercolor_gpu_frame::servo::{
    ServoGpuAdapterIdentity, ServoGpuFrameImporter, ServoGpuImportFailure, ServoGpuImportSession,
    ServoRenderPlatform,
};
use paint_api::rendering_context::RenderingContext;

use crate::servo_context::{MacosHardwareRenderingContext, MacosServoNativeFrame};
use crate::{
    ImportedEffectFrame, MacosGpuInteropError, MacosIosurfaceImportDescriptor,
    MacosIosurfaceImporter,
};

/// Build the macOS Servo platform.
#[must_use]
pub fn servo_render_platform() -> Option<Box<dyn ServoRenderPlatform>> {
    Some(Box::new(MacosServoPlatform))
}

/// macOS Servo platform. Software rendering uses Servo's portable context;
/// GPU import uses the IOSurface-backed hardware context.
#[derive(Debug, Default)]
pub struct MacosServoPlatform;

impl ServoRenderPlatform for MacosServoPlatform {
    fn name(&self) -> &'static str {
        "macOS"
    }

    fn create_cpu_rendering_context(
        &mut self,
        _width: u32,
        _height: u32,
    ) -> anyhow::Result<Option<Rc<dyn RenderingContext>>> {
        Ok(None)
    }

    fn create_gpu_import_session(
        &mut self,
        width: u32,
        height: u32,
        _adapter: Option<ServoGpuAdapterIdentity>,
    ) -> anyhow::Result<ServoGpuImportSession> {
        let context = Rc::new(MacosHardwareRenderingContext::new(width, height)?);
        let rendering_context: Rc<dyn RenderingContext> = context.clone();
        Ok(ServoGpuImportSession {
            rendering_context,
            importer: Box::new(MacosServoFrameImporter {
                context,
                importer: None,
            }),
        })
    }
}

/// Imports the hardware context's IOSurface frames into wgpu.
struct MacosServoFrameImporter {
    context: Rc<MacosHardwareRenderingContext>,
    importer: Option<MacosIosurfaceImporter>,
}

impl MacosServoFrameImporter {
    fn native_frame(&self) -> Result<MacosServoNativeFrame, MacosGpuInteropError> {
        self.context.native_frame()
    }

    fn ensure_importer(
        &mut self,
        device: &wgpu::Device,
        native_frame: &MacosServoNativeFrame,
    ) -> anyhow::Result<()> {
        let descriptor = MacosIosurfaceImportDescriptor::new(
            native_frame.width,
            native_frame.height,
            native_frame.format,
        )?;
        let should_recreate = self
            .importer
            .as_ref()
            .is_none_or(|importer| importer.descriptor() != descriptor);
        if !should_recreate {
            return Ok(());
        }
        self.importer = Some(MacosIosurfaceImporter::new(device, descriptor)?);
        Ok(())
    }
}

impl ServoGpuFrameImporter for MacosServoFrameImporter {
    fn warm(&mut self, device: &wgpu::Device, _width: u32, _height: u32) -> anyhow::Result<()> {
        let native_frame = self.native_frame()?;
        self.ensure_importer(device, &native_frame)
    }

    fn import_frame(
        &mut self,
        device: &wgpu::Device,
        _width: u32,
        _height: u32,
    ) -> Result<ImportedEffectFrame, ServoGpuImportFailure> {
        let native_frame = self.native_frame().map_err(anyhow::Error::from)?;
        self.ensure_importer(device, &native_frame)?;
        let importer = self
            .importer
            .as_mut()
            .ok_or_else(|| anyhow!("Servo GPU importer was not initialized"))?;
        importer
            .import_iosurface(
                device,
                &native_frame.iosurface,
                native_frame.content_generation,
                native_frame.origin,
            )
            .map_err(|error| anyhow::Error::from(error).into())
    }

    fn clear(&mut self) {
        self.importer = None;
    }

    fn native_surface_summary(&self) -> Option<String> {
        Some(match self.native_frame() {
            Ok(frame) => format!(
                "IOSurface FBO {}x{} surface_id={} content_generation={} format={:?} origin={:?}",
                frame.width,
                frame.height,
                frame.surface_id,
                frame.content_generation,
                frame.format,
                frame.origin
            ),
            Err(error) => format!("IOSurface diagnostics unavailable: {error}"),
        })
    }
}
