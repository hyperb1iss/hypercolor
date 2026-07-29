use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use crate::preview_runtime::PreviewPixelFormat;

use super::InteractivePreviewSpec;

const RGBA_BYTES_PER_PIXEL: u64 = 4;
const RGB_BYTES_PER_PIXEL: u64 = 3;
const PREVIEW_SCENE_SURFACE_SLOTS: u64 = 2;
const COMPOSITOR_SURFACE_SLOTS: u64 = 4;
const WIRE_FIXED_BYTES: u64 = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PreviewResourceLedger {
    pub surface_bytes: u64,
    pub renderer_bytes: u64,
    pub gpu_bytes: u64,
    pub encoder_workspace_bytes: u64,
    pub encoded_transport_bytes: u64,
    pub metadata_bytes: u64,
}

impl PreviewResourceLedger {
    pub fn for_lane(
        spec: InteractivePreviewSpec,
        canvas_width: u32,
        canvas_height: u32,
        gpu: bool,
        preview_id_bytes: usize,
    ) -> Result<Self, PreviewResourceError> {
        let target_pixels = pixel_count(spec.width, spec.height)?;
        let canvas_pixels = pixel_count(canvas_width, canvas_height)?;
        let target_rgba = target_pixels
            .checked_mul(RGBA_BYTES_PER_PIXEL)
            .ok_or(PreviewResourceError::Overflow)?;
        let canvas_rgba = canvas_pixels
            .checked_mul(RGBA_BYTES_PER_PIXEL)
            .ok_or(PreviewResourceError::Overflow)?;
        let (encoder_workspace_bytes, encoded_body_bytes) = match spec.format {
            PreviewPixelFormat::Rgb => (
                target_pixels
                    .checked_mul(RGB_BYTES_PER_PIXEL + RGBA_BYTES_PER_PIXEL)
                    .ok_or(PreviewResourceError::Overflow)?,
                target_pixels
                    .checked_mul(RGB_BYTES_PER_PIXEL)
                    .ok_or(PreviewResourceError::Overflow)?,
            ),
            PreviewPixelFormat::Rgba => (target_rgba, target_rgba),
            PreviewPixelFormat::Jpeg => (
                target_pixels
                    .checked_mul(RGB_BYTES_PER_PIXEL + RGBA_BYTES_PER_PIXEL)
                    .ok_or(PreviewResourceError::Overflow)?,
                target_rgba,
            ),
        };
        let metadata_bytes = u64::try_from(preview_id_bytes)
            .map_err(|_| PreviewResourceError::Overflow)?
            .checked_add(u64::try_from(std::mem::size_of::<InteractivePreviewSpec>()).unwrap_or(0))
            .and_then(|bytes| bytes.checked_add(WIRE_FIXED_BYTES))
            .ok_or(PreviewResourceError::Overflow)?;
        Ok(Self {
            surface_bytes: target_rgba
                .checked_mul(2)
                .ok_or(PreviewResourceError::Overflow)?,
            renderer_bytes: canvas_rgba
                .checked_mul(PREVIEW_SCENE_SURFACE_SLOTS + COMPOSITOR_SURFACE_SLOTS)
                .ok_or(PreviewResourceError::Overflow)?,
            gpu_bytes: if gpu {
                target_rgba
                    .checked_mul(2)
                    .and_then(|bytes| bytes.checked_add(canvas_rgba.checked_mul(4)?))
                    .ok_or(PreviewResourceError::Overflow)?
            } else {
                0
            },
            encoder_workspace_bytes,
            encoded_transport_bytes: encoded_body_bytes
                .checked_add(metadata_bytes)
                .ok_or(PreviewResourceError::Overflow)?,
            metadata_bytes,
        })
    }

    pub fn total_bytes(self) -> Result<u64, PreviewResourceError> {
        [
            self.surface_bytes,
            self.renderer_bytes,
            self.gpu_bytes,
            self.encoder_workspace_bytes,
            self.encoded_transport_bytes,
            self.metadata_bytes,
        ]
        .into_iter()
        .try_fold(0_u64, |total, bytes| {
            total
                .checked_add(bytes)
                .ok_or(PreviewResourceError::Overflow)
        })
    }

    fn checked_add(self, other: Self) -> Result<Self, PreviewResourceError> {
        Ok(Self {
            surface_bytes: checked_add(self.surface_bytes, other.surface_bytes)?,
            renderer_bytes: checked_add(self.renderer_bytes, other.renderer_bytes)?,
            gpu_bytes: checked_add(self.gpu_bytes, other.gpu_bytes)?,
            encoder_workspace_bytes: checked_add(
                self.encoder_workspace_bytes,
                other.encoder_workspace_bytes,
            )?,
            encoded_transport_bytes: checked_add(
                self.encoded_transport_bytes,
                other.encoded_transport_bytes,
            )?,
            metadata_bytes: checked_add(self.metadata_bytes, other.metadata_bytes)?,
        })
    }

    fn saturating_sub(self, other: Self) -> Self {
        Self {
            surface_bytes: self.surface_bytes.saturating_sub(other.surface_bytes),
            renderer_bytes: self.renderer_bytes.saturating_sub(other.renderer_bytes),
            gpu_bytes: self.gpu_bytes.saturating_sub(other.gpu_bytes),
            encoder_workspace_bytes: self
                .encoder_workspace_bytes
                .saturating_sub(other.encoder_workspace_bytes),
            encoded_transport_bytes: self
                .encoded_transport_bytes
                .saturating_sub(other.encoded_transport_bytes),
            metadata_bytes: self.metadata_bytes.saturating_sub(other.metadata_bytes),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreviewCapacityLedger {
    inner: Arc<PreviewCapacityInner>,
}

#[derive(Debug)]
struct PreviewCapacityInner {
    capacity_bytes: u64,
    used: Mutex<PreviewResourceLedger>,
}

impl PreviewCapacityLedger {
    #[must_use]
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            inner: Arc::new(PreviewCapacityInner {
                capacity_bytes,
                used: Mutex::new(PreviewResourceLedger::default()),
            }),
        }
    }

    pub fn try_reserve(
        &self,
        requested: PreviewResourceLedger,
    ) -> Result<PreviewResourceLease, PreviewCapacityError> {
        let mut used = self
            .inner
            .used
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let projected = used
            .checked_add(requested)
            .map_err(|_| PreviewCapacityError {
                requested,
                used: *used,
                capacity_bytes: self.inner.capacity_bytes,
            })?;
        if projected.total_bytes().unwrap_or(u64::MAX) > self.inner.capacity_bytes {
            return Err(PreviewCapacityError {
                requested,
                used: *used,
                capacity_bytes: self.inner.capacity_bytes,
            });
        }
        *used = projected;
        Ok(PreviewResourceLease {
            capacity: Arc::clone(&self.inner),
            reserved: requested,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> PreviewCapacitySnapshot {
        let used = *self
            .inner
            .used
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        PreviewCapacitySnapshot {
            capacity_bytes: self.inner.capacity_bytes,
            used,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewCapacitySnapshot {
    pub capacity_bytes: u64,
    pub used: PreviewResourceLedger,
}

pub struct PreviewResourceLease {
    capacity: Arc<PreviewCapacityInner>,
    reserved: PreviewResourceLedger,
}

impl fmt::Debug for PreviewResourceLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreviewResourceLease")
            .field("reserved", &self.reserved)
            .finish_non_exhaustive()
    }
}

impl Drop for PreviewResourceLease {
    fn drop(&mut self) {
        let mut used = self
            .capacity
            .used
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *used = used.saturating_sub(self.reserved);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewResourceError {
    Overflow,
}

impl fmt::Display for PreviewResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("interactive preview resource arithmetic overflow")
    }
}

impl std::error::Error for PreviewResourceError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewCapacityError {
    pub requested: PreviewResourceLedger,
    pub used: PreviewResourceLedger,
    pub capacity_bytes: u64,
}

impl fmt::Display for PreviewCapacityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "interactive preview resources require {} bytes with {} of {} bytes already used",
            self.requested.total_bytes().unwrap_or(u64::MAX),
            self.used.total_bytes().unwrap_or(u64::MAX),
            self.capacity_bytes
        )
    }
}

impl std::error::Error for PreviewCapacityError {}

fn pixel_count(width: u32, height: u32) -> Result<u64, PreviewResourceError> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(PreviewResourceError::Overflow)
}

fn checked_add(left: u64, right: u64) -> Result<u64, PreviewResourceError> {
    left.checked_add(right)
        .ok_or(PreviewResourceError::Overflow)
}
