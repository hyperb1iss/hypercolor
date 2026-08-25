//! Platform seam for Servo rendering contexts and GPU frame import.
//!
//! Neutral code (the Servo worker in `hypercolor-core`) drives Servo through
//! these traits and never names a platform. Each GPU interop crate
//! implements [`ServoRenderPlatform`] for its operating system and exposes a
//! stub-everywhere constructor that returns `None` off-platform, so the
//! worker selects a platform by chaining those constructors rather than by
//! `cfg(target_os)`.

use std::rc::Rc;

use paint_api::rendering_context::RenderingContext;

use crate::ImportedEffectFrame;

/// PCI identity of the wgpu adapter that owns the import device.
///
/// Platforms that must pin their Servo context to the same physical GPU
/// (Windows ANGLE against wgpu's Vulkan adapter) consume it; the others
/// ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServoGpuAdapterIdentity {
    /// PCI vendor identifier.
    pub vendor_id: u32,
    /// PCI device identifier.
    pub device_id: u32,
}

/// Snapshot of a pipelined importer's slot pool for telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServoGpuImportSlotState {
    /// Number of import slots in the pool.
    pub slot_count: usize,
    /// Slots currently waiting on a GL fence.
    pub pending_slots: usize,
    /// Slots whose fence has signaled and are ready to hand out.
    pub completed_slots: usize,
    /// Slots free for the next import.
    pub available_slots: usize,
    /// Age of the oldest pending slot, in milliseconds.
    pub oldest_pending_age_ms: Option<u64>,
}

/// A failed frame import together with any platform diagnostics worth
/// logging next to the neutral session context.
#[derive(Debug)]
pub struct ServoGpuImportFailure {
    /// The failure itself. Platform error types stay downcastable through
    /// the chain so neutral code can classify the fallback reason.
    pub error: anyhow::Error,
    /// Platform-side state formatted for a debug log line, when available.
    pub diagnostics: Option<String>,
}

impl From<anyhow::Error> for ServoGpuImportFailure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            error,
            diagnostics: None,
        }
    }
}

/// Imports frames rendered by one Servo GPU rendering context into the
/// wgpu device.
///
/// Implementations own their platform rendering context handle and any
/// lazily created importer state, so the neutral caller only supplies the
/// wgpu device and the requested canvas size.
pub trait ServoGpuFrameImporter {
    /// Prepare importer resources for `width` x `height` frames ahead of the
    /// first import so the first frame does not pay the setup cost.
    ///
    /// # Errors
    ///
    /// Returns the platform error when the importer cannot be created.
    fn warm(&mut self, device: &wgpu::Device, width: u32, height: u32) -> anyhow::Result<()>;

    /// Import the most recently presented frame.
    ///
    /// # Errors
    ///
    /// Returns the platform failure, with diagnostics when the platform has
    /// state worth recording next to the neutral session context.
    fn import_frame(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<ImportedEffectFrame, ServoGpuImportFailure>;

    /// Release importer resources; the next import recreates them.
    fn clear(&mut self);

    /// Slot pool state after the most recent import attempt, for platforms
    /// that pipeline imports through a slot pool.
    fn slot_state(&self) -> Option<ServoGpuImportSlotState> {
        None
    }

    /// Human-readable description of the native surface backing the
    /// rendering context, for platforms that expose one.
    fn native_surface_summary(&self) -> Option<String> {
        None
    }
}

/// A Servo rendering context paired with the importer that reads its
/// frames back into wgpu.
pub struct ServoGpuImportSession {
    /// Context Servo renders into.
    pub rendering_context: Rc<dyn RenderingContext>,
    /// Importer bound to `rendering_context`.
    pub importer: Box<dyn ServoGpuFrameImporter>,
}

/// Platform-specific Servo rendering context construction.
///
/// Implementations may keep shared state across calls (a parent GL device
/// that hosts many offscreen targets, a hidden native window keepalive).
pub trait ServoRenderPlatform {
    /// Platform name for log lines.
    fn name(&self) -> &'static str;

    /// Create a CPU-readback rendering context when the platform needs a
    /// native one instead of Servo's portable software context.
    ///
    /// `Ok(None)` means the software context is the right choice here.
    ///
    /// # Errors
    ///
    /// Returns the platform error when the native CPU context cannot be
    /// created.
    fn create_cpu_rendering_context(
        &mut self,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Option<Rc<dyn RenderingContext>>>;

    /// Create a rendering context whose frames can be imported into wgpu
    /// without a CPU readback.
    ///
    /// # Errors
    ///
    /// Returns the platform error when GPU import is unsupported or the
    /// context cannot be created.
    fn create_gpu_import_session(
        &mut self,
        width: u32,
        height: u32,
        adapter: Option<ServoGpuAdapterIdentity>,
    ) -> anyhow::Result<ServoGpuImportSession>;
}
