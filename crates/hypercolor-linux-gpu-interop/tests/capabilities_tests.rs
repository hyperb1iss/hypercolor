use std::ffi::{CStr, c_void};
use std::ptr;

use hypercolor_linux_gpu_interop::{
    GlExternalMemoryFunctions, ImportedFrameFormat, LinuxGlFramebufferImportDescriptor,
    LinuxGpuInteropError, missing_gl_external_memory_functions,
};

#[test]
fn imported_frame_format_maps_rgba8() {
    assert_eq!(
        ImportedFrameFormat::Rgba8Unorm.wgpu_format(),
        wgpu::TextureFormat::Rgba8Unorm
    );
    assert_eq!(
        ImportedFrameFormat::Rgba8Unorm.gl_internal_format(),
        glow::RGBA8
    );
}

#[test]
fn import_descriptor_rejects_invalid_dimensions() {
    assert!(matches!(
        LinuxGlFramebufferImportDescriptor::new(0, 64, ImportedFrameFormat::Rgba8Unorm),
        Err(LinuxGpuInteropError::InvalidDimensions {
            width: 0,
            height: 64
        })
    ));
    assert!(
        LinuxGlFramebufferImportDescriptor::new(64, 64, ImportedFrameFormat::Rgba8Unorm).is_ok()
    );
}

#[test]
fn import_slot_exhaustion_error_reports_slot_count() {
    let error = LinuxGpuInteropError::ImportSlotsExhausted { slot_count: 8 };

    assert_eq!(error.to_string(), "all 8 GPU import slots are still in use");
}

#[test]
fn missing_gl_function_report_is_stable() {
    let missing = missing_gl_external_memory_functions(|_| ptr::null());

    assert_eq!(
        missing,
        vec![
            "glCreateMemoryObjectsEXT",
            "glMemoryObjectParameterivEXT",
            "glImportMemoryFdEXT",
            "glTexStorageMem2DEXT",
            "glDeleteMemoryObjectsEXT",
        ]
    );
}

#[test]
fn gl_loader_reports_first_missing_symbol() {
    let result = GlExternalMemoryFunctions::load_from(|_| ptr::null());

    #[cfg(target_os = "linux")]
    assert!(matches!(
        result,
        Err(LinuxGpuInteropError::MissingGlFunction(
            "glCreateMemoryObjectsEXT"
        ))
    ));
    #[cfg(not(target_os = "linux"))]
    assert!(matches!(
        result,
        Err(LinuxGpuInteropError::UnsupportedPlatform)
    ));
}

#[test]
fn gl_loader_accepts_all_required_symbols() {
    unsafe extern "system" fn create_memory_objects_ext(_count: i32, _objects: *mut u32) {}
    unsafe extern "system" fn memory_object_parameteriv_ext(
        _memory: u32,
        _pname: u32,
        _params: *const i32,
    ) {
    }
    unsafe extern "system" fn import_memory_fd_ext(
        _memory: u32,
        _size: u64,
        _handle_type: u32,
        _fd: i32,
    ) {
    }
    unsafe extern "system" fn tex_storage_mem_2d_ext(
        _target: u32,
        _levels: i32,
        _internal_format: u32,
        _width: i32,
        _height: i32,
        _memory: u32,
        _offset: u64,
    ) {
    }
    unsafe extern "system" fn delete_memory_objects_ext(_count: i32, _objects: *const u32) {}

    let result = GlExternalMemoryFunctions::load_from(|symbol| {
        symbol_ptr(
            symbol,
            create_memory_objects_ext,
            memory_object_parameteriv_ext,
            import_memory_fd_ext,
            tex_storage_mem_2d_ext,
            delete_memory_objects_ext,
        )
    });

    #[cfg(target_os = "linux")]
    assert!(result.is_ok());
    #[cfg(not(target_os = "linux"))]
    assert!(matches!(
        result,
        Err(LinuxGpuInteropError::UnsupportedPlatform)
    ));
}

fn symbol_ptr(
    symbol: &CStr,
    create_memory_objects_ext: unsafe extern "system" fn(i32, *mut u32),
    memory_object_parameteriv_ext: unsafe extern "system" fn(u32, u32, *const i32),
    import_memory_fd_ext: unsafe extern "system" fn(u32, u64, u32, i32),
    tex_storage_mem_2d_ext: unsafe extern "system" fn(u32, i32, u32, i32, i32, u32, u64),
    delete_memory_objects_ext: unsafe extern "system" fn(i32, *const u32),
) -> *const c_void {
    match symbol.to_bytes() {
        b"glCreateMemoryObjectsEXT" => create_memory_objects_ext as *const c_void,
        b"glMemoryObjectParameterivEXT" => memory_object_parameteriv_ext as *const c_void,
        b"glImportMemoryFdEXT" => import_memory_fd_ext as *const c_void,
        b"glTexStorageMem2DEXT" => tex_storage_mem_2d_ext as *const c_void,
        b"glDeleteMemoryObjectsEXT" => delete_memory_objects_ext as *const c_void,
        _ => ptr::null(),
    }
}
