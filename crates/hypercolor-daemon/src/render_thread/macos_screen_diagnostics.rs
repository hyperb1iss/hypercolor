use std::sync::{Arc, mpsc};

use anyhow::{Context, Result, anyhow};
use hypercolor_core::input::screen::{
    CapturePixelFormat, ResolvedScreenPublicationDescriptor, ScreenBranchPublication,
    ScreenPublicationFreshness, ScreenPublicationHealth,
};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::event::ZoneColors;
use thiserror::Error;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use super::gpu_device::GpuRenderDevice;
use super::producer_queue::GpuTextureFrame;
use super::sparkleflinger::SparkleFlinger;

const REQUEST_CAPACITY: usize = 1;

#[derive(Clone)]
pub(crate) struct MacosScreenParityDiagnosticHandle {
    sender: tokio_mpsc::Sender<MacosScreenParityRequest>,
}

pub(crate) struct MacosScreenParityDiagnosticMailbox {
    receiver: tokio_mpsc::Receiver<MacosScreenParityRequest>,
}

struct MacosScreenParityRequest {
    response: oneshot::Sender<
        std::result::Result<MacosScreenParityLiveSnapshot, MacosScreenParitySnapshotError>,
    >,
}

pub(crate) struct MacosScreenParityLiveSnapshot {
    pub(crate) publication: Arc<ScreenBranchPublication>,
    pub(crate) descriptor: ResolvedScreenPublicationDescriptor,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba8: Vec<u8>,
    pub(crate) zones: Vec<ZoneColors>,
    pub(crate) spatial_engine: SpatialEngine,
}

#[derive(Debug, Error)]
pub(crate) enum MacosScreenParitySnapshotError {
    #[error("the active renderer stopped before servicing the parity request")]
    RendererStopped,
    #[error("the active renderer has no live screen publication")]
    NoActiveScreenPublication,
    #[error("the active publication and descriptor identities do not match")]
    PublicationIdentityChanged,
    #[error("the active screen branch does not publish RGBA8 output")]
    UnsupportedOutputFormat,
    #[error("the active native screen reduction could not be copied")]
    NativeReductionFailed,
    #[error("the active native screen surface could not be read back")]
    SurfaceReadbackFailed,
    #[error("the active GPU sampler could not accept the diagnostic output")]
    SamplingUnavailable,
    #[error("the active spatial sampler could not produce final zone colors")]
    SpatialSamplingFailed,
}

pub(crate) fn macos_screen_parity_diagnostic_channel() -> (
    MacosScreenParityDiagnosticHandle,
    MacosScreenParityDiagnosticMailbox,
) {
    let (sender, receiver) = tokio_mpsc::channel(REQUEST_CAPACITY);
    (
        MacosScreenParityDiagnosticHandle { sender },
        MacosScreenParityDiagnosticMailbox { receiver },
    )
}

impl MacosScreenParityDiagnosticHandle {
    pub(crate) async fn snapshot(
        &self,
    ) -> std::result::Result<MacosScreenParityLiveSnapshot, MacosScreenParitySnapshotError> {
        let (response, receiver) = oneshot::channel();
        self.sender
            .send(MacosScreenParityRequest { response })
            .await
            .map_err(|_| MacosScreenParitySnapshotError::RendererStopped)?;
        receiver
            .await
            .map_err(|_| MacosScreenParitySnapshotError::RendererStopped)?
    }
}

impl MacosScreenParityDiagnosticMailbox {
    pub(crate) fn service(
        &mut self,
        render_device: &GpuRenderDevice,
        sparkleflinger: &mut SparkleFlinger,
        publication: Option<&Arc<ScreenBranchPublication>>,
        descriptor: Option<&ResolvedScreenPublicationDescriptor>,
        spatial_engine: &SpatialEngine,
    ) {
        let Ok(request) = self.receiver.try_recv() else {
            return;
        };
        let result = capture_active_snapshot(
            render_device,
            sparkleflinger,
            publication,
            descriptor,
            spatial_engine,
        );
        let _ = request.response.send(result);
    }
}

fn capture_active_snapshot(
    render_device: &GpuRenderDevice,
    sparkleflinger: &mut SparkleFlinger,
    publication: Option<&Arc<ScreenBranchPublication>>,
    descriptor: Option<&ResolvedScreenPublicationDescriptor>,
    spatial_engine: &SpatialEngine,
) -> std::result::Result<MacosScreenParityLiveSnapshot, MacosScreenParitySnapshotError> {
    let publication = publication
        .cloned()
        .ok_or(MacosScreenParitySnapshotError::NoActiveScreenPublication)?;
    let descriptor = descriptor
        .cloned()
        .ok_or(MacosScreenParitySnapshotError::NoActiveScreenPublication)?;
    if descriptor.source_epoch() != publication.source_epoch() {
        return Err(MacosScreenParitySnapshotError::PublicationIdentityChanged);
    }
    if descriptor.processing_profile().target_pixel_format() != CapturePixelFormat::Rgba8 {
        return Err(MacosScreenParitySnapshotError::UnsupportedOutputFormat);
    }
    if publication.freshness_at(std::time::Instant::now()) != ScreenPublicationFreshness::Fresh
        || publication.health() == ScreenPublicationHealth::Failed
    {
        return Err(MacosScreenParitySnapshotError::NoActiveScreenPublication);
    }
    let frame = sparkleflinger
        .copy_screen_publication(&publication)
        .map_err(|_| MacosScreenParitySnapshotError::NativeReductionFailed)?
        .ok_or(MacosScreenParitySnapshotError::NativeReductionFailed)?;
    let rgba8 = read_rgba8(render_device, &frame)
        .map_err(|_| MacosScreenParitySnapshotError::SurfaceReadbackFailed)?;
    let zones = sparkleflinger
        .sample_texture_zone_plan(&frame, spatial_engine.sampling_plan().as_ref())
        .map_err(|_| MacosScreenParitySnapshotError::SpatialSamplingFailed)?
        .ok_or(MacosScreenParitySnapshotError::SamplingUnavailable)?;
    Ok(MacosScreenParityLiveSnapshot {
        publication,
        descriptor,
        width: rgba8.width,
        height: rgba8.height,
        rgba8: rgba8.rgba8,
        zones,
        spatial_engine: spatial_engine.clone(),
    })
}

struct Rgba8Readback {
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
}

fn read_rgba8(render_device: &GpuRenderDevice, frame: &GpuTextureFrame) -> Result<Rgba8Readback> {
    anyhow::ensure!(
        frame.texture.format() == wgpu::TextureFormat::Rgba8Unorm,
        "the parity diagnostic requires an RGBA8 live target"
    );
    let row_bytes = frame
        .width
        .checked_mul(4)
        .context("live parity row length overflowed")?;
    let padded_row_bytes = row_bytes
        .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        .checked_mul(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        .context("live parity row alignment overflowed")?;
    let buffer_bytes = u64::from(padded_row_bytes)
        .checked_mul(u64::from(frame.height))
        .context("live parity readback length overflowed")?;
    let device = render_device.device();
    let queue = render_device.queue();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Hypercolor macOS screen parity readback"),
        size: buffer_bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Hypercolor macOS screen parity readback"),
    });
    encoder.copy_texture_to_buffer(
        frame.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_bytes),
                rows_per_image: Some(frame.height),
            },
        },
        wgpu::Extent3d {
            width: frame.width,
            height: frame.height,
            depth_or_array_layers: 1,
        },
    );
    let submission = queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    let (completion_tx, completion_rx) = mpsc::sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = completion_tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .map_err(|error| anyhow!("live parity GPU wait failed: {error}"))?;
    completion_rx
        .recv()
        .context("live parity map callback was dropped")?
        .map_err(|error| anyhow!("live parity buffer map failed: {error}"))?;
    let mapped = slice.get_mapped_range();
    let output_bytes = usize::try_from(row_bytes)
        .ok()
        .and_then(|row| row.checked_mul(usize::try_from(frame.height).ok()?))
        .context("live parity output length overflowed")?;
    let mut rgba8 = Vec::new();
    rgba8
        .try_reserve_exact(output_bytes)
        .map_err(|_| anyhow!("live parity output allocation failed"))?;
    let row_bytes = usize::try_from(row_bytes).context("live parity row is not addressable")?;
    let padded_row_bytes =
        usize::try_from(padded_row_bytes).context("live parity row pitch is not addressable")?;
    for row in mapped.chunks_exact(padded_row_bytes) {
        rgba8.extend_from_slice(&row[..row_bytes]);
    }
    drop(mapped);
    buffer.unmap();
    anyhow::ensure!(
        rgba8.len() == output_bytes,
        "live parity readback returned an incomplete surface"
    );
    Ok(Rgba8Readback {
        width: frame.width,
        height: frame.height,
        rgba8,
    })
}
