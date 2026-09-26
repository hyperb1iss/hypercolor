//! Explicit scene activation fields and original admission evidence.

use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::Scene;

use super::{DomainError, ResourceKind};
use crate::runtime_state::RuntimeSessionSnapshot;
use std::sync::{Arc, Mutex};

/// Original runtime projection write result, without later flush inference.
#[derive(Debug)]
pub enum ProjectionDurability {
    /// The supplied payload was written durably.
    Written,
    /// Another admitted payload superseded this write.
    Superseded,
    /// The writer could not admit this payload.
    BeforeAdmission(String),
    /// Admission occurred but durable completion remains unresolved.
    Retrying(String),
}

/// Exact payload and original outcome at the owning runtime writer.
#[derive(Debug)]
pub struct ProjectionWriteEvidence {
    /// None only when preparation failed before a payload was observed.
    pub projection: Option<RuntimeSessionSnapshot>,
    /// Original outcome, never inferred from a later current snapshot.
    pub durability: ProjectionDurability,
}

/// A layout attempt includes candidate writes even when live publication fails.
#[derive(Debug)]
pub struct SelectedLayoutOutcome {
    /// Exact selected named layout, not a later catalog read.
    pub layout: hypercolor_types::spatial::SpatialLayout,
    /// Live publication result; failure does not erase precommit writes.
    pub publication: Result<(), String>,
    /// Precommit followed by any rollback writes, in actual execution order.
    pub writes: Vec<ProjectionWriteEvidence>,
    /// Scene captured under the successful publication lock.
    pub admitted_scene: Option<ObservedScene>,
}

/// Surviving authored fields of one immutable scene activation intent.
#[derive(Debug, Clone)]
pub struct SelectedSceneFields {
    /// Owned target definition and source revision.
    pub expected: ObservedScene,
    /// Exact transition; only duration may override the authored transition.
    pub context: Option<hypercolor_types::scene::TransitionSpec>,
    /// Complete observed named layout; None preserves current layout.
    pub layout: Option<hypercolor_types::spatial::SpatialLayout>,
    /// Explicit authored global brightness; None preserves brightness and power.
    pub brightness: Option<f32>,
}

/// Independent results retain admission evidence when a later field fails.
#[derive(Debug)]
pub struct SelectedSceneOutcome {
    /// Original context commit, absent when context was omitted.
    pub context: Option<super::commit::SceneCommit>,
    /// Layout publication plus escaped precommit and rollback evidence.
    pub layout: Option<SelectedLayoutOutcome>,
    /// Brightness admission or its refusal, absent when omitted.
    pub brightness: Option<Result<SelectedBrightnessOutcome, DomainError>>,
    /// Exact runtime save after selected context/connectivity changes.
    pub runtime_session: Option<super::context::RuntimeSessionSaveOutcome>,
}

/// Apply only explicitly selected fields of the observed authored scene.
///
/// The caller owns authorization. Existing activation, layout publication and
/// output guards own engine admission.
/// No implicit wake occurs. Partial results never imply mutation rollback.
///
/// # Errors
/// Returns preflight or context-CAS refusal before any field was admitted.
/// Later layout/brightness failures are retained in the returned field results.
pub async fn activate_selected_fields(
    ctx: &super::scene::SceneLibraryContext,
    command: SelectedSceneFields,
) -> Result<SelectedSceneOutcome, DomainError> {
    use hypercolor_types::event::SceneChangeReason;
    if command.context.is_none() && command.layout.is_none() && command.brightness.is_none() {
        return Err(DomainError::validation("Select at least one scene field"));
    }
    if let Some(brightness) = command.brightness
        && (!(0.0..=1.0).contains(&brightness)
            || command.expected.scene.activation_brightness != Some(brightness))
    {
        return Err(DomainError::validation(
            "Selected brightness must match the observed scene",
        ));
    }
    if let Some(transition) = &command.context {
        let mut authored = command.expected.scene.transition.clone();
        authored.duration_ms = transition.duration_ms;
        if &authored != transition {
            return Err(DomainError::validation(
                "Selected transition must preserve the authored transition policy",
            ));
        }
    }
    let media = ctx.scene.media_admission_context().await;
    let display = ctx
        .layout
        .connected_display_surface_layouts(ctx.scene.layout_runtime())
        .await;
    let _activation = if command.context.is_some() {
        Some(ctx.layout.acquire_scene_activation_guard().await)
    } else {
        None
    };
    let layout_guard = if command.context.is_some() || command.layout.is_some() {
        Some(ctx.layout.acquire_update_guard().await)
    } else {
        None
    };
    let mut mutation = ctx.scene.begin_mutation().await;
    command
        .expected
        .validate(mutation.scenes(), mutation.base_revision())?;
    if let Some(layout) = &command.layout {
        let layout_id = hypercolor_types::identity::LayoutId::new(layout.id.clone())
            .map_err(|error| DomainError::validation(error.to_string()))?;
        if command.expected.scene.layout_id.as_ref() != Some(&layout_id)
            || ctx.layout.get(&layout_id).await.as_ref() != Some(layout)
        {
            return Err(DomainError::conflict(
                "The observed scene layout changed or is unavailable",
            ));
        }
    }
    let mut expected = command.expected;
    let mut context = None;
    if let Some(transition) = command.context {
        let admission = media.evaluate(&expected.scene);
        if let Some(message) = admission.rejection_message() {
            return Err(DomainError::validation(message.to_owned()));
        }
        mutation.hydrate_existing_display_surfaces(expected.scene.id, &display)?;
        mutation.activate(
            expected.scene.id,
            Some(transition),
            SceneChangeReason::UserActivate,
        )?;
        mutation.sync_active_display_surfaces(&display);
        let admitted = mutation
            .scenes()
            .get(&expected.scene.id)
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, expected.scene.id))?
            .clone();
        let commit = ctx.scene.commit(mutation).await?;
        expected = ObservedScene {
            scene: admitted,
            revision: commit.revision(),
        };
        ctx.scene
            .apply_media_soft_admission(
                expected.scene.id,
                &expected.scene.name,
                admission.estimated_cost_us,
            )
            .await;
        context = Some(commit);
    }
    let layout = if let Some(layout) = command.layout {
        let result = ctx
            .layout
            .apply_selected_under_guard(
                layout_guard
                    .as_ref()
                    .expect("selected layout owns its update guard"),
                layout,
                expected.clone(),
                ctx.scene.layout_runtime(),
            )
            .await;
        if let Some(admitted) = &result.admitted_scene {
            expected = admitted.clone();
        }
        Some(result)
    } else {
        None
    };
    drop(layout_guard);
    let brightness = if let Some(brightness) = command.brightness {
        Some(
            ctx.scene
                .set_selected_brightness(&ctx.output, &expected, brightness)
                .await,
        )
    } else {
        None
    };
    let changed_context = context.is_some()
        || layout
            .as_ref()
            .is_some_and(|result| result.publication.is_ok());
    let runtime_session = if changed_context {
        ctx.layout
            .sync_runtime_connectivity(ctx.scene.layout_runtime())
            .await;
        Some(ctx.scene.save_runtime_session_with_outcome().await)
    } else {
        None
    };
    Ok(SelectedSceneOutcome {
        context,
        layout,
        brightness,
        runtime_session,
    })
}

#[derive(Debug)]
pub(crate) struct LayoutSceneFence {
    pub(crate) expected: ObservedScene,
    admitted: Mutex<Option<ObservedScene>>,
}

impl LayoutSceneFence {
    pub(crate) fn new(expected: ObservedScene) -> Arc<Self> {
        Arc::new(Self {
            expected,
            admitted: Mutex::new(None),
        })
    }

    pub(crate) fn publish(&self, manager: &SceneManager, revision: u64) {
        let scene = manager.get(&self.expected.scene.id).cloned();
        *self
            .admitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            scene.map(|scene| ObservedScene { scene, revision });
    }

    pub(crate) fn admitted(&self) -> Option<ObservedScene> {
        self.admitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// An owned authored definition and the manager revision observed with it.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedScene {
    /// Complete expected scene, including its behavior and authored metadata.
    pub scene: Scene,
    /// Revision of the manager that supplied the definition.
    pub revision: u64,
}

impl ObservedScene {
    pub(crate) fn validate(
        &self,
        manager: &SceneManager,
        revision: u64,
    ) -> Result<(), DomainError> {
        let current = manager
            .get(&self.scene.id)
            .ok_or_else(|| DomainError::not_found(ResourceKind::Scene, self.scene.id))?;
        if revision != self.revision || current != &self.scene {
            return Err(DomainError::conflict(
                "The observed scene definition changed",
            ));
        }
        Ok(())
    }
}

/// Stage-aware persistence of the original selected brightness value.
#[derive(Debug)]
pub enum BrightnessDurability {
    /// The exact selected settings payload was durably written.
    Written,
    /// Replacement was admitted but durable completion remains unresolved.
    Retrying(String),
}

/// Brightness admission never changes the output pause or sleep state.
#[derive(Debug)]
pub struct SelectedBrightnessOutcome {
    /// The value admitted while the scene definition guard was held.
    pub value: f32,
    /// Brightness immediately before this admission.
    pub previous: f32,
    /// Actual owning settings writer outcome.
    pub durability: BrightnessDurability,
}
