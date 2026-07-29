use std::mem::size_of;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::E_OUTOFMEMORY;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_ASYNC_GETDATA_DONOTFLUSH, D3D11_QUERY_DESC, D3D11_QUERY_EVENT, D3D11_TEXTURE2D_DESC,
    ID3D11Device, ID3D11DeviceContext, ID3D11Query, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::core::BOOL;

use super::{
    BYTES_PER_PIXEL, CaptureMetadata, MappedTexture, RetainedDesktop, checked_rgba_len,
    classify_windows_error, create_staging_texture,
};
use crate::{
    CaptureError, CaptureExtent, CaptureLane, CaptureResult, CpuDesktopFrame, DisplayRotation,
    GpuAdapterLuid, GpuSurfaceSourceColorSpace,
};

struct PendingReadback {
    metadata: CaptureMetadata,
}

struct ReadbackSlot {
    staging: ID3D11Texture2D,
    query: ID3D11Query,
    pending: Option<PendingReadback>,
    progress_kicked: bool,
}

/// Fixed-capacity asynchronous native BGRA readback prepared for one session.
pub struct PreparedCpuDesktopReadback {
    source_id: Arc<str>,
    topology_generation: u64,
    duplication_generation: u64,
    adapter_luid: GpuAdapterLuid,
    source_extent: CaptureExtent,
    source_rotation: DisplayRotation,
    source_color_space: GpuSurfaceSourceColorSpace,
    context: ID3D11DeviceContext,
    slots: Vec<ReadbackSlot>,
    write_index: usize,
    read_index: usize,
    frame_pool: Arc<Mutex<Vec<Vec<u8>>>>,
    allocation_byte_len: u64,
    mapped_byte_len: u64,
    last_submitted_sequence: Option<u64>,
}

impl std::fmt::Debug for PreparedCpuDesktopReadback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedCpuDesktopReadback")
            .field("source_id", &self.source_id)
            .field("topology_generation", &self.topology_generation)
            .field("duplication_generation", &self.duplication_generation)
            .field("adapter_luid", &self.adapter_luid)
            .field("source_extent", &self.source_extent)
            .field("slot_count", &self.slots.len())
            .field("allocation_byte_len", &self.allocation_byte_len)
            .field("mapped_byte_len", &self.mapped_byte_len)
            .finish_non_exhaustive()
    }
}

impl PreparedCpuDesktopReadback {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        source_id: Arc<str>,
        topology_generation: u64,
        duplication_generation: u64,
        adapter_luid: GpuAdapterLuid,
        source_extent: CaptureExtent,
        source_rotation: DisplayRotation,
        source_color_space: GpuSurfaceSourceColorSpace,
        slot_count: NonZeroU32,
    ) -> CaptureResult<Self> {
        let frame_bytes = checked_rgba_len(
            source_extent.width(),
            source_extent.height(),
            "allocate native CPU capture frame",
        )?;
        let slot_count =
            usize::try_from(slot_count.get()).map_err(|_| CaptureError::ResourceExhausted {
                operation: "allocate native CPU readback slots",
                requested_bytes: usize::MAX,
            })?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: source_extent.width(),
            Height: source_extent.height(),
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..D3D11_TEXTURE2D_DESC::default()
        };
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(slot_count)
            .map_err(|_| CaptureError::ResourceExhausted {
                operation: "allocate native CPU readback slots",
                requested_bytes: slot_count.saturating_mul(size_of::<ReadbackSlot>()),
            })?;
        for _ in 0..slot_count {
            slots.push(ReadbackSlot {
                staging: create_staging_texture(device, &desc)?,
                query: create_event_query(device)?,
                pending: None,
                progress_kicked: false,
            });
        }

        let mut buffers = Vec::new();
        buffers
            .try_reserve_exact(slot_count)
            .map_err(|_| CaptureError::ResourceExhausted {
                operation: "allocate native CPU frame pool",
                requested_bytes: slot_count.saturating_mul(size_of::<Vec<u8>>()),
            })?;
        for _ in 0..slot_count {
            let mut buffer = Vec::new();
            buffer
                .try_reserve_exact(frame_bytes)
                .map_err(|_| CaptureError::ResourceExhausted {
                    operation: "allocate native CPU capture frame",
                    requested_bytes: frame_bytes,
                })?;
            buffers.push(buffer);
        }

        let allocation_byte_len = u64::try_from(frame_bytes)
            .unwrap_or(u64::MAX)
            .checked_mul(u64::try_from(slot_count).unwrap_or(u64::MAX))
            .and_then(|bytes| bytes.checked_mul(2))
            .ok_or(CaptureError::GeometryOverflow {
                operation: "account native CPU readback",
                width: source_extent.width(),
                height: source_extent.height(),
            })?;
        Ok(Self {
            source_id,
            topology_generation,
            duplication_generation,
            adapter_luid,
            source_extent,
            source_rotation,
            source_color_space,
            context: context.clone(),
            slots,
            write_index: 0,
            read_index: 0,
            frame_pool: Arc::new(Mutex::new(buffers)),
            allocation_byte_len,
            mapped_byte_len: 0,
            last_submitted_sequence: None,
        })
    }

    /// Fixed number of independently pending GPU readbacks.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Checked bytes reserved by staging textures and pooled output planes.
    #[must_use]
    pub const fn allocation_byte_len(&self) -> u64 {
        self.allocation_byte_len
    }

    /// Cumulative bytes copied from mapped staging surfaces.
    #[must_use]
    pub const fn mapped_byte_len(&self) -> u64 {
        self.mapped_byte_len
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn matches_source(
        &self,
        source_id: &str,
        topology_generation: u64,
        duplication_generation: u64,
        adapter_luid: GpuAdapterLuid,
        source_extent: CaptureExtent,
        source_rotation: DisplayRotation,
        source_color_space: GpuSurfaceSourceColorSpace,
    ) -> bool {
        self.source_id.as_ref() == source_id
            && self.topology_generation == topology_generation
            && self.duplication_generation == duplication_generation
            && self.adapter_luid == adapter_luid
            && self.source_extent == source_extent
            && self.source_rotation == source_rotation
            && self.source_color_space == source_color_space
    }

    pub(super) fn submit(&mut self, clean: &RetainedDesktop) -> CaptureResult<bool> {
        let slot = &mut self.slots[self.write_index];
        if slot.pending.is_some() {
            return Ok(false);
        }
        let mut observed = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: GetDesc fills caller-owned storage and cannot fail.
        unsafe { clean.texture.GetDesc(&mut observed) };
        if observed.Width != self.source_extent.width()
            || observed.Height != self.source_extent.height()
            || observed.Format != DXGI_FORMAT_B8G8R8A8_UNORM
        {
            return Err(CaptureError::GpuSurfacePlanInvalidated);
        }
        // SAFETY: staging and clean share exact geometry and format.
        unsafe {
            self.context.CopyResource(&slot.staging, &clean.texture);
            self.context.End(&slot.query);
        }
        slot.pending = Some(PendingReadback {
            metadata: clean.metadata.clone(),
        });
        self.last_submitted_sequence = Some(clean.metadata.sequence);
        slot.progress_kicked = false;
        self.write_index = (self.write_index + 1) % self.slots.len();
        Ok(true)
    }

    pub(super) fn should_submit(&self, clean: &RetainedDesktop) -> bool {
        self.last_submitted_sequence != Some(clean.metadata.sequence)
    }

    pub(super) fn has_pending(&self) -> bool {
        self.slots.iter().any(|slot| slot.pending.is_some())
    }

    pub(super) fn poll(&mut self) -> CaptureLane<CpuDesktopFrame> {
        match self.poll_inner() {
            Ok(outcome) => outcome,
            Err(error) => CaptureLane::Failed(error),
        }
    }

    fn poll_inner(&mut self) -> CaptureResult<CaptureLane<CpuDesktopFrame>> {
        let slot = &mut self.slots[self.read_index];
        if slot.pending.is_none() {
            return Ok(CaptureLane::Idle);
        }
        let mut ready = BOOL::default();
        let flags = if slot.progress_kicked {
            D3D11_ASYNC_GETDATA_DONOTFLUSH.0.cast_unsigned()
        } else {
            slot.progress_kicked = true;
            0
        };
        // SAFETY: the event query is live and ready is valid BOOL storage.
        unsafe {
            self.context.GetData(
                &slot.query,
                Some((&raw mut ready).cast()),
                u32::try_from(size_of::<BOOL>()).unwrap_or(u32::MAX),
                flags,
            )
        }
        .map_err(|source| classify_windows_error("poll native CPU readback", source))?;
        if !ready.as_bool() {
            return Ok(CaptureLane::Idle);
        }

        let Some(mut bgra) = self
            .frame_pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
        else {
            return Ok(CaptureLane::Busy);
        };
        let copy = copy_mapped_rows(&self.context, &slot.staging, self.source_extent, &mut bgra);
        if let Err(error) = copy {
            bgra.clear();
            self.frame_pool
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(bgra);
            return Err(error);
        }
        let pending = slot
            .pending
            .take()
            .expect("a ready native CPU query has pending metadata");
        slot.progress_kicked = false;
        self.read_index = (self.read_index + 1) % self.slots.len();
        self.mapped_byte_len = self
            .mapped_byte_len
            .saturating_add(u64::try_from(bgra.len()).unwrap_or(u64::MAX));
        let metadata = pending.metadata;
        Ok(CaptureLane::Ready(CpuDesktopFrame::new(
            metadata.source_id,
            metadata.topology_generation,
            self.duplication_generation,
            metadata.sequence,
            metadata.captured_at,
            metadata.cursor,
            metadata.source_width,
            metadata.source_height,
            metadata.origin_x,
            metadata.origin_y,
            metadata.rotation,
            metadata.source_color_space,
            bgra,
            Arc::clone(&self.frame_pool),
        )))
    }
}

fn create_event_query(device: &ID3D11Device) -> CaptureResult<ID3D11Query> {
    let desc = D3D11_QUERY_DESC {
        Query: D3D11_QUERY_EVENT,
        MiscFlags: 0,
    };
    let mut query = None;
    // SAFETY: the query descriptor and out-pointer remain live.
    unsafe { device.CreateQuery(&desc, Some(&mut query)) }.map_err(|source| {
        if source.code() == E_OUTOFMEMORY {
            CaptureError::ResourceExhausted {
                operation: "create native CPU readback query",
                requested_bytes: size_of::<D3D11_QUERY_DESC>(),
            }
        } else {
            classify_windows_error("create native CPU readback query", source)
        }
    })?;
    query.ok_or_else(|| {
        CaptureError::windows(
            "create native CPU readback query",
            "D3D11 returned no query",
        )
    })
}

fn copy_mapped_rows(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    extent: CaptureExtent,
    bgra: &mut Vec<u8>,
) -> CaptureResult<()> {
    let output_len = checked_rgba_len(extent.width(), extent.height(), "copy native CPU readback")?;
    if bgra.capacity() < output_len {
        return Err(CaptureError::ResourceExhausted {
            operation: "reuse native CPU capture frame",
            requested_bytes: output_len,
        });
    }
    let mapped = MappedTexture::map(context, staging)?;
    let rows = mapped.rows(extent.width(), extent.height())?;
    let row_bytes = extent.width() as usize * BYTES_PER_PIXEL;
    bgra.resize(output_len, 0);
    for row in 0..extent.height() as usize {
        let source = row * rows.row_pitch;
        let target = row * row_bytes;
        bgra[target..target + row_bytes].copy_from_slice(&rows.bytes[source..source + row_bytes]);
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn packed_readback_for_test(
    bgra: &[u8],
    width: u32,
    height: u32,
) -> CaptureResult<CpuDesktopFrame> {
    let (device, context) = super::gpu_reduction::test_device()
        .map_err(|error| CaptureError::windows("create CPU readback fixture", error))?;
    let source = super::gpu_reduction::test_source(&device, bgra, width, height)
        .map_err(|error| CaptureError::windows("create CPU readback fixture source", error))?;
    let metadata = test_metadata(width, height, 1);
    let clean = RetainedDesktop {
        srv: super::gpu_reduction::create_srv(&device, &source)
            .map_err(|error| CaptureError::windows("create CPU readback fixture view", error))?,
        texture: source,
        metadata,
    };
    let mut readback = PreparedCpuDesktopReadback::prepare(
        &device,
        &context,
        Arc::from("fixture:cpu-readback"),
        3,
        5,
        GpuAdapterLuid::new(0, 0),
        CaptureExtent::try_new(width, height)?,
        DisplayRotation::Identity,
        GpuSurfaceSourceColorSpace::RgbFullG22P709,
        NonZeroU32::MIN,
    )?;
    assert!(readback.submit(&clean)?);
    await_frame(&mut readback)
}

#[cfg(test)]
pub(super) fn retained_frame_exhausts_bounded_pool_for_test() -> CaptureResult<(bool, u64, u64)> {
    let (device, context) = super::gpu_reduction::test_device()
        .map_err(|error| CaptureError::windows("create CPU pool fixture", error))?;
    let pixels = [10, 20, 30, 0xFF].repeat(15);
    let source = super::gpu_reduction::test_source(&device, &pixels, 5, 3)
        .map_err(|error| CaptureError::windows("create CPU pool fixture source", error))?;
    let mut clean = RetainedDesktop {
        srv: super::gpu_reduction::create_srv(&device, &source)
            .map_err(|error| CaptureError::windows("create CPU pool fixture view", error))?,
        texture: source,
        metadata: test_metadata(5, 3, 1),
    };
    let mut readback = PreparedCpuDesktopReadback::prepare(
        &device,
        &context,
        Arc::from("fixture:cpu-readback"),
        3,
        5,
        GpuAdapterLuid::new(0, 0),
        CaptureExtent::try_new(5, 3)?,
        DisplayRotation::Identity,
        GpuSurfaceSourceColorSpace::RgbFullG22P709,
        NonZeroU32::MIN,
    )?;
    assert!(readback.submit(&clean)?);
    let first = await_frame(&mut readback)?;
    let first_mapped = readback.mapped_byte_len();

    clean.metadata = test_metadata(5, 3, 2);
    assert!(readback.submit(&clean)?);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let busy = loop {
        match readback.poll() {
            CaptureLane::Busy => break true,
            CaptureLane::Idle if std::time::Instant::now() < deadline => {
                std::thread::yield_now();
            }
            CaptureLane::Failed(error) => return Err(error),
            CaptureLane::Ready(_) => break false,
            CaptureLane::Idle | CaptureLane::NotRequested => {
                return Err(CaptureError::windows(
                    "poll CPU pool fixture",
                    "readback did not complete within two seconds",
                ));
            }
        }
    };
    let still_mapped = readback.mapped_byte_len();
    drop(first);
    let second = await_frame(&mut readback)?;
    drop(second);
    Ok((
        busy,
        still_mapped.saturating_sub(first_mapped),
        readback.mapped_byte_len(),
    ))
}

#[cfg(test)]
fn await_frame(readback: &mut PreparedCpuDesktopReadback) -> CaptureResult<CpuDesktopFrame> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match readback.poll() {
            CaptureLane::Ready(frame) => return Ok(frame),
            CaptureLane::Failed(error) => return Err(error),
            CaptureLane::Idle if std::time::Instant::now() < deadline => {
                std::thread::yield_now();
            }
            CaptureLane::Busy => {
                return Err(CaptureError::windows(
                    "poll CPU readback fixture",
                    "bounded output pool is unexpectedly busy",
                ));
            }
            CaptureLane::Idle | CaptureLane::NotRequested => {
                return Err(CaptureError::windows(
                    "poll CPU readback fixture",
                    "readback did not complete within two seconds",
                ));
            }
        }
    }
}

#[cfg(test)]
fn test_metadata(width: u32, height: u32, sequence: u64) -> CaptureMetadata {
    CaptureMetadata {
        source_id: Arc::from("fixture:cpu-readback"),
        topology_generation: 3,
        sequence,
        captured_at: std::time::Instant::now(),
        cursor: crate::CursorInfo::default(),
        pointer: Arc::new(super::PointerState::default()),
        source_width: width,
        source_height: height,
        origin_x: -17,
        origin_y: 23,
        rotation: DisplayRotation::Identity,
        source_color_space: GpuSurfaceSourceColorSpace::RgbFullG22P709,
        region: crate::CaptureRegion::full(width, height),
    }
}
