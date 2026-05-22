use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use hypercolor_types::config::ServoGpuImportMode;

static SERVO_GPU_IMPORT_DEVICE: OnceLock<wgpu::Device> = OnceLock::new();
static SERVO_GPU_IMPORT_ADAPTER_INFO: OnceLock<ServoGpuImportAdapterInfo> = OnceLock::new();
static SERVO_GPU_IMPORT_MODE: AtomicU8 =
    AtomicU8::new(servo_gpu_import_mode_to_u8(ServoGpuImportMode::Off));
static SERVO_GPU_IMPORT_CLOCK_START: OnceLock<Instant> = OnceLock::new();
static SERVO_GPU_IMPORT_AUTO_BACKOFF_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

const SERVO_GPU_IMPORT_AUTO_BACKOFF: Duration = Duration::from_mins(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServoGpuImportAdapterInfo {
    pub vendor_id: u32,
    pub device_id: u32,
}

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
    SERVO_GPU_IMPORT_AUTO_BACKOFF_UNTIL_MS.store(0, Ordering::Relaxed);
    SERVO_GPU_IMPORT_MODE.store(servo_gpu_import_mode_to_u8(mode), Ordering::Relaxed);
}

pub fn servo_gpu_import_mode() -> ServoGpuImportMode {
    servo_gpu_import_mode_from_u8(SERVO_GPU_IMPORT_MODE.load(Ordering::Relaxed))
}

pub fn servo_gpu_import_should_attempt() -> bool {
    match servo_gpu_import_mode() {
        ServoGpuImportMode::Off => false,
        ServoGpuImportMode::Auto => {
            SERVO_GPU_IMPORT_DEVICE.get().is_some()
                && servo_gpu_import_auto_backoff_remaining().is_none()
        }
        ServoGpuImportMode::On => true,
    }
}

#[must_use]
pub fn servo_gpu_import_note_failure() -> Option<Duration> {
    if !matches!(servo_gpu_import_mode(), ServoGpuImportMode::Auto) {
        return None;
    }

    let retry_after = servo_gpu_import_now_ms()
        .saturating_add(duration_millis_u64(SERVO_GPU_IMPORT_AUTO_BACKOFF));
    SERVO_GPU_IMPORT_AUTO_BACKOFF_UNTIL_MS.store(retry_after, Ordering::Relaxed);
    Some(SERVO_GPU_IMPORT_AUTO_BACKOFF)
}

pub fn servo_gpu_import_note_success() {
    SERVO_GPU_IMPORT_AUTO_BACKOFF_UNTIL_MS.store(0, Ordering::Relaxed);
}

#[must_use]
pub fn servo_gpu_import_auto_backoff_remaining() -> Option<Duration> {
    let retry_after = SERVO_GPU_IMPORT_AUTO_BACKOFF_UNTIL_MS.load(Ordering::Relaxed);
    if retry_after == 0 {
        return None;
    }

    let now = servo_gpu_import_now_ms();
    (retry_after > now).then(|| Duration::from_millis(retry_after.saturating_sub(now)))
}

pub fn install_servo_gpu_import_device(
    device: wgpu::Device,
    adapter_info: Option<ServoGpuImportAdapterInfo>,
) -> Result<()> {
    SERVO_GPU_IMPORT_DEVICE
        .set(device)
        .map_err(|_| anyhow::anyhow!("Servo GPU import device is already installed"))?;
    if let Some(adapter_info) = adapter_info {
        let _ = SERVO_GPU_IMPORT_ADAPTER_INFO.set(adapter_info);
    }
    Ok(())
}

pub fn servo_gpu_import_adapter_info() -> Option<ServoGpuImportAdapterInfo> {
    SERVO_GPU_IMPORT_ADAPTER_INFO.get().copied()
}

pub fn servo_gpu_import_device() -> Result<&'static wgpu::Device> {
    let Some(device) = SERVO_GPU_IMPORT_DEVICE.get() else {
        bail!("Servo GPU import device is not installed");
    };
    Ok(device)
}

fn servo_gpu_import_now_ms() -> u64 {
    duration_millis_u64(
        SERVO_GPU_IMPORT_CLOCK_START
            .get_or_init(Instant::now)
            .elapsed(),
    )
}

fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
