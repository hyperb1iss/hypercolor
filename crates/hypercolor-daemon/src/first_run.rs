//! Defaults a genuinely fresh install starts with.
//!
//! A new install would otherwise open on a black canvas and an empty
//! favorites list. Instead the default scene runs [`FIRST_RUN_SCENE_EFFECT`]
//! and the library favorites start with [`FIRST_RUN_FAVORITES`].
//!
//! Both seeds fire only on a fresh install, one where initialization found
//! no runtime session (at its state or legacy path), no named-scene store,
//! and no library store. Any earlier run that reached a scene change or a
//! clean shutdown left the session behind, so an upgraded install never sees
//! a seed, even one that never touched its favorites. A user who clears the
//! scene or removes every favorite rewrites those stores, so the choice
//! survives every restart.
//!
//! Picks are bundled effect file stems. A bundled effect's id hashes its
//! stem alone ([`bundled_html_effect_id`]), so a stem names the same effect on
//! every install and platform. Only files under a bundled effect root carry
//! that id; a user effect that shares the file name or the title has a
//! path-derived id instead. A pick this build lacks, or cannot render, is
//! skipped.

use std::collections::HashMap;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::effect::bundled_html_effect_id;
use hypercolor_types::effect::EffectCategory;
use hypercolor_types::event::{HypercolorEvent, LibraryChangeKind, LibraryCollection};
use tracing::{debug, info, warn};

use crate::api::library::unix_epoch_ms;
use crate::domain::MutationContext;
use crate::domain::effect::{
    ApplyEffect, EffectContext, RequestedTransition, ResolvedEffect, apply_effect,
};
use crate::library::LibraryStore;
use crate::resource_summary::is_runnable_source;

/// Bundled effect the default scene starts with on a fresh install.
pub const FIRST_RUN_SCENE_EFFECT: &str = "hyperbloom";

/// Bundled effects the library favorites start with, in the order clients
/// list them. Every pick looks good with no devices attached.
pub const FIRST_RUN_FAVORITES: &[&str] = &[
    "hyperbloom",
    "borealis",
    "bubble-garden",
    "plasma-engine",
    "spectral-fire",
];

/// Resolve a pick to an effect this build can render on LEDs.
async fn runnable_pick(effects: &EffectContext, stem: &str) -> Option<ResolvedEffect> {
    let effect = effects
        .metadata_for_mutation(bundled_html_effect_id(stem))
        .await?;
    (is_runnable_source(&effect.source) && effect.category != EffectCategory::Display)
        .then_some(effect)
}

/// Load [`FIRST_RUN_SCENE_EFFECT`] into the active scene's primary zone.
///
/// The apply takes the same domain path a client's apply does, so the scene
/// revision, events, and runtime-session save all happen as usual. The saved
/// session is what keeps a later start from seeding again.
pub(crate) async fn seed_scene(effects: &EffectContext) {
    if effects.active_primary_effect().await.is_some() {
        debug!("First-run scene seed skipped; the active scene already runs an effect");
        return;
    }
    let Some(effect) = runnable_pick(effects, FIRST_RUN_SCENE_EFFECT).await else {
        info!(
            effect = FIRST_RUN_SCENE_EFFECT,
            "First-run scene left empty; its effect is missing or cannot render in this build"
        );
        return;
    };

    let command = ApplyEffect {
        effect,
        controls: HashMap::new(),
        preset_id: None,
        target_zone: None,
        expected_revision: None,
        transition: RequestedTransition::cut(),
        // Starting up never changes the output power a user chose.
        wake_output: false,
    };
    match apply_effect(effects, command, MutationContext::system()).await {
        Ok(applied) => info!(
            effect = %applied.effect.name,
            zone = %applied.zone.name,
            "Seeded the default scene for a fresh install"
        ),
        Err(error) => warn!(
            effect = FIRST_RUN_SCENE_EFFECT,
            %error,
            "Failed to seed the default scene for a fresh install"
        ),
    }
}

/// Favorite every [`FIRST_RUN_FAVORITES`] pick this build can run.
///
/// Favorites list newest first, so each pick is stamped one millisecond
/// before the one above it in the table and clients show the table's order.
pub(crate) async fn seed_favorites(
    effects: &EffectContext,
    library: &dyn LibraryStore,
    event_bus: &HypercolorBus,
) {
    // Revisions survive a removed favorite, so this also catches a library
    // that saw any favorite change after it opened.
    if !library.project_favorites().await.is_empty() {
        debug!("First-run favorites seed skipped; the library already holds favorites");
        return;
    }

    let _admission = effects.admit_current().await;
    let newest = unix_epoch_ms();
    let mut seeded = Vec::new();
    let mut skipped = Vec::new();
    for (rank, stem) in (0_u64..).zip(FIRST_RUN_FAVORITES.iter().copied()) {
        let Some(effect) = runnable_pick(effects, stem).await else {
            skipped.push(stem);
            continue;
        };
        match library
            .upsert_favorite(effect.id, newest.saturating_sub(rank))
            .await
        {
            Ok(favorite) => {
                event_bus.publish(HypercolorEvent::LibraryStoreChanged {
                    collection: LibraryCollection::Favorites,
                    entry_id: favorite.effect_id.to_string(),
                    kind: LibraryChangeKind::Upserted,
                });
                seeded.push(stem);
            }
            Err(error) => {
                warn!(effect = stem, %error, "Failed to seed a first-run favorite");
            }
        }
    }

    if !skipped.is_empty() {
        info!(
            ?skipped,
            "First-run favorites skipped effects that are missing or cannot render in this build"
        );
    }
    if !seeded.is_empty() {
        info!(?seeded, "Seeded library favorites for a fresh install");
    }
}
