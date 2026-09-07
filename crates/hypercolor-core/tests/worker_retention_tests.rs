use hypercolor_core::input::worker_retention_service_identity as core_service_identity;
use hypercolor_macos_input::worker_retention_service_identity as macos_service_identity;
use hypercolor_windows_input::worker_retention_service_identity as windows_service_identity;

#[test]
fn all_input_clients_share_one_process_cleanup_service() {
    let core = core_service_identity().expect("core cleanup service initializes");
    let windows = windows_service_identity().expect("Windows cleanup service initializes");
    let macos = macos_service_identity().expect("macOS cleanup service initializes");

    assert_eq!(core, windows);
    assert_eq!(core, macos);
}
