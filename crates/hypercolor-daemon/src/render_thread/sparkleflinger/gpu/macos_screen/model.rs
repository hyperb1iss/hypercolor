use std::sync::{Arc, Mutex};

use hypercolor_core::input::screen::{
    ResolvedScreenPublicationDescriptor, ScreenNativeExecutionTargetId,
};
use hypercolor_macos_gpu_interop::MacosNativeReductionTarget;

#[derive(Debug)]
pub(in crate::render_thread::sparkleflinger::gpu) struct PreparedMacosPhysicalTarget {
    pub(in crate::render_thread::sparkleflinger::gpu) target: MacosNativeReductionTarget,
    pub(in crate::render_thread::sparkleflinger::gpu) storage_id: u64,
    pub(in crate::render_thread::sparkleflinger::gpu) content_sequence: Mutex<Option<u64>>,
}

#[derive(Debug)]
pub(crate) struct PreparedMacosScreenTarget {
    pub(in crate::render_thread::sparkleflinger::gpu) target_id: ScreenNativeExecutionTargetId,
    pub(in crate::render_thread::sparkleflinger::gpu) resource_generation: u64,
    pub(in crate::render_thread::sparkleflinger::gpu) descriptor:
        Arc<ResolvedScreenPublicationDescriptor>,
    pub(in crate::render_thread::sparkleflinger::gpu) physical:
        Option<Arc<PreparedMacosPhysicalTarget>>,
    pub(in crate::render_thread::sparkleflinger::gpu) logical_target:
        Option<MacosNativeReductionTarget>,
    pub(in crate::render_thread::sparkleflinger::gpu) logical_storage_id: Option<u64>,
    pub(in crate::render_thread::sparkleflinger::gpu) logical_content_sequence: Mutex<Option<u64>>,
}

impl Clone for PreparedMacosScreenTarget {
    fn clone(&self) -> Self {
        Self {
            target_id: self.target_id,
            resource_generation: self.resource_generation,
            descriptor: Arc::clone(&self.descriptor),
            physical: self.physical.clone(),
            logical_target: self.logical_target.clone(),
            logical_storage_id: self.logical_storage_id,
            logical_content_sequence: Mutex::new(
                *self
                    .logical_content_sequence
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ),
        }
    }
}
