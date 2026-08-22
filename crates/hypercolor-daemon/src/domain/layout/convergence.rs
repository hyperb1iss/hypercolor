use std::collections::{HashMap, HashSet};

use hypercolor_core::device::DeviceLifecycleManager;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};

use crate::discovery::DiscoveryRuntime;
use crate::logical_devices::LogicalDevice;
use crate::scene_transactions::PreparedLayoutUpdate;

use super::auto_layout::{
    append_auto_layout_zones_for_device, reconcile_auto_layout_zones_for_device,
};
use super::catalog::LayoutCatalog;
use super::exclusions::LayoutExclusions;
use super::publication::LayoutPublication;

#[derive(Clone)]
pub(super) struct LayoutConvergence {
    catalog: LayoutCatalog,
    exclusions: LayoutExclusions,
    publication: LayoutPublication,
}

impl LayoutConvergence {
    pub(super) fn new(
        catalog: LayoutCatalog,
        exclusions: LayoutExclusions,
        publication: LayoutPublication,
    ) -> Self {
        Self {
            catalog,
            exclusions,
            publication,
        }
    }

    pub(super) async fn resolved_layout_device_id(
        &self,
        runtime: &DiscoveryRuntime,
        device_info: &DeviceInfo,
    ) -> String {
        if let Some(layout_device_id) = {
            let lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle
                .layout_device_id_for(device_info.id)
                .map(ToOwned::to_owned)
        } {
            return layout_device_id;
        }

        let fingerprint = runtime
            .device_registry
            .fingerprint_for_id(&device_info.id)
            .await;
        DeviceLifecycleManager::canonical_layout_device_id(device_info, fingerprint.as_ref())
    }

    pub(super) async fn layout_outputs_for(
        &self,
        runtime: &DiscoveryRuntime,
        requested_layout_id: &str,
    ) -> Vec<Output> {
        let tracked = runtime.device_registry.list().await;
        for device in &tracked {
            let layout_device_id = self.resolved_layout_device_id(runtime, &device.info).await;
            if layout_device_id != requested_layout_id {
                continue;
            }
            let mut scratch = SpatialLayout {
                id: format!("mint-{layout_device_id}"),
                name: device.info.name.clone(),
                description: None,
                canvas_width: 1,
                canvas_height: 1,
                zones: Vec::new(),
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                spaces: None,
                version: 1,
            };
            let _ =
                append_auto_layout_zones_for_device(&mut scratch, &layout_device_id, &device.info);
            return scratch.zones;
        }
        Vec::new()
    }

    pub(super) async fn connected_display_surface_layouts(
        &self,
        runtime: &DiscoveryRuntime,
    ) -> Vec<(DeviceId, String, SpatialLayout)> {
        runtime
            .device_registry
            .list()
            .await
            .into_iter()
            .filter(|tracked| tracked.state.is_renderable())
            .filter_map(|tracked| {
                let surface = crate::domain::display::display_surface_info(&tracked.info)?;
                Some((
                    tracked.info.id,
                    tracked.info.name.clone(),
                    crate::domain::display::display_face_layout(
                        tracked.info.id,
                        tracked.info.name.as_str(),
                        surface,
                    ),
                ))
            })
            .collect()
    }

    pub(super) async fn active_layout_targets_enabled_device(
        &self,
        runtime: &DiscoveryRuntime,
        physical_id: DeviceId,
        layout_device_id: &str,
    ) -> bool {
        let candidate_ids = enabled_layout_targets(runtime, physical_id, layout_device_id).await;
        if self
            .publication
            .current()
            .zones
            .iter()
            .any(|zone| candidate_ids.contains(&zone.device_id))
        {
            return true;
        }

        self.publication
            .scenes()
            .snapshot()
            .await
            .active_render_groups()
            .iter()
            .flat_map(|group| group.layout.zones.iter())
            .any(|zone| candidate_ids.contains(&zone.device_id))
    }

    pub(super) async fn sync_connectivity(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        let convergence = self.clone();
        if let Err(error) = tokio::spawn(async move {
            convergence
                .sync_connectivity_workflow(&runtime, limit_to_devices.as_ref())
                .await;
        })
        .await
        {
            tracing::warn!(%error, "layout connectivity workflow failed");
        }
    }

    async fn sync_connectivity_workflow(
        &self,
        runtime: &DiscoveryRuntime,
        limit_to_devices: Option<&HashSet<DeviceId>>,
    ) {
        for tracked in runtime.device_registry.list().await {
            let device_id = tracked.info.id;
            if limit_to_devices.is_some_and(|allowed| !allowed.contains(&device_id)) {
                continue;
            }

            let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;
            let connect_behavior = crate::discovery::desired_connect_behavior(
                runtime,
                device_id,
                &tracked.info,
                fingerprint.as_ref(),
                tracked.connect_behavior,
                tracked.user_settings.enabled,
            )
            .await;
            let actions = {
                let mut lifecycle = runtime.lifecycle_manager.lock().await;
                lifecycle.on_discovered_with_behavior(
                    device_id,
                    &tracked.info,
                    fingerprint.as_ref(),
                    connect_behavior,
                )
            };
            if actions.is_empty() {
                continue;
            }

            crate::discovery::execute_lifecycle_actions(runtime.clone(), actions).await;
            crate::discovery::sync_registry_state(runtime, device_id).await;
        }

        self.sync_active_layout_workflow(runtime, limit_to_devices)
            .await;
    }

    pub(super) async fn sync_active_layout(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        let convergence = self.clone();
        if let Err(error) = tokio::spawn(async move {
            convergence
                .sync_active_layout_workflow(&runtime, limit_to_devices.as_ref())
                .await;
        })
        .await
        {
            tracing::warn!(%error, "auto-layout repair workflow failed");
        }
    }

    async fn sync_active_layout_workflow(
        &self,
        runtime: &DiscoveryRuntime,
        limit_to_devices: Option<&HashSet<DeviceId>>,
    ) {
        let tracked_devices = runtime
            .device_registry
            .list()
            .await
            .into_iter()
            .map(|tracked| TrackedLayoutDevice {
                info: tracked.info,
                renderable: tracked.state.is_renderable(),
            })
            .collect::<Vec<_>>();
        let logical_store = runtime.logical_devices.read().await.clone();
        let canonical_layout_ids = canonical_layout_ids(runtime, &tracked_devices).await;
        let guard = self.publication.acquire_update_guard().await;
        let original_layout = self.publication.current();
        let mut layout = original_layout.clone();
        let excluded_layout_device_ids = self.exclusions.excluded_device_ids(&layout).await;
        let inactive_ids = inactive_device_ids(runtime, &layout).await;
        let repair = repair_renderable_devices(
            &mut layout,
            tracked_devices,
            &logical_store,
            &canonical_layout_ids,
            &excluded_layout_device_ids,
            &inactive_ids,
            limit_to_devices,
        );
        if repair.devices.is_empty() {
            return;
        }

        let prepared = match PreparedLayoutUpdate::try_new(layout.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::warn!(%error, "rejected auto-layout repair before persistence");
                return;
            }
        };
        if let Err(error) = self
            .publication
            .apply_prepared_under_guard(&guard, prepared)
            .await
        {
            tracing::warn!(%error, "rejected auto-layout repair before persistence");
            return;
        }
        if !self.persist_repair(&guard, &layout, original_layout).await {
            return;
        }

        tracing::info!(
            layout_id = %layout.id,
            repaired_device_count = repair.devices.len(),
            repaired_zone_count = repair.zone_count,
            repaired_devices = ?repair.devices,
            "reconciled existing auto-layout zones in the active layout"
        );
    }

    async fn persist_repair(
        &self,
        guard: &crate::scene_transactions::LayoutUpdateGuard,
        layout: &SpatialLayout,
        original_layout: SpatialLayout,
    ) -> bool {
        let (previous_saved_layout, snapshot) = {
            let mut layouts = self.catalog.entries().write().await;
            let previous = layouts.insert(layout.id.clone(), layout.clone());
            (previous, layouts.clone())
        };
        let Err(error) = self.catalog.save_snapshot(snapshot).await else {
            return true;
        };

        let rollback_layout = previous_saved_layout
            .as_ref()
            .cloned()
            .unwrap_or(original_layout);
        let rollback_snapshot = {
            let mut layouts = self.catalog.entries().write().await;
            if let Some(previous) = previous_saved_layout {
                layouts.insert(layout.id.clone(), previous);
            } else {
                layouts.remove(&layout.id);
            }
            layouts.clone()
        };
        let layout_store_rollback = self.catalog.save_snapshot(rollback_snapshot).await.err();
        let renderer_rollback = match PreparedLayoutUpdate::try_new(rollback_layout) {
            Ok(prepared) => self
                .publication
                .apply_prepared_under_guard(guard, prepared)
                .await
                .err()
                .map(|error| error.to_string()),
            Err(error) => Some(error.to_string()),
        };
        tracing::warn!(
            path = %self.catalog.path().display(),
            %error,
            layout_store_rollback = ?layout_store_rollback,
            renderer_rollback = ?renderer_rollback,
            "failed to persist auto-updated layout store; restored previous layout"
        );
        false
    }
}

async fn enabled_layout_targets(
    runtime: &DiscoveryRuntime,
    physical_id: DeviceId,
    layout_device_id: &str,
) -> HashSet<String> {
    let logical_store = runtime.logical_devices.read().await;
    let mut candidates = crate::logical_devices::list_for_physical(&logical_store, physical_id)
        .into_iter()
        .filter(|entry| entry.enabled)
        .map(|entry| entry.id)
        .collect::<HashSet<_>>();
    if logical_store
        .get(layout_device_id)
        .is_none_or(|entry| entry.enabled)
    {
        candidates.insert(layout_device_id.to_owned());
    }
    candidates
}

async fn canonical_layout_ids(
    runtime: &DiscoveryRuntime,
    tracked_devices: &[TrackedLayoutDevice],
) -> HashMap<DeviceId, String> {
    let lifecycle_layout_ids = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        tracked_devices
            .iter()
            .map(|tracked| {
                (
                    tracked.info.id,
                    lifecycle
                        .layout_device_id_for(tracked.info.id)
                        .map(ToOwned::to_owned),
                )
            })
            .collect::<HashMap<_, _>>()
    };
    let mut canonical = HashMap::with_capacity(tracked_devices.len());
    for tracked in tracked_devices {
        let device_id = tracked.info.id;
        let layout_id = if let Some(Some(layout_id)) = lifecycle_layout_ids.get(&device_id) {
            layout_id.clone()
        } else {
            let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;
            DeviceLifecycleManager::canonical_layout_device_id(&tracked.info, fingerprint.as_ref())
        };
        canonical.insert(device_id, layout_id);
    }
    canonical
}

async fn inactive_device_ids(
    runtime: &DiscoveryRuntime,
    layout: &SpatialLayout,
) -> HashSet<DeviceId> {
    let manager = runtime.backend_manager.lock().await;
    manager
        .connected_devices_without_layout_targets(layout)
        .into_iter()
        .map(|(_, device_id)| device_id)
        .collect()
}

struct LayoutRepair {
    devices: Vec<String>,
    zone_count: usize,
}

struct TrackedLayoutDevice {
    info: DeviceInfo,
    renderable: bool,
}

fn repair_renderable_devices(
    layout: &mut SpatialLayout,
    tracked_devices: Vec<TrackedLayoutDevice>,
    logical_store: &HashMap<String, LogicalDevice>,
    canonical_layout_ids: &HashMap<DeviceId, String>,
    excluded_layout_device_ids: &HashSet<String>,
    inactive_ids: &HashSet<DeviceId>,
    limit_to_devices: Option<&HashSet<DeviceId>>,
) -> LayoutRepair {
    let mut repair = LayoutRepair {
        devices: Vec::new(),
        zone_count: 0,
    };
    for tracked in tracked_devices {
        let device_id = tracked.info.id;
        if !tracked.renderable
            || limit_to_devices.is_some_and(|allowed| !allowed.contains(&device_id))
        {
            continue;
        }
        let layout_device_id = canonical_layout_ids
            .get(&device_id)
            .expect("tracked device should have a canonical layout id");
        let default_enabled = logical_store
            .get(layout_device_id)
            .is_none_or(|entry| entry.enabled);
        if !default_enabled || excluded_layout_device_ids.contains(layout_device_id) {
            continue;
        }

        let repaired =
            reconcile_auto_layout_zones_for_device(layout, layout_device_id, &tracked.info);
        if repaired > 0 {
            repair.zone_count = repair.zone_count.saturating_add(repaired);
            repair
                .devices
                .push(format!("{} ({device_id})", tracked.info.name));
        }
        if inactive_ids.contains(&device_id) {
            tracing::debug!(
                device_id = %device_id,
                layout_device_id,
                "leaving layout-inactive device unmapped until explicitly targeted"
            );
        }
    }
    repair
}
