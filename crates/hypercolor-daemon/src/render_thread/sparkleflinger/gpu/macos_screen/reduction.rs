use anyhow::{Context, Result};
use hypercolor_core::input::screen::{
    CaptureRotation, ResolvedScreenPublicationDescriptor, ScreenReductionFilter,
    ScreenSourceReflection,
};
use hypercolor_macos_gpu_interop::{
    ImportedMacosScreenFrame, MacosNativeReductionDescriptor, MacosNativeReductionFilter,
};

use super::MacosScreenBridge;
use super::color::{native_color_transform, native_letterbox_fill};
use super::model::PreparedMacosScreenTarget;
use crate::render_thread::producer_queue::MacosScreenTextureLease;
use crate::render_thread::sparkleflinger::gpu::GpuSparkleFlinger;

fn reduction_descriptor(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<MacosNativeReductionDescriptor> {
    let source = descriptor.source();
    let geometry = source.geometry();
    anyhow::ensure!(
        geometry.rotation() == CaptureRotation::Identity
            && source.reflection() == ScreenSourceReflection::None
            && geometry.native_extent() == geometry.storage_extent()
            && geometry.source_scale().numerator() == geometry.source_scale().denominator(),
        "macOS native reduction received unsupported pending source geometry"
    );
    let crop = geometry.crop();
    let crop_x = crop.map_or(0, hypercolor_core::input::screen::PixelRect::x);
    let crop_y = crop.map_or(0, hypercolor_core::input::screen::PixelRect::y);
    let region = descriptor.physical().source_region();
    let rational = |value: hypercolor_core::input::screen::ScreenRational| {
        value.numerator() as f32 / value.denominator().get() as f32
    };
    let source_rect = [
        crop_x as f32 + rational(region.x()),
        crop_y as f32 + rational(region.y()),
        rational(region.width()),
        rational(region.height()),
    ];
    let output = descriptor.physical().reduction_extent();
    let filter = match descriptor.physical().reduction_filter() {
        ScreenReductionFilter::Nearest => MacosNativeReductionFilter::Nearest,
        ScreenReductionFilter::Bilinear => MacosNativeReductionFilter::Bilinear,
        ScreenReductionFilter::Area => MacosNativeReductionFilter::Area,
    };
    MacosNativeReductionDescriptor::new(
        [output.width(), output.height()],
        [0, 0, output.width(), output.height()],
        source_rect,
        filter,
        native_color_transform(descriptor)?,
    )
    .map_err(anyhow::Error::from)
}

pub(super) struct ResolvedNativeFrame {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) storage_id: u64,
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
}

pub(super) struct ReducedNativeFrame {
    pub(super) frame: ResolvedNativeFrame,
    pub(super) submitted: bool,
}

pub(super) fn reduce_imported_frame(
    gpu: &mut GpuSparkleFlinger,
    bridge: &MacosScreenBridge,
    imported: &ImportedMacosScreenFrame,
    prepared: &PreparedMacosScreenTarget,
    content_generation: u64,
    submission_lease: &MacosScreenTextureLease,
) -> Result<ReducedNativeFrame> {
    let mut submitted = false;
    let physical = prepared
        .physical
        .as_ref()
        .context("native macOS work has no prepared physical target")?;
    let mut physical_sequence = physical
        .content_sequence
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *physical_sequence != Some(content_generation) {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger macOS native screen reduction"),
            });
        let reduction = bridge.reducer.encode(
            imported,
            &physical.target,
            reduction_descriptor(&prepared.descriptor)?,
            &mut encoder,
        );
        if let Err(error) = reduction {
            return Err(error.into());
        }
        let submission_index = gpu.queue.submit(Some(encoder.finish()));
        gpu.retire_native_screen_leases(submission_index, vec![submission_lease.clone()]);
        submitted = true;
        *physical_sequence = Some(content_generation);
    }
    drop(physical_sequence);

    let frame = if let Some(logical_target) = prepared.logical_target.as_ref() {
        let mut logical_sequence = prepared
            .logical_content_sequence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *logical_sequence != Some(content_generation) {
            let geometry = prepared.descriptor.geometry();
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("SparkleFlinger macOS native screen materialization"),
                });
            let materialization = bridge.reducer.encode_materialization(
                &physical.target,
                logical_target,
                [
                    geometry.content_x(),
                    geometry.content_y(),
                    geometry.content_extent().width(),
                    geometry.content_extent().height(),
                ],
                native_letterbox_fill(&prepared.descriptor)?,
                &mut encoder,
            );
            if let Err(error) = materialization {
                return Err(error.into());
            }
            let submission_index = gpu.queue.submit(Some(encoder.finish()));
            gpu.retire_native_screen_leases(submission_index, vec![submission_lease.clone()]);
            submitted = true;
            *logical_sequence = Some(content_generation);
        }
        ResolvedNativeFrame {
            width: logical_target.width(),
            height: logical_target.height(),
            storage_id: prepared
                .logical_storage_id
                .context("logical macOS target has no storage identity")?,
            texture: logical_target.texture().clone(),
            view: logical_target.view().clone(),
        }
    } else {
        ResolvedNativeFrame {
            width: physical.target.width(),
            height: physical.target.height(),
            storage_id: physical.storage_id,
            texture: physical.target.texture().clone(),
            view: physical.target.view().clone(),
        }
    };
    Ok(ReducedNativeFrame { frame, submitted })
}

pub(super) fn identity_frame(
    imported: &ImportedMacosScreenFrame,
    prepared: &PreparedMacosScreenTarget,
    storage_id: u64,
    published_width: u32,
    published_height: u32,
) -> Result<ResolvedNativeFrame> {
    let extent = prepared.descriptor.geometry().output_extent();
    anyhow::ensure!(
        published_width == extent.width() && published_height == extent.height(),
        "native macOS identity surface extent does not match its target"
    );
    Ok(ResolvedNativeFrame {
        width: extent.width(),
        height: extent.height(),
        storage_id,
        texture: imported
            .texture()
            .context("native macOS identity publication has no wgpu texture")?
            .as_ref()
            .clone(),
        view: imported
            .view()
            .context("native macOS identity publication has no wgpu texture view")?
            .as_ref()
            .clone(),
    })
}
