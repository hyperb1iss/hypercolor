//! Buffers created with their contents already in place.

/// Create a buffer that holds `contents`, the way
/// `wgpu::util::DeviceExt::create_buffer_init` does, without panicking when
/// the device could not create it.
///
/// A buffer whose creation failed, because the device ran out of memory or
/// was lost, is invalid and has no mapping, and `create_buffer_init` panics
/// on that missing mapping. The creation failure has already reached the
/// device's error handling (an open error scope or the uncaptured-error
/// handler), so this returns the invalid buffer unwritten and every command
/// that uses it fails validation instead of the render thread unwinding.
pub(super) fn create_buffer_with_contents(
    device: &wgpu::Device,
    label: &str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let align_mask = wgpu::COPY_BUFFER_ALIGNMENT - 1;
    let size = ((contents.len() as wgpu::BufferAddress).saturating_add(align_mask) & !align_mask)
        .max(wgpu::COPY_BUFFER_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: true,
    });
    let Ok(mut mapped) = buffer.slice(..).get_mapped_range_mut() else {
        return buffer;
    };
    mapped.slice(..contents.len()).copy_from_slice(contents);
    drop(mapped);
    buffer.unmap();
    buffer
}
