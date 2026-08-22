use std::collections::{HashMap, VecDeque};
use std::sync::Weak;

use super::super::super::CompositionLayer;
use super::super::{COMPOSE_PARAM_BYTES, GpuCompositorPipeline, GpuCompositorSurfaceSet};
#[cfg(feature = "allocation-contract-tests")]
use super::has_screen_upload_layers;
use crate::render_thread::producer_queue::GpuTextureFrameLease;
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
use crate::render_thread::producer_queue::NativeScreenCacheLease;
#[cfg(feature = "allocation-contract-tests")]
use crate::render_thread::producer_queue::ProducerFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ComposeSourceBindGroupKey {
    target_generation: u64,
    source_storage_id: u64,
    front_as_current: bool,
}

pub(crate) struct PreparedProjectedComposeBindGroups {
    target_generation: u64,
    entries: HashMap<ComposeSourceBindGroupKey, CachedComposeSourceBindGroup>,
    retired_entries: HashMap<ComposeSourceBindGroupKey, CachedComposeSourceBindGroup>,
}

impl PreparedProjectedComposeBindGroups {
    pub(in crate::render_thread::sparkleflinger::gpu) fn empty(target_generation: u64) -> Self {
        Self {
            target_generation,
            entries: HashMap::new(),
            retired_entries: HashMap::new(),
        }
    }
}

#[derive(Default)]
#[allow(
    clippy::struct_field_names,
    reason = "the suffix distinguishes active, retired, and transient entry ownership"
)]
pub(in crate::render_thread::sparkleflinger::gpu) struct ComposeSourceBindGroupCache {
    projected_entries: HashMap<ComposeSourceBindGroupKey, CachedComposeSourceBindGroup>,
    retired_projected_entries: HashMap<ComposeSourceBindGroupKey, CachedComposeSourceBindGroup>,
    transient_entries: VecDeque<CachedComposeSourceBindGroup>,
    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) creation_count: usize,
}

#[derive(Clone)]
struct CachedComposeSourceBindGroup {
    key: ComposeSourceBindGroupKey,
    source_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    source_lease: Option<Weak<GpuTextureFrameLease>>,
    #[cfg(any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    ))]
    native_screen_lease: Option<NativeScreenCacheLease>,
}

const COMPOSE_SOURCE_BIND_GROUP_CACHE_CAP: usize = 4;

impl ComposeSourceBindGroupCache {
    pub(in crate::render_thread::sparkleflinger::gpu) fn prepare_projected(
        &self,
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        target_generation: u64,
        front_view: &wgpu::TextureView,
        back_view: &wgpu::TextureView,
        source_count: usize,
        sources: impl IntoIterator<Item = (u64, wgpu::TextureView, Weak<GpuTextureFrameLease>)>,
    ) -> std::result::Result<
        (PreparedProjectedComposeBindGroups, usize),
        std::collections::TryReserveError,
    > {
        let mut entries = HashMap::new();
        entries.try_reserve(source_count.saturating_mul(2))?;
        let mut creation_count = 0_usize;
        for (source_storage_id, source_view, source_lease) in sources {
            for front_as_current in [true, false] {
                let key = ComposeSourceBindGroupKey {
                    target_generation,
                    source_storage_id,
                    front_as_current,
                };
                let cached = self
                    .projected_entries
                    .get(&key)
                    .filter(|cached| cached.source_view == source_view);
                let entry = if let Some(cached) = cached {
                    cached.clone()
                } else {
                    let (current_view, output_view) = if front_as_current {
                        (front_view, back_view)
                    } else {
                        (back_view, front_view)
                    };
                    creation_count = creation_count.saturating_add(1);
                    CachedComposeSourceBindGroup {
                        key,
                        source_view: source_view.clone(),
                        bind_group: create_compose_bind_group(
                            device,
                            pipeline,
                            current_view,
                            &source_view,
                            output_view,
                            "SparkleFlinger admitted projected-source bind group",
                        ),
                        source_lease: Some(source_lease.clone()),
                        #[cfg(any(
                            target_os = "windows",
                            all(target_os = "macos", feature = "screen-capture")
                        ))]
                        native_screen_lease: None,
                    }
                };
                entries.insert(key, entry);
            }
        }
        let mut retired_entries = HashMap::new();
        retired_entries.try_reserve(
            self.retired_projected_entries
                .len()
                .saturating_add(self.projected_entries.len()),
        )?;
        for (key, entry) in &self.retired_projected_entries {
            let remains_leased = entry
                .source_lease
                .as_ref()
                .is_some_and(|lease| lease.strong_count() > 0);
            if remains_leased && !entries.contains_key(key) {
                retired_entries.insert(*key, entry.clone());
            }
        }
        for (key, entry) in &self.projected_entries {
            let has_external_lease = entry
                .source_lease
                .as_ref()
                .is_some_and(|lease| lease.strong_count() > 1);
            if has_external_lease && !entries.contains_key(key) {
                retired_entries.insert(*key, entry.clone());
            }
        }
        Ok((
            PreparedProjectedComposeBindGroups {
                target_generation,
                entries,
                retired_entries,
            },
            creation_count,
        ))
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn install_projected(
        &mut self,
        prepared: PreparedProjectedComposeBindGroups,
        target_generation: u64,
    ) {
        debug_assert_eq!(prepared.target_generation, target_generation);
        debug_assert!(
            prepared
                .entries
                .keys()
                .all(|key| key.target_generation == target_generation)
        );
        self.projected_entries = prepared.entries;
        self.retired_projected_entries = prepared.retired_entries;
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn clear_projected(&mut self) {
        self.projected_entries.clear();
        self.retired_projected_entries.clear();
    }

    pub(super) fn get_projected(
        &self,
        target_generation: u64,
        source_storage_id: u64,
        source_view: &wgpu::TextureView,
        front_as_current: bool,
    ) -> Option<wgpu::BindGroup> {
        let key = ComposeSourceBindGroupKey {
            target_generation,
            source_storage_id,
            front_as_current,
        };
        exact_projected_entry(
            &self.projected_entries,
            &self.retired_projected_entries,
            &key,
        )
        .filter(|cached| cached.source_view == *source_view)
        .map(|cached| cached.bind_group.clone())
    }

    pub(super) fn get_or_create_transient(
        &mut self,
        device: &wgpu::Device,
        pipeline: &GpuCompositorPipeline,
        target_generation: u64,
        source_storage_id: u64,
        source_view: &wgpu::TextureView,
        front_as_current: bool,
        current_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
        #[cfg(any(
            target_os = "windows",
            all(target_os = "macos", feature = "screen-capture")
        ))]
        native_screen_lease: Option<NativeScreenCacheLease>,
    ) -> wgpu::BindGroup {
        let key = ComposeSourceBindGroupKey {
            target_generation,
            source_storage_id,
            front_as_current,
        };
        if let Some(cached) = self
            .transient_entries
            .iter()
            .find(|cached| cached.key == key && cached.source_view == *source_view)
        {
            return cached.bind_group.clone();
        }
        let bind_group = create_compose_bind_group(
            device,
            pipeline,
            current_view,
            source_view,
            output_view,
            "SparkleFlinger GPU imported producer bind group",
        );
        #[cfg(test)]
        {
            self.creation_count = self.creation_count.saturating_add(1);
        }
        if self.transient_entries.len() >= COMPOSE_SOURCE_BIND_GROUP_CACHE_CAP {
            self.transient_entries.pop_front();
        }
        self.transient_entries
            .push_back(CachedComposeSourceBindGroup {
                key,
                source_view: source_view.clone(),
                bind_group: bind_group.clone(),
                source_lease: None,
                #[cfg(any(
                    target_os = "windows",
                    all(target_os = "macos", feature = "screen-capture")
                ))]
                native_screen_lease,
            });
        bind_group
    }

    pub(super) fn release_source(&mut self, source_storage_id: u64) {
        self.projected_entries
            .retain(|key, _| key.source_storage_id != source_storage_id);
        self.retired_projected_entries
            .retain(|key, _| key.source_storage_id != source_storage_id);
        self.transient_entries
            .retain(|entry| entry.key.source_storage_id != source_storage_id);
    }

    #[cfg(any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    ))]
    pub(in crate::render_thread::sparkleflinger::gpu) fn release_native_screen_entries(&mut self) {
        self.projected_entries
            .retain(|_, entry| entry.native_screen_lease.is_none());
        self.retired_projected_entries
            .retain(|_, entry| entry.native_screen_lease.is_none());
        self.transient_entries
            .retain(|entry| entry.native_screen_lease.is_none());
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fn projected_entry_count(&self) -> usize {
        self.projected_entries.len()
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fn projected_source_storage_ids(
        &self,
    ) -> Vec<u64> {
        let mut source_storage_ids = self
            .projected_entries
            .keys()
            .map(|key| key.source_storage_id)
            .collect::<Vec<_>>();
        source_storage_ids.sort_unstable();
        source_storage_ids.dedup();
        source_storage_ids
    }

    #[cfg(test)]
    pub(in crate::render_thread::sparkleflinger::gpu) fn retired_projected_entry_count(
        &self,
    ) -> usize {
        self.retired_projected_entries.len()
    }
}

fn exact_projected_entry<'a, T>(
    entries: &'a HashMap<ComposeSourceBindGroupKey, T>,
    retired_entries: &'a HashMap<ComposeSourceBindGroupKey, T>,
    key: &ComposeSourceBindGroupKey,
) -> Option<&'a T> {
    entries.get(key).or_else(|| retired_entries.get(key))
}

#[cfg(feature = "allocation-contract-tests")]
pub(crate) struct ProjectedLookupAllocationFixture {
    entries: HashMap<ComposeSourceBindGroupKey, std::sync::Arc<()>>,
    retired_entries: HashMap<ComposeSourceBindGroupKey, std::sync::Arc<()>>,
    keys: Vec<ComposeSourceBindGroupKey>,
    zero_screen_layers: [CompositionLayer; 2],
}

#[cfg(feature = "allocation-contract-tests")]
impl ProjectedLookupAllocationFixture {
    pub(crate) fn new(source_count: usize) -> Self {
        let target_generation = 41;
        let mut entries = HashMap::with_capacity(source_count.saturating_mul(2));
        let mut keys = Vec::with_capacity(source_count.saturating_mul(2));
        for source_storage_id in 1..=u64::try_from(source_count).unwrap_or(u64::MAX) {
            for front_as_current in [true, false] {
                let key = ComposeSourceBindGroupKey {
                    target_generation,
                    source_storage_id,
                    front_as_current,
                };
                entries.insert(key, std::sync::Arc::new(()));
                keys.push(key);
            }
        }
        Self {
            entries,
            retired_entries: HashMap::new(),
            keys,
            zero_screen_layers: [
                CompositionLayer::replace_opaque(ProducerFrame::Canvas(
                    hypercolor_core::types::canvas::Canvas::new(4, 4),
                )),
                CompositionLayer::replace_opaque(ProducerFrame::Canvas(
                    hypercolor_core::types::canvas::Canvas::new(4, 4),
                )),
            ],
        }
    }

    pub(crate) fn run_round(&self) -> bool {
        let mut hits = 0_usize;
        for key in &self.keys {
            if exact_projected_entry(&self.entries, &self.retired_entries, key)
                .map(std::sync::Arc::clone)
                .is_some()
            {
                hits = hits.saturating_add(1);
            }
        }
        hits == self.keys.len() && !has_screen_upload_layers(&self.zero_screen_layers)
    }
}

pub(in crate::render_thread::sparkleflinger::gpu) fn create_compose_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    current: &wgpu::TextureView,
    source: &wgpu::TextureView,
    output: &wgpu::TextureView,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pipeline.compose_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(current),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(source),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pipeline.compose_params.binding(),
            },
        ],
    })
}

/// Uploads compose params into the uniform ring and returns the dynamic
/// offset the dispatch must bind. Byte-identical params re-bind the previous
/// slot without writing, as long as that slot came from a ring write.
pub(super) fn encode_compose_params_upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut GpuCompositorPipeline,
    surfaces: &mut GpuCompositorSurfaceSet,
    encoder: &mut wgpu::CommandEncoder,
    params: &[u8; COMPOSE_PARAM_BYTES],
) -> u32 {
    if surfaces.cached_compose_params.as_ref() == Some(params)
        && let Some(offset) = surfaces.cached_compose_params_offset
    {
        pipeline.compose_params.pin_last_slot();
        return offset;
    }
    let write = pipeline.compose_params.write(
        device,
        queue,
        encoder,
        &mut surfaces.pending_upload_buffers,
        params,
    );
    surfaces.cached_compose_params = Some(*params);
    surfaces.cached_compose_params_offset = write.reusable.then_some(write.offset);
    #[cfg(test)]
    {
        surfaces.compose_param_write_count = surfaces.compose_param_write_count.saturating_add(1);
    }
    write.offset
}

pub(in crate::render_thread::sparkleflinger::gpu) fn encode_compose_params(
    width: u32,
    height: u32,
    mode: ComposeShaderMode,
    layer: &CompositionLayer,
    source_flip_y: bool,
) -> [u8; COMPOSE_PARAM_BYTES] {
    let mut bytes = [0u8; COMPOSE_PARAM_BYTES];
    let transform = layer.transform.unwrap_or_default();
    let adjust = layer.adjust.unwrap_or_default();
    bytes[0..4].copy_from_slice(&width.to_le_bytes());
    bytes[4..8].copy_from_slice(&height.to_le_bytes());
    bytes[8..12].copy_from_slice(&(mode as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&(fit_mode(transform.fit) as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&layer.frame.width().to_le_bytes());
    bytes[20..24].copy_from_slice(&layer.frame.height().to_le_bytes());
    let processing = if layer.needs_processing_for_size(width, height) {
        1_u32
    } else {
        0_u32
    };
    bytes[24..28].copy_from_slice(&processing.to_le_bytes());
    bytes[28..32].copy_from_slice(&u32::from(source_flip_y).to_le_bytes());
    bytes[32..36].copy_from_slice(&layer.opacity.to_le_bytes());
    bytes[36..40].copy_from_slice(&transform.anchor.x.to_le_bytes());
    bytes[40..44].copy_from_slice(&transform.anchor.y.to_le_bytes());
    bytes[44..48].copy_from_slice(&transform.scale[0].to_le_bytes());
    bytes[48..52].copy_from_slice(&transform.scale[1].to_le_bytes());
    bytes[52..56].copy_from_slice(&transform.rotation.cos().to_le_bytes());
    bytes[56..60].copy_from_slice(&transform.rotation.sin().to_le_bytes());
    let sample_target_space = if transform.sample_target_space {
        1.0_f32
    } else {
        0.0_f32
    };
    bytes[60..64].copy_from_slice(&sample_target_space.to_le_bytes());
    bytes[64..68].copy_from_slice(&adjust.brightness.to_le_bytes());
    bytes[68..72].copy_from_slice(&adjust.saturation.to_le_bytes());
    bytes[72..76].copy_from_slice(&adjust.hue_shift.to_le_bytes());
    let tint_strength = (adjust.tint_strength * adjust.tint[3].clamp(0.0, 1.0)).clamp(0.0, 1.0);
    bytes[76..80].copy_from_slice(&tint_strength.to_le_bytes());
    bytes[80..84].copy_from_slice(&adjust.tint[0].to_le_bytes());
    bytes[84..88].copy_from_slice(&adjust.tint[1].to_le_bytes());
    bytes[88..92].copy_from_slice(&adjust.tint[2].to_le_bytes());
    bytes[92..96].copy_from_slice(&adjust.contrast.to_le_bytes());
    bytes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(in crate::render_thread::sparkleflinger::gpu) enum ComposeShaderMode {
    Replace = 0,
    Alpha = 1,
    Add = 2,
    Screen = 3,
    Multiply = 4,
    Overlay = 5,
    SoftLight = 6,
    ColorDodge = 7,
    Difference = 8,
    Tint = 9,
    LumaReveal = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ComposeFitMode {
    Contain = 0,
    Cover = 1,
    Stretch = 2,
    Tile = 3,
    Mirror = 4,
}

fn fit_mode(mode: hypercolor_types::viewport::FitMode) -> ComposeFitMode {
    match mode {
        hypercolor_types::viewport::FitMode::Contain => ComposeFitMode::Contain,
        hypercolor_types::viewport::FitMode::Cover => ComposeFitMode::Cover,
        hypercolor_types::viewport::FitMode::Stretch => ComposeFitMode::Stretch,
        hypercolor_types::viewport::FitMode::Tile => ComposeFitMode::Tile,
        hypercolor_types::viewport::FitMode::Mirror => ComposeFitMode::Mirror,
    }
}
