use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_core::types::canvas::{
    PublishedSurface, RenderSurfacePool, SurfaceDescriptor, SurfaceStateCounts,
};

use super::super::super::{ComposedFrameSet, PreviewSurfaceRequest};
use super::super::frame_set::{gpu_composed_from_surface, gpu_composed_without_surfaces};
use super::super::preview::{PreparedPreviewSurfaceChange, preview_request_matches_plan};
use super::super::readback::{CachedReadbackKey, copy_mapped_readback_buffer_into_surface};
use super::super::{
    GpuCompositorOutputSurface, GpuSparkleFlinger, padded_bytes_per_row, texture_extent,
};
#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use crate::render_thread::producer_queue::MacosScreenTextureLease;

const SAMPLING_READBACK_SLOT_COUNT: usize = 2;
const SAMPLING_READBACK_SURFACE_SLOTS: usize = 3;

/// One-frame readback latch that lets CPU spatial sampling follow GPU-only
/// composition plans (Servo imports and media producer textures) one frame
/// behind. Plans containing GPU producer frames have no `CachedReadbackKey`,
/// so the keyed readback cache can never service them; instead every compose
/// stages a full-canvas copy of the freshly composed output and the next
/// compose returns the previously staged surface.
#[derive(Default)]
pub(in crate::render_thread::sparkleflinger::gpu) struct SamplingReadbackLatch {
    buffers: Option<SamplingReadbackBuffers>,
    pending: Option<PendingSamplingReadback>,
    latched: Option<LatchedSamplingSurface>,
    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) last_readback_bytes: u64,
}

pub(in crate::render_thread::sparkleflinger::gpu) struct SamplingReadbackBuffers {
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    readbacks: [wgpu::Buffer; SAMPLING_READBACK_SLOT_COUNT],
    next_slot: usize,
    surfaces: RenderSurfacePool,
}

impl SamplingReadbackBuffers {
    fn try_new(
        device: &wgpu::Device,
        max_buffer_size: u64,
        width: u32,
        height: u32,
        fail_after_prepare: bool,
    ) -> Result<Self> {
        super::super::ensure_readback_buffer_capacity(max_buffer_size, width, height, true)?;
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let size = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(height))
            .context("GPU sampling latch readback byte size overflowed")?;
        let surfaces = RenderSurfacePool::try_with_slot_count(
            SurfaceDescriptor::rgba8888(width, height),
            SAMPLING_READBACK_SURFACE_SLOTS,
        )
        .context("GPU sampling latch CPU surface allocation failed")?;
        let readbacks = super::super::try_create_gpu_resources(
            device,
            "GPU sampling latch buffer allocation failed",
            || {
                std::array::from_fn(|_| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("SparkleFlinger GPU sampling latch readback"),
                        size,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    })
                })
            },
        )?;
        let prepared = Self {
            width,
            height,
            padded_bytes_per_row,
            readbacks,
            next_slot: 0,
            surfaces,
        };
        super::super::reject_injected_gpu_preparation(fail_after_prepare, "sampling readback")?;
        Ok(prepared)
    }
}

impl SamplingReadbackLatch {
    pub(in crate::render_thread::sparkleflinger::gpu) fn install_buffers(
        &mut self,
        buffers: Option<SamplingReadbackBuffers>,
    ) {
        self.buffers = buffers;
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fn buffer_extent(
        &self,
    ) -> Option<(u32, u32)> {
        self.buffers
            .as_ref()
            .map(|buffers| (buffers.width, buffers.height))
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn surface_pool_counts(
        &mut self,
    ) -> SurfaceStateCounts {
        self.buffers
            .as_mut()
            .map_or_else(SurfaceStateCounts::default, |buffers| {
                buffers.surfaces.slot_counts()
            })
    }
}

struct PendingSamplingReadback {
    width: u32,
    height: u32,
    output_generation: u64,
    slot: usize,
    used_bytes: u64,
    submission_index: wgpu::SubmissionIndex,
    receiver: mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
}

struct LatchedSamplingSurface {
    width: u32,
    height: u32,
    output_generation: u64,
    surface: PublishedSurface,
}

impl GpuSparkleFlinger {
    pub(super) fn read_back_current_output_surface(
        &mut self,
        width: u32,
        height: u32,
        readback_key: Option<CachedReadbackKey>,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
        encoder: Option<wgpu::CommandEncoder>,
        #[cfg(all(target_os = "macos", feature = "screen-capture"))] native_screen_leases: Vec<
            MacosScreenTextureLease,
        >,
        prepared_preview_surface: Option<PreparedPreviewSurfaceChange>,
    ) -> Result<ComposedFrameSet> {
        if requires_cpu_sampling_canvas {
            // Keyed (all-CPU) plans keep their historical behavior: the keyed
            // readback cache and the bypass paths already service them, so
            // the compose encoder is deferred for a later flush. Keyless
            // plans carry GPU producer frames and can only reach the CPU
            // sampler through the one-frame readback latch.
            if readback_key.is_some() {
                if let Some(encoder) = encoder {
                    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                    self.stage_frame_in_flight_with_native_screen_leases(
                        encoder,
                        None,
                        native_screen_leases,
                    );
                    #[cfg(not(all(target_os = "macos", feature = "screen-capture")))]
                    self.stage_frame_in_flight(encoder, None);
                }
                return Ok(gpu_composed_without_surfaces());
            }
            return self.latch_sampling_surface_readback(
                width,
                height,
                encoder,
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                native_screen_leases,
            );
        }
        let Some(current_output) = self.current_output else {
            anyhow::bail!("GPU readback requested without a composed output surface");
        };
        if let Some(request) = preview_surface_request {
            let cache_as_full_size = preview_request_matches_plan(Some(request), width, height);
            return self.stage_preview_surface_readback(
                current_output,
                width,
                height,
                readback_key,
                request,
                cache_as_full_size,
                encoder,
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                native_screen_leases,
                prepared_preview_surface,
            );
        }
        if let Some(encoder) = encoder {
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            self.stage_frame_in_flight_with_native_screen_leases(
                encoder,
                None,
                native_screen_leases,
            );
            #[cfg(not(all(target_os = "macos", feature = "screen-capture")))]
            self.stage_frame_in_flight(encoder, None);
        }
        Ok(gpu_composed_without_surfaces())
    }

    /// Services `requires_cpu_sampling_canvas` for keyless plans with a
    /// one-frame readback latch: each compose stages a full-canvas readback
    /// of the freshly composed output and returns the surface staged by the
    /// previous compose. The very first compose returns no canvas — callers
    /// already tolerate one `Unavailable` sampling frame.
    ///
    /// Preview coexistence: when a scaled preview is requested in the same
    /// frame, sampling wins this readback; the full-size sampling canvas is
    /// published as the scene canvas and preview consumers downscale it
    /// CPU-side, so no preview frame is lost — it just skips the GPU scale
    /// pass for that frame.
    ///
    /// Pipeline correctness: the staged copy is appended to the compose
    /// encoder (or the stashed deferred submission) and submitted eagerly,
    /// then resolved with zero-timeout polls on later composes — the render
    /// thread never blocks on this path.
    fn latch_sampling_surface_readback(
        &mut self,
        width: u32,
        height: u32,
        encoder: Option<wgpu::CommandEncoder>,
        #[cfg(all(target_os = "macos", feature = "screen-capture"))] native_screen_leases: Vec<
            MacosScreenTextureLease,
        >,
    ) -> Result<ComposedFrameSet> {
        self.resolve_pending_sampling_readback();
        let latched = self
            .sampling_latch
            .latched
            .as_ref()
            .filter(|latched| latched.width == width && latched.height == height)
            .map(|latched| latched.surface.clone());
        self.stage_sampling_surface_readback(
            width,
            height,
            encoder,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            native_screen_leases,
        )?;
        Ok(match latched {
            Some(surface) => gpu_composed_from_surface(surface, true),
            None => gpu_composed_without_surfaces(),
        })
    }

    /// Polls the in-flight sampling readback without blocking and moves it
    /// into the latch once its buffer map has completed.
    fn resolve_pending_sampling_readback(&mut self) {
        let Some(pending) = self.sampling_latch.pending.take() else {
            return;
        };
        match self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(pending.submission_index.clone()),
            timeout: Some(Duration::ZERO),
        }) {
            Ok(_) | Err(wgpu::PollError::Timeout) => {}
            Err(error) => {
                tracing::debug!(%error, "GPU sampling latch readiness poll failed");
                self.unmap_sampling_readback_slot(pending.slot);
                return;
            }
        }
        if let Err(error) = self.device.poll(wgpu::PollType::Poll) {
            tracing::debug!(%error, "GPU sampling latch callback poll failed");
            self.unmap_sampling_readback_slot(pending.slot);
            return;
        }
        match pending.receiver.try_recv() {
            Ok(Ok(())) => {}
            Err(TryRecvError::Empty) => {
                self.sampling_latch.pending = Some(pending);
                return;
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "GPU sampling latch buffer mapping failed");
                self.unmap_sampling_readback_slot(pending.slot);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                tracing::debug!("GPU sampling latch channel closed before map completion");
                self.unmap_sampling_readback_slot(pending.slot);
                return;
            }
        }
        let SamplingReadbackLatch {
            buffers,
            latched,
            #[cfg(test)]
            last_readback_bytes,
            ..
        } = &mut self.sampling_latch;
        let Some(buffers) = buffers.as_mut() else {
            return;
        };
        let readback = buffers.readbacks[pending.slot].clone();
        match copy_mapped_readback_buffer_into_surface(
            &readback,
            pending.used_bytes,
            pending.width,
            pending.height,
            buffers.padded_bytes_per_row,
            &mut buffers.surfaces,
            #[cfg(test)]
            last_readback_bytes,
        ) {
            Ok(surface) => {
                *latched = Some(LatchedSamplingSurface {
                    width: pending.width,
                    height: pending.height,
                    output_generation: pending.output_generation,
                    surface,
                });
            }
            Err(error) => {
                tracing::debug!(%error, "GPU sampling latch readback copy failed");
            }
        }
    }

    /// Encodes a full-canvas copy of the current output texture, submits it
    /// together with any pending compose work, and begins the asynchronous
    /// buffer map that the next compose resolves.
    fn stage_sampling_surface_readback(
        &mut self,
        width: u32,
        height: u32,
        encoder: Option<wgpu::CommandEncoder>,
        #[cfg(all(target_os = "macos", feature = "screen-capture"))] mut native_screen_leases: Vec<
            MacosScreenTextureLease,
        >,
    ) -> Result<()> {
        // A staged preview readback shares the deferred-submission slot.
        // Route it through the preview machinery first so its buffer map
        // still begins before this path claims the queue.
        if self.pending_preview_readback().is_some()
            && self.has_pending_output_submission()
            && let Err(error) = self.submit_pending_preview_work()
        {
            tracing::debug!(%error, "GPU preview submit ahead of sampling readback failed");
        }
        let stashed = self
            .has_pending_output_submission()
            .then(|| self.supersede_frame_in_flight("sampling readback chained"))
            .flatten();
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        let encoder = match (encoder, stashed) {
            (Some(encoder), Some(stashed)) => {
                // Submit the stashed encoder first so its work stays ordered
                // ahead of the compose encoder we are extending.
                let submission_index = self.queue.submit(Some(stashed.encoder.finish()));
                self.retire_native_screen_leases(submission_index, stashed.native_screen_leases);
                Some(encoder)
            }
            (Some(encoder), None) => Some(encoder),
            (None, Some(stashed)) => {
                native_screen_leases.extend(stashed.native_screen_leases);
                Some(stashed.encoder)
            }
            (None, None) => None,
        };
        #[cfg(not(all(target_os = "macos", feature = "screen-capture")))]
        let encoder = match (encoder, stashed) {
            (Some(encoder), Some(stashed)) => {
                // Submit the stashed encoder first so its work stays ordered
                // ahead of the compose encoder we are extending.
                self.queue.submit(Some(stashed.encoder.finish()));
                Some(encoder)
            }
            (Some(encoder), None) => Some(encoder),
            (None, Some(stashed)) => Some(stashed.encoder),
            (None, None) => None,
        };

        let generation = self.output_generation;
        let staged_already = self.sampling_latch.pending.as_ref().is_some_and(|pending| {
            pending.output_generation == generation
                && pending.width == width
                && pending.height == height
        }) || self.sampling_latch.latched.as_ref().is_some_and(|latched| {
            latched.output_generation == generation
                && latched.width == width
                && latched.height == height
        });
        let source_texture = self.surfaces.as_ref().and_then(|surfaces| {
            Some(match self.current_output? {
                GpuCompositorOutputSurface::Front => surfaces.front.texture.clone(),
                GpuCompositorOutputSurface::Back => surfaces.back.texture.clone(),
            })
        });
        // Keep at most one readback in flight: an unresolved readback keeps
        // its buffer map outstanding, so the latch keeps serving the older
        // surface until the GPU catches up instead of dropping work.
        if staged_already
            || self.sampling_latch.pending.is_some()
            || width == 0
            || height == 0
            || source_texture.is_none()
        {
            self.submit_sampling_encoder(
                encoder,
                #[cfg(all(target_os = "macos", feature = "screen-capture"))]
                native_screen_leases,
            );
            return Ok(());
        }
        let Some(source_texture) = source_texture else {
            return Ok(());
        };

        self.ensure_sampling_readback_buffers(width, height)?;
        let mut encoder = encoder.unwrap_or_else(|| {
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger GPU sampling readback"),
                })
        });
        let buffers = self
            .sampling_latch
            .buffers
            .as_mut()
            .expect("sampling readback buffers should exist after allocation");
        let slot = buffers.next_slot;
        buffers.next_slot = (slot + 1) % SAMPLING_READBACK_SLOT_COUNT;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &source_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffers.readbacks[slot],
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(buffers.padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            texture_extent(width, height),
        );
        let used_bytes = u64::from(buffers.padded_bytes_per_row).saturating_mul(u64::from(height));
        let readback = buffers.readbacks[slot].clone();
        let submission_index = self.queue.submit(Some(encoder.finish()));
        self.finish_pending_uploads(submission_index.clone());
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        self.retire_native_screen_leases(submission_index.clone(), native_screen_leases);
        self.release_retired_uniform_slots();
        let (sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
        readback
            .slice(..used_bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.sampling_latch.pending = Some(PendingSamplingReadback {
            width,
            height,
            output_generation: generation,
            slot,
            used_bytes,
            submission_index,
            receiver,
        });
        Ok(())
    }

    fn submit_sampling_encoder(
        &mut self,
        encoder: Option<wgpu::CommandEncoder>,
        #[cfg(all(target_os = "macos", feature = "screen-capture"))] native_screen_leases: Vec<
            MacosScreenTextureLease,
        >,
    ) {
        if let Some(encoder) = encoder {
            let submission_index = self.queue.submit(Some(encoder.finish()));
            self.finish_pending_uploads(submission_index.clone());
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            self.retire_native_screen_leases(submission_index, native_screen_leases);
            self.release_retired_uniform_slots();
        }
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn prepare_sampling_readback_buffers(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Option<SamplingReadbackBuffers>> {
        if self
            .sampling_latch
            .buffers
            .as_ref()
            .is_some_and(|buffers| buffers.width == width && buffers.height == height)
        {
            return Ok(None);
        }
        let fail_after_prepare = self.take_sampling_readback_failure_injection();
        SamplingReadbackBuffers::try_new(
            &self.device,
            self.max_buffer_size,
            width,
            height,
            fail_after_prepare,
        )
        .map(Some)
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn prepare_sampling_readback_buffers_for_canvas_resize(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Option<SamplingReadbackBuffers>> {
        if self.sampling_latch.buffers.is_none() {
            return Ok(None);
        }
        let fail_after_prepare = self.take_sampling_readback_failure_injection();
        SamplingReadbackBuffers::try_new(
            &self.device,
            self.max_buffer_size,
            width,
            height,
            fail_after_prepare,
        )
        .map(Some)
    }

    pub(super) fn commit_sampling_readback_buffers(
        &mut self,
        replacement: Option<SamplingReadbackBuffers>,
    ) {
        let Some(replacement) = replacement else {
            return;
        };
        // The pending readback's mapped buffer belongs to the old set; drop
        // it before replacing the buffers.
        self.discard_pending_sampling_readback();
        self.sampling_latch.buffers = Some(replacement);
    }

    fn ensure_sampling_readback_buffers(&mut self, width: u32, height: u32) -> Result<()> {
        let replacement = self.prepare_sampling_readback_buffers(width, height)?;
        self.commit_sampling_readback_buffers(replacement);
        Ok(())
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn discard_pending_sampling_readback(
        &mut self,
    ) {
        if let Some(pending) = self.sampling_latch.pending.take() {
            self.unmap_sampling_readback_slot(pending.slot);
        }
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn clear_sampling_readback_latch(&mut self) {
        self.discard_pending_sampling_readback();
        self.sampling_latch.latched = None;
        self.sampling_latch.buffers = None;
    }

    fn unmap_sampling_readback_slot(&mut self, slot: usize) {
        if let Some(buffers) = self.sampling_latch.buffers.as_ref() {
            buffers.readbacks[slot].unmap();
        }
    }
}
