use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_core::types::canvas::BYTES_PER_PIXEL;
use thiserror::Error;

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
    descriptor_identity: u64,
    branch_sequence: u64,
    width: u32,
    height: u32,
}

impl ScreenUploadContentKey {
    pub(super) const fn new(
        plan_generation: u64,
        descriptor_identity: u64,
        branch_sequence: u64,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            plan_generation,
            descriptor_identity,
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
    policy: ScreenUploadResidencyPolicy,
    epoch: u64,
    #[cfg(test)]
    pub(super) allocation_count: usize,
    #[cfg(test)]
    pub(super) reuse_count: usize,
    #[cfg(test)]
    pub(super) upload_count: usize,
    #[cfg(test)]
    fail_next_allocation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScreenUploadResidencyPolicy {
    max_textures: usize,
}

impl ScreenUploadResidencyPolicy {
    pub(super) const fn compositor_pipeline() -> Self {
        Self { max_textures: 2 }
    }

    #[cfg(test)]
    pub(super) const fn with_max_textures(max_textures: usize) -> Self {
        Self {
            max_textures: if max_textures == 0 { 1 } else { max_textures },
        }
    }
}

#[derive(Debug, Error)]
#[error(
    "GPU screen upload pool is saturated at {resident_textures}/{max_textures} textures ({resident_bytes} resident bytes)"
)]
pub(super) struct ScreenUploadPoolSaturated {
    resident_textures: usize,
    max_textures: usize,
    resident_bytes: u64,
}

impl ScreenUploadPoolSaturated {
    #[cfg(test)]
    pub(super) const fn for_test() -> Self {
        Self {
            resident_textures: 2,
            max_textures: 2,
            resident_bytes: 0,
        }
    }
}

impl ScreenPublicationUploadPool {
    pub(super) const fn new(policy: ScreenUploadResidencyPolicy) -> Self {
        Self {
            textures: Vec::new(),
            resident_bytes: 0,
            policy,
            epoch: 0,
            #[cfg(test)]
            allocation_count: 0,
            #[cfg(test)]
            reuse_count: 0,
            #[cfg(test)]
            upload_count: 0,
            #[cfg(test)]
            fail_next_allocation: false,
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
            self.ensure_texture_slot(&mut release_cached_source)?;
            #[cfg(test)]
            super::reject_injected_gpu_preparation(
                std::mem::take(&mut self.fail_next_allocation),
                "screen upload texture",
            )?;
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

    pub(super) fn preflight_uploads(
        &mut self,
        device: &wgpu::Device,
        content_keys: impl IntoIterator<Item = ScreenUploadContentKey>,
    ) -> Result<()> {
        self.reclaim_completed(device)?;
        let mut projected = self
            .textures
            .iter()
            .map(|texture| {
                let reusable_after_supersede = matches!(
                    &texture.state,
                    UploadTextureState::Free
                        | UploadTextureState::Encoding {
                            prior_submission: None
                        }
                );
                (
                    texture.width,
                    texture.height,
                    texture.content_key,
                    reusable_after_supersede,
                )
            })
            .collect::<Vec<_>>();
        for content_key in content_keys {
            if let Some(slot) = projected
                .iter()
                .position(|(_, _, key, _)| *key == Some(content_key))
            {
                projected[slot].3 = false;
                continue;
            }
            let reusable = projected
                .iter()
                .position(|(width, height, _, free)| {
                    *free && *width == content_key.width && *height == content_key.height
                })
                .or_else(|| projected.iter().position(|(_, _, _, free)| *free));
            if let Some(slot) = reusable {
                projected[slot] = (
                    content_key.width,
                    content_key.height,
                    Some(content_key),
                    false,
                );
                continue;
            }
            if projected.len() < self.policy.max_textures {
                projected.push((
                    content_key.width,
                    content_key.height,
                    Some(content_key),
                    false,
                ));
                continue;
            }
            return Err(ScreenUploadPoolSaturated {
                resident_textures: projected.len(),
                max_textures: self.policy.max_textures,
                resident_bytes: self.resident_bytes,
            }
            .into());
        }
        Ok(())
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

    #[cfg(test)]
    pub(super) fn fail_next_allocation(&mut self) {
        self.fail_next_allocation = true;
    }

    #[cfg(test)]
    pub(super) const fn allocation_failure_is_armed(&self) -> bool {
        self.fail_next_allocation
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

    fn ensure_texture_slot<F>(&mut self, release_cached_source: &mut F) -> Result<()>
    where
        F: FnMut(u64),
    {
        if self.textures.len() < self.policy.max_textures {
            return Ok(());
        }
        let Some(slot) = self
            .textures
            .iter()
            .enumerate()
            .filter(|(_, texture)| matches!(texture.state, UploadTextureState::Free))
            .min_by_key(|(_, texture)| texture.last_used_epoch)
            .map(|(slot, _)| slot)
        else {
            return Err(ScreenUploadPoolSaturated {
                resident_textures: self.textures.len(),
                max_textures: self.policy.max_textures,
                resident_bytes: self.resident_bytes,
            }
            .into());
        };
        let evicted = self.textures.swap_remove(slot);
        self.resident_bytes = self
            .resident_bytes
            .checked_sub(evicted.bytes)
            .context("screen upload pool byte accounting underflowed")?;
        release_cached_source(evicted.texture.storage_id);
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
        immutable_lease: None,
        #[cfg(target_os = "windows")]
        windows_screen_lease: None,
        #[cfg(all(target_os = "macos", feature = "screen-capture"))]
        macos_screen_lease: None,
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
