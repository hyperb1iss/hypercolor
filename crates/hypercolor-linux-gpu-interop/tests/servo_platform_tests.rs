#![cfg(feature = "servo-context")]

#[test]
fn servo_platform_exists_only_on_its_operating_system() {
    let platform = hypercolor_linux_gpu_interop::servo_render_platform();
    assert_eq!(platform.is_some(), cfg!(target_os = "linux"));
    if let Some(platform) = platform {
        assert!(!platform.name().is_empty());
    }
}
