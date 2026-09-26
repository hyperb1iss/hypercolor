//! Exact, revision-fenced lighting mutations for native domain consumers.

use hypercolor_color::Rgb;
use hypercolor_types::scene::{SceneId, SceneKind, Zone, ZoneId};

use super::DomainError;
use super::commit::{CommitDurability, SceneCommit};
use super::context::{RuntimeSessionSaveOutcome, SceneContext};
use crate::persistence::AtomicWriteOutcome;

/// A zone observed together with its active scene and scene commit revision.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeZoneTarget {
    /// Immutable active scene identity.
    pub scene_id: SceneId,
    /// Immutable zone identity within the scene.
    pub zone_id: ZoneId,
    /// Revision captured from the same owned scene snapshot.
    pub revision: u64,
}

/// Original scene admission and the separate runtime projection save attempt.
///
/// Named scenes persist through the scene store; default-scene layers persist
/// through the runtime projection. Neither Superseded nor a newer unrelated
/// snapshot proves that this operation's layers were written or remain current.
#[derive(Debug)]
pub struct RuntimeZoneColorOutcome {
    /// Exact identity and observed revision used for admission.
    pub target: RuntimeZoneTarget,
    /// Original scene kind, which determines whether the scene store owns it.
    pub scene_kind: SceneKind,
    /// The admitted zone, not a subsequent read of a potentially newer tree.
    pub zone: Zone,
    /// Original scene commit and its durability evidence.
    pub commit: SceneCommit,
    /// Actual runtime projection persistence attempt after admission.
    pub runtime_session: RuntimeSessionSaveOutcome,
}

impl RuntimeZoneColorOutcome {
    /// Whether an actual Written payload contains this operation's layers.
    ///
    /// A named scene uses its original scene-store commit. A default scene
    /// requires the exact layer identities and content in a Written runtime
    /// projection. This proves persistence, not current output or device apply.
    #[must_use]
    pub fn has_written_layers(&self) -> bool {
        if !self.target.scene_id.is_default() {
            return self.scene_kind == SceneKind::Named
                && self.commit.durability() == CommitDurability::Written;
        }
        matches!(&self.runtime_session,
            RuntimeSessionSaveOutcome::Attempted {
                projection,
                snapshot: Ok(AtomicWriteOutcome::Written),
                ..
            } if projection.default_scene_zones.iter().any(|zone|
                zone.id == self.zone.id && zone.layers == self.zone.layers)
        )
    }
}

/// Replace only the selected live lighting zone's layers with an opaque color.
///
/// RGB is encoded sRGB, explicitly linearized for the engine's ColorFill source.
/// The caller owns authorization; this boundary owns scene/zone/revision and
/// role validation. No scene activation, output wake or brightness write occurs.
///
/// # Errors
///
/// Returns target validation or commit-CAS errors before admission. Persistence
/// failures after scene admission remain visible in the returned outcomes.
pub async fn set_color(
    ctx: &SceneContext,
    target: RuntimeZoneTarget,
    color: Rgb,
) -> Result<RuntimeZoneColorOutcome, DomainError> {
    let mut mutation = ctx.begin_mutation().await;
    let zone = mutation.set_runtime_zone_color(target, color)?;
    let scene_kind = mutation
        .scenes()
        .get(&target.scene_id)
        .ok_or_else(|| DomainError::not_found(super::ResourceKind::Scene, target.scene_id))?
        .kind;
    let commit = ctx.commit(mutation).await?;
    let runtime_session = ctx.save_runtime_session_with_outcome().await;
    Ok(RuntimeZoneColorOutcome {
        target,
        scene_kind,
        zone,
        commit,
        runtime_session,
    })
}
