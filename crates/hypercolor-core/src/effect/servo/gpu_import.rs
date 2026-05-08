use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::{Result, bail};
use hypercolor_types::config::ServoGpuImportMode;

static SERVO_GPU_IMPORT_DEVICE: OnceLock<wgpu::Device> = OnceLock::new();
static SERVO_GPU_IMPORT_MODE: AtomicU8 =
    AtomicU8::new(servo_gpu_import_mode_to_u8(ServoGpuImportMode::Off));

const fn servo_gpu_import_mode_to_u8(mode: ServoGpuImportMode) -> u8 {
    match mode {
        ServoGpuImportMode::Off => 0,
        ServoGpuImportMode::Auto => 1,
        ServoGpuImportMode::On => 2,
    }
}

const fn servo_gpu_import_mode_from_u8(mode: u8) -> ServoGpuImportMode {
    match mode {
        1 => ServoGpuImportMode::Auto,
        2 => ServoGpuImportMode::On,
        _ => ServoGpuImportMode::Off,
    }
}

pub fn set_servo_gpu_import_mode(mode: ServoGpuImportMode) {
    SERVO_GPU_IMPORT_MODE.store(servo_gpu_import_mode_to_u8(mode), Ordering::Relaxed);
}

pub fn servo_gpu_import_mode() -> ServoGpuImportMode {
    servo_gpu_import_mode_from_u8(SERVO_GPU_IMPORT_MODE.load(Ordering::Relaxed))
}

pub fn servo_gpu_import_should_attempt() -> bool {
    match servo_gpu_import_mode() {
        ServoGpuImportMode::Off => false,
        ServoGpuImportMode::Auto => SERVO_GPU_IMPORT_DEVICE.get().is_some(),
        ServoGpuImportMode::On => true,
    }
}

pub fn install_servo_gpu_import_device(device: wgpu::Device) -> Result<()> {
    SERVO_GPU_IMPORT_DEVICE
        .set(device)
        .map_err(|_| anyhow::anyhow!("Servo GPU import device is already installed"))?;
    Ok(())
}

pub fn servo_gpu_import_device() -> Result<&'static wgpu::Device> {
    let Some(device) = SERVO_GPU_IMPORT_DEVICE.get() else {
        bail!("Servo GPU import device is not installed");
    };
    Ok(device)
}
