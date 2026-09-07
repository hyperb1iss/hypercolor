#![cfg(feature = "servo-context")]

#[test]
fn servo_platform_exists_only_on_its_operating_system() {
    let platform = hypercolor_macos_gpu_interop::servo_render_platform();
    assert_eq!(platform.is_some(), cfg!(target_os = "macos"));
    if let Some(platform) = platform {
        assert!(!platform.name().is_empty());
    }
}
