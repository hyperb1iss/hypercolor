//! Prepared portable scene replacement under the normal scene write authority.

use std::collections::BTreeMap;

use hypercolor_types::portable_setup::{PortableScene, SetupBindings};
use uuid::Uuid;

use super::{
    DomainError, HypercolorEvent, OwnedRwLockWriteGuard, SceneId, SceneLibraryChangeKind,
    SceneManager, SceneService, SceneStore, SceneStoreSave,
};
use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};

pub(crate) struct PreparedPortableScenes {
    service: SceneService,
    manager: OwnedRwLockWriteGuard<SceneManager>,
    store: OwnedRwLockWriteGuard<SceneStore>,
    candidate: Option<SceneManager>,
    pending: Option<SceneStoreSave>,
    stored: Option<std::collections::HashMap<SceneId, hypercolor_types::scene::Scene>>,
    events: Vec<HypercolorEvent>,
    _snapshot_guard: OwnedRwLockWriteGuard<()>,
}

pub(crate) enum PortableSceneSelection {
    Preserve,
    Apply(Option<SceneId>),
}

impl SceneService {
    pub(crate) async fn prepare_portable_replace(
        &self,
        expected: &BTreeMap<Uuid, PortableScene>,
        desired: &BTreeMap<Uuid, PortableScene>,
        bindings: &SetupBindings,
        projection_bindings: &SetupBindings,
        canvas: (u32, u32),
        devices: &std::collections::HashMap<
            hypercolor_types::device::DeviceId,
            hypercolor_types::device::DeviceInfo,
        >,
    ) -> Result<PreparedPortableScenes, DomainError> {
        let snapshot_guard = self.0.snapshot_save_gate.clone().write_owned().await;
        let manager = self.0.manager.clone().write_owned().await;
        let mut projected = manager
            .list()
            .into_iter()
            .map(|scene| {
                PortableScene::project(scene, projection_bindings).map(|value| (scene.id.0, value))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(|error| DomainError::validation(error.to_string()))?;
        for scene in projected.values_mut() {
            scene.zones.retain(|zone| {
                zone.display_target
                    .as_ref()
                    .is_none_or(|target| bindings.local_device(target.device).is_some())
            });
        }
        let materialized_expected = expected
            .iter()
            .map(|(id, scene)| {
                PortableScene::project(&scene.materialize(SceneId(*id), bindings, canvas), bindings)
                    .map(|value| (*id, value))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(|error| DomainError::validation(error.to_string()))?;
        if projected != materialized_expected {
            return Err(DomainError::conflict(
                "scenes changed before setup restoration",
            ));
        }
        let mut candidate = manager.clone();
        let mut events = Vec::new();
        for scene in manager.list() {
            if !desired.contains_key(&scene.id.0) {
                candidate.delete(&scene.id).map_err(DomainError::Internal)?;
                events.push(HypercolorEvent::SceneLibraryChanged {
                    scene_id: scene.id,
                    kind: SceneLibraryChangeKind::Deleted,
                    name: None,
                });
            }
        }
        for (id, definition) in desired {
            let mut scene = definition.materialize(SceneId(*id), bindings, canvas);
            for zone in &mut scene.zones {
                if let Some(surface) = zone
                    .display_target
                    .as_ref()
                    .and_then(|target| devices.get(&target.device_id))
                    .and_then(crate::domain::display::display_surface_info)
                {
                    zone.layout.canvas_width = surface.width;
                    zone.layout.canvas_height = surface.height;
                }
            }
            let kind = if let Some(old) = manager.get(&scene.id) {
                scene.zones_revision = old.zones_revision;
                for zone in &mut scene.zones {
                    if let Some(old) = old.zones.iter().find(|old| old.id == zone.id) {
                        zone.layers_version = old.layers_version;
                        zone.controls_version = old.controls_version;
                    }
                }
                if &scene == old {
                    continue;
                }
                scene.zones_revision = scene.zones_revision.saturating_add(1);
                for zone in &mut scene.zones {
                    zone.layers_version = zone.layers_version.saturating_add(1);
                    zone.controls_version = zone.controls_version.saturating_add(1);
                }
                candidate
                    .update(scene.clone())
                    .map_err(DomainError::Internal)?;
                SceneLibraryChangeKind::Updated
            } else {
                candidate
                    .create(scene.clone())
                    .map_err(DomainError::Internal)?;
                SceneLibraryChangeKind::Created
            };
            events.push(HypercolorEvent::SceneLibraryChanged {
                scene_id: scene.id,
                kind,
                name: Some(scene.name),
            });
        }
        let store = self
            .0
            .store
            .as_ref()
            .ok_or_else(|| DomainError::conflict("scene persistence is unavailable"))?
            .clone()
            .write_owned()
            .await;
        let pending = store
            .reserve_save(candidate.list().into_iter().cloned())
            .map_err(|error| DomainError::Internal(error.into()))?;
        Ok(PreparedPortableScenes {
            service: self.clone(),
            manager,
            store,
            candidate: Some(candidate),
            pending: Some(pending),
            stored: None,
            events,
            _snapshot_guard: snapshot_guard,
        })
    }
}

impl PreparedPortableScenes {
    pub(crate) fn prepare_runtime(
        &mut self,
        expected_active: Option<SceneId>,
        selection: PortableSceneSelection,
        preferences: &crate::display_preferences::DisplayPreferencesStore,
        devices: &std::collections::HashMap<
            hypercolor_types::device::DeviceId,
            hypercolor_types::device::DeviceInfo,
        >,
    ) -> Result<(), DomainError> {
        if self.manager.active_scene_id().copied() != expected_active {
            return Err(DomainError::conflict(
                "active scene changed before setup restoration",
            ));
        }
        let candidate = self
            .candidate
            .as_mut()
            .expect("unpublished scene candidate");
        if let PortableSceneSelection::Apply(selected) = selection {
            let previous = candidate.active_scene_id().copied();
            candidate.priority_stack_mut().clear();
            if let Some(id) = selected {
                let mut transition = candidate
                    .get(&id)
                    .ok_or_else(|| DomainError::validation("restored active scene is missing"))?
                    .transition
                    .clone();
                transition.duration_ms = 0;
                candidate
                    .activate(&id, Some(transition))
                    .map_err(DomainError::Internal)?;
                if let Some(current) = candidate.active_scene() {
                    self.events.push(super::active_scene_changed_event(
                        previous,
                        current,
                        hypercolor_types::event::SceneChangeReason::UserActivate,
                    ));
                }
            } else {
                candidate.invalidate_resolved_zones();
            }
        }
        for (id, info) in devices {
            let Some(preference) = preferences.get(*id) else {
                candidate.remove_default_display_zone(*id);
                continue;
            };
            let Some(surface) = crate::domain::display::display_surface_info(info) else {
                continue;
            };
            let layout = crate::domain::display::display_face_layout(*id, &info.name, surface);
            candidate.set_default_display_zone(crate::domain::display::build_default_display_zone(
                *id,
                &info.name,
                preference.effect_id,
                preference,
                layout,
            ));
        }
        Ok(())
    }

    pub(crate) fn candidate(&self) -> &SceneManager {
        self.candidate
            .as_ref()
            .expect("unpublished scene candidate")
    }

    pub(crate) fn persist(&mut self, shutdown: &PersistenceShutdown) -> AtomicWriteWaitOutcome {
        if shutdown.is_shutdown() {
            return AtomicWriteWaitOutcome::ShutdownPending;
        }
        self.service.forget_store_durability();
        let pending = self.pending.take().expect("scene candidate persisted once");
        let (stored, result) = pending
            .admit()
            .commit_and_wait_durable_or_shutdown(shutdown);
        if matches!(result, AtomicWriteWaitOutcome::Written) {
            self.stored = Some(stored);
        }
        result
    }

    pub(crate) fn publish(&mut self) {
        let stored = self
            .stored
            .take()
            .expect("scene candidate durable before publication");
        self.store.replace_named_scenes(stored.into_values());
        *self.manager = self
            .candidate
            .take()
            .expect("scene candidate published once");
        let ticket = self
            .service
            .0
            .commits
            .admit(self.service.0.event_bus.clone());
        let generation = ticket.generation();
        self.service.move_store_projection(generation);
        self.service
            .note_store_receipt(generation, self.service.store_epoch());
        self.service
            .0
            .plan
            .store(std::sync::Arc::new(self.manager.plan_snapshot(generation)));
        ticket.release(std::mem::take(&mut self.events));
    }
}

#[cfg(test)]
mod tests {
    use crate::app_state::AppState;
    use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};
    use hypercolor_types::portable_setup::{PortableScene, SetupBindings};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn preparation_fences_ordinary_scene_writes_until_durable_publication() {
        let state = Arc::new(AppState::new());
        let service = &state.scene_manager;
        let bindings = SetupBindings::default();
        let manager = service.snapshot().await;
        let expected = manager
            .list()
            .into_iter()
            .map(|scene| {
                (
                    scene.id.0,
                    PortableScene::project(scene, &bindings)
                        .expect("portable fixture should be valid"),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut desired = expected.clone();
        desired
            .values_mut()
            .next()
            .expect("portable fixture should be valid")
            .description = Some("Restored setup".into());
        let mut prepared = service
            .prepare_portable_replace(
                &expected,
                &desired,
                &bindings,
                &bindings,
                (640, 480),
                &std::collections::HashMap::default(),
            )
            .await
            .expect("portable fixture should be valid");
        assert!(service.0.manager.try_write().is_err());
        assert!(matches!(
            prepared.persist(&PersistenceShutdown::default()),
            AtomicWriteWaitOutcome::Written
        ));
        prepared.publish();
        drop(prepared);
        let actual = service.snapshot().await;
        assert_eq!(
            actual.list()[0].description.as_deref(),
            Some("Restored setup")
        );
        assert!(
            service
                .prepare_portable_replace(
                    &expected,
                    &desired,
                    &bindings,
                    &bindings,
                    (640, 480),
                    &std::collections::HashMap::default()
                )
                .await
                .is_err()
        );
    }
}
