use hypercolor_macos_gpu_interop::{
    ImportedFrameFormat, MacosGpuInteropError, MacosIosurfaceImportDescriptor,
};

#[test]
fn descriptor_rejects_zero_sized_frames() {
    assert!(MacosIosurfaceImportDescriptor::new(0, 1, ImportedFrameFormat::Bgra8Unorm).is_err());
    assert!(MacosIosurfaceImportDescriptor::new(1, 0, ImportedFrameFormat::Bgra8Unorm).is_err());
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
