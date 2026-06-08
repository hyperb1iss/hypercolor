use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Error, Result, anyhow};
use servo::RenderingContext;
use tracing::debug;

use super::telemetry::{
    ServoGpuImportFallbackReason, record_servo_gpu_import_frame, record_servo_gpu_import_slot_state,
};
use super::worker_client::ServoSessionId;
use crate::effect::servo_bootstrap::ServoRenderingContextHandle;
use crate::effect::traits::ImportedEffectFrame;

const SERVO_GPU_IMPORT_TRANSIENT_RETRY: Duration = Duration::from_millis(250);

#[derive(Debug, thiserror::Error)]
#[error(
    "Servo GPU framebuffer import is temporarily unavailable: {reason} ({detail}); retry in {retry_ms}ms"
)]
pub(super) struct ServoFrameUnavailable {
    reason: &'static str,
    detail: String,
    retry_ms: u64,
}

impl ServoFrameUnavailable {
    pub(super) const fn new(reason: &'static str, detail: String, retry_ms: u64) -> Self {
        Self {
            reason,
            detail,
            retry_ms,
        }
    }

    pub(super) const fn reason(&self) -> &'static str {
        self.reason
    }

    pub(super) fn detail(&self) -> &str {
        &self.detail
    }

    pub(super) const fn retry_ms(&self) -> u64 {
        self.retry_ms
    }
}

pub(super) struct ServoGpuImportBackend {
    #[cfg(target_os = "linux")]
    linux_context: Option<Rc<hypercolor_linux_gpu_interop::LinuxServoRenderingContext>>,
    #[cfg(target_os = "macos")]
    macos_hardware_context: Option<Rc<hypercolor_macos_gpu_interop::MacosHardwareRenderingContext>>,
    #[cfg(target_os = "windows")]
    windows_angle_context: Option<Rc<hypercolor_windows_gpu_interop::WindowsAngleRenderingContext>>,
    #[cfg(target_os = "linux")]
    importer: Option<hypercolor_linux_gpu_interop::LinuxGlFramebufferImporter>,
    #[cfg(target_os = "macos")]
    importer: Option<hypercolor_macos_gpu_interop::MacosIosurfaceImporter>,
    #[cfg(target_os = "windows")]
    importer: Option<hypercolor_windows_gpu_interop::WindowsD3d11SharedTextureImporter>,
    retry_after: Option<Instant>,
    transient_failures: u32,
    last_frame: Option<ImportedEffectFrame>,
}

pub(super) struct ServoGpuImportSessionContext<'a> {
    pub(super) session_id: ServoSessionId,
    pub(super) rendering_context: &'a Rc<dyn RenderingContext>,
    pub(super) loaded_html_path: Option<&'a Path>,
    pub(super) loaded_at: Option<Instant>,
    pub(super) renders_since_load: u64,
}

impl ServoGpuImportBackend {
    pub(super) fn new(handle: &mut ServoRenderingContextHandle) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            linux_context: handle.linux_context.take(),
            #[cfg(target_os = "macos")]
            macos_hardware_context: handle.macos_hardware_context.take(),
            #[cfg(target_os = "windows")]
            windows_angle_context: handle.windows_angle_context.take(),
            #[cfg(target_os = "linux")]
            importer: None,
            #[cfg(target_os = "macos")]
            importer: None,
            #[cfg(target_os = "windows")]
            importer: None,
            retry_after: None,
            transient_failures: 0,
            last_frame: None,
        }
    }

    pub(super) fn warm_if_available(
        &mut self,
        rendering_context: &Rc<dyn RenderingContext>,
        width: u32,
        height: u32,
    ) {
        if !super::servo_gpu_import_should_attempt() {
            return;
        }
        let Ok(device) = super::gpu_import::servo_gpu_import_device() else {
            return;
        };
        if let Err(error) = self.warm_platform_importer(rendering_context, device, width, height) {
            debug!(%error, "Servo GPU import pool warmup skipped");
        }
    }

    pub(super) fn import_frame(
        &mut self,
        context: ServoGpuImportSessionContext<'_>,
        width: u32,
        height: u32,
    ) -> Result<ImportedEffectFrame> {
        let device = super::gpu_import::servo_gpu_import_device()?;
        self.import_platform_frame(context, device, width, height)
    }

    pub(super) fn clear_importer(&mut self, rendering_context: &Rc<dyn RenderingContext>) {
        self.clear_platform_importer(rendering_context);
        self.last_frame = None;
    }

    pub(super) fn reset_retry_state(&mut self) {
        self.retry_after = None;
        self.transient_failures = 0;
        self.last_frame = None;
    }

    pub(super) fn retry_delay(&self, now: Instant) -> Option<Duration> {
        self.retry_after
            .and_then(|retry_after| retry_after.checked_duration_since(now))
    }

    pub(super) fn schedule_transient_retry(&mut self) -> u64 {
        self.retry_after = Some(Instant::now() + SERVO_GPU_IMPORT_TRANSIENT_RETRY);
        self.transient_failures = self.transient_failures.saturating_add(1);
        duration_millis_u64(SERVO_GPU_IMPORT_TRANSIENT_RETRY)
    }

    pub(super) fn transient_failures(&self) -> u32 {
        self.transient_failures
    }

    pub(super) fn note_success(&mut self) {
        self.retry_after = None;
        self.transient_failures = 0;
    }

    pub(super) fn cached_frame(&self) -> Option<&ImportedEffectFrame> {
        self.last_frame.as_ref()
    }

    pub(super) fn store_frame(&mut self, frame: ImportedEffectFrame) {
        self.last_frame = Some(frame);
    }

    pub(super) fn clear_cached_frame(&mut self) {
        self.last_frame = None;
    }

    #[cfg(target_os = "macos")]
    pub(super) fn trace_macos_native_surface(&self, session_id: ServoSessionId) {
        let Some(context) = self.macos_hardware_context.as_ref() else {
            return;
        };
        match context.native_frame() {
            Ok(frame) => {
                debug!(
                    width = frame.width,
                    height = frame.height,
                    surface_id = frame.surface_id,
                    format = ?frame.format,
                    origin = ?frame.origin,
                    "macOS Servo hardware context exposes IOSurface"
                );
            }
            Err(error) => {
                debug!(%error, ?session_id, "macOS Servo IOSurface diagnostics unavailable");
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn refresh_linux_surface_after_peer_destroy(
        &mut self,
        destroyed_session_id: ServoSessionId,
        session_id: ServoSessionId,
        rendering_context: &Rc<dyn RenderingContext>,
    ) -> Result<()> {
        let Some(linux_context) = self.linux_context.clone() else {
            return Ok(());
        };
        let before_surface = linux_context.surface_snapshot();

        linux_context
            .refresh_surface()
            .map_err(|error| anyhow!("failed to refresh Linux Servo surface: {error:?}"))?;
        rendering_context.prepare_for_rendering();
        self.clear_platform_importer(rendering_context);
        self.reset_retry_state();
        rendering_context.prepare_for_rendering();
        let after_surface = linux_context.surface_snapshot();
        let size = rendering_context.size();
        debug!(
            ?destroyed_session_id,
            ?session_id,
            width = size.width,
            height = size.height,
            ?before_surface,
            ?after_surface,
            "Refreshed surviving Servo Linux surface after peer session teardown"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn warm_platform_importer(
        &mut self,
        rendering_context: &Rc<dyn RenderingContext>,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let descriptor = hypercolor_linux_gpu_interop::LinuxGlFramebufferImportDescriptor::new(
            width,
            height,
            hypercolor_linux_gpu_interop::ImportedFrameFormat::Rgba8Unorm,
        )?;
        self.ensure_linux_importer(rendering_context, device, descriptor)
    }

    #[cfg(target_os = "windows")]
    fn warm_platform_importer(
        &mut self,
        _rendering_context: &Rc<dyn RenderingContext>,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let descriptor =
            hypercolor_windows_gpu_interop::WindowsD3d11SharedTextureImportDescriptor::new(
                width,
                height,
                hypercolor_windows_gpu_interop::ImportedFrameFormat::Bgra8Unorm,
            )?;
        self.ensure_windows_importer(device, descriptor)
    }

    #[cfg(target_os = "macos")]
    fn warm_platform_importer(
        &mut self,
        _rendering_context: &Rc<dyn RenderingContext>,
        device: &wgpu::Device,
        _width: u32,
        _height: u32,
    ) -> Result<()> {
        let native_frame = self.macos_native_frame()?;
        let descriptor = hypercolor_macos_gpu_interop::MacosIosurfaceImportDescriptor::new(
            native_frame.width,
            native_frame.height,
            native_frame.format,
        )?;
        self.ensure_macos_importer(device, descriptor)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    fn warm_platform_importer(
        &mut self,
        _rendering_context: &Rc<dyn RenderingContext>,
        _device: &wgpu::Device,
        _width: u32,
        _height: u32,
    ) -> Result<()> {
        anyhow::bail!("Servo GPU import is unsupported on this platform")
    }

    #[cfg(target_os = "linux")]
    fn import_platform_frame(
        &mut self,
        context: ServoGpuImportSessionContext<'_>,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<ImportedEffectFrame> {
        use hypercolor_linux_gpu_interop::{
            GlFramebufferSource, ImportedFrameFormat, LinuxGlFramebufferImportDescriptor,
        };

        let descriptor = LinuxGlFramebufferImportDescriptor::new(
            width,
            height,
            ImportedFrameFormat::Rgba8Unorm,
        )?;
        context
            .rendering_context
            .make_current()
            .map_err(|error| anyhow!("failed to make Servo GL context current: {error:?}"))?;
        context.rendering_context.prepare_for_rendering();
        let linux_context = self
            .linux_context
            .as_ref()
            .ok_or_else(|| anyhow!("Linux Servo GPU import context is unavailable"))?;
        let linux_surface = linux_context.surface_snapshot();
        let framebuffer = linux_context.framebuffer().ok_or_else(|| {
            anyhow!(
                "Linux Servo GPU import surface did not expose a framebuffer: {linux_surface:?}"
            )
        })?;
        let source_framebuffer = GlFramebufferSource::Framebuffer(Some(framebuffer));
        let gl = context.rendering_context.glow_gl_api();

        if let Err(error) =
            self.ensure_linux_importer(context.rendering_context, device, descriptor)
        {
            let loaded_html_path = context
                .loaded_html_path
                .map_or_else(String::new, |path| path.display().to_string());
            debug!(
                %error,
                ?context.session_id,
                width = descriptor.width,
                height = descriptor.height,
                loaded_html_path,
                "Servo GPU importer setup failed"
            );
            return Err(error);
        }
        let size = context.rendering_context.size();
        let importer = self
            .importer
            .as_mut()
            .ok_or_else(|| anyhow!("Servo GPU importer was not initialized"))?;
        let importer_before = importer.state_snapshot();
        record_linux_gpu_importer_state(importer_before);
        let blit_before = importer.framebuffer_state_for_blit(gl.as_ref(), source_framebuffer);
        let import_result = importer.import_framebuffer_pipelined(gl.as_ref(), source_framebuffer);
        record_linux_gpu_importer_state(importer.state_snapshot());
        if let Err(error) = import_result.as_ref() {
            let importer_after = importer.state_snapshot();
            record_linux_gpu_importer_state(importer_after);
            let blit_after = importer.framebuffer_state_for_blit(gl.as_ref(), source_framebuffer);
            let loaded_html_path = context
                .loaded_html_path
                .map_or_else(String::new, |path| path.display().to_string());
            debug!(
                %error,
                ?context.session_id,
                width = descriptor.width,
                height = descriptor.height,
                context_width = size.width,
                context_height = size.height,
                loaded_html_path,
                page_age_ms = context
                    .loaded_at
                    .map(|loaded_at| duration_millis_u64(loaded_at.elapsed())),
                renders_since_load = context.renders_since_load,
                ?source_framebuffer,
                ?linux_surface,
                ?importer_before,
                ?importer_after,
                ?blit_before,
                ?blit_after,
                "Servo GPU import GL diagnostics"
            );
        }
        Ok(import_result?)
    }

    #[cfg(target_os = "windows")]
    fn import_platform_frame(
        &mut self,
        _context: ServoGpuImportSessionContext<'_>,
        device: &wgpu::Device,
        _width: u32,
        _height: u32,
    ) -> Result<ImportedEffectFrame> {
        let context = self.windows_angle_context()?;
        let Some(native_frame) = context.publish_current_frame()? else {
            return Err(ServoFrameUnavailable::new(
                "windows_import_warmup",
                "Windows ANGLE fence ring has no completed frame yet".to_owned(),
                0,
            )
            .into());
        };
        let descriptor =
            hypercolor_windows_gpu_interop::WindowsD3d11SharedTextureImportDescriptor::new(
                native_frame.width,
                native_frame.height,
                native_frame.format,
            )?;
        self.ensure_windows_importer(device, descriptor)?;
        let importer = self
            .importer
            .as_mut()
            .ok_or_else(|| anyhow!("Servo GPU importer was not initialized"))?;
        Ok(importer.import_servo_native_frame(device, native_frame)?)
    }

    #[cfg(target_os = "macos")]
    fn import_platform_frame(
        &mut self,
        _context: ServoGpuImportSessionContext<'_>,
        device: &wgpu::Device,
        _width: u32,
        _height: u32,
    ) -> Result<ImportedEffectFrame> {
        let native_frame = self.macos_native_frame()?;
        let descriptor = hypercolor_macos_gpu_interop::MacosIosurfaceImportDescriptor::new(
            native_frame.width,
            native_frame.height,
            native_frame.format,
        )?;
        self.ensure_macos_importer(device, descriptor)?;
        let importer = self
            .importer
            .as_mut()
            .ok_or_else(|| anyhow!("Servo GPU importer was not initialized"))?;
        Ok(importer.import_iosurface(device, &native_frame.iosurface)?)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    fn import_platform_frame(
        &mut self,
        _context: ServoGpuImportSessionContext<'_>,
        _device: &wgpu::Device,
        _width: u32,
        _height: u32,
    ) -> Result<ImportedEffectFrame> {
        anyhow::bail!("Servo GPU import is unsupported on this platform")
    }

    #[cfg(target_os = "linux")]
    fn ensure_linux_importer(
        &mut self,
        rendering_context: &Rc<dyn RenderingContext>,
        device: &wgpu::Device,
        descriptor: hypercolor_linux_gpu_interop::LinuxGlFramebufferImportDescriptor,
    ) -> Result<()> {
        let should_recreate = self
            .importer
            .as_ref()
            .is_none_or(|importer| importer.descriptor() != descriptor);
        if !should_recreate {
            return Ok(());
        }

        rendering_context
            .make_current()
            .map_err(|error| anyhow!("failed to make Servo GL context current: {error:?}"))?;
        rendering_context.prepare_for_rendering();
        let gl = rendering_context.glow_gl_api();

        if let Some(importer) = self.importer.as_mut() {
            importer.destroy_gl_resources(gl.as_ref());
        }
        self.importer = None;
        self.last_frame = None;
        let importer = hypercolor_linux_gpu_interop::LinuxGlFramebufferImporter::new_from_process(
            device,
            gl.as_ref(),
            descriptor,
        )?;
        self.importer = Some(importer);

        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn ensure_windows_importer(
        &mut self,
        device: &wgpu::Device,
        descriptor: hypercolor_windows_gpu_interop::WindowsD3d11SharedTextureImportDescriptor,
    ) -> Result<()> {
        let should_recreate = self
            .importer
            .as_ref()
            .is_none_or(|importer| importer.descriptor() != descriptor);
        if !should_recreate {
            return Ok(());
        }

        self.importer = Some(
            hypercolor_windows_gpu_interop::WindowsD3d11SharedTextureImporter::new(
                device, descriptor,
            )?,
        );
        self.last_frame = None;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn ensure_macos_importer(
        &mut self,
        device: &wgpu::Device,
        descriptor: hypercolor_macos_gpu_interop::MacosIosurfaceImportDescriptor,
    ) -> Result<()> {
        let should_recreate = self
            .importer
            .as_ref()
            .is_none_or(|importer| importer.descriptor() != descriptor);
        if !should_recreate {
            return Ok(());
        }

        self.importer = Some(hypercolor_macos_gpu_interop::MacosIosurfaceImporter::new(
            device, descriptor,
        )?);
        self.last_frame = None;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn windows_angle_context(
        &self,
    ) -> Result<Rc<hypercolor_windows_gpu_interop::WindowsAngleRenderingContext>> {
        self.windows_angle_context.as_ref().cloned().ok_or_else(|| {
            hypercolor_windows_gpu_interop::WindowsGpuInteropError::MissingWindowsAngleContext
                .into()
        })
    }

    #[cfg(target_os = "macos")]
    fn macos_native_frame(&self) -> Result<hypercolor_macos_gpu_interop::MacosServoNativeFrame> {
        let context = self
            .macos_hardware_context
            .as_ref()
            .ok_or(hypercolor_macos_gpu_interop::MacosGpuInteropError::MissingServoSurface)?;
        Ok(context.native_frame()?)
    }

    #[cfg(target_os = "linux")]
    fn clear_platform_importer(&mut self, rendering_context: &Rc<dyn RenderingContext>) {
        if let Err(error) = rendering_context.make_current() {
            debug!(?error, "Servo GPU import pool cleanup skipped");
            return;
        }
        let gl = rendering_context.glow_gl_api();
        if let Some(importer) = self.importer.as_mut() {
            importer.destroy_gl_resources(gl.as_ref());
        }
        self.importer = None;
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn clear_platform_importer(&mut self, _rendering_context: &Rc<dyn RenderingContext>) {
        self.importer = None;
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    fn clear_platform_importer(&mut self, _rendering_context: &Rc<dyn RenderingContext>) {}
}

#[cfg(target_os = "linux")]
fn record_linux_gpu_importer_state(
    snapshot: hypercolor_linux_gpu_interop::LinuxGlImporterStateSnapshot,
) {
    record_servo_gpu_import_slot_state(
        snapshot.slot_count,
        snapshot.pending_slots,
        snapshot.completed_slots,
        snapshot.available_slots,
        snapshot.oldest_pending_age_ms,
    );
}

pub(super) fn record_imported_frame(frame: &ImportedEffectFrame) {
    #[cfg(target_os = "linux")]
    {
        record_servo_gpu_import_frame(
            frame.timings.blit_us,
            frame.timings.sync_us,
            frame.timings.total_us,
        );
    }
    #[cfg(target_os = "macos")]
    {
        record_servo_gpu_import_frame(frame.timings.wrap_us, 0, frame.timings.total_us);
    }
    #[cfg(target_os = "windows")]
    {
        record_servo_gpu_import_frame(
            frame.timings.wrap_us,
            frame.timings.sync_us,
            frame.timings.total_us,
        );
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        record_servo_gpu_import_frame(0, 0, frame.timings.total_us);
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

pub(super) fn failure_is_transient(reason: ServoGpuImportFallbackReason) -> bool {
    matches!(
        reason,
        ServoGpuImportFallbackReason::GlOperation
            | ServoGpuImportFallbackReason::GlFramebufferIncomplete
            | ServoGpuImportFallbackReason::ImportSlotsExhausted
            | ServoGpuImportFallbackReason::MissingMacosServoSurface
            | ServoGpuImportFallbackReason::WindowsImportStaleFrame
    )
}

pub(super) fn failure_should_clear_importer(reason: ServoGpuImportFallbackReason) -> bool {
    !failure_is_transient(reason)
}

pub(super) fn failure_detail(error: &Error) -> String {
    for cause in error.chain() {
        #[cfg(target_os = "linux")]
        if let Some(error) =
            cause.downcast_ref::<hypercolor_linux_gpu_interop::LinuxGpuInteropError>()
        {
            return match error {
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::GlOperation {
                    operation,
                    code,
                } => format!("gl_operation={operation} gl_error=0x{code:04x}"),
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::GlFramebufferIncomplete {
                    status,
                } => format!("gl_framebuffer_status=0x{status:04x}"),
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::ImportSlotsExhausted {
                    slot_count,
                } => format!("import_slots_exhausted={slot_count}"),
                _ => error.to_string(),
            };
        }

        #[cfg(target_os = "macos")]
        if let Some(error) =
            cause.downcast_ref::<hypercolor_macos_gpu_interop::MacosGpuInteropError>()
        {
            return error.to_string();
        }

        #[cfg(target_os = "windows")]
        if let Some(error) =
            cause.downcast_ref::<hypercolor_windows_gpu_interop::WindowsGpuInteropError>()
        {
            return error.to_string();
        }
    }

    error.to_string()
}

pub(super) fn classify_failure(error: &Error) -> ServoGpuImportFallbackReason {
    for cause in error.chain() {
        #[cfg(target_os = "macos")]
        if let Some(error) =
            cause.downcast_ref::<hypercolor_macos_gpu_interop::MacosGpuInteropError>()
        {
            return match error {
                hypercolor_macos_gpu_interop::MacosGpuInteropError::MissingWgpuMetalDevice => {
                    ServoGpuImportFallbackReason::MissingWgpuMetalDevice
                }
                hypercolor_macos_gpu_interop::MacosGpuInteropError::InvalidDimensions { .. }
                | hypercolor_macos_gpu_interop::MacosGpuInteropError::IosurfaceShapeMismatch {
                    ..
                }
                | hypercolor_macos_gpu_interop::MacosGpuInteropError::PixelBufferSizeMismatch {
                    ..
                } => ServoGpuImportFallbackReason::InvalidDimensions,
                hypercolor_macos_gpu_interop::MacosGpuInteropError::ServoContext { .. }
                | hypercolor_macos_gpu_interop::MacosGpuInteropError::MissingServoSurface => {
                    ServoGpuImportFallbackReason::MissingMacosServoSurface
                }
                hypercolor_macos_gpu_interop::MacosGpuInteropError::IosurfacePixelFormatMismatch {
                    ..
                } => ServoGpuImportFallbackReason::IosurfacePixelFormatMismatch,
                hypercolor_macos_gpu_interop::MacosGpuInteropError::MetalTextureCreateFailed => {
                    ServoGpuImportFallbackReason::MetalTextureCreateFailed
                }
                _ => ServoGpuImportFallbackReason::Other,
            };
        }

        #[cfg(target_os = "windows")]
        if let Some(error) =
            cause.downcast_ref::<hypercolor_windows_gpu_interop::WindowsGpuInteropError>()
        {
            use hypercolor_windows_gpu_interop::WINDOWS_ANGLE_CLIENT_BUFFER_SURFACE_OPERATION;

            return match error {
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::MissingWgpuVulkanDevice => {
                    ServoGpuImportFallbackReason::MissingWgpuVulkanDevice
                }
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::MissingVulkanExternalMemoryWin32 => {
                    ServoGpuImportFallbackReason::MissingVulkanExternalMemoryWin32
                }
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::MissingWindowsAngleContext => {
                    ServoGpuImportFallbackReason::MissingWindowsAngleContext
                }
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::WindowsImportStaleFrame => {
                    ServoGpuImportFallbackReason::WindowsImportStaleFrame
                }
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::DxgiAdapterNotFound {
                    ..
                } => ServoGpuImportFallbackReason::AdapterLuidMismatch,
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::D3d11DeviceCreateFailed {
                    ..
                }
                | hypercolor_windows_gpu_interop::WindowsGpuInteropError::D3d11ImmediateContextUnavailable => {
                    ServoGpuImportFallbackReason::D3d11DeviceCreateFailed
                }
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::D3d11SharedTextureCreateFailed {
                    ..
                }
                | hypercolor_windows_gpu_interop::WindowsGpuInteropError::D3d11TextureInterfaceQueryFailed {
                    ..
                } => ServoGpuImportFallbackReason::D3d11SharedTextureCreateFailed,
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::D3d11SharedHandleCreateFailed {
                    ..
                } => ServoGpuImportFallbackReason::D3d11SharedHandleCreateFailed,
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::InvalidDimensions {
                    ..
                } => ServoGpuImportFallbackReason::InvalidDimensions,
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::VulkanD3d11ImportFailed => {
                    ServoGpuImportFallbackReason::VulkanD3d11ImportFailed
                }
                hypercolor_windows_gpu_interop::WindowsGpuInteropError::ServoContext {
                    operation,
                    ..
                } if *operation == WINDOWS_ANGLE_CLIENT_BUFFER_SURFACE_OPERATION => {
                    ServoGpuImportFallbackReason::AngleClientBufferSurfaceFailed
                }
                _ => ServoGpuImportFallbackReason::Other,
            };
        }

        if let Some(error) =
            cause.downcast_ref::<hypercolor_linux_gpu_interop::LinuxGpuInteropError>()
        {
            #[cfg(target_os = "linux")]
            return match error {
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::MissingWgpuVulkanDevice => {
                    ServoGpuImportFallbackReason::MissingWgpuVulkanDevice
                }
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::MissingVulkanDeviceExtension(
                    _,
                ) => ServoGpuImportFallbackReason::MissingVulkanExternalMemoryFd,
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::MissingGlFunction(_) => {
                    ServoGpuImportFallbackReason::MissingGlFunction
                }
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::GlProcLoaderUnavailable => {
                    ServoGpuImportFallbackReason::GlProcLoaderUnavailable
                }
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::InvalidDimensions { .. } => {
                    ServoGpuImportFallbackReason::InvalidDimensions
                }
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::Vulkan { .. }
                | hypercolor_linux_gpu_interop::LinuxGpuInteropError::MemoryTypeUnavailable
                | hypercolor_linux_gpu_interop::LinuxGpuInteropError::DuplicateFdFailed {
                    ..
                } => ServoGpuImportFallbackReason::Vulkan,
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::GlCreateResource {
                    ..
                } => ServoGpuImportFallbackReason::GlResource,
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::GlOperation { .. } => {
                    ServoGpuImportFallbackReason::GlOperation
                }
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::GlFramebufferIncomplete {
                    ..
                } => ServoGpuImportFallbackReason::GlFramebufferIncomplete,
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::ImportSlotsExhausted {
                    ..
                } => ServoGpuImportFallbackReason::ImportSlotsExhausted,
                _ => ServoGpuImportFallbackReason::Other,
            };
            #[cfg(not(target_os = "linux"))]
            return match error {
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::UnsupportedPlatform => {
                    ServoGpuImportFallbackReason::UnsupportedPlatform
                }
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::InvalidDimensions {
                    ..
                } => ServoGpuImportFallbackReason::InvalidDimensions,
                hypercolor_linux_gpu_interop::LinuxGpuInteropError::ImportSlotsExhausted {
                    ..
                } => ServoGpuImportFallbackReason::ImportSlotsExhausted,
                _ => ServoGpuImportFallbackReason::Other,
            };
        }
    }

    let message = error.to_string().to_ascii_lowercase();
    if message.contains("not installed") || message.contains("device is not installed") {
        ServoGpuImportFallbackReason::DeviceUnavailable
    } else {
        ServoGpuImportFallbackReason::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_gpu_import_slot_exhaustion() {
        let error = anyhow::anyhow!(
            hypercolor_linux_gpu_interop::LinuxGpuInteropError::ImportSlotsExhausted {
                slot_count: 8
            }
        );

        assert_eq!(
            classify_failure(&error),
            ServoGpuImportFallbackReason::ImportSlotsExhausted
        );
    }

    #[test]
    fn transient_gpu_import_failures_skip_global_auto_backoff() {
        assert!(failure_is_transient(
            ServoGpuImportFallbackReason::GlOperation
        ));
        assert!(failure_is_transient(
            ServoGpuImportFallbackReason::GlFramebufferIncomplete
        ));
        assert!(failure_is_transient(
            ServoGpuImportFallbackReason::ImportSlotsExhausted
        ));
        assert!(!failure_is_transient(
            ServoGpuImportFallbackReason::MissingWgpuVulkanDevice
        ));
    }

    #[test]
    fn transient_gpu_import_failures_preserve_importer_state() {
        assert!(!failure_should_clear_importer(
            ServoGpuImportFallbackReason::GlOperation
        ));
        assert!(!failure_should_clear_importer(
            ServoGpuImportFallbackReason::GlFramebufferIncomplete
        ));
        assert!(!failure_should_clear_importer(
            ServoGpuImportFallbackReason::ImportSlotsExhausted
        ));
        assert!(failure_should_clear_importer(
            ServoGpuImportFallbackReason::GlResource
        ));
        assert!(failure_should_clear_importer(
            ServoGpuImportFallbackReason::MissingWgpuVulkanDevice
        ));
    }
}
