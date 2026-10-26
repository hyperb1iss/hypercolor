//! Buffers created with their contents already in place.

use anyhow::{Result, bail};

/// Create a buffer that holds `contents`, the way
/// `wgpu::util::DeviceExt::create_buffer_init` does, and report a buffer the
/// device could not create as an error instead of panicking.
///
/// A buffer whose creation failed, because the device ran out of memory or
/// was lost, is invalid and has no mapping; `create_buffer_init` panics on
/// that missing mapping. The missing mapping is also the signal here, so the
/// success path costs nothing extra. wgpu treats a buffer allocation that
/// runs out of memory as recoverable, so callers must not record the upload
/// as done when this fails: the next frame has to retry it.
///
/// The check only sees failures the backend reports. The Vulkan, DX12, and
/// Metal backends report a failed allocation, but wgpu-hal 30.0.1's GLES
/// backend calls `glBufferStorage` / `glBufferData` in `create_buffer`
/// without checking for an allocation error (wgpu-hal `src/gles/device.rs`),
/// and a mapped-at-creation buffer there can map a separate staging
/// allocation. On GL a destination buffer that failed to allocate can
/// therefore still come back with a mapping and pass this check.
pub(super) fn create_buffer_with_contents(
    device: &wgpu::Device,
    label: &str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> Result<wgpu::Buffer> {
    let align_mask = wgpu::COPY_BUFFER_ALIGNMENT - 1;
    let size = ((contents.len() as wgpu::BufferAddress).saturating_add(align_mask) & !align_mask)
        .max(wgpu::COPY_BUFFER_ALIGNMENT);
    #[cfg(test)]
    let usage = if test_failure_due() {
        // No usage is a validation error, so the device stays alive and the
        // buffer comes back invalid, the way an out-of-memory buffer does.
        wgpu::BufferUsages::empty()
    } else {
        usage
    };
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: true,
    });
    let Ok(mut mapped) = buffer.slice(..).get_mapped_range_mut() else {
        bail!("GPU could not create the {label} buffer");
    };
    mapped.slice(..contents.len()).copy_from_slice(contents);
    drop(mapped);
    buffer.unmap();
    Ok(buffer)
}

#[cfg(test)]
thread_local! {
    /// Successful calls left before one fails, when a failure is armed.
    static CALLS_BEFORE_FAILURE: std::cell::Cell<Option<u32>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn test_failure_due() -> bool {
    match CALLS_BEFORE_FAILURE.get() {
        Some(0) => {
            CALLS_BEFORE_FAILURE.set(None);
            true
        }
        Some(remaining) => {
            CALLS_BEFORE_FAILURE.set(Some(remaining - 1));
            false
        }
        None => false,
    }
}

/// Let `succeeding` more `create_buffer_with_contents` calls on this thread
/// succeed, then hand back one invalid buffer, the way an allocation that
/// ran out of memory does.
#[cfg(test)]
pub(super) fn fail_buffer_with_contents_after_for_test(succeeding: u32) {
    CALLS_BEFORE_FAILURE.set(Some(succeeding));
}
