//! Scene domain services: the owned-candidate mutation API, the commit
//! that admits it, and scene activation (Spec 76 §2.3).
//!
//! The mutation model matches the persistence layer's generation-based
//! convergence rather than fighting it. A [`SceneMutation`] is an owned
//! candidate: it is cloned out under a brief read lock, mutated with no
//! lock held at all, and either committed or dropped. Dropping one
//! discards a local candidate — there is nothing to roll back, because
//! nothing global ever changed.
//!
//! [`commit_scene`] is where the candidate becomes real. It takes the
//! scene write lock, compares the candidate's base revision against the
//! live one, retires transient previews for resources crossing the
//! mutation boundary, installs the candidate, admits the snapshot bytes,
//! and releases the lock. Only then
//! does it persist and publish. `Err` therefore means one thing and one
//! thing only: the mutation was rejected *before* admission. Everything
//! that can happen after admission is a [`CommitDurability`] on the
//! returned [`SceneCommit`], because after admission the retry
//! supervisor owns the bytes and the mutation is going to land.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};
use hypercolor_core::bus::{HypercolorBus, TimestampedEvent};
use hypercolor_core::scene::{
    LayerMutationError, OutputPlacement, SceneManager, ScenePlanSnapshot, ZoneMetaPatch,
    ZoneMutationError, default_primary_group,
};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::api::scene::SideEffectOutcome;
use hypercolor_types::api::scenes::{
    ReplaceSceneLayerRequest, ReplaceSceneRequest, ReplaceZoneRequest, SceneLayoutActivationOutcome,
};
use hypercolor_types::asset::AssetId;
use hypercolor_types::config::MediaConfig;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{ControlBinding, EffectMetadata};
use hypercolor_types::event::{
    ChangeTrigger, EffectRef, EffectStopReason, HypercolorEvent, LayerStackChangeKind,
    SceneChangeReason, SceneLibraryChangeKind, SceneSettingsChangeKind, Severity, ZoneChangeKind,
};
use hypercolor_types::layer::{BlendMode, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{
    ColorInterpolation, DisplayFaceTarget, EasingFunction, Scene, SceneId, SceneKind,
    SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, Zone, ZoneId,
};
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use crate::domain::commit::SceneCommitSequencer;
use crate::domain::commit::{CommitDurability, SceneCommit, SceneRevision};
use crate::domain::context::SceneContext;
use crate::domain::effect::IdentityMigrationPersistence;
use crate::domain::layout::LayoutContext;
use crate::domain::output::OutputContext;
use crate::domain::spatial::SpatialService;
use crate::domain::{DomainError, ResourceKind};
use crate::persistence::AtomicWriteOutcome;
use crate::scene_store::{AdmittedSceneStoreSave, SceneStore, SceneStoreSave};
use crate::scene_transactions::{LayoutTransactionRejection, LayoutUpdateGuard};
use crate::zone_layout_preview::ZoneLayoutPreviewStore;

// ── Owning service ───────────────────────────────────────────────────────

/// Cloneable authority for scene state, commit order, and scene events.
#[derive(Clone)]
pub struct SceneService(Arc<SceneServiceInner>);

pub(crate) struct SceneEffectIdMigration {
    base_revision: SceneRevision,
    candidate: SceneManager,
    store: Option<SceneStoreSave>,
    migrated: usize,
    snapshot_save_guard: OwnedRwLockWriteGuard<()>,
}

pub(crate) struct AdmittedSceneEffectIdMigration {
    base_revision: SceneRevision,
    candidate: SceneManager,
    store: Option<AdmittedSceneStoreSave>,
    snapshot_save_guard: OwnedRwLockWriteGuard<()>,
}

pub(crate) struct PersistedSceneEffectIdMigration {
    base_revision: SceneRevision,
    candidate: SceneManager,
    stored_scenes: Option<HashMap<SceneId, Scene>>,
    snapshot_save_guard: OwnedRwLockWriteGuard<()>,
}

pub(crate) struct SceneEffectIdMigrationPublication {
    manager: OwnedRwLockWriteGuard<SceneManager>,
    store: Option<OwnedRwLockWriteGuard<SceneStore>>,
    candidate: Option<SceneManager>,
    stored_scenes: Option<HashMap<SceneId, Scene>>,
    _snapshot_save_guard: OwnedRwLockWriteGuard<()>,
}

struct SceneServiceInner {
    manager: Arc<RwLock<SceneManager>>,
    store: Option<Arc<RwLock<SceneStore>>>,
    _temporary_store_root: Option<tempfile::TempDir>,
    snapshot_save_gate: Arc<RwLock<()>>,
    zone_layout_previews: Arc<ZoneLayoutPreviewStore>,
    commits: Arc<SceneCommitSequencer>,
    event_bus: Arc<HypercolorBus>,
    plan: ArcSwap<ScenePlanSnapshot>,
    #[cfg(all(test, feature = "persistence-test-hooks"))]
    persistence_test_barrier: std::sync::Mutex<Option<Arc<ScenePersistenceTestBarrier>>>,
}

#[cfg(all(test, feature = "persistence-test-hooks"))]
pub(crate) struct ScenePersistenceTestBarrier {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

/// Lock-free render-side access to the latest admitted scene plan.
#[derive(Clone)]
pub struct ScenePlanReader(Arc<SceneServiceInner>);

/// Named scene library and activation authority shared by every transport.
#[derive(Clone)]
pub struct SceneLibraryContext {
    scene: SceneContext,
    effects: crate::domain::effect::EffectContext,
    layout: LayoutContext,
    output: OutputContext,
    event_bus: Arc<HypercolorBus>,
}

impl SceneLibraryContext {
    pub(crate) fn new(
        scene: SceneContext,
        effects: crate::domain::effect::EffectContext,
        layout: LayoutContext,
        output: OutputContext,
        event_bus: Arc<HypercolorBus>,
    ) -> Self {
        Self {
            scene,
            effects,
            layout,
            output,
            event_bus,
        }
    }
}

impl ScenePlanReader {
    /// Borrow the latest admitted scene plan without cloning its `Arc`.
    #[must_use]
    pub fn load(&self) -> Guard<Arc<ScenePlanSnapshot>> {
        self.0.plan.load()
    }
}

impl SceneService {
    /// Own a non-durable scene manager for isolated consumers.
    #[must_use]
    pub fn in_memory(manager: SceneManager, event_bus: Arc<HypercolorBus>) -> Self {
        Self::build(
            manager,
            event_bus,
            None,
            None,
            Arc::new(ZoneLayoutPreviewStore::default()),
        )
    }

    /// Own a temporary scene store for isolated persistence harnesses.
    ///
    /// # Errors
    ///
    /// Returns an error when the temporary store cannot be prepared.
    #[doc(hidden)]
    pub fn with_temporary_store(
        manager: SceneManager,
        event_bus: Arc<HypercolorBus>,
    ) -> anyhow::Result<Self> {
        let (store, root) = SceneStore::temporary()?;
        Ok(Self::build(
            manager,
            event_bus,
            Some(Arc::new(tokio::sync::RwLock::new(store))),
            Some(root),
            Arc::new(ZoneLayoutPreviewStore::default()),
        ))
    }

    /// Own a scene manager together with its durable and transient stores.
    #[must_use]
    pub(crate) fn new(
        manager: SceneManager,
        event_bus: Arc<HypercolorBus>,
        store: SceneStore,
        zone_layout_previews: Arc<ZoneLayoutPreviewStore>,
    ) -> Self {
        Self::build(
            manager,
            event_bus,
            Some(Arc::new(tokio::sync::RwLock::new(store))),
            None,
            zone_layout_previews,
        )
    }

    fn build(
        manager: SceneManager,
        event_bus: Arc<HypercolorBus>,
        store: Option<Arc<tokio::sync::RwLock<SceneStore>>>,
        temporary_store_root: Option<tempfile::TempDir>,
        zone_layout_previews: Arc<ZoneLayoutPreviewStore>,
    ) -> Self {
        let commits = Arc::new(SceneCommitSequencer::new());
        let plan = ArcSwap::from_pointee(manager.plan_snapshot(commits.revision()));
        Self(Arc::new(SceneServiceInner {
            manager: Arc::new(RwLock::new(manager)),
            store,
            _temporary_store_root: temporary_store_root,
            snapshot_save_gate: Arc::new(RwLock::new(())),
            zone_layout_previews,
            commits,
            event_bus,
            plan,
            #[cfg(all(test, feature = "persistence-test-hooks"))]
            persistence_test_barrier: std::sync::Mutex::new(None),
        }))
    }

    /// Capture an owned scene-manager snapshot under one brief read lock.
    pub async fn snapshot(&self) -> SceneManager {
        self.0.manager.read().await.clone()
    }

    /// Return the current admitted scene revision.
    #[must_use]
    pub fn revision(&self) -> SceneRevision {
        self.0.commits.revision()
    }

    /// Create a lock-free reader for the render thread.
    #[must_use]
    pub fn plan_reader(&self) -> ScenePlanReader {
        ScenePlanReader(Arc::clone(&self.0))
    }

    /// Observe published events without gaining access to the event sink.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<TimestampedEvent> {
        self.0.event_bus.subscribe_all()
    }

    /// Snapshot the live scene state into an owned candidate.
    pub async fn begin_mutation(&self) -> SceneMutation {
        let manager = self.0.manager.read().await;
        SceneMutation {
            candidate: manager.clone(),
            base_revision: self.0.commits.revision(),
            events: Vec::new(),
            persists_scene_content: false,
            preview_scenes_to_clear: HashSet::new(),
            preview_zones_to_clear: HashSet::new(),
        }
    }

    pub(crate) async fn prepare_effect_id_migration(
        &self,
        migrations: &HashMap<
            hypercolor_types::effect::EffectId,
            hypercolor_types::effect::EffectId,
        >,
    ) -> Result<SceneEffectIdMigration, DomainError> {
        let snapshot_save_guard = Arc::clone(&self.0.snapshot_save_gate).write_owned().await;
        let manager = self.0.manager.read().await;
        let mut candidate = manager.clone();
        let migrated = candidate.remap_effect_ids(migrations);
        let store = match self.0.store.as_ref() {
            Some(store) => Some(
                store
                    .read()
                    .await
                    .reserve_save(candidate.list().into_iter().cloned())
                    .map_err(|error| DomainError::Internal(error.into()))?,
            ),
            None => None,
        };
        Ok(SceneEffectIdMigration {
            base_revision: self.0.commits.revision(),
            candidate,
            store,
            migrated,
            snapshot_save_guard,
        })
    }

    pub(crate) async fn prepare_effect_id_migration_publication(
        &self,
        migration: PersistedSceneEffectIdMigration,
    ) -> Result<SceneEffectIdMigrationPublication, DomainError> {
        let manager = Arc::clone(&self.0.manager).write_owned().await;
        let current_revision = self.0.commits.revision();
        if current_revision != migration.base_revision {
            return Err(DomainError::conflict_details(
                "effect ID migration was superseded by newer scene state",
                serde_json::json!({
                    "kind": "effect_id_migration_superseded",
                    "expected_revision": migration.base_revision,
                    "current_revision": current_revision,
                }),
            ));
        }

        let store = if migration.stored_scenes.is_some() {
            let store = self
                .0
                .store
                .as_ref()
                .expect("persisted scene migration must retain its owning store");
            Some(Arc::clone(store).write_owned().await)
        } else {
            None
        };

        let PersistedSceneEffectIdMigration {
            base_revision: _,
            candidate,
            stored_scenes,
            snapshot_save_guard,
        } = migration;
        Ok(SceneEffectIdMigrationPublication {
            manager,
            store,
            candidate: Some(candidate),
            stored_scenes,
            _snapshot_save_guard: snapshot_save_guard,
        })
    }

    pub(crate) fn publish_effect_id_migration(
        &self,
        publication: &mut SceneEffectIdMigrationPublication,
    ) -> SceneCommit {
        let SceneEffectIdMigrationPublication {
            manager,
            store,
            candidate,
            stored_scenes,
            _snapshot_save_guard: _,
        } = publication;

        if let (Some(store), Some(stored_scenes)) = (store.as_mut(), stored_scenes.take()) {
            store.replace_named_scenes(stored_scenes.into_values());
        }

        **manager = candidate
            .take()
            .expect("scene migration publication must publish exactly once");
        let ticket = self.0.commits.admit(Arc::clone(&self.0.event_bus));
        self.0
            .plan
            .store(Arc::new(manager.plan_snapshot(ticket.generation())));
        let generation = ticket.generation();
        ticket.release(Vec::new());
        SceneCommit::new(generation, generation, CommitDurability::Written, None)
    }

    pub(crate) async fn stage_zone_layout_preview<E, F>(
        &self,
        owner: crate::zone_layout_preview::ZoneLayoutPreviewOwner,
        zone_id: ZoneId,
        validate: F,
    ) -> Result<Option<SceneId>, E>
    where
        F: FnOnce(&Scene) -> Result<SpatialLayout, E>,
    {
        let manager = self.0.manager.read().await;
        let Some(scene) = manager.active_scene() else {
            return Ok(None);
        };
        let scene_id = scene.id;
        let layout = validate(scene)?;
        self.0
            .zone_layout_previews
            .set(owner, scene_id, zone_id, layout)
            .await;
        Ok(Some(scene_id))
    }

    #[cfg(test)]
    pub(crate) fn scene_write_is_blocked_for_test(&self) -> bool {
        self.0.manager.try_write().is_err()
    }

    #[cfg(all(test, feature = "persistence-test-hooks"))]
    pub(crate) fn pause_next_persistence_for_test(&self) -> Arc<ScenePersistenceTestBarrier> {
        let barrier = Arc::new(ScenePersistenceTestBarrier {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        *self
            .0
            .persistence_test_barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&barrier));
        barrier
    }

    /// Admit an owned candidate through persistence and ordered publication.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Conflict`] when the candidate revision is
    /// stale, or [`DomainError::Internal`] when persistence cannot be
    /// reserved before admission.
    pub async fn commit_mutation(
        &self,
        mutation: SceneMutation,
    ) -> Result<SceneCommit, DomainError> {
        let SceneMutation {
            candidate,
            base_revision,
            events,
            persists_scene_content,
            preview_scenes_to_clear,
            preview_zones_to_clear,
        } = mutation;

        let (ticket, pending) = {
            let mut manager = self.0.manager.write().await;
            let current_revision = self.0.commits.revision();
            if current_revision != base_revision {
                return Err(DomainError::conflict_details(
                    format!(
                        "Scene state changed while applying this request; current revision is {current_revision}"
                    ),
                    serde_json::json!({
                        "kind": "scene_commit_superseded",
                        "expected_revision": base_revision,
                        "current_revision": current_revision,
                    }),
                ));
            }

            let pending = if persists_scene_content {
                let Some(store) = self.0.store.as_ref() else {
                    return Err(DomainError::Internal(anyhow::anyhow!(
                        "durable scene mutation has no owning scene store"
                    )));
                };
                match store
                    .read()
                    .await
                    .reserve_save(candidate.list().into_iter().cloned())
                {
                    Ok(pending) => Some(pending),
                    Err(error) => {
                        return Err(DomainError::Internal(anyhow::anyhow!(
                            "Failed to persist scene: {error}"
                        )));
                    }
                }
            } else {
                None
            };

            self.0
                .zone_layout_previews
                .clear_at_scene_commit(
                    &preview_scenes_to_clear.into_iter().collect::<Vec<_>>(),
                    &preview_zones_to_clear.into_iter().collect::<Vec<_>>(),
                )
                .await;
            *manager = candidate;
            let ticket = self.0.commits.admit(Arc::clone(&self.0.event_bus));
            self.0
                .plan
                .store(Arc::new(manager.plan_snapshot(ticket.generation())));
            (ticket, pending)
        };

        let generation = ticket.generation();
        let Some(pending) = pending else {
            ticket.release(events);
            return Ok(SceneCommit::new(
                generation,
                generation,
                CommitDurability::Written,
                None,
            ));
        };

        let store = self
            .0
            .store
            .as_ref()
            .expect("persistent scene commit must retain its owning store");
        #[cfg(all(test, feature = "persistence-test-hooks"))]
        self.pause_before_persistence_for_test().await;
        let outcome = store.write().await.save_reserved(pending);
        match outcome {
            Ok(AtomicWriteOutcome::Written) => {
                ticket.release(events);
                Ok(SceneCommit::new(
                    generation,
                    generation,
                    CommitDurability::Written,
                    None,
                ))
            }
            Ok(AtomicWriteOutcome::Superseded) => {
                ticket.discard();
                Ok(SceneCommit::new(
                    generation,
                    generation,
                    CommitDurability::Superseded,
                    None,
                ))
            }
            Err(error) => {
                ticket.discard();
                Ok(SceneCommit::new(
                    generation,
                    generation,
                    CommitDurability::Retrying,
                    Some(error.to_string()),
                ))
            }
        }
    }

    #[cfg(all(test, feature = "persistence-test-hooks"))]
    async fn pause_before_persistence_for_test(&self) {
        let barrier = self
            .0
            .persistence_test_barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(barrier) = barrier {
            barrier.entered.notify_one();
            barrier.release.notified().await;
        }
    }

    /// Persist the current named-scene projection through the owning store.
    pub async fn save_snapshot(&self) -> anyhow::Result<()> {
        self.persist_snapshot().await.map(|_| ())
    }

    pub(crate) async fn persist_snapshot(&self) -> anyhow::Result<Option<AtomicWriteOutcome>> {
        let Some(store) = self.0.store.as_ref() else {
            return Ok(None);
        };
        #[cfg(all(test, feature = "persistence-test-hooks"))]
        self.pause_before_persistence_for_test().await;
        let _snapshot_save_guard = Arc::clone(&self.0.snapshot_save_gate).read_owned().await;
        let pending = {
            let manager = self.snapshot().await;
            store
                .read()
                .await
                .reserve_save(manager.list().into_iter().cloned())?
        };
        store.write().await.save_reserved(pending).map(Some)
    }

    pub(crate) async fn publish_layout_activation<F>(
        &self,
        spatial_engine: &SpatialService,
        candidate_spatial_engine: SpatialEngine,
        expected_layout: &SpatialLayout,
        expected_active_scene_id: Option<SceneId>,
        expected_active_zones_revision: u64,
        publish_renderer_state: F,
    ) -> Result<(), LayoutTransactionRejection>
    where
        F: FnOnce(SpatialEngine) -> Result<(), LayoutTransactionRejection>,
    {
        let mut manager = self.0.manager.write().await;
        let source_is_current = manager.active_scene_id().copied() == expected_active_scene_id
            && manager.active_render_groups_revision() == expected_active_zones_revision
            && spatial_engine.has_layout(expected_layout);
        if !source_is_current {
            return Err(LayoutTransactionRejection::Superseded);
        }

        publish_renderer_state(candidate_spatial_engine.clone())?;
        manager.sync_primary_group_layout(candidate_spatial_engine.layout().as_ref());
        let ticket = self.0.commits.admit(Arc::clone(&self.0.event_bus));
        self.0
            .plan
            .store(Arc::new(manager.plan_snapshot(ticket.generation())));
        spatial_engine.replace(candidate_spatial_engine);
        ticket.release(Vec::new());
        Ok(())
    }
}

#[cfg(all(test, feature = "persistence-test-hooks"))]
impl ScenePersistenceTestBarrier {
    pub(crate) async fn wait_until_entered(&self) {
        self.entered.notified().await;
    }

    pub(crate) fn release(&self) {
        self.release.notify_one();
    }
}

impl SceneEffectIdMigration {
    pub(crate) fn candidate(&self) -> &SceneManager {
        &self.candidate
    }

    pub(crate) fn migrated(&self) -> usize {
        self.migrated
    }

    pub(crate) fn admit(self) -> AdmittedSceneEffectIdMigration {
        AdmittedSceneEffectIdMigration {
            base_revision: self.base_revision,
            candidate: self.candidate,
            store: self.store.map(SceneStoreSave::admit),
            snapshot_save_guard: self.snapshot_save_guard,
        }
    }
}

impl AdmittedSceneEffectIdMigration {
    pub(crate) fn persist(
        self,
    ) -> (
        PersistedSceneEffectIdMigration,
        IdentityMigrationPersistence,
    ) {
        let Self {
            base_revision,
            candidate,
            store,
            snapshot_save_guard,
        } = self;
        let (stored_scenes, persistence) = store.map_or_else(
            || (None, IdentityMigrationPersistence::Written),
            |store| {
                let (scenes, outcome) = store.commit_stage_aware();
                let persistence = match outcome {
                    crate::persistence::AtomicWriteCommitResult::DurableWritten => {
                        IdentityMigrationPersistence::Written
                    }
                    crate::persistence::AtomicWriteCommitResult::Superseded => {
                        IdentityMigrationPersistence::Superseded
                    }
                    crate::persistence::AtomicWriteCommitResult::FailedBeforeReplacement(error)
                    | crate::persistence::AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                        IdentityMigrationPersistence::Retrying(error.to_string())
                    }
                };
                (Some(scenes), persistence)
            },
        );
        (
            PersistedSceneEffectIdMigration {
                base_revision,
                candidate,
                stored_scenes,
                snapshot_save_guard,
            },
            persistence,
        )
    }
}

// ── Owned candidate ──────────────────────────────────────────────────────

/// An owned candidate scene state, its base revision, and the events its
/// intent methods recorded.
///
/// Nothing here is shared. The candidate is a full [`SceneManager`]
/// clone, so intent methods are ordinary `&mut self` calls with no
/// locking, and abandoning the mutation costs a drop.
#[derive(Debug)]
pub struct SceneMutation {
    candidate: SceneManager,
    base_revision: SceneRevision,
    events: Vec<HypercolorEvent>,
    persists_scene_content: bool,
    preview_scenes_to_clear: HashSet<SceneId>,
    preview_zones_to_clear: HashSet<(SceneId, ZoneId)>,
}

impl SceneMutation {
    fn record_zone_change(&mut self, scene_id: SceneId, zone: &Zone, kind: ZoneChangeKind) {
        self.events.retain(|event| {
            !matches!(
                event,
                HypercolorEvent::ZoneChanged {
                    scene_id: existing_scene_id,
                    zone_id,
                    ..
                } if *existing_scene_id == scene_id && *zone_id == zone.id
            )
        });
        self.events.push(zone_changed_event(scene_id, zone, kind));
    }

    fn record_layer_change(&mut self, scene_id: SceneId, zone: &Zone, kind: LayerStackChangeKind) {
        let zone_kind = if kind == LayerStackChangeKind::ControlsPatched {
            ZoneChangeKind::ControlsPatched
        } else {
            ZoneChangeKind::Updated
        };
        self.record_zone_change(scene_id, zone, zone_kind);
        self.record_layer_stack_event(scene_id, zone, kind);
    }

    fn record_layer_stack_event(
        &mut self,
        scene_id: SceneId,
        zone: &Zone,
        kind: LayerStackChangeKind,
    ) {
        self.events.retain(|event| {
            !matches!(
                event,
                HypercolorEvent::LayerStackChanged {
                    scene_id: existing_scene_id,
                    zone_id,
                    ..
                } if *existing_scene_id == scene_id && *zone_id == zone.id
            )
        });
        self.events.push(HypercolorEvent::LayerStackChanged {
            scene_id,
            zone_id: zone.id,
            revision: self.base_revision.saturating_add(1),
            kind,
        });
    }

    /// The revision this candidate was snapshotted from.
    #[must_use]
    pub const fn base_revision(&self) -> SceneRevision {
        self.base_revision
    }

    /// Read the candidate. Every intent method's effect is visible here
    /// immediately, so callers compose reads and writes freely.
    #[must_use]
    pub const fn scenes(&self) -> &SceneManager {
        &self.candidate
    }

    pub fn retire_scene_previews(&mut self, scene_id: SceneId) {
        self.preview_scenes_to_clear.insert(scene_id);
    }

    pub fn retire_zone_preview(&mut self, scene_id: SceneId, zone_id: ZoneId) {
        self.preview_zones_to_clear.insert((scene_id, zone_id));
    }

    /// The active scene's id, refusing scenes that forbid runtime
    /// rewriting.
    ///
    /// Snapshot scenes are a deliberate user choice: runtime effect and
    /// face actions must not silently edit them.
    pub fn active_scene_for_runtime_mutation(&self, action: &str) -> Result<SceneId, DomainError> {
        active_scene_for_runtime_mutation(&self.candidate, action)
    }

    /// The active scene's primary zone id, when it has one.
    #[must_use]
    pub fn primary_zone_id(&self) -> Option<ZoneId> {
        self.candidate
            .active_scene()
            .and_then(Scene::primary_zone)
            .map(|zone| zone.id)
    }

    /// The effect currently loaded in one of the active scene's zones.
    #[must_use]
    pub fn zone_effect(&self, zone_id: ZoneId) -> Option<hypercolor_types::effect::EffectId> {
        self.candidate
            .active_scene()?
            .zones
            .iter()
            .find(|zone| zone.id == zone_id)
            .and_then(|zone| zone.effect_ids().next())
    }

    /// Load an effect into the active scene's primary zone, creating the
    /// zone when the scene has none.
    pub fn upsert_primary_zone(
        &mut self,
        metadata: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        preset_id: Option<PresetId>,
        layout: SpatialLayout,
        trigger: ChangeTrigger,
        previous: Option<EffectRef>,
    ) -> Result<Zone, DomainError> {
        let scene_id = self
            .candidate
            .active_scene_id()
            .copied()
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))?;
        let kind = if self.primary_zone_id().is_some() {
            ZoneChangeKind::Updated
        } else {
            ZoneChangeKind::Created
        };
        let zone = self
            .candidate
            .upsert_primary_group(metadata, controls, preset_id, layout)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!(
                    "Failed to update active scene primary group: {error}"
                ))
            })?
            .clone();
        self.persists_scene_content = true;
        self.record_effect_started(metadata, &zone, trigger, previous);
        self.record_zone_change(scene_id, &zone, kind);
        Ok(zone)
    }

    /// Load an effect into a named zone, which keeps its own layout.
    pub fn apply_effect_to_zone(
        &mut self,
        zone_id: ZoneId,
        metadata: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        preset_id: Option<PresetId>,
        trigger: ChangeTrigger,
        previous: Option<EffectRef>,
    ) -> Result<Zone, DomainError> {
        let scene_id = self
            .candidate
            .active_scene_id()
            .copied()
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))?;
        let zone = self
            .candidate
            .apply_effect_to_group(zone_id, metadata, controls, preset_id)
            .map_err(|error| {
                DomainError::validation(format!("Failed to apply effect to zone: {error}"))
            })?
            .clone();
        self.persists_scene_content = true;
        self.record_effect_started(metadata, &zone, trigger, previous);
        self.record_zone_change(scene_id, &zone, ZoneChangeKind::Updated);
        Ok(zone)
    }

    fn record_effect_started(
        &mut self,
        metadata: &EffectMetadata,
        zone: &Zone,
        trigger: ChangeTrigger,
        previous: Option<EffectRef>,
    ) {
        self.events.push(HypercolorEvent::EffectStarted {
            effect: EffectRef {
                id: metadata.id.to_string(),
                name: metadata.name.clone(),
                engine: "servo".to_owned(),
            },
            trigger,
            previous,
            transition: None,
            zone_id: Some(zone.id),
            zone_name: Some(zone.name.clone()),
        });
    }

    /// Make a scene the exclusive current one.
    ///
    /// Activation moves the priority stack and the transition state,
    /// neither of which is persisted scene content, so this intent does
    /// not arm a scene-store write.
    pub fn activate(
        &mut self,
        scene_id: SceneId,
        transition: Option<TransitionSpec>,
        reason: SceneChangeReason,
    ) -> Result<(), DomainError> {
        let previous_scene_id = self.candidate.active_scene_id().copied();
        self.candidate
            .activate(&scene_id, transition)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!("Failed to activate scene: {error}"))
            })?;
        if previous_scene_id != Some(scene_id) {
            if let Some(previous_scene_id) = previous_scene_id {
                self.retire_scene_previews(previous_scene_id);
            }
            if let Some(current) = self.candidate.active_scene() {
                self.events.push(active_scene_changed_event(
                    previous_scene_id,
                    current,
                    reason,
                ));
            }
        }
        Ok(())
    }

    /// Return to the synthesized default scene.
    ///
    /// Like [`Self::activate`], this moves only the priority stack.
    pub fn deactivate_current(&mut self, reason: SceneChangeReason) {
        let previous_scene = self.candidate.active_scene().cloned();
        self.candidate.deactivate_current();
        let current_scene = self.candidate.active_scene().cloned();
        if previous_scene.as_ref().map(|scene| scene.id)
            != current_scene.as_ref().map(|scene| scene.id)
            && let Some(current) = current_scene.as_ref()
        {
            if let Some(previous) = previous_scene.as_ref() {
                self.retire_scene_previews(previous.id);
            }
            self.events.push(active_scene_changed_event(
                previous_scene.as_ref().map(|scene| scene.id),
                current,
                reason,
            ));
        }
    }

    /// Align the active primary zone with a newly authoritative layout.
    pub fn sync_primary_layout(&mut self, layout: &SpatialLayout) {
        self.candidate.sync_primary_group_layout(layout);
    }

    /// Restore a persisted scene without scheduling a redundant store write.
    pub fn restore_scene(&mut self, scene: Scene) -> Result<(), DomainError> {
        self.candidate.update(scene).map_err(|error| {
            DomainError::Internal(anyhow::anyhow!("Failed to restore scene: {error}"))
        })
    }

    // ── Scene library ────────────────────────────────────────────────

    /// Add a scene to the library.
    pub fn create_scene(&mut self, scene: Scene) -> Result<(), DomainError> {
        let event = HypercolorEvent::SceneLibraryChanged {
            scene_id: scene.id,
            kind: SceneLibraryChangeKind::Created,
            name: Some(scene.name.clone()),
        };
        self.candidate
            .create(scene)
            .map_err(|error| DomainError::conflict(format!("Failed to create scene: {error}")))?;
        self.persists_scene_content = true;
        self.events.push(event);
        Ok(())
    }

    /// Replace a scene's stored definition.
    pub fn update_scene(&mut self, scene: Scene) -> Result<(), DomainError> {
        let event = HypercolorEvent::SceneLibraryChanged {
            scene_id: scene.id,
            kind: SceneLibraryChangeKind::Updated,
            name: Some(scene.name.clone()),
        };
        self.candidate.update(scene).map_err(|error| {
            DomainError::Internal(anyhow::anyhow!("Failed to update scene: {error}"))
        })?;
        self.persists_scene_content = true;
        self.events.push(event);
        Ok(())
    }

    /// Remove a scene from the library.
    pub fn delete_scene(&mut self, scene_id: &SceneId) -> Result<Scene, DomainError> {
        let previous_scene_id = self.candidate.active_scene_id().copied();
        let scene = self
            .candidate
            .delete(scene_id)
            .map_err(|error| DomainError::not_found(ResourceKind::Scene, error))?;
        self.persists_scene_content = true;
        let current_scene = self.candidate.active_scene().cloned();
        if previous_scene_id != current_scene.as_ref().map(|current| current.id)
            && let Some(current) = current_scene.as_ref()
        {
            self.events.push(active_scene_changed_event(
                previous_scene_id,
                current,
                SceneChangeReason::UserDeactivate,
            ));
        }
        self.events.push(HypercolorEvent::SceneLibraryChanged {
            scene_id: *scene_id,
            kind: SceneLibraryChangeKind::Deleted,
            name: None,
        });
        Ok(scene)
    }

    // ── Zones ────────────────────────────────────────────────────────

    /// Add a custom zone to a scene.
    pub fn create_zone(
        &mut self,
        scene_id: SceneId,
        name: String,
        color: Option<String>,
        fallback_canvas: (u32, u32),
    ) -> Result<ZoneId, ZoneMutationError> {
        let zone_id =
            self.candidate
                .create_render_group(&scene_id, name, color, fallback_canvas)?;
        self.persists_scene_content = true;
        if let Some(zone) = self
            .candidate
            .get(&scene_id)
            .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
        {
            self.events
                .push(zone_changed_event(scene_id, zone, ZoneChangeKind::Created));
        }
        Ok(zone_id)
    }

    /// Patch a zone's presentation metadata.
    pub fn update_zone_meta(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        patch: ZoneMetaPatch,
    ) -> Result<Zone, ZoneMutationError> {
        let zone = self
            .candidate
            .update_render_group_meta(&scene_id, zone_id, patch)?;
        self.persists_scene_content = true;
        self.events
            .push(zone_changed_event(scene_id, &zone, ZoneChangeKind::Updated));
        Ok(zone)
    }

    /// Remove a custom zone from a scene.
    pub fn delete_zone(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
    ) -> Result<(), ZoneMutationError> {
        let removed = self
            .candidate
            .get(&scene_id)
            .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
            .cloned();
        self.candidate.delete_render_group(&scene_id, zone_id)?;
        self.persists_scene_content = true;
        if let Some(zone) = removed {
            self.events
                .push(zone_changed_event(scene_id, &zone, ZoneChangeKind::Removed));
        }
        Ok(())
    }

    /// Move one output into a zone.
    pub fn assign_output(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        output: Output,
        placement: OutputPlacement,
    ) -> Result<(), ZoneMutationError> {
        self.candidate
            .assign_device_zone(&scene_id, zone_id, output, placement)?;
        self.persists_scene_content = true;
        if let Some(zone) = self
            .candidate
            .get(&scene_id)
            .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
            .cloned()
        {
            self.record_zone_change(scene_id, &zone, ZoneChangeKind::Updated);
        }
        Ok(())
    }

    /// Drop one output out of whatever zone holds it.
    pub fn unassign_output(
        &mut self,
        scene_id: SceneId,
        output_id: &str,
    ) -> Result<(), ZoneMutationError> {
        let zone_id = self.candidate.get(&scene_id).and_then(|scene| {
            scene
                .zones
                .iter()
                .find(|zone| {
                    zone.layout
                        .zones
                        .iter()
                        .any(|output| output.id == output_id)
                })
                .map(|zone| zone.id)
        });
        self.candidate.unassign_device_zone(&scene_id, output_id)?;
        self.persists_scene_content = true;
        if let Some(zone) = zone_id.and_then(|zone_id| {
            self.candidate
                .get(&scene_id)
                .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
                .cloned()
        }) {
            self.record_zone_change(scene_id, &zone, ZoneChangeKind::Updated);
        }
        Ok(())
    }

    /// Reposition a zone's outputs without changing which it owns.
    pub fn set_zone_layout(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layout: SpatialLayout,
    ) -> Result<Zone, ZoneMutationError> {
        let zone = self
            .candidate
            .update_zone_layout(&scene_id, zone_id, layout)?;
        self.persists_scene_content = true;
        self.record_zone_change(scene_id, &zone, ZoneChangeKind::Updated);
        Ok(zone)
    }

    /// Choose what a scene does with outputs no zone claims.
    pub fn set_unassigned_behavior(
        &mut self,
        scene_id: SceneId,
        behavior: UnassignedBehavior,
    ) -> Result<UnassignedBehavior, ZoneMutationError> {
        let behavior = self
            .candidate
            .set_unassigned_behavior(&scene_id, behavior)?;
        self.persists_scene_content = true;
        self.events.push(HypercolorEvent::SceneSettingsChanged {
            scene_id,
            revision: self.base_revision.saturating_add(1),
            kind: SceneSettingsChangeKind::UnassignedBehavior,
        });
        Ok(behavior)
    }

    // ── Effect slots and controls ────────────────────────────────────

    /// Unload whatever effect a zone runs.
    pub fn clear_zone_effect(
        &mut self,
        zone_id: ZoneId,
        stopped_effect: Option<EffectRef>,
        reason: EffectStopReason,
    ) -> Option<Zone> {
        let scene_id = self.candidate.active_scene_id().copied()?;
        let zone = self.candidate.clear_group_effect(zone_id).cloned()?;
        self.persists_scene_content = true;
        if let Some(effect) = stopped_effect {
            self.events.push(HypercolorEvent::EffectStopped {
                effect,
                reason,
                zone_id: Some(zone.id),
                zone_name: Some(zone.name.clone()),
            });
        }
        self.record_zone_change(scene_id, &zone, ZoneChangeKind::Updated);
        Some(zone)
    }

    /// Merge control overrides into a zone with no effect precondition.
    pub fn patch_zone_controls(
        &mut self,
        zone_id: ZoneId,
        updates: HashMap<String, ControlValue>,
    ) -> Option<Zone> {
        let scene_id = self.candidate.active_scene_id().copied()?;
        let zone = self
            .candidate
            .patch_group_controls(zone_id, updates)?
            .clone();
        self.persists_scene_content = true;
        self.record_zone_change(scene_id, &zone, ZoneChangeKind::ControlsPatched);
        Some(zone)
    }

    /// Attach one live control binding to an effect zone.
    pub fn set_zone_control_binding(
        &mut self,
        zone_id: ZoneId,
        control_id: String,
        binding: ControlBinding,
    ) -> Option<Zone> {
        let scene_id = self.candidate.active_scene_id().copied()?;
        let zone = self
            .candidate
            .set_group_control_binding(zone_id, control_id, binding)?
            .clone();
        self.persists_scene_content = true;
        self.record_zone_change(scene_id, &zone, ZoneChangeKind::ControlsPatched);
        Some(zone)
    }

    /// Force the active scene's resolved zones to be recomputed.
    ///
    /// The resolved zones are derived state, so this bumps the
    /// render-group revision without touching persisted scene content.
    pub fn invalidate_active_zones(&mut self) {
        self.candidate.invalidate_active_render_groups();
    }

    // ── Layer stacks ─────────────────────────────────────────────────

    /// Insert a layer into a zone's stack.
    pub fn insert_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer: SceneLayer,
        index: Option<usize>,
        expected_version: Option<u64>,
    ) -> Result<Zone, LayerMutationError> {
        let (zone, _version) = self.candidate.insert_scene_group_layer(
            scene_id,
            zone_id,
            layer,
            index,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        self.record_layer_change(scene_id, &zone, LayerStackChangeKind::Created);
        Ok(zone)
    }

    /// Drop one layer out of a zone's stack.
    pub fn remove_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        expected_version: Option<u64>,
    ) -> Result<Zone, LayerMutationError> {
        let (zone, _version) = self.candidate.remove_scene_group_layer(
            scene_id,
            zone_id,
            layer_id,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        self.record_layer_change(scene_id, &zone, LayerStackChangeKind::Removed);
        Ok(zone)
    }

    /// Replace one layer in place while publishing one coherent stack change.
    pub fn replace_layer(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        layer: SceneLayer,
        index: usize,
    ) -> Result<Zone, LayerMutationError> {
        self.candidate
            .remove_scene_group_layer(scene_id, zone_id, layer_id, None)?;
        let (zone, _version) =
            self.candidate
                .insert_scene_group_layer(scene_id, zone_id, layer, Some(index), None)?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        self.record_layer_change(scene_id, &zone, LayerStackChangeKind::Updated);
        Ok(zone)
    }

    /// Rewrite a zone's layer order.
    pub fn reorder_layers(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_ids: Vec<SceneLayerId>,
        expected_version: Option<u64>,
    ) -> Result<Zone, LayerMutationError> {
        let (zone, _version) = self.candidate.reorder_scene_group_layers(
            scene_id,
            zone_id,
            layer_ids,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        self.record_layer_change(scene_id, &zone, LayerStackChangeKind::Reordered);
        Ok(zone)
    }

    /// Merge control overrides into one effect layer.
    pub fn patch_layer_controls(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        updates: HashMap<String, ControlValue>,
        expected_version: Option<u64>,
    ) -> Result<Zone, LayerMutationError> {
        let (zone, _version) = self.candidate.patch_scene_layer_effect_controls(
            scene_id,
            zone_id,
            layer_id,
            updates,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        self.record_layer_change(scene_id, &zone, LayerStackChangeKind::ControlsPatched);
        Ok(zone)
    }

    /// Merge control overrides and drop named input bindings in one
    /// mutation (Spec 78 §1.6).
    pub fn patch_layer_controls_and_bindings(
        &mut self,
        scene_id: SceneId,
        zone_id: ZoneId,
        layer_id: SceneLayerId,
        updates: HashMap<String, ControlValue>,
        clear_bindings: &[String],
        expected_version: Option<u64>,
        trigger: ChangeTrigger,
        previous_values: &HashMap<String, ControlValue>,
    ) -> Result<Zone, LayerMutationError> {
        let effect_id = self
            .candidate
            .get(&scene_id)
            .and_then(|scene| scene.zones.iter().find(|zone| zone.id == zone_id))
            .and_then(|zone| zone.layers.iter().find(|layer| layer.id == layer_id))
            .and_then(|layer| match &layer.source {
                LayerSource::Effect { effect_id, .. } => Some(*effect_id),
                _ => None,
            });
        let changed_values = updates.clone();
        let (zone, _version) = self.candidate.patch_scene_layer_controls_and_bindings(
            scene_id,
            zone_id,
            layer_id,
            updates,
            clear_bindings,
            expected_version,
        )?;
        let zone = zone.clone();
        self.persists_scene_content = true;
        self.record_zone_change(scene_id, &zone, ZoneChangeKind::ControlsPatched);
        if let Some(effect_id) = effect_id {
            for (control_id, new_value) in changed_values {
                let Some(old_value) = previous_values.get(&control_id) else {
                    continue;
                };
                if old_value == &new_value {
                    continue;
                }
                self.events.push(HypercolorEvent::EffectControlChanged {
                    effect_id: effect_id.to_string(),
                    control_id,
                    old_value: old_value.clone(),
                    new_value,
                    zone_id,
                    layer_id,
                    trigger: trigger.clone(),
                });
            }
        }
        self.record_layer_stack_event(scene_id, &zone, LayerStackChangeKind::ControlsPatched);
        Ok(zone)
    }

    // ── Display zones ────────────────────────────────────────────────

    /// Assign a face to a display in the active scene, creating the
    /// display zone when the scene has none for that device.
    pub fn upsert_display_zone(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        effect: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        layout: SpatialLayout,
        target: DisplayFaceTarget,
    ) -> Result<Zone, DomainError> {
        let scene_id = self
            .candidate
            .active_scene_id()
            .copied()
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))?;
        let kind = if self
            .candidate
            .active_scene()
            .is_some_and(|scene| scene.display_zone_for(device_id).is_some())
        {
            ZoneChangeKind::Updated
        } else {
            ZoneChangeKind::Created
        };
        let zone_id = self
            .candidate
            .upsert_display_group(device_id, device_name, effect, controls, layout)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!("Failed to update active scene: {error}"))
            })?
            .id;
        let zone = self
            .candidate
            .patch_display_group_target(zone_id, Some(target.blend_mode), Some(target.opacity))
            .ok_or_else(|| {
                DomainError::Internal(anyhow::anyhow!("Failed to update display face composition"))
            })?
            .clone();
        self.persists_scene_content = true;
        self.record_zone_change(scene_id, &zone, kind);
        Ok(zone)
    }

    /// Keep a display zone's surface aligned with the device's geometry.
    ///
    /// Returns whether the zone actually moved, so callers can skip a
    /// commit that would change nothing.
    pub fn ensure_display_surface(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        layout: SpatialLayout,
    ) -> Result<bool, DomainError> {
        let scene_id = self.candidate.active_scene_id().copied();
        let before = self.active_zones_revision();
        self.candidate
            .ensure_display_group_surface(device_id, device_name, layout)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!(
                    "Failed to sync display screen surface: {error}"
                ))
            })?;
        let changed = self.active_zones_revision() != before;
        if changed {
            self.persists_scene_content = true;
            if let (Some(scene_id), Some(zone)) = (
                scene_id,
                self.candidate
                    .active_scene()
                    .and_then(|scene| scene.display_zone_for(device_id))
                    .cloned(),
            ) {
                self.record_zone_change(scene_id, &zone, ZoneChangeKind::Updated);
            }
        }
        Ok(changed)
    }

    /// Refresh geometry for display zones that already belong to one scene.
    pub fn hydrate_existing_display_surfaces(
        &mut self,
        scene_id: SceneId,
        displays: &[(DeviceId, String, SpatialLayout)],
    ) -> Result<bool, DomainError> {
        let mut scene = self
            .candidate
            .get(&scene_id)
            .cloned()
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, scene_id))?;
        let mut changed_zones = Vec::new();
        for (device_id, _, layout) in displays {
            let Some(zone) = scene.display_zone_for_mut(*device_id) else {
                continue;
            };
            if zone.layout != *layout {
                zone.layout.clone_from(layout);
                changed_zones.push(zone.clone());
            }
        }
        if !changed_zones.is_empty() {
            scene.zones_revision = scene.zones_revision.saturating_add(1);
            self.candidate.update(scene).map_err(|error| {
                DomainError::Internal(anyhow::anyhow!("Failed to update scene: {error}"))
            })?;
            self.persists_scene_content = true;
            for zone in &changed_zones {
                self.record_zone_change(scene_id, zone, ZoneChangeKind::Updated);
            }
        }
        Ok(!changed_zones.is_empty())
    }

    /// Update how a display zone's face composes over the effect layer.
    pub fn patch_display_target(
        &mut self,
        zone_id: ZoneId,
        blend_mode: Option<BlendMode>,
        opacity: Option<f32>,
    ) -> Option<Zone> {
        let scene_id = self.candidate.active_scene_id().copied()?;
        let zone = self
            .candidate
            .patch_display_group_target(zone_id, blend_mode, opacity)?
            .clone();
        self.persists_scene_content = true;
        self.record_zone_change(scene_id, &zone, ZoneChangeKind::Updated);
        Some(zone)
    }

    /// Strip the face assignment off a display zone, keeping the zone.
    pub fn clear_display_assignment(
        &mut self,
        device_id: DeviceId,
        device_name: &str,
        layout: SpatialLayout,
    ) -> Result<Zone, DomainError> {
        let scene_id = self
            .candidate
            .active_scene_id()
            .copied()
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, "active"))?;
        let kind = if self
            .candidate
            .active_scene()
            .is_some_and(|scene| scene.display_zone_for(device_id).is_some())
        {
            ZoneChangeKind::Updated
        } else {
            ZoneChangeKind::Created
        };
        let zone = self
            .candidate
            .clear_display_group_assignment(device_id, device_name, layout)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!("Failed to update active scene: {error}"))
            })?
            .clone();
        self.persists_scene_content = true;
        self.record_zone_change(scene_id, &zone, kind);
        Ok(zone)
    }

    /// Drop every scene's display zone for one device.
    pub fn remove_display_zones_for_device(&mut self, device_id: DeviceId) -> Vec<(SceneId, Zone)> {
        let removed = self.candidate.remove_display_groups_for_device(device_id);
        if !removed.is_empty() {
            self.persists_scene_content = true;
            for (scene_id, zone) in &removed {
                self.record_zone_change(*scene_id, zone, ZoneChangeKind::Removed);
            }
        }
        removed
    }

    /// Install the runtime overlay zone a display preference resolves to,
    /// reporting whether it moved anything.
    ///
    /// Default display zones are materialized from the preference store
    /// on every run, so they are runtime state rather than persisted
    /// scene content — and re-materializing the same preference is the
    /// common case, reached from device connects and from several read
    /// paths. Reporting `false` there lets the caller skip a commit that
    /// would mint a scene revision and invalidate every in-flight
    /// candidate for no change at all.
    ///
    /// The comparison normalizes runtime identities because a freshly
    /// built overlay mints them before it can discover the installed
    /// equivalent.
    pub fn set_default_display_zone(&mut self, zone: Zone) -> bool {
        let Some(device_id) = zone.display_target.as_ref().map(|target| target.device_id) else {
            return false;
        };
        let previous = self.candidate.default_display_group_for(device_id).cloned();
        let unchanged = previous.as_ref().is_some_and(|installed| {
            let mut candidate = zone.clone();
            candidate.id = installed.id;
            if candidate.layers.len() == installed.layers.len() {
                for (candidate_layer, installed_layer) in
                    candidate.layers.iter_mut().zip(&installed.layers)
                {
                    candidate_layer.id = installed_layer.id;
                }
            }
            *installed == candidate
        });
        if unchanged {
            return false;
        }
        self.candidate.set_default_display_group(zone);
        if let Some(installed) = self.candidate.default_display_group_for(device_id).cloned() {
            let scene_id = self
                .candidate
                .active_scene_id()
                .copied()
                .unwrap_or(SceneId::DEFAULT);
            let kind = if previous.is_some() {
                ZoneChangeKind::Updated
            } else {
                ZoneChangeKind::Created
            };
            self.record_zone_change(scene_id, &installed, kind);
        }
        true
    }

    /// Remove a display's runtime default overlay zone.
    pub fn remove_default_display_zone(&mut self, device_id: DeviceId) -> Option<Zone> {
        let existing = self.candidate.default_display_group_for(device_id).cloned();
        self.candidate.remove_default_display_group(device_id);
        if let Some(zone) = existing.as_ref() {
            let scene_id = self
                .candidate
                .active_scene_id()
                .copied()
                .unwrap_or(SceneId::DEFAULT);
            self.record_zone_change(scene_id, zone, ZoneChangeKind::Removed);
        }
        existing
    }

    fn active_zones_revision(&self) -> u64 {
        self.candidate
            .active_scene()
            .map_or(0, |scene| scene.zones_revision)
    }
}

// ── Commit ───────────────────────────────────────────────────────────────

/// Install a candidate, admit its snapshot, then persist and publish.
///
/// The compare-and-swap on the base revision refuses a candidate built
/// from a revision that no longer exists, rather than letting it
/// silently overwrite whatever landed in between. That swap is the
/// whole concurrency story: the revision advances in
/// [`SceneCommitSequencer::admit`](crate::domain::commit::SceneCommitSequencer),
/// which this function and the frame-boundary layout transaction call,
/// and every scene mutation the daemon serves joins that admission
/// order.
///
/// # Errors
///
/// Only pre-admission rejections: a stale base revision
/// ([`DomainError::Conflict`]) or a snapshot that will not
/// serialize ([`DomainError::Internal`]). Once the bytes are admitted
/// the mutation is committed, and where they ended up is reported by
/// [`SceneCommit::durability`].
pub async fn commit_scene(
    ctx: &SceneContext,
    mutation: SceneMutation,
) -> Result<SceneCommit, DomainError> {
    ctx.commit(mutation).await
}

/// How many times an idempotent reconciliation rebuilds its candidate
/// before giving up on the compare-and-swap.
pub const COMMIT_ATTEMPTS: usize = 4;

/// Build and commit an idempotent reconciliation, rebuilding the
/// candidate whenever a concurrent commit wins the swap.
///
/// Only reconciliations belong here. A request the user made should
/// surface the conflict so the caller can rebase against current state,
/// but a sweep that recomputes its whole intent from live state has
/// nothing to rebase and nobody to tell. `build` runs against a fresh
/// candidate on every attempt, so it must not carry state forward from a
/// previous one.
///
/// A `build` that returns `None` found nothing to do, and the
/// reconciliation ends there without a commit — which matters, because
/// a commit mints a scene revision and invalidates every in-flight
/// candidate whether or not it changed anything.
///
/// # Errors
///
/// Whatever `build` returns, or the last [`DomainError::Conflict`] when
/// every attempt loses.
pub async fn commit_retrying<T>(
    ctx: &SceneContext,
    build: impl FnMut(&mut SceneMutation) -> Result<Option<T>, DomainError>,
) -> Result<Option<(T, SceneCommit)>, DomainError> {
    ctx.commit_retrying(build).await
}

// ── Scene media admission ────────────────────────────────────────────────

pub(crate) const MEDIA_SOFT_PRODUCER_COST_US: u64 = 60_000;
const LOTTIE_PRODUCER_COST_US: u64 = 8_000;
const VIDEO_PRODUCER_COST_US: u64 = 20_000;
const LIVESTREAM_PRODUCER_COST_US: u64 = 25_000;

/// Structured producer-cap violation details shared by every adapter.
#[derive(Debug)]
pub struct MediaAdmissionViolationDetails {
    pub message: String,
    pub caps: serde_json::Value,
    pub counts: serde_json::Value,
    pub layers: serde_json::Value,
}

#[derive(Debug, Default)]
struct MediaAdmissionCounts {
    video_asset_ids: HashSet<AssetId>,
    livestream_asset_ids: HashSet<AssetId>,
    lottie_asset_ids: HashSet<AssetId>,
    estimated_cost_us: u64,
    video_layers: Vec<serde_json::Value>,
    livestream_layers: Vec<serde_json::Value>,
}

impl MediaAdmissionCounts {
    const fn estimated_cost_us(&self) -> u64 {
        self.estimated_cost_us
    }
}

/// What activating a scene would cost the compositor, and whether it
/// exceeds the hard producer caps.
#[derive(Debug)]
pub struct SceneMediaAdmission {
    /// Estimated per-frame producer cost in microseconds.
    pub estimated_cost_us: u64,
    /// The hard-cap violation, when the scene has one.
    pub violation: Option<MediaAdmissionViolationDetails>,
}

/// Media-cap inputs resolved before a candidate transaction begins.
#[derive(Debug)]
pub struct MediaAdmissionContext {
    asset_mime_types: HashMap<AssetId, String>,
    media_config: MediaConfig,
}

impl MediaAdmissionContext {
    pub(crate) fn new(
        asset_mime_types: HashMap<AssetId, String>,
        media_config: MediaConfig,
    ) -> Self {
        Self {
            asset_mime_types,
            media_config,
        }
    }

    /// Reject a mutated candidate that exceeds the configured producer caps.
    pub fn validate(&self, scene: &Scene) -> Result<(), DomainError> {
        let admission = self.evaluate(scene);
        let Some(violation) = admission.violation else {
            return Ok(());
        };
        Err(DomainError::validation_details(
            violation.message,
            serde_json::json!({
                "caps": violation.caps,
                "counts": violation.counts,
                "layers": violation.layers,
            }),
        ))
    }

    /// Evaluate a complete scene against these resolved inputs.
    #[must_use]
    pub fn evaluate(&self, scene: &Scene) -> SceneMediaAdmission {
        evaluate_scene_media_admission(scene, &self.asset_mime_types, &self.media_config)
    }
}

impl SceneMediaAdmission {
    /// The violation message, when the scene exceeds its caps.
    #[must_use]
    pub fn rejection_message(&self) -> Option<&str> {
        self.violation
            .as_ref()
            .map(|violation| violation.message.as_str())
    }
}

/// Evaluate a scene's media producer admission against live config.
///
/// Both transports render their own frozen shape for a violation, so
/// they call this for the details and then call
/// [`activate_scene`], which enforces the same rule again. The check
/// cannot be skipped by an adapter that forgets it.
pub fn evaluate_scene_media_admission(
    scene: &Scene,
    asset_mime_types: &HashMap<AssetId, String>,
    media_config: &MediaConfig,
) -> SceneMediaAdmission {
    let counts = scene_media_admission_counts(scene, asset_mime_types);
    SceneMediaAdmission {
        estimated_cost_us: counts.estimated_cost_us(),
        violation: scene_media_admission_violation_details(&counts, media_config),
    }
}

/// Enforce the hard media-producer caps for a complete scene candidate.
///
/// # Errors
///
/// [`DomainError::Validation`] with the canonical cap, count, and layer
/// details when the candidate exceeds a configured producer limit.
pub fn validate_scene_media_admission(
    scene: &Scene,
    asset_mime_types: &HashMap<AssetId, String>,
    media_config: &MediaConfig,
) -> Result<(), DomainError> {
    let admission = evaluate_scene_media_admission(scene, asset_mime_types, media_config);
    let Some(violation) = admission.violation else {
        return Ok(());
    };
    Err(DomainError::validation_details(
        violation.message,
        serde_json::json!({
            "caps": violation.caps,
            "counts": violation.counts,
            "layers": violation.layers,
        }),
    ))
}

fn scene_media_admission_violation_details(
    counts: &MediaAdmissionCounts,
    media_config: &MediaConfig,
) -> Option<MediaAdmissionViolationDetails> {
    let video_cap = usize::from(media_config.max_video_producers.clamp(1, 4));
    let livestream_cap = usize::from(media_config.max_livestream_producers.clamp(0, 2));
    let video_count = counts.video_asset_ids.len();
    let livestream_count = counts.livestream_asset_ids.len();

    if video_count <= video_cap && livestream_count <= livestream_cap {
        return None;
    }

    let mut violations = Vec::new();
    if video_count > video_cap {
        violations.push(format!("video producers {video_count}/{video_cap}"));
    }
    if livestream_count > livestream_cap {
        violations.push(format!(
            "livestream producers {livestream_count}/{livestream_cap}"
        ));
    }

    Some(MediaAdmissionViolationDetails {
        message: format!(
            "Scene exceeds media producer caps: {}",
            violations.join(", ")
        ),
        caps: serde_json::json!({
            "video": video_cap,
            "livestream": livestream_cap,
        }),
        counts: serde_json::json!({
            "video": video_count,
            "livestream": livestream_count,
        }),
        layers: serde_json::json!({
            "video": counts.video_layers,
            "livestream": counts.livestream_layers,
        }),
    })
}

fn scene_media_admission_counts(
    scene: &Scene,
    asset_mime_types: &HashMap<AssetId, String>,
) -> MediaAdmissionCounts {
    let mut counts = MediaAdmissionCounts::default();

    for zone in scene.zones.iter().filter(|zone| zone.enabled) {
        for layer in zone.layers.iter().filter(|layer| layer.enabled) {
            let LayerSource::Media { asset_id, .. } = &layer.source else {
                continue;
            };
            let Some(mime_type) = asset_mime_types.get(asset_id) else {
                continue;
            };

            match mime_type.as_str() {
                "video/mp4" | "video/webm" => {
                    if counts.video_asset_ids.insert(*asset_id) {
                        counts.estimated_cost_us = counts
                            .estimated_cost_us
                            .saturating_add(VIDEO_PRODUCER_COST_US);
                    }
                    counts.video_layers.push(media_admission_layer_detail(
                        zone, layer, *asset_id, mime_type,
                    ));
                }
                "application/vnd.hypercolor.stream-url" => {
                    if counts.livestream_asset_ids.insert(*asset_id) {
                        counts.estimated_cost_us = counts
                            .estimated_cost_us
                            .saturating_add(LIVESTREAM_PRODUCER_COST_US);
                    }
                    counts.livestream_layers.push(media_admission_layer_detail(
                        zone, layer, *asset_id, mime_type,
                    ));
                }
                "application/json" if counts.lottie_asset_ids.insert(*asset_id) => {
                    counts.estimated_cost_us = counts
                        .estimated_cost_us
                        .saturating_add(LOTTIE_PRODUCER_COST_US);
                }
                _ => {}
            }
        }
    }

    counts
}

fn media_admission_layer_detail(
    zone: &Zone,
    layer: &SceneLayer,
    asset_id: AssetId,
    mime_type: &str,
) -> serde_json::Value {
    serde_json::json!({
        "zone_id": zone.id.to_string(),
        "zone_name": &zone.name,
        "layer_id": layer.id.to_string(),
        "layer_name": &layer.name,
        "asset_id": asset_id.to_string(),
        "mime_type": mime_type,
    })
}

// ── activate_scene ───────────────────────────────────────────────────────

/// Make a scene the exclusive current one.
#[derive(Debug, Clone)]
pub struct ActivateScene {
    /// Which scene to activate. Adapters resolve names to ids.
    pub scene_id: SceneId,
    /// Overrides the scene's authored transition duration when present.
    pub transition_ms: Option<u64>,
}

/// The outcome of a scene activation.
#[derive(Debug)]
pub struct SceneActivated {
    /// The scene that is now current.
    pub scene_id: SceneId,
    /// Its name, resolved before activation.
    pub scene_name: String,
    /// Which scene was current before, when one was.
    pub previous_scene_id: Option<SceneId>,
    /// The estimated producer cost that drove soft admission.
    pub estimated_cost_us: u64,
    /// The post-commit named-layout outcome.
    pub layout: SceneLayoutActivationOutcome,
    /// The post-commit activation-brightness outcome.
    pub brightness: SideEffectOutcome,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// Activate a scene: validate its media admission, switch the exclusive
/// current scene, apply soft admission, reconcile connectivity, then
/// persist the converged runtime projection.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown scene,
/// [`DomainError::Validation`] when the scene exceeds its hard media
/// producer caps, and [`DomainError::Conflict`] when a
/// concurrent scene mutation lands first.
pub async fn activate_scene(
    ctx: &SceneLibraryContext,
    command: ActivateScene,
) -> Result<SceneActivated, DomainError> {
    let media_admission = ctx.scene.media_admission_context().await;
    let display_surfaces = ctx
        .layout
        .connected_display_surface_layouts(ctx.scene.layout_runtime())
        .await;
    let _activation_guard = ctx.layout.acquire_scene_activation_guard().await;
    let layout_guard = ctx.layout.acquire_update_guard().await;

    let mut mutation = ctx.scene.begin_mutation().await;
    let previous_scene_id = mutation.scenes().active_scene_id().copied();

    let scene = mutation
        .scenes()
        .get(&command.scene_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, command.scene_id))?;
    let scene_name = scene.name.clone();
    let layout_id = scene.layout_id.clone();
    let activation_brightness = scene.activation_brightness;
    let transition = command.transition_ms.map(|duration_ms| {
        let mut transition = scene.transition.clone();
        transition.duration_ms = duration_ms;
        transition
    });
    let admission = media_admission.evaluate(scene);
    if let Some(message) = admission.rejection_message() {
        return Err(DomainError::validation(message.to_owned()));
    }
    mutation.hydrate_existing_display_surfaces(command.scene_id, &display_surfaces)?;

    mutation.activate(
        command.scene_id,
        transition,
        SceneChangeReason::UserActivate,
    )?;

    let commit = ctx.scene.commit(mutation).await?;

    ctx.scene
        .apply_media_soft_admission(command.scene_id, &scene_name, admission.estimated_cost_us)
        .await;
    let layout = apply_activation_layout(ctx, layout_guard, layout_id).await;
    let brightness = apply_activation_brightness(ctx, activation_brightness).await;

    // Which scene is active decides which devices are worth connecting.
    ctx.layout
        .sync_runtime_connectivity(ctx.scene.layout_runtime())
        .await;
    ctx.scene.save_runtime_session().await;

    Ok(SceneActivated {
        scene_id: command.scene_id,
        scene_name,
        previous_scene_id,
        estimated_cost_us: admission.estimated_cost_us,
        layout,
        brightness,
        commit,
    })
}

async fn apply_activation_layout(
    ctx: &SceneLibraryContext,
    guard: LayoutUpdateGuard,
    layout_id: Option<hypercolor_types::identity::LayoutId>,
) -> SceneLayoutActivationOutcome {
    let Some(layout_id) = layout_id else {
        return SceneLayoutActivationOutcome {
            layout_id: None,
            applied: false,
            message: None,
        };
    };
    let layout = ctx.layout.get(&layout_id).await;
    let Some(layout) = layout else {
        let message = format!("scene layout '{layout_id}' is not available");
        ctx.event_bus.publish(HypercolorEvent::Error {
            code: "scene_layout_unavailable".to_owned(),
            message: message.clone(),
            severity: Severity::Warning,
        });
        return SceneLayoutActivationOutcome {
            layout_id: Some(layout_id),
            applied: false,
            message: Some(message),
        };
    };

    let result = ctx
        .layout
        .admit_persisted_update_under_guard(&guard, layout, ctx.scene.layout_runtime())
        .await;
    drop(guard);
    match result {
        Ok(()) => SceneLayoutActivationOutcome {
            layout_id: Some(layout_id),
            applied: true,
            message: None,
        },
        Err(error) => SceneLayoutActivationOutcome {
            layout_id: Some(layout_id),
            applied: false,
            message: Some(format!(
                "layout did not apply: {error}; retry through the layout resource"
            )),
        },
    }
}

async fn apply_activation_brightness(
    ctx: &SceneLibraryContext,
    brightness: Option<f32>,
) -> SideEffectOutcome {
    let Some(brightness) = brightness else {
        return SideEffectOutcome {
            applied: false,
            message: None,
        };
    };

    match crate::domain::output::set_brightness(&ctx.output, brightness).await {
        Ok(_) => SideEffectOutcome::applied(),
        Err(error) => SideEffectOutcome::failed(format!(
            "brightness did not apply: {error}; patch /output to retry"
        )),
    }
}

// ── Scene library CRUD ───────────────────────────────────────────────────

/// Add a scene to the library.
#[derive(Debug, Clone)]
pub struct CreateScene {
    /// Human-readable name.
    pub name: String,
    /// What the scene does.
    pub description: Option<String>,
    /// Whether the scene is selectable. Defaults to enabled.
    pub enabled: Option<bool>,
    /// Whether runtime effect and face actions may rewrite the scene.
    pub mutation_mode: Option<SceneMutationMode>,
    /// Free-form provenance the adapter wants recorded on the scene.
    pub metadata: HashMap<String, String>,
}

/// Save the active runtime scene as a snapshot-locked named scene.
#[derive(Debug, Clone)]
pub struct SnapshotScene {
    /// Human-readable name for the saved scene.
    pub name: String,
    /// Optional long-form description for the saved scene.
    pub description: Option<String>,
}

/// Replace one stored scene with the complete client-authored document.
#[derive(Debug, Clone)]
pub struct ReplaceScene {
    /// Scene resolved from the route path.
    pub scene_id: SceneId,
    /// Complete replacement document without server-owned fields.
    pub document: ReplaceSceneRequest,
    /// Optional scene commit generation from `If-Match`.
    pub expected_revision: Option<u64>,
}

/// The outcome of a scene library mutation.
#[derive(Debug)]
pub struct SceneWritten {
    /// The scene as it now stands.
    pub scene: Scene,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// The outcome of deleting a scene.
#[derive(Debug)]
pub struct SceneDeleted {
    /// The scene that was removed.
    pub scene: Scene,
    /// Which scene is current now, when the deletion changed it.
    pub current_scene: Option<Scene>,
    /// Which scene was current before.
    pub previous_scene_id: Option<SceneId>,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// The outcome of returning to the synthesized default scene.
#[derive(Debug)]
pub struct SceneDeactivated {
    /// The scene that was current, when one was.
    pub previous_scene: Option<Scene>,
    /// The scene that is current now.
    pub current_scene: Option<Scene>,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// Create a scene, seeded with a Default zone holding the current
/// device output roster.
///
/// Every scene is born with that zone so the Studio scene selector
/// always has one to select (Spec 65 §5.2); the user renames it freely.
///
/// # Errors
///
/// [`DomainError::Conflict`] when the scene cannot be added, and
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn create_scene(
    ctx: &SceneLibraryContext,
    command: CreateScene,
) -> Result<SceneWritten, DomainError> {
    let default_layout = ctx.layout.current();
    let scene = Scene {
        id: SceneId::new(),
        name: command.name,
        description: command.description,
        zones: vec![default_primary_group(default_layout)],
        zones_revision: 0,
        transition: TransitionSpec {
            duration_ms: 1000,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: command.enabled.unwrap_or(true),
        metadata: command.metadata,
        unassigned_behavior: UnassignedBehavior::Off,
        layout_id: None,
        activation_brightness: None,
        kind: SceneKind::Named,
        mutation_mode: command.mutation_mode.unwrap_or(SceneMutationMode::Live),
    };

    let mut mutation = ctx.scene.begin_mutation().await;
    mutation.create_scene(scene.clone())?;
    let commit = ctx.scene.commit(mutation).await?;

    Ok(SceneWritten { scene, commit })
}

/// Save the active runtime tree as a snapshot-locked named scene.
///
/// The layout guard and scene mutation lock produce one coherent view
/// of the current scene and active spatial layout. Zone and layer ids
/// remain stable so clients may keep addressing the captured resources.
///
/// # Errors
///
/// [`DomainError::Conflict`] when no scene is active or the snapshot
/// cannot be added, and [`DomainError::Conflict`] when a concurrent
/// scene mutation lands first.
pub async fn snapshot_scene(
    ctx: &SceneLibraryContext,
    command: SnapshotScene,
) -> Result<SceneWritten, DomainError> {
    let _layout_guard = ctx.layout.acquire_update_guard().await;
    let mut mutation = ctx.scene.begin_mutation().await;
    let active = mutation
        .scenes()
        .active_scene()
        .cloned()
        .ok_or_else(|| DomainError::conflict("no active scene to snapshot"))?;
    let layout_id = ctx.layout.active_layout_id()?;
    let scene = Scene {
        id: SceneId::new(),
        name: command.name,
        description: command.description,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Snapshot,
        layout_id: Some(layout_id),
        activation_brightness: None,
        ..active
    };

    mutation.create_scene(scene.clone())?;
    let commit = ctx.scene.commit(mutation).await?;

    Ok(SceneWritten { scene, commit })
}

/// Replace every client-authored field of one stored scene.
///
/// Supplied zone and layer identities must already belong to the scene.
/// Omitting either identity mints a fresh one. The one scene commit
/// generation fences the entire replacement.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown scene,
/// [`DomainError::Validation`] for invalid or foreign identities,
/// [`DomainError::PreconditionFailed`] for a stale revision, and
/// [`DomainError::Conflict`] when a concurrent mutation lands first.
pub async fn replace_scene(
    ctx: &SceneLibraryContext,
    mut command: ReplaceScene,
) -> Result<SceneWritten, DomainError> {
    if command.document.id.is_some_and(|id| id != command.scene_id) {
        return Err(DomainError::validation_field(
            "id",
            "scene id must match the route path",
        ));
    }

    let _effect_admission = ctx
        .effects
        .admit_layer_sources(
            command
                .document
                .zones
                .iter_mut()
                .flat_map(|zone| zone.layers.iter_mut())
                .map(|layer| &mut layer.source),
        )
        .await?;

    let default_layout = ctx.layout.current();
    let mut mutation = ctx.scene.begin_mutation().await;
    crate::domain::scene_tree::check_scene_revision(&mutation, command.expected_revision)?;
    let existing = mutation
        .scenes()
        .get(&command.scene_id)
        .cloned()
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, command.scene_id))?;

    if command.document.kind != existing.kind {
        return Err(DomainError::validation_field(
            "kind",
            "scene kind cannot be changed",
        ));
    }
    if command.scene_id.is_default() && command.document.name != existing.name {
        return Err(DomainError::validation_field(
            "name",
            "default scene cannot be renamed",
        ));
    }

    validate_replacement_identities(&existing, &command.document)?;
    let groups = replacement_zones(&existing, &default_layout, command.document.zones)?;
    let updated = Scene {
        id: existing.id,
        name: command.document.name,
        description: command.document.description,
        zones: groups,
        zones_revision: existing.zones_revision.saturating_add(1),
        transition: command.document.transition,
        priority: command.document.priority,
        enabled: command.document.enabled,
        metadata: command.document.metadata,
        unassigned_behavior: command.document.unassigned_behavior,
        layout_id: command.document.layout_id,
        activation_brightness: command.document.activation_brightness,
        kind: existing.kind,
        mutation_mode: command.document.mutation_mode,
    };

    if let Err(errors) = updated.validate() {
        return Err(DomainError::validation(errors.join("; ")));
    }
    mutation.update_scene(updated.clone())?;
    crate::domain::layer::validate_candidate_media_admission(&ctx.scene, &mutation, updated.id)
        .await?;
    mutation.retire_scene_previews(updated.id);
    let commit = ctx.scene.commit(mutation).await?;
    ctx.scene.save_runtime_session().await;
    ctx.layout
        .sync_runtime_connectivity(ctx.scene.layout_runtime())
        .await;

    Ok(SceneWritten {
        scene: updated,
        commit,
    })
}

fn validate_replacement_identities(
    existing: &Scene,
    document: &ReplaceSceneRequest,
) -> Result<(), DomainError> {
    let zone_ids = existing
        .zones
        .iter()
        .map(|zone| zone.id)
        .collect::<HashSet<_>>();
    let layer_ids = existing
        .zones
        .iter()
        .flat_map(|zone| zone.layers.iter())
        .map(|layer| layer.id)
        .collect::<HashSet<_>>();
    let mut requested_zones = HashSet::new();
    let mut requested_layers = HashSet::new();

    for zone in &document.zones {
        if let Some(zone_id) = zone.id {
            if !zone_ids.contains(&zone_id) {
                return Err(DomainError::validation_details(
                    "supplied zone id does not belong to this scene",
                    serde_json::json!({ "zone_id": zone_id }),
                ));
            }
            if !requested_zones.insert(zone_id) {
                return Err(DomainError::validation_details(
                    "zone ids must be unique",
                    serde_json::json!({ "zone_id": zone_id }),
                ));
            }
        }
        for layer in &zone.layers {
            if let Some(layer_id) = layer.id {
                if !layer_ids.contains(&layer_id) {
                    return Err(DomainError::validation_details(
                        "supplied layer id does not belong to this scene",
                        serde_json::json!({ "layer_id": layer_id }),
                    ));
                }
                if !requested_layers.insert(layer_id) {
                    return Err(DomainError::validation_details(
                        "layer ids must be unique",
                        serde_json::json!({ "layer_id": layer_id }),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn replacement_zones(
    existing: &Scene,
    default_layout: &SpatialLayout,
    zones: Vec<ReplaceZoneRequest>,
) -> Result<Vec<Zone>, DomainError> {
    zones
        .into_iter()
        .map(|zone| {
            let zone_id = zone.id.unwrap_or_default();
            let stored = existing
                .zones
                .iter()
                .find(|candidate| candidate.id == zone_id);
            let layout = replacement_zone_layout(stored, default_layout, &zone)?;
            let layers = zone
                .layers
                .into_iter()
                .map(replacement_layer)
                .collect::<Vec<_>>();
            Ok(Zone {
                id: zone_id,
                name: zone.name,
                description: zone.description,
                layers,
                layout,
                brightness: zone.brightness,
                enabled: zone.enabled,
                color: zone.color,
                display_target: zone.display_target,
                role: zone.role,
                controls_version: stored
                    .map_or(0, |stored| stored.controls_version.saturating_add(1)),
                layers_version: stored.map_or(0, |stored| stored.layers_version.saturating_add(1)),
            })
        })
        .collect()
}

fn replacement_layer(request: ReplaceSceneLayerRequest) -> SceneLayer {
    SceneLayer {
        id: request.id.unwrap_or_default(),
        name: request.name,
        source: request.source,
        blend: request.blend,
        opacity: request.opacity,
        transform: request.transform,
        adjust: request.adjust,
        bindings: request.bindings,
        enabled: request.enabled,
    }
}

fn replacement_zone_layout(
    stored_zone: Option<&Zone>,
    default_layout: &SpatialLayout,
    request: &ReplaceZoneRequest,
) -> Result<SpatialLayout, DomainError> {
    let mut layout = stored_zone
        .map(|zone| zone.layout.clone())
        .unwrap_or_else(|| default_layout.clone());
    let placements = request
        .layout
        .as_ref()
        .map_or(&[][..], |layout| layout.placements.as_slice());
    let members = request
        .members
        .iter()
        .map(|member| (member.id.0.as_str(), member))
        .collect::<HashMap<_, _>>();
    if members.len() != request.members.len() || placements.len() != request.members.len() {
        return Err(DomainError::validation(
            "layout placements must name exactly the zone members, each once",
        ));
    }

    let stored_outputs = stored_zone
        .map(|zone| zone.layout.zones.as_slice())
        .unwrap_or_default();
    let mut outputs = Vec::with_capacity(placements.len());
    let mut placed = HashSet::new();
    for placement in placements {
        let member_id = placement.member.0.as_str();
        let Some(member) = members.get(member_id) else {
            return Err(DomainError::validation_details(
                "layout placement names an unknown zone member",
                serde_json::json!({ "member": member_id }),
            ));
        };
        if !placed.insert(member_id) {
            return Err(DomainError::validation_details(
                "layout placements must name each zone member once",
                serde_json::json!({ "member": member_id }),
            ));
        }
        let mut output = stored_outputs
            .iter()
            .find(|output| output.id == member_id)
            .cloned()
            .unwrap_or_else(|| Output {
                id: member.id.0.clone(),
                name: member.name.clone(),
                device_id: member.device_id.clone(),
                zone_name: member.segment.clone(),
                position: placement.position,
                size: placement.size,
                rotation: placement.rotation,
                scale: placement.scale,
                display_order: 0,
                orientation: placement.orientation,
                topology: placement.topology.clone(),
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: None,
                edge_behavior: None,
                shape: None,
                shape_preset: None,
                attachment: None,
                brightness: None,
            });
        output.id.clone_from(&member.id.0);
        output.name.clone_from(&member.name);
        output.device_id.clone_from(&member.device_id);
        output.zone_name.clone_from(&member.segment);
        output.position = placement.position;
        output.size = placement.size;
        output.rotation = placement.rotation;
        output.scale = placement.scale;
        output.orientation = placement.orientation;
        output.topology.clone_from(&placement.topology);
        output.led_positions = hypercolor_core::spatial::generate_positions(&output.topology);
        outputs.push(output);
    }
    if placed.len() != members.len() {
        return Err(DomainError::validation(
            "layout placements must name exactly the zone members, each once",
        ));
    }

    layout.zones = outputs;
    if stored_zone.is_none() {
        layout.default_sampling_mode = SamplingMode::Bilinear;
        layout.default_edge_behavior = EdgeBehavior::Clamp;
    }
    Ok(layout)
}

/// Remove a scene from the library, deactivating it first when it is
/// the current one.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown scene,
/// [`DomainError::Conflict`] for the Default scene, and
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn delete_scene(
    ctx: &SceneLibraryContext,
    scene_id: SceneId,
) -> Result<SceneDeleted, DomainError> {
    if scene_id.is_default() {
        return Err(DomainError::conflict("Default scene cannot be deleted"));
    }

    let _activation_guard = ctx.layout.acquire_scene_activation_guard().await;
    let is_active = ctx.scene.snapshot().await.active_scene_id().copied() == Some(scene_id);
    let layout_guard = if is_active {
        Some(ctx.layout.acquire_update_guard().await)
    } else {
        None
    };

    let mut mutation = ctx.scene.begin_mutation().await;
    let previous_scene_id = mutation.scenes().active_scene_id().copied();
    let scene = mutation.delete_scene(&scene_id)?;
    let current_scene = mutation.scenes().active_scene().cloned();

    mutation.retire_scene_previews(scene_id);
    let commit = ctx.scene.commit(mutation).await?;
    drop(layout_guard);
    ctx.scene.save_runtime_session().await;
    if is_active {
        ctx.layout
            .sync_runtime_connectivity(ctx.scene.layout_runtime())
            .await;
    }

    Ok(SceneDeleted {
        scene,
        current_scene,
        previous_scene_id,
        commit,
    })
}

/// Return to the synthesized default scene.
///
/// # Errors
///
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn deactivate_scene(ctx: &SceneLibraryContext) -> Result<SceneDeactivated, DomainError> {
    let _activation_guard = ctx.layout.acquire_scene_activation_guard().await;
    let layout_guard = ctx.layout.acquire_update_guard().await;

    let mut mutation = ctx.scene.begin_mutation().await;
    let previous_scene = mutation.scenes().active_scene().cloned();
    mutation.deactivate_current(SceneChangeReason::UserDeactivate);
    let current_scene = mutation.scenes().active_scene().cloned();

    let commit = ctx.scene.commit(mutation).await?;
    drop(layout_guard);
    ctx.scene.save_runtime_session().await;

    // Which scene is active decides which devices are worth connecting.
    ctx.layout
        .sync_runtime_connectivity(ctx.scene.layout_runtime())
        .await;

    Ok(SceneDeactivated {
        previous_scene,
        current_scene,
        commit,
    })
}

/// The active scene's id, refusing scenes that forbid runtime rewriting.
///
/// Snapshot scenes are a deliberate user choice: runtime effect and face
/// actions must not silently edit them. Adapters that need the refusal
/// *before* opening a candidate — because their transport renders it in
/// a shape the canonical projection would not produce — call this
/// against a read guard; the mutation checks it again regardless.
///
/// # Errors
///
/// [`DomainError::Conflict`] for a snapshot-locked scene and
/// [`DomainError::Internal`] when no scene is active at all.
pub fn active_scene_for_runtime_mutation(
    manager: &SceneManager,
    action: &str,
) -> Result<SceneId, DomainError> {
    let active = manager
        .active_scene()
        .ok_or_else(|| DomainError::Internal(anyhow::anyhow!("No active scene available")))?;
    if active.blocks_runtime_mutation() {
        return Err(DomainError::conflict(format!(
            "Active scene '{}' is in snapshot mode; return to Default or deactivate it before {action}",
            active.name
        )));
    }
    Ok(active.id)
}

// ── Shared event helpers ─────────────────────────────────────────────────

/// The active-scene-changed event every activation path records.
#[must_use]
pub fn active_scene_changed_event(
    previous: Option<SceneId>,
    current: &Scene,
    reason: SceneChangeReason,
) -> HypercolorEvent {
    HypercolorEvent::ActiveSceneChanged {
        previous,
        current: current.id,
        current_name: current.name.clone(),
        current_kind: current.kind,
        current_mutation_mode: current.mutation_mode,
        current_snapshot_locked: current.blocks_runtime_mutation(),
        reason,
    }
}

/// The zone-changed event both effect-apply paths record.
#[must_use]
pub fn zone_changed_event(scene_id: SceneId, zone: &Zone, kind: ZoneChangeKind) -> HypercolorEvent {
    HypercolorEvent::ZoneChanged {
        scene_id,
        zone_id: zone.id,
        role: zone.role,
        kind,
    }
}
