use hypercolor_macos_gpu_interop::{
    ImportedFrameFormat, MacosGpuInteropError, MacosIosurfaceImportDescriptor,
    MacosMetal4CapabilityProbe,
};

#[test]
fn descriptor_rejects_zero_sized_frames() {
    assert!(MacosIosurfaceImportDescriptor::new(0, 1, ImportedFrameFormat::Bgra8Unorm).is_err());
    assert!(MacosIosurfaceImportDescriptor::new(1, 0, ImportedFrameFormat::Bgra8Unorm).is_err());
}

#[test]
fn metal4_probe_requires_every_facility() {
    let complete = MacosMetal4CapabilityProbe {
        metal_registry_id: 42,
        metal4_family: true,
        command_allocator: true,
        command_queue: true,
        command_buffer: true,
        residency_set: true,
    };
    assert!(complete.all_required_facilities());
    assert_eq!(complete.missing_facilities(), [None; 5]);

    let missing_command_buffer = MacosMetal4CapabilityProbe {
        command_buffer: false,
        ..complete
    };
    assert!(!missing_command_buffer.all_required_facilities());
    assert_eq!(
        missing_command_buffer.missing_facilities(),
        [None, None, None, Some("command_buffer"), None]
    );
}

#[test]
fn descriptor_rejects_iosurface_row_shapes_that_exceed_cfnumber_i32() {
    let width = i32::MAX as u32 / 4 + 1;
    let error = MacosIosurfaceImportDescriptor::new(width, 1, ImportedFrameFormat::Bgra8Unorm)
        .expect_err("IOSurface row byte count must fit CFNumber i32");

    assert_eq!(
        error,
        MacosGpuInteropError::InvalidDimensions { width, height: 1 }
    );
}

#[test]
fn descriptor_accepts_largest_iosurface_row_shape() {
    let width = i32::MAX as u32 / 4;
    let descriptor = MacosIosurfaceImportDescriptor::new(width, 1, ImportedFrameFormat::Bgra8Unorm)
        .expect("maximum IOSurface row byte count should fit CFNumber i32");

    assert_eq!(descriptor.width, width);
    assert_eq!(descriptor.height, 1);
    assert_eq!(descriptor.format, ImportedFrameFormat::Bgra8Unorm);
}

#[test]
fn capture_plane_formats_map_to_exact_wgpu_formats() {
    let mappings = [
        (
            ImportedFrameFormat::Bgra8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
        ),
        (
            ImportedFrameFormat::Rgba16Float,
            wgpu::TextureFormat::Rgba16Float,
        ),
        (ImportedFrameFormat::R8Unorm, wgpu::TextureFormat::R8Unorm),
        (ImportedFrameFormat::Rg8Unorm, wgpu::TextureFormat::Rg8Unorm),
        (ImportedFrameFormat::R16Unorm, wgpu::TextureFormat::R16Unorm),
        (
            ImportedFrameFormat::Rg16Unorm,
            wgpu::TextureFormat::Rg16Unorm,
        ),
    ];

    for (format, expected) in mappings {
        assert_eq!(format.wgpu_format(), expected);
    }
}

#[test]
fn descriptor_bounds_each_format_by_its_exact_texel_width() {
    let formats = [
        (ImportedFrameFormat::R8Unorm, 1),
        (ImportedFrameFormat::Rg8Unorm, 2),
        (ImportedFrameFormat::R16Unorm, 2),
        (ImportedFrameFormat::Bgra8Unorm, 4),
        (ImportedFrameFormat::Rg16Unorm, 4),
        (ImportedFrameFormat::Rgba16Float, 8),
    ];

    for (format, bytes_per_texel) in formats {
        let maximum_width = i32::MAX as u32 / bytes_per_texel;
        assert!(MacosIosurfaceImportDescriptor::new(maximum_width, 1, format).is_ok());
        assert!(MacosIosurfaceImportDescriptor::new(maximum_width + 1, 1, format).is_err());
    }
}
