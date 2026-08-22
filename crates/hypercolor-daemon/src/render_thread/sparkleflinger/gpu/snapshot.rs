use std::sync::Arc;

use anyhow::{Context, Result};
use hypercolor_types::scene::ZoneId;

use crate::render_thread::producer_queue::{
    GpuTextureFrame, GpuTextureFrameLease, GpuTextureFrameOrigin,
};

use super::{
    GpuCompositorOutputSurface, GpuCompositorTexture, GpuImmutableSceneSnapshot,
    GpuProjectionSnapshot, GpuSparkleFlinger, texture_extent,
};

impl GpuSparkleFlinger {
    pub(crate) fn current_output_frame(&mut self) -> Result<Option<GpuTextureFrame>> {
        self.flush_pending_output_submission()?;
        let Some(surfaces) = self.surfaces.as_ref() else {
            return Ok(None);
        };
        let Some(texture) = self.current_output.map(|output| match output {
            GpuCompositorOutputSurface::Front => &surfaces.front,
            GpuCompositorOutputSurface::Back => &surfaces.back,
        }) else {
            return Ok(None);
        };
        Ok(Some(GpuTextureFrame {
            width: surfaces.width,
            height: surfaces.height,
            storage_id: texture.storage_id,
            content_generation: self.output_generation,
            origin: GpuTextureFrameOrigin::CompositorOutput,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
            immutable_lease: None,
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        }))
    }

    pub(crate) fn snapshot_projected_group_frame(
        &mut self,
        group_id: ZoneId,
        frame: GpuTextureFrame,
    ) -> Result<GpuTextureFrame> {
        debug_assert_eq!(frame.origin, GpuTextureFrameOrigin::CompositorOutput);
        self.flush_pending_output_submission()?;
        let snapshot = self
            .projected_group_snapshots
            .get_mut(&group_id)
            .and_then(Option::as_mut)
            .context("projected group GPU snapshot was not admitted before rendering")?;
        anyhow::ensure!(
            snapshot.width == frame.width && snapshot.height == frame.height,
            "projected group GPU snapshot dimensions do not match the rendered frame"
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger projected group snapshot"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &snapshot.texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            texture_extent(frame.width, frame.height),
        );
        let _ = self.queue.submit(Some(encoder.finish()));
        snapshot.content_generation = snapshot.content_generation.saturating_add(1);
        Ok(GpuTextureFrame {
            width: snapshot.width,
            height: snapshot.height,
            storage_id: snapshot.texture.storage_id,
            content_generation: snapshot.content_generation,
            origin: GpuTextureFrameOrigin::ProjectionSnapshot,
            texture: snapshot.texture.texture.clone(),
            view: snapshot.texture.view.clone(),
            immutable_lease: Some(Arc::clone(&snapshot.lease)),
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        })
    }

    pub(crate) fn snapshot_current_output_frame(&mut self) -> Result<Option<GpuTextureFrame>> {
        let Some(frame) = self.current_output_frame()? else {
            return Ok(None);
        };
        self.snapshot_scene_frame(frame).map(Some)
    }

    pub(crate) fn opaque_black_frame(&self) -> Option<GpuTextureFrame> {
        let texture = self.opaque_black_texture.as_ref()?;
        Some(GpuTextureFrame {
            width: 1,
            height: 1,
            storage_id: texture.storage_id,
            content_generation: 1,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
            immutable_lease: None,
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        })
    }

    pub(crate) fn snapshot_scene_frame(
        &mut self,
        frame: GpuTextureFrame,
    ) -> Result<GpuTextureFrame> {
        if frame.origin == GpuTextureFrameOrigin::ImmutableSnapshot {
            return Ok(frame);
        }
        self.flush_pending_output_submission()?;
        let snapshot = self
            .immutable_scene_snapshots
            .iter_mut()
            .find(|snapshot| {
                snapshot.width == frame.width
                    && snapshot.height == frame.height
                    && Arc::strong_count(&snapshot.lease) == 1
            })
            .context("all pre-admitted immutable GPU scene snapshots are still leased")?;
        anyhow::ensure!(
            snapshot.texture.storage_id != frame.storage_id,
            "immutable GPU scene snapshot cannot alias its source texture"
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger immutable scene snapshot"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &snapshot.texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            texture_extent(frame.width, frame.height),
        );
        let _ = self.queue.submit(Some(encoder.finish()));
        snapshot.content_generation = snapshot.content_generation.saturating_add(1);
        Ok(GpuTextureFrame {
            width: snapshot.width,
            height: snapshot.height,
            storage_id: snapshot.texture.storage_id,
            content_generation: snapshot.content_generation,
            origin: GpuTextureFrameOrigin::ImmutableSnapshot,
            texture: snapshot.texture.texture.clone(),
            view: snapshot.texture.view.clone(),
            immutable_lease: Some(Arc::clone(&snapshot.lease)),
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        })
    }

    pub(crate) fn restore_scene_frame(&mut self, frame: &GpuTextureFrame) -> Result<()> {
        self.flush_pending_output_submission()?;
        let surfaces = self
            .surfaces
            .as_mut()
            .filter(|surfaces| surfaces.width == frame.width && surfaces.height == frame.height)
            .context("retained GPU scene dimensions do not match admitted compositor surfaces")?;
        anyhow::ensure!(
            surfaces.front.storage_id != frame.storage_id,
            "retained GPU scene cannot alias its restore destination"
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger retained scene restore"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &surfaces.front.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            texture_extent(frame.width, frame.height),
        );
        let _ = self.queue.submit(Some(encoder.finish()));
        surfaces.front_contents = None;
        surfaces.back_contents = None;
        self.current_output = Some(GpuCompositorOutputSurface::Front);
        self.output_generation = self.output_generation.saturating_add(1);
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.cached_sample_result = None;
        Ok(())
    }
}

impl GpuProjectionSnapshot {
    pub(in crate::render_thread::sparkleflinger::gpu) fn try_new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let texture = GpuCompositorTexture::try_new(
            device,
            width,
            height,
            "SparkleFlinger Projected Group Snapshot",
        )?;
        Ok(Self {
            width,
            height,
            texture,
            content_generation: 0,
            lease: Arc::new(GpuTextureFrameLease),
        })
    }
}

impl GpuImmutableSceneSnapshot {
    pub(in crate::render_thread::sparkleflinger::gpu) fn try_new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let texture = GpuCompositorTexture::try_new(
            device,
            width,
            height,
            "SparkleFlinger Immutable Scene Snapshot",
        )?;
        Ok(Self {
            width,
            height,
            texture,
            content_generation: 0,
            lease: Arc::new(GpuTextureFrameLease),
        })
    }
}
