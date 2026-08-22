//! Layer-stack domain services (Spec 76 §2.2, §2.3).
//!
//! Every zone owns an ordered layer stack, and each of these six
//! transactions moves it: insert, remove, reorder, and control patch.
//! All of them are persisted scene content guarded by the scene revision,
//! and all of them publish the same
//! pair of events — the zone change the compositor reacts to, and the
//! layer-stack change the Studio panel reacts to.
//!
//! A refusal the stack itself raises comes back as the inner `Err`
//! rather than as a [`DomainError`], because [`LayerMutationError`]
//! carries structure the canonical error set cannot express — the
//! per-field validation errors of a rejected layer payload, and the
//! index and length of an out-of-range insertion. Those stay domain
//! vocabulary and each transport renders them; the outer `Err` is
//! reserved for the errors every scene mutation shares.

use std::collections::HashMap;

use hypercolor_core::scene::LayerMutationError;
use hypercolor_types::effect::ControlValue;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{SceneId, Zone, ZoneId};

use crate::domain::commit::SceneCommit;
use crate::domain::context::SceneContext;
use crate::domain::effect::EffectContext;
use crate::domain::scene::SceneMutation;
use crate::domain::{DomainError, ResourceKind};

/// The outcome of a layer-stack mutation.
#[derive(Debug)]
pub struct LayerStackWritten {
    /// Every zone the mutation touched, in the order it touched them.
    pub zones: Vec<Zone>,
    /// The commit receipt.
    pub commit: SceneCommit,
}

impl LayerStackWritten {
    /// The single zone a one-zone mutation reports.
    ///
    /// # Panics
    ///
    /// Never in practice: only the batch insert produces more than one
    /// zone, and every mutation produces at least one.
    #[must_use]
    pub fn zone(&self) -> &Zone {
        self.zones.first().expect("a mutation touches one zone")
    }
}

/// A layer-stack mutation that either lands or is refused by the stack.
pub type LayerResult = Result<LayerStackWritten, LayerMutationError>;

/// Insert a layer into a zone's stack.
///
/// # Errors
///
/// [`DomainError::Conflict`] when a concurrent scene mutation
/// lands first.
pub async fn insert_layer(
    effects: &EffectContext,
    zone_id: ZoneId,
    layer: SceneLayer,
    index: Option<usize>,
    expected_revision: Option<u64>,
) -> Result<LayerResult, DomainError> {
    let _effect_admission = effects
        .admit_layer_sources(std::iter::once(&layer.source))
        .await?;
    let ctx = effects.scene_context();
    let media_admission = ctx.media_admission_for_layer(&layer).await;
    let mut mutation = ctx.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("creating a layer")?;
    crate::domain::scene_tree::check_scene_revision(&mutation, expected_revision)?;
    crate::domain::scene_tree::ensure_live_zone_mutable(&mutation, zone_id)?;
    let zone = match mutation.insert_layer(scene_id, zone_id, layer, index, None) {
        Ok(zone) => zone,
        Err(refusal) => return Ok(Err(refusal)),
    };
    if let Some(media_admission) = media_admission {
        let scene = mutation
            .scenes()
            .get(&scene_id)
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, scene_id))?;
        media_admission.validate(scene)?;
    }
    finish(ctx, mutation, vec![zone]).await
}

/// Drop one layer out of a zone's stack.
///
/// # Errors
///
/// As [`insert_layer`].
pub async fn remove_layer(
    ctx: &SceneContext,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
    expected_revision: Option<u64>,
) -> Result<LayerResult, DomainError> {
    let mut mutation = ctx.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("deleting a layer")?;
    crate::domain::scene_tree::check_scene_revision(&mutation, expected_revision)?;
    crate::domain::scene_tree::ensure_live_zone_mutable(&mutation, zone_id)?;
    let zone = match mutation.remove_layer(scene_id, zone_id, layer_id, None) {
        Ok(zone) => zone,
        Err(refusal) => return Ok(Err(refusal)),
    };
    finish(ctx, mutation, vec![zone]).await
}

/// Rewrite a zone's layer order.
///
/// # Errors
///
/// As [`insert_layer`].
pub async fn reorder_layers(
    ctx: &SceneContext,
    zone_id: ZoneId,
    layer_ids: Vec<SceneLayerId>,
    expected_revision: Option<u64>,
) -> Result<LayerResult, DomainError> {
    let mut mutation = ctx.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("reordering layers")?;
    crate::domain::scene_tree::check_scene_revision(&mutation, expected_revision)?;
    crate::domain::scene_tree::ensure_live_zone_mutable(&mutation, zone_id)?;
    let zone = match mutation.reorder_layers(scene_id, zone_id, layer_ids, None) {
        Ok(zone) => zone,
        Err(refusal) => return Ok(Err(refusal)),
    };
    finish(ctx, mutation, vec![zone]).await
}

pub(crate) async fn validate_candidate_media_admission(
    ctx: &SceneContext,
    mutation: &SceneMutation,
    scene_id: SceneId,
) -> Result<(), DomainError> {
    let scene = mutation
        .scenes()
        .get(&scene_id)
        .ok_or_else(|| DomainError::not_found(crate::domain::ResourceKind::Scene, scene_id))?;
    ctx.media_admission_context().await.validate(scene)
}

/// Merge control overrides into one effect layer.
///
/// # Errors
///
/// As [`insert_layer`].
pub async fn patch_layer_controls(
    ctx: &SceneContext,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
    controls: HashMap<String, ControlValue>,
    expected_revision: Option<u64>,
) -> Result<LayerResult, DomainError> {
    let mut mutation = ctx.begin_mutation().await;
    let scene_id = mutation.active_scene_for_runtime_mutation("patching layer controls")?;
    crate::domain::scene_tree::check_scene_revision(&mutation, expected_revision)?;
    crate::domain::scene_tree::ensure_live_zone_mutable(&mutation, zone_id)?;
    let zone = match mutation.patch_layer_controls(scene_id, zone_id, layer_id, controls, None) {
        Ok(zone) => zone,
        Err(refusal) => return Ok(Err(refusal)),
    };
    finish(ctx, mutation, vec![zone]).await
}

/// Record both events every layer mutation publishes, commit, and record
/// the new stack in the session snapshot.
async fn finish(
    ctx: &SceneContext,
    mutation: SceneMutation,
    zones: Vec<Zone>,
) -> Result<LayerResult, DomainError> {
    let commit = ctx.commit(mutation).await?;
    ctx.save_runtime_session().await;

    Ok(Ok(LayerStackWritten { zones, commit }))
}
