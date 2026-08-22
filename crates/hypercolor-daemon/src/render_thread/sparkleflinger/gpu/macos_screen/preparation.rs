use std::alloc::Layout;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use anyhow::{Context, Result};
use hypercolor_core::input::screen::{
    LED_TONE_MAP_ALGORITHM_REVISION, MacosNativeTargetManifest, PlatformGpuApi,
    ResolvedScreenPublicationDescriptor, ScreenCaptureBackend, ScreenColorTransformCapabilities,
    ScreenNativeExecutionTarget, ScreenNativeExecutionTargetId, ScreenNativePreparationPayload,
    ScreenNativeRetentionQuote, ScreenNativeTargetPreparation, ScreenNativeTargetPreparer,
    ScreenPhysicalGpuDeviceIdentity, ScreenPlanGeneration, ScreenPublicationKind,
    ScreenResourceApi,
};
use hypercolor_macos_gpu_interop::{
    MacosNativeReducer, MacosScreenBridge as MacosInteropScreenBridge,
};

use super::MacosScreenBridge;
use super::cache::{MacosScreenCache, next_texture_storage_id};
use super::color::{native_color_transform, native_letterbox_fill};
use super::contract::{macos_native_target_format, requires_native_work};
use super::model::{PreparedMacosPhysicalTarget, PreparedMacosScreenTarget};
use crate::render_thread::sparkleflinger::gpu::NEXT_SCREEN_TARGET_ID;

pub(super) struct MacosScreenTargetPreparer {
    bridge: Weak<MacosScreenBridge>,
    target_id: ScreenNativeExecutionTargetId,
}

impl MacosScreenTargetPreparer {
    fn bridge(&self) -> Result<Arc<MacosScreenBridge>> {
        let bridge = self
            .bridge
            .upgrade()
            .context("macOS screen renderer was retired during target preparation")?;
        anyhow::ensure!(
            bridge.target_id() == self.target_id,
            "macOS screen renderer target identity changed during preparation"
        );
        Ok(bridge)
    }
}

pub(super) fn prepare_target(
    bridge: &MacosScreenBridge,
    descriptor: &ResolvedScreenPublicationDescriptor,
    plan_generation: ScreenPlanGeneration,
) -> Result<PreparedMacosScreenTarget> {
    native_color_transform(descriptor)?;
    let physical = if requires_native_work(descriptor) {
        let physical = descriptor.physical();
        Some(
            bridge
                .cache
                .physical_target(plan_generation, physical, || {
                    let extent = physical.reduction_extent();
                    let format = macos_native_target_format(physical.target_pixel_format())?;
                    Ok(PreparedMacosPhysicalTarget {
                        target: bridge.reducer.create_target(
                            bridge.interop_device(),
                            extent.width(),
                            extent.height(),
                            format,
                        )?,
                        storage_id: next_texture_storage_id()?,
                        content_sequence: Mutex::new(None),
                    })
                })?,
        )
    } else {
        None
    };
    let geometry = descriptor.geometry();
    let needs_materialization = physical.is_some() && !geometry.content_fills_output();
    if needs_materialization {
        native_letterbox_fill(descriptor)?;
    }
    let logical_target = if needs_materialization {
        let extent = geometry.output_extent();
        Some(bridge.reducer.create_target(
            bridge.interop_device(),
            extent.width(),
            extent.height(),
            macos_native_target_format(descriptor.physical().target_pixel_format())?,
        )?)
    } else {
        None
    };
    let logical_storage_id = logical_target
        .as_ref()
        .map(|_| next_texture_storage_id())
        .transpose()?;
    Ok(PreparedMacosScreenTarget {
        target_id: bridge.target_id(),
        resource_generation: descriptor.source().resources().resource_generation(),
        descriptor: Arc::new(descriptor.clone()),
        physical,
        logical_target,
        logical_storage_id,
        logical_content_sequence: Mutex::new(None),
    })
}

fn prepared_macos_screen_target_metadata_bytes() -> Result<u64> {
    checked_macos_arc_allocation_bytes::<PreparedMacosScreenTarget>()?
        .checked_add(checked_macos_arc_allocation_bytes::<
            ResolvedScreenPublicationDescriptor,
        >()?)
        .context("macOS prepared target metadata accounting overflow")
}

pub(in crate::render_thread::sparkleflinger::gpu) fn prepared_macos_screen_target_exclusive_bytes(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<u64> {
    let mut bytes = prepared_macos_screen_target_metadata_bytes()?;
    if !requires_native_work(descriptor) {
        return Ok(bytes);
    }
    if !descriptor.geometry().content_fills_output() {
        let logical_texture_bytes = target_texture_bytes(descriptor.geometry().output_extent())
            .context("macOS logical target texture accounting overflow")?;
        bytes = bytes
            .checked_add(logical_texture_bytes)
            .context("macOS logical target accounting overflow")?;
    }
    Ok(bytes)
}

fn prepared_macos_screen_target_shared_bytes(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<u64> {
    if !requires_native_work(descriptor) {
        return Ok(0);
    }
    let physical_texture_bytes = target_texture_bytes(descriptor.physical().reduction_extent())
        .context("macOS physical target texture accounting overflow")?;
    checked_macos_arc_allocation_bytes::<PreparedMacosPhysicalTarget>()?
        .checked_add(physical_texture_bytes)
        .context("macOS physical target accounting overflow")
}

pub(in crate::render_thread::sparkleflinger::gpu) fn prepared_macos_screen_target_retention(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<ScreenNativeRetentionQuote> {
    Ok(ScreenNativeRetentionQuote::split(
        prepared_macos_screen_target_exclusive_bytes(descriptor)?,
        prepared_macos_screen_target_shared_bytes(descriptor)?,
    ))
}

fn target_texture_bytes(extent: hypercolor_core::input::screen::PixelExtent) -> Option<u64> {
    u64::from(extent.width())
        .checked_mul(u64::from(extent.height()))
        .and_then(|pixels| pixels.checked_mul(4))
}

fn checked_macos_arc_allocation_bytes<T>() -> Result<u64> {
    let (layout, _) = Layout::new::<[AtomicUsize; 2]>()
        .extend(Layout::new::<T>())
        .context("macOS Arc allocation layout overflow")?;
    u64::try_from(layout.pad_to_align().size()).context("macOS Arc allocation exceeds u64")
}

impl ScreenNativeTargetPreparer for MacosScreenTargetPreparer {
    fn quote_retained_bytes(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> Result<u64> {
        let manifest = platform
            .downcast_ref::<MacosNativeTargetManifest>()
            .context("macOS screen target received an unknown preparation manifest")?;
        validate_target_manifest(descriptor, manifest)?;
        self.bridge()?;
        prepared_macos_screen_target_exclusive_bytes(descriptor)
    }

    fn quote_retention(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> Result<ScreenNativeRetentionQuote> {
        self.quote_retained_bytes(descriptor, platform)?;
        prepared_macos_screen_target_retention(descriptor)
    }

    fn prepare(
        &self,
        descriptor: &ResolvedScreenPublicationDescriptor,
        platform: &ScreenNativePreparationPayload,
    ) -> Result<ScreenNativeTargetPreparation> {
        let manifest = platform
            .downcast_ref::<MacosNativeTargetManifest>()
            .context("macOS screen target received an unknown preparation manifest")?;
        validate_target_manifest(descriptor, manifest)?;
        let bridge = self.bridge()?;
        let prepared = bridge.prepare_target(descriptor, platform.plan_generation())?;
        Ok(ScreenNativeTargetPreparation::with_retention(
            ScreenNativePreparationPayload::new(
                descriptor,
                platform.plan_generation(),
                Arc::new(prepared),
            ),
            prepared_macos_screen_target_retention(descriptor)?,
        ))
    }
}

fn validate_target_manifest(
    descriptor: &ResolvedScreenPublicationDescriptor,
    manifest: &MacosNativeTargetManifest,
) -> Result<()> {
    anyhow::ensure!(
        descriptor.kind() == ScreenPublicationKind::Surface,
        "macOS native target requires a Surface descriptor"
    );
    let source = descriptor.source();
    let resources = source.resources();
    anyhow::ensure!(
        resources.backend() == &ScreenCaptureBackend::MacosScreenCaptureKit
            && resources.api() == &ScreenResourceApi::PlatformGpu(PlatformGpuApi::Metal),
        "macOS target manifest was paired with a non-Metal source"
    );
    anyhow::ensure!(
        matches!(
            resources.physical_gpu_device(),
            Some(ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(registry_id))
                if *registry_id == manifest.metal_registry_id()
        ),
        "macOS target manifest Metal device does not match the resolved source"
    );
    anyhow::ensure!(
        descriptor.source_epoch().session_generation == manifest.capture_session_generation()
            && resources.device_generation() == manifest.capture_session_generation(),
        "macOS target manifest capture session does not match the resolved source"
    );
    anyhow::ensure!(
        resources.resource_generation() == manifest.resource_generation(),
        "macOS target manifest resource generation does not match the resolved source"
    );
    Ok(())
}

pub(in crate::render_thread::sparkleflinger::gpu) fn create_screen_bridge(
    device: &wgpu::Device,
    max_texture_dimension: u32,
) -> Result<(Arc<MacosScreenBridge>, ScreenNativeExecutionTarget)> {
    let interop = MacosInteropScreenBridge::new(device)
        .context("renderer does not expose a Metal screen-import target")?;
    let reducer = MacosNativeReducer::new(device)
        .context("renderer does not expose a native Metal screen reducer")?;
    let target_id = next_screen_target_id()?;
    let bridge = Arc::new(MacosScreenBridge {
        device: device.clone(),
        interop,
        reducer,
        cache: MacosScreenCache::new(),
        target_id,
    });
    let target = bridge.execution_target(max_texture_dimension);
    Ok((bridge, target))
}

pub(super) fn create_screen_target(
    bridge: &Arc<MacosScreenBridge>,
    max_texture_dimension: u32,
) -> ScreenNativeExecutionTarget {
    ScreenNativeExecutionTarget::new(
        bridge.target_id(),
        PlatformGpuApi::Metal,
        ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(bridge.interop.metal_registry_id()),
        NonZeroU32::new(max_texture_dimension)
            .expect("wgpu devices expose a non-zero texture dimension limit"),
        Arc::new(MacosScreenTargetPreparer {
            bridge: Arc::downgrade(bridge),
            target_id: bridge.target_id(),
        }),
    )
    .with_color_capabilities(ScreenColorTransformCapabilities::new(
        true,
        true,
        true,
        LED_TONE_MAP_ALGORITHM_REVISION,
    ))
}

fn next_screen_target_id() -> Result<ScreenNativeExecutionTargetId> {
    let target_id = NEXT_SCREEN_TARGET_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| anyhow::anyhow!("screen target identity space is exhausted"))?;
    Ok(ScreenNativeExecutionTargetId::new(
        NonZeroU64::new(target_id).expect("screen target identities start at one"),
    ))
}
