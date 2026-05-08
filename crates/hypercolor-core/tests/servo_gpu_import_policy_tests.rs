#![cfg(feature = "servo-gpu-import")]

use hypercolor_core::effect::{
    servo_gpu_import_mode, servo_gpu_import_should_attempt, set_servo_gpu_import_mode,
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
