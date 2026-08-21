//! Effect domain services (Spec 76 §2.2, §2.3).
//!
//! One `apply_effect` serves REST and MCP. Everything transport-shaped
//! stays in the adapters. Each adapter validates its wire contract and
//! resolves the effect through its canonical selector before entering
//! this module. Both arrive here with a resolved [`EffectId`] and a
//! [`RequestedTransition`], and both get the same validation, scene
//! mutation, and ordered events.

use std::collections::HashMap;
use std::str::FromStr;

use strum::VariantNames;

use hypercolor_types::api::scene::SideEffectOutcome;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::event::{EffectRef, ZoneChangeKind};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{SceneId, Zone, ZoneId};

use crate::api::AppState;
use crate::domain::commit::SceneCommit;
use crate::domain::scene::commit_scene;
use crate::domain::{DomainError, MutationContext};

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
    /// The effect to load, already resolved.
    ///
    /// Adapters own identity resolution and hand over what they found.
    /// REST resolves its path segment while MCP applies the deterministic
    /// selector policy. The service
    /// does not look it up again: re-resolving would reintroduce a
    /// window where a rescan between the adapter's lookup and the
    /// service's turns a request the adapter already accepted into a
    /// not-found, and it would give the domain a second opinion about
    /// identity that the adapter contract deliberately owns.
    pub effect: EffectMetadata,
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
    state: &AppState,
    command: ApplyEffect,
    meta: MutationContext,
) -> Result<EffectApplied, DomainError> {
    let transition = command.transition.resolve()?;
    let metadata = command.effect;

    // Resolving the outgoing effect's name needs the registry, and the
    // outgoing effect is not known until the scene snapshot is in hand.
    // Taking the index now keeps every await out of the window between
    // the snapshot and its compare-and-swap.
    let effect_refs = {
        let registry = state.effect_registry.read().await;
        registry
            .iter()
            .map(|(id, entry)| (*id, crate::api::effects::effect_ref(&entry.metadata)))
            .collect::<HashMap<EffectId, EffectRef>>()
    };

    if metadata.category == EffectCategory::Display {
        return Err(DomainError::validation(format!(
            "Effect '{}' is a display face and must be assigned to a display device, not applied to the LED pipeline",
            metadata.name
        )));
    }

    let layout = crate::api::effects::resolve_full_scope_layout(state).await;

    let mut mutation = state.scene_manager.begin_mutation().await;
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

    let effect = crate::api::effects::effect_ref(&metadata);

    let commit = commit_scene(state, mutation).await?;

    // Every refusal above returns before this point, so nothing the
    // caller can get rejected for has woken output (Spec 78 §2.3).
    let output =
        if command.wake_output && !crate::api::effects::wake_output_for_effect_start(state).await {
            SideEffectOutcome::failed("output did not resume; patch /output to retry")
        } else {
            SideEffectOutcome::applied()
        };

    crate::api::save_runtime_session_snapshot(state).await;

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
pub async fn invalidate_active_zones(state: &AppState) -> Result<SceneCommit, DomainError> {
    // A dropped invalidation leaves the active scene's resolved zones
    // pointing at pre-reload effect metadata until something else
    // invalidates, so this reconciliation retries rather than losing.
    let ((), commit) = crate::domain::scene::commit_retrying(state, |mutation| {
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
pub async fn list_catalog(state: &AppState, query: &EffectCatalogQuery) -> Vec<EffectMetadata> {
    let mut matched: Vec<EffectMetadata> = {
        let registry = state.effect_registry.read().await;
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

pub(super) async fn effect_ref_index(state: &AppState) -> HashMap<EffectId, EffectRef> {
    let registry = state.effect_registry.read().await;
    registry
        .iter()
        .map(|(id, entry)| (*id, crate::api::effects::effect_ref(&entry.metadata)))
        .collect()
}
