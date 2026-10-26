use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_core::bus::DisplayYuv420Frame;

use super::PendingGpuDisplayFinalize;

/// How long the test-only blocking finalizers wait for a readback.
///
/// Production never blocks here: `poll_display_finalize_readback_ready`
/// polls with a zero timeout. The wait only has to outlast the slowest
/// adapter the tests run on, which in CI is the WARP software rasterizer.
#[cfg(test)]
const GPU_READBACK_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn begin_display_finalize_readback(
    mut pending: PendingGpuDisplayFinalize,
) -> PendingGpuDisplayFinalize {
    let slice = pending.buffer.slice(..pending.mapped_bytes);
    let (sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    pending.receiver = Some(receiver);
    pending
}

pub(super) fn poll_display_finalize_readback_ready(
    device: &wgpu::Device,
    pending: &mut PendingGpuDisplayFinalize,
) -> Result<bool> {
    if pending.map_ready {
        return Ok(true);
    }

    device
        .poll(wgpu::PollType::Poll)
        .context("GPU display finalize callback poll failed")?;
    if take_display_finalize_readback_ready(pending)? {
        return Ok(true);
    }

    match device.poll(wgpu::PollType::Wait {
        submission_index: Some(pending.submission_index.clone()),
        timeout: Some(Duration::ZERO),
    }) {
        Ok(_) | Err(wgpu::PollError::Timeout) => {}
        Err(error) => {
            return Err(error).context("GPU display finalize readiness poll failed");
        }
    }

    device
        .poll(wgpu::PollType::Poll)
        .context("GPU display finalize callback poll failed")?;
    take_display_finalize_readback_ready(pending)
}

#[cfg(test)]
pub(super) fn wait_for_display_finalize_readback(
    device: &wgpu::Device,
    pending: &mut PendingGpuDisplayFinalize,
) -> Result<bool> {
    if pending.map_ready {
        return Ok(true);
    }

    match device.poll(wgpu::PollType::Wait {
        submission_index: Some(pending.submission_index.clone()),
        timeout: Some(GPU_READBACK_WAIT_TIMEOUT),
    }) {
        Ok(_) => {}
        Err(wgpu::PollError::Timeout) => return Ok(false),
        Err(error) => return Err(error).context("GPU display finalize wait failed"),
    }

    if take_display_finalize_readback_ready(pending)? {
        return Ok(true);
    }

    device
        .poll(wgpu::PollType::Poll)
        .context("GPU display finalize callback poll failed")?;
    take_display_finalize_readback_ready(pending)
}

fn take_display_finalize_readback_ready(pending: &mut PendingGpuDisplayFinalize) -> Result<bool> {
    let Some(receiver) = pending.receiver.take() else {
        return Ok(pending.map_ready);
    };
    match receiver.try_recv() {
        Ok(Ok(())) => {
            pending.map_ready = true;
            Ok(true)
        }
        // The caller's failed-map cleanup unmaps the buffer.
        Ok(Err(error)) => Err(error).context("GPU display finalize buffer mapping failed"),
        Err(TryRecvError::Disconnected) => {
            anyhow::bail!("GPU display finalize channel closed before map completion");
        }
        Err(TryRecvError::Empty) => {
            pending.receiver = Some(receiver);
            Ok(false)
        }
    }
}

/// Copy a mapped YUV display readback out and unmap its buffer, on success
/// and on error, the way the shared RGBA readback copy does.
pub(super) fn finish_yuv420_display_readback(
    pending: &PendingGpuDisplayFinalize,
) -> Result<DisplayYuv420Frame> {
    let slice = pending.buffer.slice(..pending.mapped_bytes);
    let mapped = match slice.get_mapped_range() {
        Ok(mapped) => mapped,
        Err(error) => {
            pending.unmap();
            return Err(super::super::readback::unmapped_readback_error(error))
                .context("GPU display finalize YUV readback");
        }
    };
    let used_len = usize::try_from(pending.used_bytes).expect("YUV readback should fit usize");
    let mut data = Vec::with_capacity(used_len);
    data.extend_from_slice(&mapped[..used_len]);
    drop(mapped);
    pending.unmap();
    super::super::record_gpu_readback(super::super::GpuReadbackKind::Display, pending.used_bytes);
    let layout = pending.yuv_layout;

    Ok(DisplayYuv420Frame::from_vec(
        data,
        pending.width,
        pending.height,
        layout.y_stride,
        layout.uv_stride,
        usize::try_from(layout.y_plane_len).expect("Y plane length should fit usize"),
        usize::try_from(layout.u_plane_len).expect("U plane length should fit usize"),
        0,
        0,
    ))
}
