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
//! live one, installs the candidate, admits the snapshot bytes, and
//! releases the lock — all without an await inside the guard. Only then
//! does it persist and publish. `Err` therefore means one thing and one
//! thing only: the mutation was rejected *before* admission. Everything
//! that can happen after admission is a [`CommitDurability`] on the
//! returned [`SceneCommit`], because after admission the retry
//! supervisor owns the bytes and the mutation is going to land.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::scene::SceneManager;
use hypercolor_types::asset::AssetId;
use hypercolor_types::config::MediaConfig;
use hypercolor_types::effect::{ControlValue, EffectMetadata};
use hypercolor_types::event::{HypercolorEvent, SceneChangeReason, ZoneChangeKind};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{Scene, SceneId, TransitionSpec, Zone, ZoneId};
use hypercolor_types::spatial::SpatialLayout;

use crate::api::AppState;
use crate::api::scenes::MediaAdmissionViolationDetails;
use crate::domain::commit::{CommitDurability, SceneCommit, SceneRevision};
use crate::domain::{DomainError, MutationContext, ResourceKind};
use crate::persistence::AtomicWriteOutcome;

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
}

impl SceneMutation {
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

    /// Record an event for ordered publication at commit time.
    ///
    /// Events never publish from a mutation that is dropped, and never
    /// publish out of admission order.
    pub fn record(&mut self, event: HypercolorEvent) {
        self.events.push(event);
    }

    /// The active scene's id, refusing scenes that forbid runtime
    /// rewriting.
    ///
    /// Snapshot scenes are a deliberate user choice: runtime effect and
    /// face actions must not silently edit them.
    pub fn active_scene_for_runtime_mutation(&self, action: &str) -> Result<SceneId, DomainError> {
        let active = self
            .candidate
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

    /// The active scene's primary zone id, when it has one.
    #[must_use]
    pub fn primary_zone_id(&self) -> Option<ZoneId> {
        self.candidate
            .active_scene()
            .and_then(Scene::primary_group)
            .map(|zone| zone.id)
    }

    /// The effect currently loaded in one of the active scene's zones.
    #[must_use]
    pub fn zone_effect(&self, zone_id: ZoneId) -> Option<hypercolor_types::effect::EffectId> {
        self.candidate
            .active_scene()?
            .groups
            .iter()
            .find(|zone| zone.id == zone_id)
            .and_then(|zone| zone.effect_id)
    }

    /// Load an effect into the active scene's primary zone, creating the
    /// zone when the scene has none.
    pub fn upsert_primary_zone(
        &mut self,
        metadata: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        preset_id: Option<PresetId>,
        layout: SpatialLayout,
    ) -> Result<Zone, DomainError> {
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
        Ok(zone)
    }

    /// Load an effect into a named zone, which keeps its own layout.
    pub fn apply_effect_to_zone(
        &mut self,
        zone_id: ZoneId,
        metadata: &EffectMetadata,
        controls: HashMap<String, ControlValue>,
        preset_id: Option<PresetId>,
    ) -> Result<Zone, DomainError> {
        let zone = self
            .candidate
            .apply_effect_to_group(zone_id, metadata, controls, preset_id)
            .map_err(|error| {
                DomainError::validation(format!("Failed to apply effect to zone: {error}"))
            })?
            .clone();
        self.persists_scene_content = true;
        Ok(zone)
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
    ) -> Result<(), DomainError> {
        self.candidate
            .activate(&scene_id, transition)
            .map_err(|error| {
                DomainError::Internal(anyhow::anyhow!("Failed to activate scene: {error}"))
            })
    }
}

impl AppState {
    /// Snapshot the live scene state into an owned candidate.
    ///
    /// The read lock is held for exactly one clone. Everything the
    /// caller does afterwards happens on its own copy.
    pub async fn begin_scene_mutation(&self) -> SceneMutation {
        let manager = self.scene_manager.read().await;
        let base_revision = self.scene_commits.revision();
        SceneMutation {
            candidate: manager.clone(),
            base_revision,
            events: Vec::new(),
            persists_scene_content: false,
        }
    }
}

// ── Commit ───────────────────────────────────────────────────────────────

/// Install a candidate, admit its snapshot, then persist and publish.
///
/// The compare-and-swap on the base revision is what makes the short
/// lock scopes safe: a candidate built from a revision that no longer
/// exists would silently overwrite whatever landed in between, so it is
/// refused with the current revision attached instead.
///
/// # Errors
///
/// Only pre-admission rejections: a stale base revision
/// ([`DomainError::PreconditionFailed`]) or a snapshot that will not
/// serialize ([`DomainError::Internal`]). Once the bytes are admitted
/// the mutation is committed, and where they ended up is reported by
/// [`SceneCommit::durability`].
pub async fn commit_scene(
    state: &AppState,
    mutation: SceneMutation,
) -> Result<SceneCommit, DomainError> {
    let SceneMutation {
        candidate,
        base_revision,
        events,
        persists_scene_content,
    } = mutation;

    let coordinator = state.scene_store.read().await.clone();

    let (ticket, pending) = {
        let mut manager = state.scene_manager.write().await;
        let current_revision = state.scene_commits.revision();
        if current_revision != base_revision {
            return Err(DomainError::PreconditionFailed {
                resource: ResourceKind::Scene,
                expected: base_revision,
                current: current_revision,
            });
        }

        let previous = std::mem::replace(&mut *manager, candidate);
        let pending = if persists_scene_content {
            match coordinator.reserve_save(manager.list().into_iter().cloned()) {
                Ok(pending) => Some(pending),
                Err(error) => {
                    // Serialization failed, so nothing was admitted and
                    // the candidate never happened.
                    *manager = previous;
                    return Err(DomainError::Internal(anyhow::anyhow!(
                        "Failed to persist scene: {error}"
                    )));
                }
            }
        } else {
            None
        };

        // The generation is assigned under the same guard that installed
        // the candidate, so admission order and revision order agree.
        let ticket = state.scene_commits.admit(Arc::clone(&state.event_bus));
        (ticket, pending)
    };

    let generation = ticket.generation();

    let Some(pending) = pending else {
        // Nothing persisted scene content, so there is nothing that
        // could be superseded or retried.
        ticket.release(events);
        return Ok(SceneCommit::new(
            generation,
            generation,
            CommitDurability::Written,
            None,
        ));
    };

    let outcome = state.scene_store.write().await.save_reserved(pending);
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
            // A newer generation owns the destination. Its payload
            // already contains this commit's changes and its own
            // publication is what subscribers should see, so this
            // commit's announcement would only walk state backwards.
            ticket.discard();
            Ok(SceneCommit::new(
                generation,
                generation,
                CommitDurability::Superseded,
                None,
            ))
        }
        Err(error) => {
            // The bytes stay the destination's newest admitted intent
            // and the supervisor converges on them, but this attempt
            // did not prove durable — adapters keep rendering that the
            // way their frozen wire always has.
            let message = format!("{error}");
            ticket.discard();
            Ok(SceneCommit::new(
                generation,
                generation,
                CommitDurability::Retrying,
                Some(message),
            ))
        }
    }
}

// ── Scene media admission ────────────────────────────────────────────────

/// What activating a scene would cost the compositor, and whether it
/// exceeds the hard producer caps.
#[derive(Debug)]
pub struct SceneMediaAdmission {
    /// Estimated per-frame producer cost in microseconds.
    pub estimated_cost_us: u64,
    /// The hard-cap violation, when the scene has one.
    pub violation: Option<MediaAdmissionViolationDetails>,
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
    let counts = crate::api::scenes::scene_media_admission_counts(scene, asset_mime_types);
    SceneMediaAdmission {
        estimated_cost_us: counts.estimated_cost_us(),
        violation: crate::api::scenes::scene_media_admission_violation_details(
            &counts,
            media_config,
        ),
    }
}

// ── activate_scene ───────────────────────────────────────────────────────

/// Make a scene the exclusive current one.
#[derive(Debug, Clone)]
pub struct ActivateScene {
    /// Which scene to activate. Adapters resolve names to ids.
    pub scene_id: SceneId,
    /// Overrides the scene's own transition spec when present.
    pub transition: Option<TransitionSpec>,
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
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// Activate a scene: validate its media admission, switch the exclusive
/// current scene, apply soft admission, then persist and reconcile
/// connectivity.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown scene,
/// [`DomainError::Validation`] when the scene exceeds its hard media
/// producer caps, and [`DomainError::PreconditionFailed`] when a
/// concurrent scene mutation lands first.
pub async fn activate_scene(
    state: &AppState,
    command: ActivateScene,
    meta: MutationContext,
) -> Result<SceneActivated, DomainError> {
    let _ = meta;

    let asset_mime_types = crate::api::scenes::asset_mime_types(state).await;
    let media_config = crate::api::scenes::current_media_config(state);

    let mut mutation = state.begin_scene_mutation().await;
    let previous_scene_id = mutation.scenes().active_scene_id().copied();

    let scene = mutation
        .scenes()
        .get(&command.scene_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, command.scene_id))?;
    let scene_name = scene.name.clone();
    let admission = evaluate_scene_media_admission(scene, &asset_mime_types, &media_config);
    if let Some(message) = admission.rejection_message() {
        return Err(DomainError::validation(message.to_owned()));
    }

    mutation.activate(command.scene_id, command.transition)?;

    let current_scene = mutation.scenes().active_scene().cloned();
    if previous_scene_id != current_scene.as_ref().map(|scene| scene.id)
        && let Some(current) = current_scene.as_ref()
    {
        mutation.record(HypercolorEvent::ActiveSceneChanged {
            previous: previous_scene_id,
            current: current.id,
            current_name: current.name.clone(),
            current_kind: current.kind,
            current_mutation_mode: current.mutation_mode,
            current_snapshot_locked: current.blocks_runtime_mutation(),
            reason: SceneChangeReason::UserActivate,
        });
    }

    let commit = commit_scene(state, mutation).await?;

    crate::api::scenes::apply_scene_media_soft_admission(
        state,
        command.scene_id,
        &scene_name,
        admission.estimated_cost_us,
    )
    .await;
    crate::api::save_runtime_session_snapshot(state).await;

    // Which scene is active decides which devices are worth connecting.
    crate::api::sync_connectivity(state).await;

    Ok(SceneActivated {
        scene_id: command.scene_id,
        scene_name,
        previous_scene_id,
        estimated_cost_us: admission.estimated_cost_us,
        commit,
    })
}

// ── Shared event helpers ─────────────────────────────────────────────────

/// The zone-changed event both effect-apply paths record.
#[must_use]
pub fn zone_changed_event(scene_id: SceneId, zone: &Zone, kind: ZoneChangeKind) -> HypercolorEvent {
    HypercolorEvent::RenderGroupChanged {
        scene_id,
        group_id: zone.id,
        role: zone.role,
        kind,
    }
}
