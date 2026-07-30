use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_core::types::canvas::BYTES_PER_PIXEL;

use super::source::write_rgba_texture;
use super::{GpuCompositorTexture, GpuTextureFrame, GpuTextureFrameOrigin};

enum UploadTextureState {
    Free,
    Encoding {
        prior_submission: Option<wgpu::SubmissionIndex>,
    },
    Submitted(wgpu::SubmissionIndex),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScreenUploadContentKey {
    plan_generation: u64,
    branch_sequence: u64,
    width: u32,
    height: u32,
}

impl ScreenUploadContentKey {
    pub(super) const fn new(
        plan_generation: u64,
        branch_sequence: u64,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            plan_generation,
            branch_sequence,
            width,
            height,
        }
    }
}

struct ScreenUploadTexture {
    width: u32,
    height: u32,
    bytes: u64,
    last_used_epoch: u64,
    state: UploadTextureState,
    content_key: Option<ScreenUploadContentKey>,
    texture: GpuCompositorTexture,
}

pub(super) struct ScreenPublicationUploadPool {
    textures: Vec<ScreenUploadTexture>,
    resident_bytes: u64,
    max_resident_bytes: u64,
    epoch: u64,
    #[cfg(test)]
    pub(super) allocation_count: usize,
    #[cfg(test)]
    pub(super) reuse_count: usize,
    #[cfg(test)]
    pub(super) upload_count: usize,
}

impl ScreenPublicationUploadPool {
    pub(super) const fn new(max_resident_bytes: u64) -> Self {
        Self {
            textures: Vec::new(),
            resident_bytes: 0,
            max_resident_bytes,
            epoch: 0,
            #[cfg(test)]
            allocation_count: 0,
            #[cfg(test)]
            reuse_count: 0,
            #[cfg(test)]
            upload_count: 0,
        }
    }

    pub(super) fn upload_rgba<F>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba_bytes: &[u8],
        content_key: ScreenUploadContentKey,
        mut release_cached_source: F,
    ) -> Result<(GpuTextureFrame, bool)>
    where
        F: FnMut(u64),
    {
        let frame_bytes = rgba_frame_bytes(width, height)?;
        let resident_bytes = resident_frame_bytes(width, height)?;
        let expected_len =
            usize::try_from(frame_bytes).context("screen frame byte size exceeds usize")?;
        anyhow::ensure!(
            rgba_bytes.len() == expected_len,
            "screen frame contains {} bytes for a {width}x{height} RGBA extent; expected {expected_len}",
            rgba_bytes.len()
        );
        anyhow::ensure!(
            content_key.width == width && content_key.height == height,
            "screen upload content identity does not match the uploaded extent"
        );
        anyhow::ensure!(
            resident_bytes <= self.max_resident_bytes,
            "screen upload requires {resident_bytes} resident bytes but the GPU pool capacity is {} bytes",
            self.max_resident_bytes
        );

        self.reclaim_completed(device)?;
        self.epoch = self.epoch.saturating_add(1);
        let epoch = self.epoch;
        if let Some(slot) = self
            .textures
            .iter()
            .position(|texture| texture.content_key == Some(content_key))
        {
            let texture = &mut self.textures[slot];
            let prior_submission = match std::mem::replace(
                &mut texture.state,
                UploadTextureState::Encoding {
                    prior_submission: None,
                },
            ) {
                UploadTextureState::Free => None,
                UploadTextureState::Encoding { prior_submission } => prior_submission,
                UploadTextureState::Submitted(submission_index) => Some(submission_index),
            };
            texture.state = UploadTextureState::Encoding { prior_submission };
            texture.last_used_epoch = epoch;
            #[cfg(test)]
            {
                self.reuse_count = self.reuse_count.saturating_add(1);
            }
            return Ok((
                gpu_texture_frame(texture, content_key.branch_sequence),
                false,
            ));
        }
        let slot = if let Some(slot) = self.textures.iter().position(|texture| {
            texture.width == width
                && texture.height == height
                && matches!(texture.state, UploadTextureState::Free)
        }) {
            #[cfg(test)]
            {
                self.reuse_count = self.reuse_count.saturating_add(1);
            }
            slot
        } else {
            self.evict_free_textures_for(resident_bytes, &mut release_cached_source)?;
            let texture = try_create_upload_texture(device, width, height)?;
            self.resident_bytes = self
                .resident_bytes
                .checked_add(resident_bytes)
                .context("screen upload pool byte accounting overflowed")?;
            self.textures.push(ScreenUploadTexture {
                width,
                height,
                bytes: resident_bytes,
                last_used_epoch: epoch,
                state: UploadTextureState::Free,
                content_key: None,
                texture,
            });
            #[cfg(test)]
            {
                self.allocation_count = self.allocation_count.saturating_add(1);
            }
            self.textures.len() - 1
        };

        let texture = &mut self.textures[slot];
        write_rgba_texture(queue, &texture.texture.texture, width, height, rgba_bytes);
        texture.last_used_epoch = epoch;
        texture.state = UploadTextureState::Encoding {
            prior_submission: None,
        };
        texture.content_key = Some(content_key);
        #[cfg(test)]
        {
            self.upload_count = self.upload_count.saturating_add(1);
        }
        Ok((
            gpu_texture_frame(texture, content_key.branch_sequence),
            true,
        ))
    }

    pub(super) fn mark_submitted(&mut self, submission_index: wgpu::SubmissionIndex) {
        for texture in &mut self.textures {
            if matches!(texture.state, UploadTextureState::Encoding { .. }) {
                texture.state = UploadTextureState::Submitted(submission_index.clone());
            }
        }
    }

    pub(super) fn discard_encoding(&mut self) {
        for texture in &mut self.textures {
            if matches!(texture.state, UploadTextureState::Encoding { .. }) {
                let previous = std::mem::replace(&mut texture.state, UploadTextureState::Free);
                if let UploadTextureState::Encoding {
                    prior_submission: Some(submission_index),
                } = previous
                {
                    texture.state = UploadTextureState::Submitted(submission_index);
                }
            }
        }
    }

    fn reclaim_completed(&mut self, device: &wgpu::Device) -> Result<()> {
        for texture in &mut self.textures {
            let completed = match &texture.state {
                UploadTextureState::Submitted(submission_index) => {
                    match device.poll(wgpu::PollType::Wait {
                        submission_index: Some(submission_index.clone()),
                        timeout: Some(Duration::ZERO),
                    }) {
                        Ok(_) => true,
                        Err(wgpu::PollError::Timeout) => false,
                        Err(error) => {
                            return Err(error).context("GPU screen upload retirement poll failed");
                        }
                    }
                }
                UploadTextureState::Free | UploadTextureState::Encoding { .. } => false,
            };
            if completed {
                texture.state = UploadTextureState::Free;
            }
        }
        Ok(())
    }

    fn evict_free_textures_for<F>(
        &mut self,
        required_bytes: u64,
        release_cached_source: &mut F,
    ) -> Result<()>
    where
        F: FnMut(u64),
    {
        while self.resident_bytes.saturating_add(required_bytes) > self.max_resident_bytes {
            let Some(slot) = self
                .textures
                .iter()
                .enumerate()
                .filter(|(_, texture)| matches!(texture.state, UploadTextureState::Free))
                .min_by_key(|(_, texture)| texture.last_used_epoch)
                .map(|(slot, _)| slot)
            else {
                anyhow::bail!(
                    "GPU screen upload pool has {} resident bytes and no completed texture to retire for a {required_bytes}-byte frame",
                    self.resident_bytes
                );
            };
            let evicted = self.textures.swap_remove(slot);
            self.resident_bytes = self
                .resident_bytes
                .checked_sub(evicted.bytes)
                .context("screen upload pool byte accounting underflowed")?;
            release_cached_source(evicted.texture.storage_id);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn state_counts(&self) -> (usize, usize, usize) {
        self.textures.iter().fold(
            (0, 0, 0),
            |(free, encoding, submitted), texture| match texture.state {
                UploadTextureState::Free => (free + 1, encoding, submitted),
                UploadTextureState::Encoding { .. } => (free, encoding + 1, submitted),
                UploadTextureState::Submitted(_) => (free, encoding, submitted + 1),
            },
        )
    }
}

fn gpu_texture_frame(texture: &ScreenUploadTexture, content_generation: u64) -> GpuTextureFrame {
    GpuTextureFrame {
        width: texture.width,
        height: texture.height,
        storage_id: texture.texture.storage_id,
        content_generation,
        origin: GpuTextureFrameOrigin::ProducerTexture,
        texture: texture.texture.texture.clone(),
        view: texture.texture.view.clone(),
        #[cfg(target_os = "windows")]
        windows_screen_lease: None,
    }
}

fn rgba_frame_bytes(width: u32, height: u32) -> Result<u64> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL as u64))
        .context("screen frame byte size overflowed")
}

pub(super) fn resident_frame_bytes(width: u32, height: u32) -> Result<u64> {
    let row_bytes = u64::from(width)
        .checked_mul(BYTES_PER_PIXEL as u64)
        .context("screen upload row byte size overflowed")?;
    let alignment = u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let aligned_row_bytes = row_bytes
        .checked_add(alignment - 1)
        .context("screen upload aligned row byte size overflowed")?
        / alignment
        * alignment;
    aligned_row_bytes
        .checked_mul(u64::from(height))
        .context("screen upload resident byte size overflowed")
}

fn try_create_upload_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<GpuCompositorTexture> {
    let out_of_memory_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let texture =
        GpuCompositorTexture::new(device, width, height, "SparkleFlinger CPU screen upload");
    let validation_error = pollster::block_on(validation_scope.pop());
    let internal_error = pollster::block_on(internal_scope.pop());
    let out_of_memory_error = pollster::block_on(out_of_memory_scope.pop());
    if let Some(error) = validation_error.or(internal_error).or(out_of_memory_error) {
        anyhow::bail!("GPU screen upload texture allocation failed: {error}");
    }
    Ok(texture)
}
