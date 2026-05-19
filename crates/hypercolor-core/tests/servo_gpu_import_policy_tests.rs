#![cfg(feature = "servo-gpu-import")]

use hypercolor_core::effect::{
    servo_gpu_import_auto_backoff_remaining, servo_gpu_import_mode, servo_gpu_import_note_failure,
    servo_gpu_import_note_success, servo_gpu_import_should_attempt, set_servo_gpu_import_mode,
};
use hypercolor_types::config::ServoGpuImportMode;

#[test]
fn servo_gpu_import_mode_controls_attempts() {
    set_servo_gpu_import_mode(ServoGpuImportMode::Off);
    assert_eq!(servo_gpu_import_mode(), ServoGpuImportMode::Off);
    assert!(!servo_gpu_import_should_attempt());

    set_servo_gpu_import_mode(ServoGpuImportMode::On);
    assert_eq!(servo_gpu_import_mode(), ServoGpuImportMode::On);
    assert!(servo_gpu_import_should_attempt());

    set_servo_gpu_import_mode(ServoGpuImportMode::Off);
}

#[test]
fn servo_gpu_import_auto_failure_enters_temporary_backoff() {
    set_servo_gpu_import_mode(ServoGpuImportMode::Auto);
    assert!(servo_gpu_import_auto_backoff_remaining().is_none());

    let cooldown =
        servo_gpu_import_note_failure().expect("auto import failure should enter cooldown");

    assert!(!cooldown.is_zero());
    assert!(servo_gpu_import_auto_backoff_remaining().is_some());

    set_servo_gpu_import_mode(ServoGpuImportMode::On);
    assert!(servo_gpu_import_auto_backoff_remaining().is_none());
    assert!(servo_gpu_import_should_attempt());

    set_servo_gpu_import_mode(ServoGpuImportMode::Off);
}

#[test]
fn servo_gpu_import_success_clears_auto_backoff() {
    set_servo_gpu_import_mode(ServoGpuImportMode::Auto);
    assert!(servo_gpu_import_note_failure().is_some());
    assert!(servo_gpu_import_auto_backoff_remaining().is_some());

    servo_gpu_import_note_success();

    assert!(servo_gpu_import_auto_backoff_remaining().is_none());

    set_servo_gpu_import_mode(ServoGpuImportMode::Off);
}
