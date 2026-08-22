//! Effect domain services (Spec 76 §2.2, §2.3).
//!
//! One `apply_effect` serves REST and MCP. Everything transport-shaped
//! stays in the adapters. Each adapter validates its wire contract and
//! resolves the effect through its canonical selector before entering
//! this module. Both arrive here with a resolved [`EffectId`] and a
//! [`RequestedTransition`], and both get the same validation, scene
//! mutation, and ordered events.

mod identity;

pub(crate) use identity::{
    EffectIdMigrations, reload_registry_file, remap_effect_id, remap_zones, rescan_registry,
};

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use hypercolor_core::effect::{EffectEntry, EffectRegistry, RescanReport};
use strum::VariantNames;

use hypercolor_types::api::scene::SideEffectOutcome;
use hypercolor_types::effect::{
    ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource,
};
use hypercolor_types::event::{EffectRef, ZoneChangeKind};
use hypercolor_types::layer::LayerSource;
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{SceneId, Zone, ZoneId};
use hypercolor_types::spatial::SpatialLayout;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, RwLockWriteGuard};

use crate::domain::commit::SceneCommit;
use crate::domain::context::SceneContext;
use crate::domain::output::OutputContext;
use crate::domain::spatial::SpatialService;
use crate::domain::{DomainError, MutationContext};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentityMigrationPersistence {
    Written,
    Superseded,
    Retrying(String),
}

/// Effect catalog and activation authority shared by every transport.
#[derive(Clone)]
pub struct EffectContext {
    registry: Arc<RwLock<EffectRegistry>>,
    scene: SceneContext,
    spatial: SpatialService,
    output: OutputContext,
    update_gate: Arc<RwLock<()>>,
    #[cfg(test)]
    resolution_test_barrier: Arc<std::sync::Mutex<Option<Arc<EffectResolutionTestBarrier>>>>,
    #[cfg(test)]
    identity_publication_test_barrier:
        Arc<std::sync::Mutex<Option<Arc<EffectResolutionTestBarrier>>>>,
}

#[cfg(test)]
pub(crate) struct EffectResolutionTestBarrier {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

pub(crate) struct EffectRegistryUpdate<'a> {
    update_guard: OwnedRwLockWriteGuard<()>,
    registry: &'a RwLock<EffectRegistry>,
    base_generation: u64,
    candidate: EffectRegistry,
    report: RescanReport,
}

pub(crate) struct EffectRegistryPublication<'a> {
    update_guard: OwnedRwLockWriteGuard<()>,
    registry: RwLockWriteGuard<'a, EffectRegistry>,
    candidate: EffectRegistry,
    report: RescanReport,
}

/// Effect metadata qualified by the catalog generation that resolved it.
#[derive(Debug, Clone)]
pub struct ResolvedEffect {
    metadata: EffectMetadata,
    registry_generation: u64,
}

impl std::ops::Deref for ResolvedEffect {
    type Target = EffectMetadata;

    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

impl ResolvedEffect {
    #[must_use]
    pub fn into_metadata(self) -> EffectMetadata {
        self.metadata
    }
}

/// Shared read admission that excludes catalog identity publication.
pub struct EffectMutationAdmission {
    _guard: OwnedRwLockReadGuard<()>,
}

impl EffectContext {
    pub(crate) fn new(
        registry: Arc<RwLock<EffectRegistry>>,
        scene: SceneContext,
        spatial: SpatialService,
        output: OutputContext,
    ) -> Self {
        Self {
            registry,
            scene,
            spatial,
            output,
            update_gate: Arc::new(RwLock::new(())),
            #[cfg(test)]
            resolution_test_barrier: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            identity_publication_test_barrier: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Current full-scope layout for a newly materialized primary zone.
    #[must_use]
    pub fn full_scope_layout(&self) -> SpatialLayout {
        self.spatial.layout().as_ref().clone()
    }

    /// Resolve one effect schema for scene-tree control validation.
    pub async fn metadata(&self, effect_id: EffectId) -> Option<EffectMetadata> {
        let registry = self.registry.read().await;
        let metadata = registry.get(&effect_id).map(|entry| entry.metadata.clone());
        drop(registry);
        #[cfg(test)]
        self.pause_after_resolution_for_test().await;
        metadata
    }

    /// Resolve an effect by canonical id or catalog lookup name.
    pub async fn resolve_metadata(&self, id_or_name: &str) -> Option<EffectMetadata> {
        let registry = self.registry.read().await;
        if let Ok(uuid) = id_or_name.parse::<uuid::Uuid>() {
            return registry
                .get(&EffectId::new(uuid))
                .map(|entry| entry.metadata.clone());
        }
        registry
            .iter()
            .find(|(_, entry)| entry.metadata.matches_lookup(id_or_name))
            .map(|(_, entry)| entry.metadata.clone())
    }

    /// Resolve one mutation target and bind it to the current catalog generation.
    pub async fn resolve_for_mutation(&self, id_or_name: &str) -> Option<ResolvedEffect> {
        let registry = self.registry.read().await;
        let metadata = if let Ok(uuid) = id_or_name.parse::<uuid::Uuid>() {
            registry
                .get(&EffectId::new(uuid))
                .map(|entry| entry.metadata.clone())
        } else {
            registry
                .iter()
                .find(|(_, entry)| entry.metadata.matches_lookup(id_or_name))
                .map(|(_, entry)| entry.metadata.clone())
        }?;
        let resolved = ResolvedEffect {
            metadata,
            registry_generation: registry.generation(),
        };
        drop(registry);
        #[cfg(test)]
        self.pause_after_resolution_for_test().await;
        Some(resolved)
    }

    /// Resolve one exact mutation target at the current catalog generation.
    pub async fn metadata_for_mutation(&self, effect_id: EffectId) -> Option<ResolvedEffect> {
        let registry = self.registry.read().await;
        let metadata = registry.get(&effect_id)?.metadata.clone();
        let resolved = ResolvedEffect {
            metadata,
            registry_generation: registry.generation(),
        };
        drop(registry);
        #[cfg(test)]
        self.pause_after_resolution_for_test().await;
        Some(resolved)
    }

    /// Capture the catalog as generation-qualified mutation targets.
    pub(crate) async fn all_for_mutation(&self) -> Vec<ResolvedEffect> {
        let registry = self.registry.read().await;
        let generation = registry.generation();
        registry
            .iter()
            .map(|(_, entry)| ResolvedEffect {
                metadata: entry.metadata.clone(),
                registry_generation: generation,
            })
            .collect()
    }

    /// Admit a previously resolved effect while excluding registry publication.
    pub(crate) async fn admit(
        &self,
        effect: &ResolvedEffect,
    ) -> Result<EffectMutationAdmission, DomainError> {
        self.admit_generation(effect.registry_generation).await
    }

    /// Admit work resolved against one catalog generation.
    async fn admit_generation(
        &self,
        expected_generation: u64,
    ) -> Result<EffectMutationAdmission, DomainError> {
        let guard = Arc::clone(&self.update_gate).read_owned().await;
        let current_generation = self.registry.read().await.generation();
        if current_generation != expected_generation {
            return Err(DomainError::conflict_details(
                "effect catalog changed after resolving this request",
                serde_json::json!({
                    "kind": "effect_resolution_superseded",
                    "expected_generation": expected_generation,
                    "current_generation": current_generation,
                }),
            ));
        }
        Ok(EffectMutationAdmission { _guard: guard })
    }

    /// Admit a mutation that will resolve its effects while the guard is held.
    pub(crate) async fn admit_current(&self) -> EffectMutationAdmission {
        EffectMutationAdmission {
            _guard: Arc::clone(&self.update_gate).read_owned().await,
        }
    }

    /// Validate every effect layer against one catalog generation and retain
    /// that generation through the caller's scene commit.
    pub(crate) async fn admit_layer_sources<'a>(
        &self,
        sources: impl IntoIterator<Item = &'a LayerSource>,
    ) -> Result<Option<EffectMutationAdmission>, DomainError> {
        let effect_ids = sources
            .into_iter()
            .filter_map(|source| match source {
                LayerSource::Effect { effect_id, .. } => Some(*effect_id),
                LayerSource::Media { .. }
                | LayerSource::ScreenRegion { .. }
                | LayerSource::WebViewport { .. }
                | LayerSource::ColorFill { .. } => None,
            })
            .collect::<Vec<_>>();
        if effect_ids.is_empty() {
            return Ok(None);
        }

        let guard = Arc::clone(&self.update_gate).read_owned().await;
        let registry = self.registry.read().await;
        if let Some(effect_id) = effect_ids
            .into_iter()
            .find(|effect_id| registry.get(effect_id).is_none())
        {
            return Err(DomainError::not_found(
                crate::domain::ResourceKind::Effect,
                effect_id,
            ));
        }
        drop(registry);
        #[cfg(test)]
        self.pause_after_resolution_for_test().await;
        Ok(Some(EffectMutationAdmission { _guard: guard }))
    }

    pub(crate) const fn scene_context(&self) -> &SceneContext {
        &self.scene
    }

    /// Capture every catalog entry as owned metadata.
    pub async fn all_metadata(&self) -> Vec<EffectMetadata> {
        self.registry
            .read()
            .await
            .iter()
            .map(|(_, entry)| entry.metadata.clone())
            .collect()
    }

    /// Return the number of registered effects.
    pub async fn len(&self) -> usize {
        self.registry.read().await.len()
    }

    /// Whether the effect catalog is empty.
    pub async fn is_empty(&self) -> bool {
        self.registry.read().await.is_empty()
    }

    /// Whether any requested effect currently requires live audio input.
    pub async fn any_audio_reactive(&self, effect_ids: impl IntoIterator<Item = EffectId>) -> bool {
        let registry = self.registry.read().await;
        effect_ids.into_iter().any(|effect_id| {
            registry
                .get(&effect_id)
                .is_some_and(|entry| entry.metadata.audio_reactive)
        })
    }

    pub(crate) async fn prepare_rescan(&self) -> EffectRegistryUpdate<'_> {
        let gate = Arc::clone(&self.update_gate).write_owned().await;
        let registry = self.registry.read().await;
        let base_generation = registry.generation();
        let mut candidate = registry.clone();
        drop(registry);
        let mut report = candidate.rescan();
        retire_legacy_registry_entries(&mut candidate, &mut report);
        EffectRegistryUpdate {
            update_guard: gate,
            registry: self.registry.as_ref(),
            base_generation,
            candidate,
            report,
        }
    }

    pub(crate) async fn prepare_reload(&self, path: &Path) -> EffectRegistryUpdate<'_> {
        let gate = Arc::clone(&self.update_gate).write_owned().await;
        let registry = self.registry.read().await;
        let base_generation = registry.generation();
        let mut candidate = registry.clone();
        drop(registry);
        let mut report = candidate.reload_single(path);
        retire_legacy_registry_entries(&mut candidate, &mut report);
        EffectRegistryUpdate {
            update_guard: gate,
            registry: self.registry.as_ref(),
            base_generation,
            candidate,
            report,
        }
    }

    /// Register one already canonical entry and report whether it replaced one.
    pub async fn register(&self, entry: EffectEntry) -> bool {
        let _update_guard = self.update_gate.write().await;
        self.registry.write().await.register(entry).is_some()
    }

    #[cfg(test)]
    pub(crate) fn registry_handle(&self) -> Arc<RwLock<EffectRegistry>> {
        Arc::clone(&self.registry)
    }

    #[cfg(test)]
    pub(crate) fn pause_next_resolution_for_test(&self) -> Arc<EffectResolutionTestBarrier> {
        let barrier = Arc::new(EffectResolutionTestBarrier {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        *self
            .resolution_test_barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&barrier));
        barrier
    }

    #[cfg(test)]
    pub(crate) fn pause_next_identity_publication_for_test(
        &self,
    ) -> Arc<EffectResolutionTestBarrier> {
        let barrier = Arc::new(EffectResolutionTestBarrier {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        *self
            .identity_publication_test_barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&barrier));
        barrier
    }

    #[cfg(test)]
    async fn pause_after_resolution_for_test(&self) {
        let barrier = self
            .resolution_test_barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(barrier) = barrier {
            barrier.entered.notify_one();
            barrier.release.notified().await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn pause_before_identity_publication_for_test(&self) {
        let barrier = self
            .identity_publication_test_barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(barrier) = barrier {
            barrier.entered.notify_one();
            barrier.release.notified().await;
        }
    }
}

fn retire_legacy_registry_entries(registry: &mut EffectRegistry, report: &mut RescanReport) {
    report.removed += report
        .legacy_effect_ids
        .keys()
        .filter(|legacy_id| registry.remove(legacy_id).is_some())
        .count();
}

#[cfg(test)]
impl EffectResolutionTestBarrier {
    pub(crate) async fn wait_until_entered(&self) {
        self.entered.notified().await;
    }

    pub(crate) fn release(&self) {
        self.release.notify_one();
    }
}

impl<'a> EffectRegistryUpdate<'a> {
    pub(crate) const fn report(&self) -> &RescanReport {
        &self.report
    }

    pub(crate) async fn prepare_publication(
        self,
    ) -> Result<EffectRegistryPublication<'a>, DomainError> {
        let registry = self.registry.write().await;
        if registry.generation() != self.base_generation {
            return Err(DomainError::conflict(
                "effect registry changed while publishing an identity migration",
            ));
        }
        Ok(EffectRegistryPublication {
            update_guard: self.update_guard,
            registry,
            candidate: self.candidate,
            report: self.report,
        })
    }
}

impl EffectRegistryPublication<'_> {
    pub(crate) fn publish(mut self) -> RescanReport {
        *self.registry = self.candidate;
        drop(self.update_guard);
        self.report
    }
}

/// A transition the caller asked for.
///
/// The daemon renders effect switches as immediate cuts today, so the
/// only request this surface can honor is a zero-duration cut. Anything
/// else is refused rather than accepted and quietly ignored — a caller
/// that asked for a crossfade must learn it did not get one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedTransition {
    /// Transition style, lowercased by the adapter. `None` means the
    /// caller named no style, which reads as a cut.
    pub style: Option<String>,
    /// Requested duration in milliseconds.
    pub duration_ms: u64,
}

impl RequestedTransition {
    /// The request a caller who said nothing about transitions makes.
    #[must_use]
    pub const fn cut() -> Self {
        Self {
            style: None,
            duration_ms: 0,
        }
    }

    /// A request carrying only a duration.
    #[must_use]
    pub const fn of_duration(duration_ms: u64) -> Self {
        Self {
            style: None,
            duration_ms,
        }
    }

    /// Resolve what the daemon will actually do.
    ///
    /// # Errors
    ///
    /// [`DomainError::Validation`] naming the unimplemented transition.
    pub fn resolve(&self) -> Result<AppliedTransition, DomainError> {
        let style = self
            .style
            .as_deref()
            .unwrap_or("cut")
            .trim()
            .to_ascii_lowercase();
        let is_cut = style.is_empty() || style == "cut";
        if is_cut && self.duration_ms == 0 {
            return Ok(AppliedTransition::cut());
        }
        if is_cut {
            return Err(DomainError::validation_field(
                "transition",
                "Effect transitions are not implemented yet; only immediate cut applies today.",
            ));
        }
        Err(DomainError::validation_field(
            "transition",
            format!(
                "Effect transition '{style}' is not implemented yet; only immediate cut applies today."
            ),
        ))
    }
}

/// The transition the daemon applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedTransition {
    /// Transition style name for the wire.
    pub style: &'static str,
    /// Applied duration in milliseconds.
    pub duration_ms: u64,
}

impl AppliedTransition {
    /// An immediate cut — today's only outcome.
    #[must_use]
    pub const fn cut() -> Self {
        Self {
            style: "cut",
            duration_ms: 0,
        }
    }
}

/// Load an effect into a zone of the active scene.
#[derive(Debug, Clone)]
pub struct ApplyEffect {
    /// The effect to load, bound to the catalog generation that resolved it.
    pub effect: ResolvedEffect,
    /// Control values, already normalized against the effect's schema.
    pub controls: HashMap<String, ControlValue>,
    /// Preset provenance to record on the zone.
    pub preset_id: Option<PresetId>,
    /// Which zone to load into. `None` targets the primary zone, which
    /// is created when the active scene has none.
    pub target_zone: Option<ZoneId>,
    /// The structural revision the caller observed, when guarded.
    pub expected_revision: Option<u64>,
    /// The requested transition.
    pub transition: RequestedTransition,
    /// Whether the post-commit output wake belongs to this gesture.
    /// Playlist advancement preserves the existing power state.
    pub wake_output: bool,
}

/// The outcome of loading an effect.
#[derive(Debug)]
pub struct EffectApplied {
    /// The effect that is now running.
    pub effect: EffectRef,
    /// The scene that owns the target zone.
    pub scene_id: SceneId,
    /// The zone as it stands after the apply.
    pub zone: Zone,
    /// Whether the zone was created or updated.
    pub zone_change: ZoneChangeKind,
    /// What the target zone ran before, when it ran anything.
    pub previous_effect: Option<EffectRef>,
    /// The transition the daemon applied.
    pub transition: AppliedTransition,
    /// Whether the requested post-commit output policy succeeded.
    /// The commit stands either way.
    pub output: SideEffectOutcome,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// Load an effect into the active scene and start rendering it.
///
/// # Errors
///
/// [`DomainError::Validation`] for a display face, an unimplemented
/// transition, or a zone that refuses the effect,
/// [`DomainError::Conflict`] when the active scene is snapshot-locked,
/// and [`DomainError::Conflict`] when a concurrent scene
/// mutation lands first.
pub async fn apply_effect(
    ctx: &EffectContext,
    command: ApplyEffect,
    meta: MutationContext,
) -> Result<EffectApplied, DomainError> {
    let transition = command.transition.resolve()?;
    let _admission = ctx.admit(&command.effect).await?;
    let metadata = command.effect.into_metadata();

    // Resolving the outgoing effect's name needs the registry, and the
    // outgoing effect is not known until the scene snapshot is in hand.
    // Taking the index now keeps every await out of the window between
    // the snapshot and its compare-and-swap.
    let effect_refs = {
        let registry = ctx.registry.read().await;
        registry
            .iter()
            .map(|(id, entry)| (*id, effect_ref(&entry.metadata)))
            .collect::<HashMap<EffectId, EffectRef>>()
    };

    if metadata.category == EffectCategory::Display {
        return Err(DomainError::validation(format!(
            "Effect '{}' is a display face and must be assigned to a display device, not applied to the LED pipeline",
            metadata.name
        )));
    }

    let layout = ctx.full_scope_layout();

    let mut mutation = ctx.scene.begin_mutation().await;
    crate::domain::scene_tree::check_scene_revision(&mutation, command.expected_revision)?;
    let scene_id = mutation.active_scene_for_runtime_mutation("applying an effect")?;

    // A target naming the primary zone — or no target at all — takes the
    // upsert path; a named non-primary zone is effect-set in place and
    // keeps its own layout.
    let primary_zone_id = mutation.primary_zone_id();
    let named_target = command
        .target_zone
        .filter(|id| Some(*id) != primary_zone_id);

    // "Previous" is whatever ran in the *target* zone, so a change in
    // zone 2 never claims the primary's effect was replaced.
    let previous_effect = named_target
        .or(primary_zone_id)
        .and_then(|zone_id| mutation.zone_effect(zone_id))
        .and_then(|effect_id| effect_refs.get(&effect_id).cloned());

    let (zone, zone_change) = if let Some(zone_id) = named_target {
        let zone = mutation.apply_effect_to_zone(
            zone_id,
            &metadata,
            command.controls,
            command.preset_id,
            meta.trigger,
            previous_effect.clone(),
        )?;
        (zone, ZoneChangeKind::Updated)
    } else {
        let zone_change = if primary_zone_id.is_some() {
            ZoneChangeKind::Updated
        } else {
            ZoneChangeKind::Created
        };
        let zone = mutation.upsert_primary_zone(
            &metadata,
            command.controls,
            command.preset_id,
            layout,
            meta.trigger,
            previous_effect.clone(),
        )?;
        (zone, zone_change)
    };

    let effect = effect_ref(&metadata);

    let commit = ctx.scene.commit(mutation).await?;

    // Every refusal above returns before this point, so nothing the
    // caller can get rejected for has woken output (Spec 78 §2.3).
    let output = if command.wake_output && !ctx.output.wake_for_effect_start().await {
        SideEffectOutcome::failed("output did not resume; patch /output to retry")
    } else {
        SideEffectOutcome::applied()
    };

    ctx.scene.save_runtime_session().await;

    Ok(EffectApplied {
        effect,
        scene_id,
        zone,
        zone_change,
        previous_effect,
        transition,
        output,
        commit,
    })
}

/// Recompute the active scene's resolved zones after the effect registry
/// changed underneath them.
///
/// The resolved zones are derived state, so this moves no persisted
/// scene content — but it does move the revision the render thread reads,
/// which is why it commits rather than writing through.
///
/// # Errors
///
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn invalidate_active_zones(ctx: &EffectContext) -> Result<SceneCommit, DomainError> {
    // A dropped invalidation leaves the active scene's resolved zones
    // pointing at pre-reload effect metadata until something else
    // invalidates, so this reconciliation retries rather than losing.
    let ((), commit) = ctx
        .scene
        .commit_retrying(|mutation| {
            mutation.invalidate_active_zones();
            Ok(Some(()))
        })
        .await?
        .ok_or_else(|| DomainError::Internal(anyhow::anyhow!("invalidation produced no commit")))?;
    Ok(commit)
}

// ── Catalog ──────────────────────────────────────────────────────────────

/// A narrowing of the effect catalog.
///
/// Every transport that lists effects builds one of these and hands it
/// to [`list_catalog`]; none of them filters on its own. `None` on a
/// field means the caller did not narrow on that axis.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectCatalogQuery {
    /// Exact category match.
    pub category: Option<EffectCategory>,
    /// Declared audio reactivity.
    pub audio_reactive: Option<bool>,
    /// Declared screen reactivity.
    pub screen_reactive: Option<bool>,
    /// Declared input reactivity.
    pub input_reactive: Option<bool>,
    /// Rendering source: `native`, `html`, or `shader`.
    pub source: Option<String>,
    /// Case-insensitive substring over name, description, author, and
    /// tags.
    ///
    /// Case is folded with `str::to_lowercase`, which is Unicode-aware
    /// but is lowercasing rather than full case folding. The pairs it
    /// still misses all change length or context: eszett against `ss`,
    /// Turkish dotted capital I, and medial against final sigma. Closing
    /// those needs a folding crate; std does not offer one.
    pub search: Option<String>,
}

impl EffectCatalogQuery {
    /// Parse the wire spellings of the narrowing axes.
    ///
    /// `category` and `source` fold with ASCII rules because both are
    /// closed vocabularies spelled in ASCII; only the free-text search
    /// term needs the Unicode fold.
    ///
    /// # Errors
    ///
    /// [`DomainError::Validation`] when a category or source names a
    /// value the type system does not have. An unrecognized filter
    /// value is a caller mistake worth reporting, not an empty list.
    pub fn parse(
        category: Option<&str>,
        source: Option<&str>,
        search: Option<&str>,
    ) -> Result<Self, DomainError> {
        let category = category
            .map(|raw| {
                EffectCategory::from_str(&raw.to_ascii_lowercase()).map_err(|_| {
                    DomainError::validation_field(
                        "category",
                        format!(
                            "unknown effect category '{raw}'; expected one of {}",
                            EffectCategory::VARIANTS.join(", ")
                        ),
                    )
                })
            })
            .transpose()?;

        let source = source
            .map(|raw| {
                let normalized = raw.to_ascii_lowercase();
                if EFFECT_SOURCE_KINDS.contains(&normalized.as_str()) {
                    Ok(normalized)
                } else {
                    Err(DomainError::validation_field(
                        "source",
                        format!(
                            "unknown effect source '{raw}'; expected one of {}",
                            EFFECT_SOURCE_KINDS.join(", ")
                        ),
                    ))
                }
            })
            .transpose()?;

        Ok(Self {
            category,
            source,
            // Unicode-aware on purpose: effect names, descriptions,
            // authors, and tags are free text, so an ASCII fold would
            // leave every non-ASCII uppercase letter unmatchable.
            search: search
                .map(str::trim)
                .filter(|term| !term.is_empty())
                .map(str::to_lowercase),
            ..Self::default()
        })
    }

    /// Whether one effect survives this narrowing.
    #[must_use]
    pub fn matches(&self, metadata: &EffectMetadata) -> bool {
        if self
            .category
            .is_some_and(|wanted| wanted != metadata.category)
        {
            return false;
        }
        if self
            .audio_reactive
            .is_some_and(|wanted| wanted != metadata.audio_reactive)
        {
            return false;
        }
        if self
            .screen_reactive
            .is_some_and(|wanted| wanted != metadata.screen_reactive)
        {
            return false;
        }
        if self
            .input_reactive
            .is_some_and(|wanted| wanted != metadata.input_reactive)
        {
            return false;
        }
        if self
            .source
            .as_deref()
            .is_some_and(|wanted| wanted != effect_source_kind(&metadata.source))
        {
            return false;
        }
        self.search
            .as_deref()
            .is_none_or(|term| effect_matches_search(metadata, term))
    }
}

/// The catalog, narrowed and ordered by name.
///
/// Ordering is case-insensitive with the raw name as the tiebreak, so
/// two effects differing only in case keep a stable relative order.
pub async fn list_catalog(ctx: &EffectContext, query: &EffectCatalogQuery) -> Vec<EffectMetadata> {
    let mut matched: Vec<EffectMetadata> = {
        let registry = ctx.registry.read().await;
        registry
            .iter()
            .map(|(_, entry)| &entry.metadata)
            .filter(|metadata| query.matches(metadata))
            .cloned()
            .collect()
    };
    matched.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    matched
}

/// The wire spelling of an effect's rendering source.
#[must_use]
pub fn effect_source_kind(source: &EffectSource) -> &'static str {
    match source {
        EffectSource::Native { .. } => "native",
        EffectSource::Html { .. } => "html",
        EffectSource::Shader { .. } => "shader",
    }
}

/// Every wire spelling [`effect_source_kind`] can produce.
pub const EFFECT_SOURCE_KINDS: [&str; 3] = ["native", "html", "shader"];

fn effect_matches_search(metadata: &EffectMetadata, term: &str) -> bool {
    metadata.name.to_lowercase().contains(term)
        || metadata.description.to_lowercase().contains(term)
        || metadata.author.to_lowercase().contains(term)
        || metadata
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(term))
}

pub(super) async fn effect_ref_index(ctx: &EffectContext) -> HashMap<EffectId, EffectRef> {
    let registry = ctx.registry.read().await;
    registry
        .iter()
        .map(|(id, entry)| (*id, effect_ref(&entry.metadata)))
        .collect()
}

/// Canonical event identity for an effect.
#[must_use]
pub fn effect_ref(metadata: &EffectMetadata) -> EffectRef {
    EffectRef {
        id: metadata.id.to_string(),
        name: metadata.name.clone(),
        engine: "servo".to_owned(),
    }
}

/// Validate and normalize typed control values against an effect schema.
#[must_use]
pub fn normalize_control_values(
    metadata: &EffectMetadata,
    control_values: &HashMap<String, ControlValue>,
) -> (HashMap<String, ControlValue>, Vec<String>) {
    let mut normalized = HashMap::new();
    let mut rejected = Vec::new();

    for (name, value) in control_values {
        let result = metadata.control_by_id(name).map_or_else(
            || Ok(value.clone()),
            |control| control.validate_value(value),
        );
        match result {
            Ok(control_value) => {
                normalized.insert(name.clone(), control_value);
            }
            Err(error) => rejected.push(format!("{name} ({error})")),
        }
    }

    (normalized, rejected)
}

/// Materialize an effect schema's default control set.
#[must_use]
pub fn default_control_values(metadata: &EffectMetadata) -> HashMap<String, ControlValue> {
    metadata
        .controls
        .iter()
        .map(|control| {
            (
                control.control_id().to_owned(),
                control.default_value.clone(),
            )
        })
        .collect()
}
