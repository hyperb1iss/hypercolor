//! Effect domain services (Spec 76 §2.2, §2.3).
//!
//! One `apply_effect` serves REST and MCP. Everything transport-shaped
//! stays in the adapters: REST resolves an id-or-name path segment and
//! parses a JSON transition object, MCP fuzzy-matches a natural-language
//! query and reads a `transition_ms` integer. Both arrive here with a
//! resolved [`EffectId`] and a [`TransitionRequest`], and both get the
//! same validation, the same scene mutation, and the same events in the
//! same order.

use std::collections::HashMap;

use hypercolor_types::api::effects::EffectLayoutApplyResult;
use hypercolor_types::effect::{ControlValue, EffectCategory, EffectId};
use hypercolor_types::event::{EffectRef, HypercolorEvent, ZoneChangeKind};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{SceneId, Zone, ZoneId};

use crate::api::AppState;
use crate::domain::commit::SceneCommit;
use crate::domain::scene::{commit_scene, zone_changed_event};
use crate::domain::{DomainError, MutationContext, ResourceKind};

/// A transition the caller asked for.
///
/// The daemon renders effect switches as immediate cuts today, so the
/// only request this surface can honor is a zero-duration cut. Anything
/// else is refused rather than accepted and quietly ignored — a caller
/// that asked for a crossfade must learn it did not get one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionRequest {
    /// Transition style, lowercased by the adapter. `None` means the
    /// caller named no style, which reads as a cut.
    pub style: Option<String>,
    /// Requested duration in milliseconds.
    pub duration_ms: u64,
}

impl TransitionRequest {
    /// The request a caller who said nothing about transitions makes.
    #[must_use]
    pub const fn cut() -> Self {
        Self {
            style: None,
            duration_ms: 0,
        }
    }

    /// A request carrying only a duration, as the MCP tools express it.
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
    /// The effect to load. Adapters resolve names and fuzzy queries.
    pub effect_id: EffectId,
    /// Control values, already normalized against the effect's schema.
    pub controls: HashMap<String, ControlValue>,
    /// Preset provenance to record on the zone.
    pub preset_id: Option<PresetId>,
    /// Which zone to load into. `None` targets the primary zone, which
    /// is created when the active scene has none.
    pub target_zone: Option<ZoneId>,
    /// The requested transition.
    pub transition: TransitionRequest,
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
    /// The layout the effect's association pulled in, primary only.
    pub applied_layout: Option<EffectLayoutApplyResult>,
    /// The transition the daemon applied.
    pub transition: AppliedTransition,
    /// The commit receipt.
    pub commit: SceneCommit,
}

/// Load an effect into the active scene and start rendering it.
///
/// # Errors
///
/// [`DomainError::NotFound`] for an unknown effect,
/// [`DomainError::Validation`] for a display face, an unimplemented
/// transition, or a zone that refuses the effect,
/// [`DomainError::Conflict`] when the active scene is snapshot-locked,
/// and [`DomainError::PreconditionFailed`] when a concurrent scene
/// mutation lands first.
pub async fn apply_effect(
    state: &AppState,
    command: ApplyEffect,
    meta: MutationContext,
) -> Result<EffectApplied, DomainError> {
    let transition = command.transition.resolve()?;

    let (metadata, effect_refs) = {
        let registry = state.effect_registry.read().await;
        let Some(entry) = registry.get(&command.effect_id) else {
            return Err(DomainError::not_found(
                ResourceKind::Effect,
                command.effect_id,
            ));
        };
        let metadata = entry.metadata.clone();
        // Resolving the outgoing effect's name needs the registry, and
        // the outgoing effect is not known until the scene snapshot is
        // in hand. Taking the index now keeps every await out of the
        // window between the snapshot and its compare-and-swap.
        let effect_refs = registry
            .iter()
            .map(|(id, entry)| (*id, crate::api::effects::effect_ref(&entry.metadata)))
            .collect::<HashMap<EffectId, EffectRef>>();
        (metadata, effect_refs)
    };

    if metadata.category == EffectCategory::Display {
        return Err(DomainError::validation(format!(
            "Effect '{}' is a display face and must be assigned to a display device, not applied to the LED pipeline",
            metadata.name
        )));
    }

    crate::api::effects::wake_output_for_effect_start(state).await;
    let layout = crate::api::effects::resolve_full_scope_layout(state).await;

    let mut mutation = state.begin_scene_mutation().await;
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
        )?;
        (zone, ZoneChangeKind::Updated)
    } else {
        let zone_change = if primary_zone_id.is_some() {
            ZoneChangeKind::Updated
        } else {
            ZoneChangeKind::Created
        };
        let zone =
            mutation.upsert_primary_zone(&metadata, command.controls, command.preset_id, layout)?;
        (zone, zone_change)
    };

    let effect = crate::api::effects::effect_ref(&metadata);
    mutation.record(HypercolorEvent::EffectStarted {
        effect: effect.clone(),
        trigger: meta.trigger,
        previous: previous_effect.clone(),
        transition: None,
        group_id: Some(zone.id),
        group_name: Some(zone.name.clone()),
    });
    mutation.record(zone_changed_event(scene_id, &zone, zone_change));

    let commit = commit_scene(state, mutation).await?;

    // A named zone keeps its own layout; only a primary apply adopts the
    // effect's associated one.
    let applied_layout = if named_target.is_some() {
        None
    } else {
        crate::api::effects::apply_associated_layout(state, &metadata.id.to_string()).await
    };
    crate::api::save_runtime_session_snapshot(state).await;

    Ok(EffectApplied {
        effect,
        scene_id,
        zone,
        zone_change,
        previous_effect,
        applied_layout,
        transition,
        commit,
    })
}
