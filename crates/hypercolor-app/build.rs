fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&[
        "is_first_run_pending",
        "mark_first_run_complete",
        "reset_first_run",
        "choose_daemon_owner",
        "execute_macos_daemon_owner_offline_remedy",
        "macos_daemon_owner_offline_status",
        "restart_macos_capture_owner",
        "detect_pawnio_support",
        "detect_daemon_launcher",
        "launch_pawnio_helper",
        "repair_smbus_service",
        "open_external_url",
        "open_macos_system_settings",
        "get_verified_daemon_connection",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to build Tauri application manifest");
}
