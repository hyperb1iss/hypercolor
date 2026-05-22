use hypercolor_windows_gpu_interop::{
    ImportedFrameFormat, WindowsD3d11SharedTextureImportDescriptor, WindowsGpuInteropError,
};

#[test]
fn descriptor_rejects_empty_dimensions() {
    assert_eq!(
        WindowsD3d11SharedTextureImportDescriptor::new(0, 600, ImportedFrameFormat::Bgra8Unorm),
        Err(WindowsGpuInteropError::InvalidDimensions {
            width: 0,
            height: 600,
        })
    );
    assert_eq!(
        WindowsD3d11SharedTextureImportDescriptor::new(800, 0, ImportedFrameFormat::Bgra8Unorm),
        Err(WindowsGpuInteropError::InvalidDimensions {
            width: 800,
            height: 0,
        })
    );
}

#[test]
fn descriptor_accepts_800_by_600_bgra() {
    let descriptor =
        WindowsD3d11SharedTextureImportDescriptor::new(800, 600, ImportedFrameFormat::Bgra8Unorm)
            .expect("800x600 BGRA should be a valid Windows import shape");

    assert_eq!(descriptor.width, 800);
    assert_eq!(descriptor.height, 600);
    assert_eq!(
        descriptor.format.wgpu_format(),
        wgpu::TextureFormat::Bgra8Unorm
    );
}
