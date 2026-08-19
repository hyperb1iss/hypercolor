//! Layer-stack domain services (Spec 76 §2.2, §2.3).
//!
//! Every zone owns an ordered layer stack, and each of these six
//! transactions moves it: insert, batch insert, update, remove, reorder,
//! and control patch. All of them are persisted scene content guarded by
//! a `layers_version` precondition, and all of them publish the same
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

use hypercolor_core::scene::{LayerMutationError, SceneGroupLayerInsert};
use hypercolor_types::effect::ControlValue;
use hypercolor_types::event::{HypercolorEvent, LayerStackChangeKind, ZoneChangeKind};
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{SceneId, Zone, ZoneId};

use crate::api::AppState;
use crate::domain::commit::SceneCommit;
use crate::domain::scene::{SceneMutation, SceneTarget, commit_scene, zone_changed_event};
use crate::domain::{DomainError, MutationContext};

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
    state: &AppState,
    target: SceneTarget,
    zone_id: ZoneId,
    layer: SceneLayer,
    index: Option<usize>,
    expected_version: Option<u64>,
    expected_scene_revision: Option<u64>,
    meta: MutationContext,
) -> Result<LayerResult, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    let scene_id = target.resolve(&mutation, "creating a layer")?;
    crate::domain::scene_tree::check_scene_revision(&mutation, expected_scene_revision)?;
    let zone = match mutation.insert_layer(scene_id, zone_id, layer, index, expected_version) {
        Ok(zone) => zone,
        Err(refusal) => return Ok(Err(refusal)),
    };
    if target.is_active() {
        validate_candidate_media_admission(state, &mutation, scene_id).await?;
    }
    finish(
        state,
        mutation,
        scene_id,
        vec![zone],
        LayerStackChangeKind::Created,
    )
    .await
}

/// Insert one layer into each of several zones, all or nothing.
///
/// # Errors
///
/// As [`insert_layer`].
pub async fn insert_layers(
    state: &AppState,
    scene_id: SceneId,
    inserts: Vec<SceneGroupLayerInsert>,
    meta: MutationContext,
) -> Result<LayerResult, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    let zones = match mutation.insert_layers(scene_id, inserts) {
        Ok(zones) => zones,
        Err(refusal) => return Ok(Err(refusal)),
    };
    finish(
        state,
        mutation,
        scene_id,
        zones,
        LayerStackChangeKind::Created,
    )
    .await
}

/// Replace one layer's definition in place.
///
/// # Errors
///
/// As [`insert_layer`].
pub async fn update_layer(
    state: &AppState,
    scene_id: SceneId,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
    layer: SceneLayer,
    expected_version: Option<u64>,
    expected_scene_revision: Option<u64>,
    meta: MutationContext,
) -> Result<LayerResult, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    crate::domain::scene_tree::check_scene_revision(&mutation, expected_scene_revision)?;
    let zone = match mutation.update_layer(scene_id, zone_id, layer_id, layer, expected_version) {
        Ok(zone) => zone,
        Err(refusal) => return Ok(Err(refusal)),
    };
    finish(
        state,
        mutation,
        scene_id,
        vec![zone],
        LayerStackChangeKind::Updated,
    )
    .await
}

/// Drop one layer out of a zone's stack.
///
/// # Errors
///
/// As [`insert_layer`].
pub async fn remove_layer(
    state: &AppState,
    target: SceneTarget,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
    expected_version: Option<u64>,
    expected_scene_revision: Option<u64>,
    meta: MutationContext,
) -> Result<LayerResult, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    let scene_id = target.resolve(&mutation, "deleting a layer")?;
    crate::domain::scene_tree::check_scene_revision(&mutation, expected_scene_revision)?;
    let zone = match mutation.remove_layer(scene_id, zone_id, layer_id, expected_version) {
        Ok(zone) => zone,
        Err(refusal) => return Ok(Err(refusal)),
    };
    finish(
        state,
        mutation,
        scene_id,
        vec![zone],
        LayerStackChangeKind::Removed,
    )
    .await
}

/// Rewrite a zone's layer order.
///
/// # Errors
///
/// As [`insert_layer`].
pub async fn reorder_layers(
    state: &AppState,
    target: SceneTarget,
    zone_id: ZoneId,
    layer_ids: Vec<SceneLayerId>,
    expected_version: Option<u64>,
    expected_scene_revision: Option<u64>,
    meta: MutationContext,
) -> Result<LayerResult, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    let scene_id = target.resolve(&mutation, "reordering layers")?;
    crate::domain::scene_tree::check_scene_revision(&mutation, expected_scene_revision)?;
    let zone = match mutation.reorder_layers(scene_id, zone_id, layer_ids, expected_version) {
        Ok(zone) => zone,
        Err(refusal) => return Ok(Err(refusal)),
    };
    finish(
        state,
        mutation,
        scene_id,
        vec![zone],
        LayerStackChangeKind::Reordered,
    )
    .await
}

pub(crate) async fn validate_candidate_media_admission(
    state: &AppState,
    mutation: &SceneMutation,
    scene_id: SceneId,
) -> Result<(), DomainError> {
    let asset_mime_types = crate::api::scenes::asset_mime_types(state).await;
    let media_config = crate::api::scenes::current_media_config(state);
    let scene = mutation
        .scenes()
        .get(&scene_id)
        .ok_or_else(|| DomainError::not_found(crate::domain::ResourceKind::Scene, scene_id))?;
    crate::domain::scene::validate_scene_media_admission(scene, &asset_mime_types, &media_config)
}

/// Merge control overrides into one effect layer.
///
/// # Errors
///
/// As [`insert_layer`].
pub async fn patch_layer_controls(
    state: &AppState,
    scene_id: SceneId,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
    controls: HashMap<String, ControlValue>,
    expected_version: Option<u64>,
    expected_scene_revision: Option<u64>,
    meta: MutationContext,
) -> Result<LayerResult, DomainError> {
    let _ = meta;

    let mut mutation = state.begin_scene_mutation().await;
    crate::domain::scene_tree::check_scene_revision(&mutation, expected_scene_revision)?;
    let zone = match mutation.patch_layer_controls(
        scene_id,
        zone_id,
        layer_id,
        controls,
        expected_version,
    ) {
        Ok(zone) => zone,
        Err(refusal) => return Ok(Err(refusal)),
    };
    finish(
        state,
        mutation,
        scene_id,
        vec![zone],
        LayerStackChangeKind::ControlsPatched,
    )
    .await
}

/// Record both events every layer mutation publishes, commit, and record
/// the new stack in the session snapshot.
async fn finish(
    state: &AppState,
    mut mutation: SceneMutation,
    scene_id: SceneId,
    zones: Vec<Zone>,
    kind: LayerStackChangeKind,
) -> Result<LayerResult, DomainError> {
    for zone in &zones {
        mutation.record(zone_changed_event(scene_id, zone, zone_change_kind(kind)));
        mutation.record(HypercolorEvent::LayerStackChanged {
            scene_id,
            zone_id: zone.id,
            layers_version: zone.layers_version,
            kind,
        });
    }

    let commit = commit_scene(state, mutation).await?;
    crate::api::save_runtime_session_snapshot(state).await;

    Ok(Ok(LayerStackWritten { zones, commit }))
}

/// How the compositor reads a layer-stack change.
const fn zone_change_kind(kind: LayerStackChangeKind) -> ZoneChangeKind {
    match kind {
        LayerStackChangeKind::ControlsPatched => ZoneChangeKind::ControlsPatched,
        LayerStackChangeKind::Created
        | LayerStackChangeKind::Updated
        | LayerStackChangeKind::Removed
        | LayerStackChangeKind::Reordered => ZoneChangeKind::Updated,
    }
}
