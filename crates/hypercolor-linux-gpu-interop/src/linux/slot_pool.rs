use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::fence::{GlFenceStatus, delete_gl_fence, poll_gl_fence, wait_for_gl_fence_completion};
use super::gl_external_memory::{
    GlExternalMemoryFunctions, GlImportedImageBinding, clear_gl_errors,
    current_gl_framebuffer_state,
};
use super::vulkan_export::ExportableVulkanImage;
use super::{
    GlFramebufferSource, GlFramebufferStateSnapshot, ImportedEffectFrame, ImportedFrameTimings,
    LinuxGlFramebufferImportDescriptor, LinuxGlImporterStateSnapshot, LinuxGpuInteropError, Result,
    elapsed_micros,
};

const MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION: u32 = 16;
static NEXT_STORAGE_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct ImportedFrameSlotPool {
    slots: Vec<ImportedFrameSlot>,
    next_slot: usize,
    recoverable_poll_errors_without_completion: u32,
}

impl ImportedFrameSlotPool {
    pub(super) fn create(
        device: &wgpu::Device,
        hal_device: &wgpu_hal::vulkan::Device,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        descriptor: LinuxGlFramebufferImportDescriptor,
        slot_count: usize,
    ) -> Result<Self> {
        let slot_count = slot_count.max(1);
        let mut slots = Vec::with_capacity(slot_count);
        for _ in 0..slot_count {
            match ImportedFrameSlot::create(device, hal_device, gl, gl_external_memory, descriptor)
            {
                Ok(slot) => slots.push(slot),
                Err(error) => {
                    for slot in &mut slots {
                        slot.destroy_gl_resources(gl, gl_external_memory);
                    }
                    return Err(error);
                }
            }
        }

        Ok(Self {
            slots,
            next_slot: 0,
            recoverable_poll_errors_without_completion: 0,
        })
    }

    pub(super) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn state_snapshot(&self) -> LinuxGlImporterStateSnapshot {
        let oldest_pending_age_ms = self
            .slots
            .iter()
            .filter_map(ImportedFrameSlot::pending_age_ms)
            .max();
        LinuxGlImporterStateSnapshot {
            slot_count: self.slots.len(),
            pending_slots: self
                .slots
                .iter()
                .filter(|slot| slot.pending.is_some())
                .count(),
            completed_slots: self
                .slots
                .iter()
                .filter(|slot| slot.completed.is_some())
                .count(),
            available_slots: self.slots.iter().filter(|slot| slot.is_available()).count(),
            oldest_pending_age_ms,
            latest_completed_storage_id: self.latest_completed_storage_id(),
        }
    }

    pub(super) fn framebuffer_state_for_blit(
        &self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> GlFramebufferStateSnapshot {
        self.slots.first().map_or_else(
            || current_gl_framebuffer_state(gl),
            |slot| slot.framebuffer_state_for_blit(gl, source_framebuffer),
        )
    }

    pub(super) fn import_framebuffer(
        &mut self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        source_framebuffer: GlFramebufferSource,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<ImportedEffectFrame> {
        let total_start = Instant::now();
        self.poll_pending_imports(gl)?;
        let slot = self.next_slot()?;
        let mut timings =
            slot.blit_from_framebuffer(gl, gl_external_memory, source_framebuffer, descriptor)?;
        timings.total_us = elapsed_micros(total_start);
        Ok(slot.frame(descriptor, timings))
    }

    pub(super) fn import_framebuffer_pipelined(
        &mut self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        source_framebuffer: GlFramebufferSource,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<ImportedEffectFrame> {
        self.poll_pending_imports(gl)?;

        let issued_slot = if let Some(index) = self.next_pipelined_slot() {
            let total_start = Instant::now();
            let slot = &mut self.slots[index];
            let mut pending = slot.blit_from_framebuffer_pipelined(
                gl,
                gl_external_memory,
                source_framebuffer,
                descriptor,
            )?;
            pending.timings.total_us = elapsed_micros(total_start);
            slot.completed = None;
            slot.pending = Some(pending);
            self.next_slot = (index + 1) % self.slots.len();
            Some(index)
        } else {
            None
        };

        self.poll_pending_imports(gl)?;
        if let Some(frame) = self.latest_completed_frame(descriptor) {
            return Ok(frame);
        }

        if let Some(index) = issued_slot {
            self.slots[index].wait_pending_import(gl)?;
            return self.slots[index].completed_frame(descriptor).ok_or(
                LinuxGpuInteropError::ImportSlotsExhausted {
                    slot_count: self.slots.len(),
                },
            );
        }

        Err(LinuxGpuInteropError::ImportSlotsExhausted {
            slot_count: self.slots.len(),
        })
    }

    pub(super) fn destroy_gl_resources(
        &mut self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
    ) {
        for slot in &mut self.slots {
            slot.destroy_gl_resources(gl, gl_external_memory);
        }
    }

    fn next_slot(&mut self) -> Result<&mut ImportedFrameSlot> {
        let slot_count = self.slots.len();
        let preferred = self.next_slot;
        let selected = (0..slot_count)
            .map(|offset| (preferred + offset) % slot_count)
            .find(|&index| self.slots[index].is_available())
            .ok_or(LinuxGpuInteropError::ImportSlotsExhausted { slot_count })?;
        self.next_slot = (selected + 1) % slot_count;
        Ok(&mut self.slots[selected])
    }

    fn next_pipelined_slot(&self) -> Option<usize> {
        let slot_count = self.slots.len();
        let preferred = self.next_slot;
        let latest_completed_index = self.latest_completed_index();
        let mut fallback = None;
        for index in (0..slot_count).map(|offset| (preferred + offset) % slot_count) {
            if !self.slots[index].is_available() {
                continue;
            }
            if Some(index) != latest_completed_index {
                return Some(index);
            }
            fallback = Some(index);
        }
        fallback
    }

    fn latest_completed_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.completed.map(|completed| (index, completed)))
            .max_by_key(|(_, completed)| completed.storage_id)
            .map(|(index, _)| index)
    }

    fn latest_completed_storage_id(&self) -> Option<u64> {
        self.slots
            .iter()
            .filter_map(|slot| slot.completed.map(|completed| completed.storage_id))
            .max()
    }

    fn latest_completed_frame(
        &self,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Option<ImportedEffectFrame> {
        self.latest_completed_index()
            .and_then(|index| self.slots[index].completed_frame(descriptor))
    }

    fn poll_pending_imports(&mut self, gl: &glow::Context) -> Result<()> {
        let latest_before = self.latest_completed_storage_id();
        let mut first_error = None;
        for slot in &mut self.slots {
            if let Err(error) = slot.poll_pending_import(gl)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        let latest_after = self.latest_completed_storage_id();
        let completed_advanced = latest_after != latest_before && latest_after.is_some();
        if completed_advanced {
            self.recoverable_poll_errors_without_completion = 0;
        }
        if let Some(error) = first_error
            && poll_error_should_propagate(
                &error,
                latest_after,
                completed_advanced,
                &mut self.recoverable_poll_errors_without_completion,
            )
        {
            return Err(error);
        }
        Ok(())
    }
}

pub(super) struct SingleFrameImport<'a> {
    pub(super) device: &'a wgpu::Device,
    pub(super) hal_device: &'a wgpu_hal::vulkan::Device,
    pub(super) gl: &'a glow::Context,
    pub(super) gl_external_memory: GlExternalMemoryFunctions,
    pub(super) source_framebuffer: GlFramebufferSource,
    pub(super) descriptor: LinuxGlFramebufferImportDescriptor,
    pub(super) image: &'a mut ExportableVulkanImage,
    pub(super) total_start: Instant,
}

pub(super) fn import_framebuffer_once(
    request: SingleFrameImport<'_>,
) -> Result<ImportedEffectFrame> {
    let mut slot = ImportedFrameSlot::create_from_image(
        request.device,
        request.hal_device,
        request.gl,
        request.gl_external_memory,
        request.descriptor,
        request.image,
    )?;
    let mut timings = slot.blit_from_framebuffer(
        request.gl,
        request.gl_external_memory,
        request.source_framebuffer,
        request.descriptor,
    )?;
    timings.total_us = elapsed_micros(request.total_start);
    let frame = slot.frame(request.descriptor, timings);
    slot.destroy_gl_resources(request.gl, request.gl_external_memory);
    Ok(frame)
}

struct ImportedFrameSlot {
    gl_binding: GlImportedImageBinding,
    texture: Arc<wgpu::Texture>,
    view: Arc<wgpu::TextureView>,
    pending: Option<PendingImport>,
    completed: Option<CompletedImport>,
}

struct PendingImport {
    fence: glow::NativeFence,
    storage_id: u64,
    issued_at: Instant,
    timings: ImportedFrameTimings,
}

#[derive(Clone, Copy)]
struct CompletedImport {
    storage_id: u64,
    timings: ImportedFrameTimings,
}

impl ImportedFrameSlot {
    fn create(
        device: &wgpu::Device,
        hal_device: &wgpu_hal::vulkan::Device,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<Self> {
        let mut image = ExportableVulkanImage::create(hal_device, descriptor)?;
        Self::create_from_image(
            device,
            hal_device,
            gl,
            gl_external_memory,
            descriptor,
            &mut image,
        )
    }

    fn create_from_image(
        device: &wgpu::Device,
        hal_device: &wgpu_hal::vulkan::Device,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        descriptor: LinuxGlFramebufferImportDescriptor,
        image: &mut ExportableVulkanImage,
    ) -> Result<Self> {
        let gl_binding = GlImportedImageBinding::create(
            gl,
            gl_external_memory,
            descriptor,
            image.memory_fd.as_raw_fd(),
            image.allocation_size,
        )?;
        let texture = image.wrap_as_wgpu_texture(device, hal_device, descriptor)?;
        Ok(Self {
            gl_binding,
            texture: texture.texture,
            view: texture.view,
            pending: None,
            completed: None,
        })
    }

    fn is_available(&self) -> bool {
        self.pending.is_none()
            && Arc::strong_count(&self.texture) == 1
            && Arc::strong_count(&self.view) == 1
    }

    fn blit_from_framebuffer(
        &self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        source_framebuffer: GlFramebufferSource,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<ImportedFrameTimings> {
        self.gl_binding.blit_from_framebuffer(
            gl,
            gl_external_memory,
            source_framebuffer,
            descriptor,
        )
    }

    fn blit_from_framebuffer_pipelined(
        &self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        source_framebuffer: GlFramebufferSource,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<PendingImport> {
        let timings = self.gl_binding.blit_from_framebuffer_pipelined(
            gl,
            gl_external_memory,
            source_framebuffer,
            descriptor,
        )?;
        Ok(PendingImport {
            fence: timings.fence,
            storage_id: NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            issued_at: Instant::now(),
            timings: timings.timings,
        })
    }

    fn framebuffer_state_for_blit(
        &self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> GlFramebufferStateSnapshot {
        self.gl_binding
            .framebuffer_state_for_blit(gl, source_framebuffer)
    }

    fn frame(
        &self,
        descriptor: LinuxGlFramebufferImportDescriptor,
        timings: ImportedFrameTimings,
    ) -> ImportedEffectFrame {
        self.frame_with_storage_id(
            descriptor,
            NEXT_STORAGE_ID.fetch_add(1, Ordering::Relaxed),
            timings,
        )
    }

    fn completed_frame(
        &self,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Option<ImportedEffectFrame> {
        self.completed.map(|completed| {
            self.frame_with_storage_id(descriptor, completed.storage_id, completed.timings)
        })
    }

    fn frame_with_storage_id(
        &self,
        descriptor: LinuxGlFramebufferImportDescriptor,
        storage_id: u64,
        timings: ImportedFrameTimings,
    ) -> ImportedEffectFrame {
        ImportedEffectFrame {
            width: descriptor.width,
            height: descriptor.height,
            format: descriptor.format,
            storage_id,
            texture: Arc::clone(&self.texture),
            view: Arc::clone(&self.view),
            timings,
        }
    }

    fn poll_pending_import(&mut self, gl: &glow::Context) -> Result<()> {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };

        let sync_start = Instant::now();
        let status = match poll_gl_fence(gl, pending.fence) {
            Ok(status) => status,
            Err(error) => {
                delete_gl_fence(gl, pending.fence);
                clear_gl_errors(gl);
                return Err(error);
            }
        };
        pending.timings.sync_us = pending
            .timings
            .sync_us
            .saturating_add(elapsed_micros(sync_start));

        match status {
            GlFenceStatus::Complete => {
                delete_gl_fence(gl, pending.fence);
                self.completed = Some(CompletedImport {
                    storage_id: pending.storage_id,
                    timings: pending.timings,
                });
            }
            GlFenceStatus::Pending => {
                self.pending = Some(pending);
            }
        }
        Ok(())
    }

    fn wait_pending_import(&mut self, gl: &glow::Context) -> Result<()> {
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        let sync_result = wait_for_gl_fence_completion(gl, pending.fence);
        delete_gl_fence(gl, pending.fence);
        let sync_us = sync_result?;
        pending.timings.sync_us = pending.timings.sync_us.saturating_add(sync_us);
        self.completed = Some(CompletedImport {
            storage_id: pending.storage_id,
            timings: pending.timings,
        });
        Ok(())
    }

    fn pending_age_ms(&self) -> Option<u64> {
        self.pending
            .as_ref()
            .map(|pending| elapsed_millis(pending.issued_at))
    }

    fn destroy_gl_resources(
        &mut self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
    ) {
        if let Some(pending) = self.pending.take() {
            delete_gl_fence(gl, pending.fence);
        }
        self.gl_binding.destroy(gl, gl_external_memory);
    }
}

fn is_recoverable_poll_error(error: &LinuxGpuInteropError) -> bool {
    matches!(
        error,
        LinuxGpuInteropError::GlOperation {
            operation: "glClientWaitSync",
            code: glow::INVALID_OPERATION,
        }
    )
}

fn should_escalate_recoverable_poll_error(errors_without_completion: u32) -> bool {
    errors_without_completion > MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION
}

fn poll_error_should_propagate(
    error: &LinuxGpuInteropError,
    latest_completed_storage_id: Option<u64>,
    completed_advanced: bool,
    errors_without_completion: &mut u32,
) -> bool {
    if latest_completed_storage_id.is_none() || !is_recoverable_poll_error(error) {
        return true;
    }
    if completed_advanced {
        return false;
    }

    *errors_without_completion = errors_without_completion.saturating_add(1);
    should_escalate_recoverable_poll_error(*errors_without_completion)
}

fn elapsed_millis(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gl_client_wait_sync_poll_errors_are_recoverable() {
        assert!(is_recoverable_poll_error(
            &LinuxGpuInteropError::GlOperation {
                operation: "glClientWaitSync",
                code: glow::INVALID_OPERATION,
            }
        ));
        assert!(!is_recoverable_poll_error(
            &LinuxGpuInteropError::GlOperation {
                operation: "glBlitFramebuffer",
                code: glow::INVALID_OPERATION,
            }
        ));
        assert!(!is_recoverable_poll_error(
            &LinuxGpuInteropError::ImportSlotsExhausted { slot_count: 8 }
        ));
    }

    #[test]
    fn recoverable_poll_errors_escalate_after_streak_limit() {
        assert!(!should_escalate_recoverable_poll_error(
            MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION,
        ));
        assert!(should_escalate_recoverable_poll_error(
            MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION + 1,
        ));
    }

    #[test]
    fn poll_error_decision_tracks_progress_and_escalation() {
        let recoverable = LinuxGpuInteropError::GlOperation {
            operation: "glClientWaitSync",
            code: glow::INVALID_OPERATION,
        };
        let non_recoverable = LinuxGpuInteropError::GlOperation {
            operation: "glBlitFramebuffer",
            code: glow::INVALID_OPERATION,
        };
        let mut streak = 0;

        assert!(poll_error_should_propagate(
            &recoverable,
            None,
            false,
            &mut streak,
        ));
        assert_eq!(streak, 0);

        assert!(!poll_error_should_propagate(
            &recoverable,
            Some(41),
            true,
            &mut streak,
        ));
        assert_eq!(streak, 0);

        for _ in 0..MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION {
            assert!(!poll_error_should_propagate(
                &recoverable,
                Some(41),
                false,
                &mut streak,
            ));
        }
        assert_eq!(streak, MAX_RECOVERABLE_POLL_ERRORS_WITHOUT_COMPLETION);
        assert!(poll_error_should_propagate(
            &recoverable,
            Some(41),
            false,
            &mut streak,
        ));

        streak = 0;
        assert!(poll_error_should_propagate(
            &non_recoverable,
            Some(41),
            false,
            &mut streak,
        ));
        assert_eq!(streak, 0);
    }
}
