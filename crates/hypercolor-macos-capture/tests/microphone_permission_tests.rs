use hypercolor_macos_capture::microphone_record_permission;
use hypercolor_types::audio::AudioInputPermission;

#[cfg(target_os = "macos")]
#[test]
fn microphone_permission_reports_a_tcc_answer_on_macos() {
    let permission = microphone_record_permission();
    println!("microphone record permission: {permission:?}");
    assert_ne!(
        permission,
        AudioInputPermission::NotRequired,
        "macOS always gates audio input behind the Microphone permission"
    );
    assert_eq!(
        microphone_record_permission(),
        permission,
        "reading the permission must not change it"
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn microphone_permission_is_not_required_off_macos() {
    assert_eq!(
        microphone_record_permission(),
        AudioInputPermission::NotRequired
    );
}
