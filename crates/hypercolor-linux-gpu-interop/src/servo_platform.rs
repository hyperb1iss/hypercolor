//! [`ServoRenderPlatform`] for Linux: a shared software GL device hosting
//! per-session offscreen targets, imported into wgpu through the pooled
//! external-memory blit.

use std::rc::Rc;

use anyhow::anyhow;
use hypercolor_gpu_frame::servo::{
    ServoGpuAdapterIdentity, ServoGpuFrameImporter, ServoGpuImportFailure, ServoGpuImportSession,
    ServoGpuImportSlotState, ServoRenderPlatform,
};
use servo::RenderingContext;

use crate::servo_context::{LinuxServoRenderDevice, LinuxServoRenderingContext};
use crate::{
    GlFramebufferSource, ImportedEffectFrame, ImportedFrameFormat,
    LinuxGlFramebufferImportDescriptor, LinuxGlFramebufferImporter,
};

/// Build the Linux Servo platform.
#[must_use]
pub fn servo_render_platform() -> Option<Box<dyn ServoRenderPlatform>> {
    Some(Box::new(LinuxServoPlatform::default()))
}

/// Linux Servo platform. Lazily creates one shared software GL device and
/// carves every session's offscreen render target out of it so imported
/// framebuffers share a context with wgpu's consumer.
#[derive(Default)]
pub struct LinuxServoPlatform {
    render_device: Option<Rc<LinuxServoRenderDevice>>,
}

impl ServoRenderPlatform for LinuxServoPlatform {
    fn name(&self) -> &'static str {
        "Linux"
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
        let parent = match self.render_device.as_ref() {
            Some(parent) => Rc::clone(parent),
            None => {
                let parent = Rc::new(
                    LinuxServoRenderDevice::new_software(width, height).map_err(|error| {
                        anyhow!(
                            "failed to create Linux Servo shared GL device ({width}x{height}): {error:?}"
                        )
                    })?,
                );
                self.render_device = Some(Rc::clone(&parent));
                parent
            }
        };
        let context = Rc::new(parent.create_rendering_context(width, height).map_err(|error| {
            anyhow!(
                "failed to create Linux Servo offscreen render target ({width}x{height}): {error:?}"
            )
        })?);
        let rendering_context: Rc<dyn RenderingContext> = context.clone();
        Ok(ServoGpuImportSession {
            rendering_context,
            importer: Box::new(LinuxServoFrameImporter {
                context,
                importer: None,
            }),
        })
    }
}

/// Imports a Linux offscreen target's framebuffer into wgpu.
struct LinuxServoFrameImporter {
    context: Rc<LinuxServoRenderingContext>,
    importer: Option<LinuxGlFramebufferImporter>,
}

impl LinuxServoFrameImporter {
    fn make_current(&self) -> anyhow::Result<()> {
        self.context
            .make_current()
            .map_err(|error| anyhow!("failed to make Servo GL context current: {error:?}"))?;
        self.context.prepare_for_rendering();
        Ok(())
    }

    fn ensure_importer(
        &mut self,
        device: &wgpu::Device,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> anyhow::Result<()> {
        let should_recreate = self
            .importer
            .as_ref()
            .is_none_or(|importer| importer.descriptor() != descriptor);
        if !should_recreate {
            return Ok(());
        }

        self.make_current()?;
        let gl = self.context.glow_gl_api();
        if let Some(importer) = self.importer.as_mut() {
            importer.destroy_gl_resources(gl.as_ref());
        }
        self.importer = None;
        self.importer = Some(LinuxGlFramebufferImporter::new_from_process(
            device,
            gl.as_ref(),
            descriptor,
        )?);
        Ok(())
    }
}

impl ServoGpuFrameImporter for LinuxServoFrameImporter {
    fn warm(&mut self, device: &wgpu::Device, width: u32, height: u32) -> anyhow::Result<()> {
        let descriptor = LinuxGlFramebufferImportDescriptor::new(
            width,
            height,
            ImportedFrameFormat::Rgba8Unorm,
        )?;
        self.ensure_importer(device, descriptor)
    }

    fn import_frame(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<ImportedEffectFrame, ServoGpuImportFailure> {
        let descriptor =
            LinuxGlFramebufferImportDescriptor::new(width, height, ImportedFrameFormat::Rgba8Unorm)
                .map_err(anyhow::Error::from)?;
        self.make_current()?;
        let linux_target = self.context.target_snapshot();
        let framebuffer = self.context.framebuffer().ok_or_else(|| {
            anyhow!("Linux Servo GPU import target did not expose a framebuffer: {linux_target:?}")
        })?;
        let source_framebuffer = GlFramebufferSource::Framebuffer(Some(framebuffer));
        let gl = self.context.glow_gl_api();

        self.ensure_importer(device, descriptor)?;
        let size = self.context.size();
        let importer = self
            .importer
            .as_mut()
            .ok_or_else(|| anyhow!("Servo GPU importer was not initialized"))?;
        match importer.import_framebuffer_pipelined(gl.as_ref(), source_framebuffer) {
            Ok(frame) => Ok(frame),
            Err(error) => {
                let importer_state = importer.state_snapshot();
                let blit_state =
                    importer.framebuffer_state_for_blit(gl.as_ref(), source_framebuffer);
                Err(ServoGpuImportFailure {
                    error: error.into(),
                    diagnostics: Some(format!(
                        "context_size={}x{} source_framebuffer={source_framebuffer:?} target={linux_target:?} importer={importer_state:?} blit={blit_state:?}",
                        size.width, size.height
                    )),
                })
            }
        }
    }

    fn clear(&mut self) {
        let Some(mut importer) = self.importer.take() else {
            return;
        };
        if self.context.make_current().is_err() {
            // Without a current context the GL names cannot be deleted; the
            // context teardown reclaims them.
            return;
        }
        let gl = self.context.glow_gl_api();
        importer.destroy_gl_resources(gl.as_ref());
    }

    fn slot_state(&self) -> Option<ServoGpuImportSlotState> {
        let snapshot = self.importer.as_ref()?.state_snapshot();
        Some(ServoGpuImportSlotState {
            slot_count: snapshot.slot_count,
            pending_slots: snapshot.pending_slots,
            completed_slots: snapshot.completed_slots,
            available_slots: snapshot.available_slots,
            oldest_pending_age_ms: snapshot.oldest_pending_age_ms,
        })
    }
}
