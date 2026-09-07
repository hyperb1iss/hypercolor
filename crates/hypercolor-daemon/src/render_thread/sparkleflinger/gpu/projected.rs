use std::collections::HashMap;
use std::sync::Arc;

#[cfg(test)]
use hypercolor_types::canvas::BYTES_PER_PIXEL;
use hypercolor_types::scene::ZoneId;

use super::super::ProjectedZoneTextureRequirement;
use super::compositor::PreparedProjectedComposeBindGroups;
use super::{
    GpuCanvasPreparation, GpuCompositorSurfaceSet, GpuProjectionSnapshot, GpuSparkleFlinger,
};

pub(crate) enum GpuProjectedScenePreparation {
    Disabled {
        projected_bind_groups: Option<PreparedProjectedComposeBindGroups>,
    },
    Admitted {
        snapshots: HashMap<ZoneId, Option<GpuProjectionSnapshot>>,
        compositor_surfaces: HashMap<(u32, u32), Option<GpuCompositorSurfaceSet>>,
        projected_bind_groups: PreparedProjectedComposeBindGroups,
        scene_extent: (u32, u32),
    },
    ResourceFallback {
        error: GpuProjectedSceneResourceError,
        projected_bind_groups: Option<PreparedProjectedComposeBindGroups>,
    },
}

impl GpuProjectedScenePreparation {
    pub(in crate::render_thread::sparkleflinger) const fn is_admitted(&self) -> bool {
        matches!(self, Self::Admitted { .. })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GpuProjectedSceneResourceError {
    #[error("GPU projection snapshot metadata allocation failed")]
    Metadata(#[source] std::collections::TryReserveError),
    #[error("GPU projection snapshot allocation failed for {width}x{height}")]
    Snapshot {
        width: u32,
        height: u32,
        #[source]
        source: anyhow::Error,
    },
    #[error("GPU compositor surface metadata allocation failed")]
    CompositorMetadata(#[source] std::collections::TryReserveError),
    #[error("GPU compositor surface allocation failed for {width}x{height}")]
    CompositorSurface {
        width: u32,
        height: u32,
        #[source]
        source: anyhow::Error,
    },
    #[error("GPU projected bind-group metadata allocation failed")]
    BindGroupMetadata(#[source] std::collections::TryReserveError),
    #[error("GPU projected compositor surface {width}x{height} was not admitted")]
    MissingCompositorSurface { width: u32, height: u32 },
    #[cfg(test)]
    #[error("GPU projection snapshot allocation failure injected by test")]
    Injected,
}

impl GpuSparkleFlinger {
    fn prepare_empty_projected_bind_groups(
        &self,
        canvas_preparation: Option<&GpuCanvasPreparation>,
    ) -> Option<PreparedProjectedComposeBindGroups> {
        let surfaces = match canvas_preparation {
            Some(preparation) => preparation.compositor_surfaces(),
            None => self.surfaces.as_ref(),
        };
        surfaces.map(|surfaces| PreparedProjectedComposeBindGroups::empty(surfaces.generation))
    }

    fn projected_scene_resource_fallback(
        &self,
        error: GpuProjectedSceneResourceError,
        canvas_preparation: Option<&GpuCanvasPreparation>,
    ) -> GpuProjectedScenePreparation {
        GpuProjectedScenePreparation::ResourceFallback {
            error,
            projected_bind_groups: self.prepare_empty_projected_bind_groups(canvas_preparation),
        }
    }

    pub(crate) fn prepare_projected_scene_resources(
        &self,
        requirements: &[ProjectedZoneTextureRequirement],
        gpu_projection_admitted: bool,
        scene_width: u32,
        scene_height: u32,
        canvas_preparation: Option<&GpuCanvasPreparation>,
    ) -> GpuProjectedScenePreparation {
        if !gpu_projection_admitted || requirements.is_empty() {
            return GpuProjectedScenePreparation::Disabled {
                projected_bind_groups: self.prepare_empty_projected_bind_groups(canvas_preparation),
            };
        }
        #[cfg(test)]
        if self.fail_next_projected_scene_preparation.replace(false) {
            return self.projected_scene_resource_fallback(
                GpuProjectedSceneResourceError::Injected,
                canvas_preparation,
            );
        }
        let mut snapshots = HashMap::new();
        if let Err(error) = snapshots.try_reserve(requirements.len()) {
            return self.projected_scene_resource_fallback(
                GpuProjectedSceneResourceError::Metadata(error),
                canvas_preparation,
            );
        }
        for requirement in requirements {
            let reusable = self
                .projected_zone_snapshots
                .get(&requirement.zone_id)
                .and_then(Option::as_ref)
                .is_some_and(|snapshot| {
                    snapshot.width == requirement.width && snapshot.height == requirement.height
                });
            let snapshot = if reusable {
                None
            } else {
                let allocation = GpuProjectionSnapshot::try_new(
                    &self.device,
                    requirement.width,
                    requirement.height,
                )
                .inspect(|_| {
                    #[cfg(test)]
                    self.snapshot_texture_allocation_count.set(
                        self.snapshot_texture_allocation_count
                            .get()
                            .saturating_add(1),
                    );
                });
                let snapshot = match allocation {
                    Ok(snapshot) => snapshot,
                    Err(source) => {
                        return self.projected_scene_resource_fallback(
                            GpuProjectedSceneResourceError::Snapshot {
                                width: requirement.width,
                                height: requirement.height,
                                source,
                            },
                            canvas_preparation,
                        );
                    }
                };
                Some(snapshot)
            };
            snapshots.insert(requirement.zone_id, snapshot);
        }
        let mut compositor_surfaces = HashMap::new();
        if let Err(error) = compositor_surfaces.try_reserve(requirements.len().saturating_add(1)) {
            return self.projected_scene_resource_fallback(
                GpuProjectedSceneResourceError::CompositorMetadata(error),
                canvas_preparation,
            );
        }
        compositor_surfaces.insert((scene_width, scene_height), None);
        for requirement in requirements {
            compositor_surfaces
                .entry((requirement.width, requirement.height))
                .or_insert(None);
        }
        for (&(width, height), surface) in &mut compositor_surfaces {
            let supplied_by_resize = canvas_preparation
                .and_then(GpuCanvasPreparation::compositor_surfaces)
                .is_some_and(|surfaces| (surfaces.width, surfaces.height) == (width, height));
            let active_reusable = canvas_preparation
                .and_then(GpuCanvasPreparation::compositor_surfaces)
                .is_none()
                && self
                    .surfaces
                    .as_ref()
                    .is_some_and(|current| current.width == width && current.height == height);
            let cached_reusable = self
                .compositor_surface_cache
                .get(&(width, height))
                .is_some_and(Option::is_some);
            if supplied_by_resize || active_reusable || cached_reusable {
                continue;
            }
            let replacement = match self.try_create_compositor_surface_set(width, height) {
                Ok(replacement) => replacement,
                Err(source) => {
                    return self.projected_scene_resource_fallback(
                        GpuProjectedSceneResourceError::CompositorSurface {
                            width,
                            height,
                            source,
                        },
                        canvas_preparation,
                    );
                }
            };
            *surface = Some(replacement);
        }
        let scene_extent = (scene_width, scene_height);
        let scene_surface = compositor_surfaces
            .get(&scene_extent)
            .and_then(Option::as_ref)
            .or_else(|| {
                canvas_preparation
                    .and_then(GpuCanvasPreparation::compositor_surfaces)
                    .filter(|surface| (surface.width, surface.height) == scene_extent)
            })
            .or_else(|| {
                self.surfaces
                    .as_ref()
                    .filter(|surface| (surface.width, surface.height) == scene_extent)
            })
            .or_else(|| {
                self.compositor_surface_cache
                    .get(&scene_extent)
                    .and_then(Option::as_ref)
            });
        let Some(scene_surface) = scene_surface else {
            return self.projected_scene_resource_fallback(
                GpuProjectedSceneResourceError::MissingCompositorSurface {
                    width: scene_width,
                    height: scene_height,
                },
                canvas_preparation,
            );
        };
        let sources = requirements.iter().map(|requirement| {
            let snapshot = snapshots
                .get(&requirement.zone_id)
                .and_then(Option::as_ref)
                .or_else(|| {
                    self.projected_zone_snapshots
                        .get(&requirement.zone_id)
                        .and_then(Option::as_ref)
                })
                .expect("projected source snapshot must be admitted before bind groups");
            (
                snapshot.texture.storage_id,
                snapshot.texture.view.clone(),
                Arc::downgrade(&snapshot.lease),
            )
        });
        let (projected_bind_groups, created_bind_groups) =
            match scene_surface.compose_source_bind_groups.prepare_projected(
                &self.device,
                &self.pipeline,
                scene_surface.generation,
                &scene_surface.front.view,
                &scene_surface.back.view,
                requirements.len(),
                sources,
            ) {
                Ok(prepared) => prepared,
                Err(error) => {
                    return self.projected_scene_resource_fallback(
                        GpuProjectedSceneResourceError::BindGroupMetadata(error),
                        canvas_preparation,
                    );
                }
            };
        #[cfg(test)]
        self.projected_bind_group_creation_count.set(
            self.projected_bind_group_creation_count
                .get()
                .saturating_add(created_bind_groups),
        );
        #[cfg(not(test))]
        let _ = created_bind_groups;
        GpuProjectedScenePreparation::Admitted {
            snapshots,
            compositor_surfaces,
            projected_bind_groups,
            scene_extent,
        }
    }

    pub(crate) fn apply_projected_scene_resources(
        &mut self,
        preparation: GpuProjectedScenePreparation,
    ) {
        let clear_non_admitted =
            |gpu: &mut Self, projected_bind_groups: Option<PreparedProjectedComposeBindGroups>| {
                if let Some(surfaces) = &mut gpu.surfaces {
                    if let Some(projected_bind_groups) = projected_bind_groups {
                        surfaces
                            .compose_source_bind_groups
                            .install_projected(projected_bind_groups, surfaces.generation);
                    } else {
                        debug_assert!(false, "active surfaces require a prepared empty bind map");
                        surfaces.compose_source_bind_groups.clear_projected();
                    }
                } else {
                    debug_assert!(projected_bind_groups.is_none());
                }
                gpu.projected_zone_snapshots.clear();
                gpu.compositor_surface_cache.clear();
            };
        let GpuProjectedScenePreparation::Admitted {
            mut snapshots,
            mut compositor_surfaces,
            projected_bind_groups,
            scene_extent,
        } = preparation
        else {
            match preparation {
                GpuProjectedScenePreparation::Disabled {
                    projected_bind_groups,
                } => clear_non_admitted(self, projected_bind_groups),
                GpuProjectedScenePreparation::ResourceFallback {
                    error,
                    projected_bind_groups,
                } => {
                    clear_non_admitted(self, projected_bind_groups);
                    tracing::warn!(
                        %error,
                        "using CPU scene projection after GPU snapshot admission failed"
                    );
                }
                GpuProjectedScenePreparation::Admitted { .. } => unreachable!(),
            }
            return;
        };
        self.discard_pending_preview_map();
        self.clear_sampling_readback_latch();
        drop(self.supersede_frame_in_flight("projected scene resources committed"));
        self.discard_pending_uploads();
        let mut installed_surfaces = std::mem::take(&mut self.compositor_surface_cache);
        let mut active_surface = self.surfaces.take();
        for (&extent, surface) in &mut compositor_surfaces {
            if surface.is_some() {
                continue;
            }
            if active_surface
                .as_ref()
                .is_some_and(|active| (active.width, active.height) == extent)
            {
                *surface = active_surface.take();
            } else {
                *surface = installed_surfaces.remove(&extent).flatten();
            }
            debug_assert!(surface.is_some());
        }
        let mut scene_surface = compositor_surfaces
            .remove(&scene_extent)
            .flatten()
            .or(active_surface);
        debug_assert!(
            scene_surface
                .as_ref()
                .is_some_and(|surfaces| { (surfaces.width, surfaces.height) == scene_extent })
        );
        if let Some(surface) = &mut scene_surface {
            surface
                .compose_source_bind_groups
                .install_projected(projected_bind_groups, surface.generation);
        }
        self.surfaces = scene_surface;
        self.compositor_surface_cache = compositor_surfaces;
        self.current_output = None;
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.preview_surfaces = None;
        self.cached_preview_surfaces.clear();
        self.pending_preview_map = None;
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
        self.spatial_sampler.clear_bind_groups();
        let mut installed = std::mem::take(&mut self.projected_zone_snapshots);
        for (zone_id, snapshot) in &mut snapshots {
            if snapshot.is_none() {
                *snapshot = installed.remove(zone_id).flatten();
            }
            debug_assert!(snapshot.is_some());
        }
        self.projected_zone_snapshots = snapshots;
    }

    pub(crate) fn has_projected_zone_resource(
        &self,
        zone_id: ZoneId,
        width: u32,
        height: u32,
    ) -> bool {
        self.projected_zone_snapshots
            .get(&zone_id)
            .and_then(Option::as_ref)
            .is_some_and(|snapshot| snapshot.width == width && snapshot.height == height)
    }

    #[cfg(test)]
    pub(crate) fn snapshot_texture_allocation_count(&self) -> usize {
        self.snapshot_texture_allocation_count.get()
    }

    #[cfg(test)]
    pub(crate) fn compositor_surface_allocation_count(&self) -> usize {
        self.compositor_surface_allocation_count.get()
    }

    #[cfg(test)]
    pub(crate) fn projected_bind_group_creation_count(&self) -> usize {
        self.projected_bind_group_creation_count.get()
    }

    #[cfg(test)]
    pub(crate) fn projected_bind_group_entry_count(&self) -> usize {
        self.surfaces.as_ref().map_or(0, |surfaces| {
            surfaces.compose_source_bind_groups.projected_entry_count()
        })
    }

    #[cfg(test)]
    pub(crate) fn projected_bind_group_source_storage_ids(&self) -> Vec<u64> {
        self.surfaces.as_ref().map_or_else(Vec::new, |surfaces| {
            surfaces
                .compose_source_bind_groups
                .projected_source_storage_ids()
        })
    }

    #[cfg(test)]
    pub(crate) fn retired_projected_bind_group_entry_count(&self) -> usize {
        self.surfaces.as_ref().map_or(0, |surfaces| {
            surfaces
                .compose_source_bind_groups
                .retired_projected_entry_count()
        })
    }

    #[cfg(test)]
    pub(crate) fn projected_snapshot_retained_bytes(&self) -> u64 {
        self.projected_zone_snapshots
            .values()
            .filter_map(Option::as_ref)
            .fold(0_u64, |total, snapshot| {
                total.saturating_add(
                    u64::from(snapshot.width)
                        .saturating_mul(u64::from(snapshot.height))
                        .saturating_mul(BYTES_PER_PIXEL as u64),
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn fail_next_projected_scene_preparation(&self) {
        self.fail_next_projected_scene_preparation.set(true);
    }
}
