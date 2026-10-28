//! Owned frame boundary for restoring several guarded authorities together.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use hypercolor_core::effect::EffectRegistry;
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::scene::ZoneId;

use super::{
    LayoutActivationControl, LayoutPublicationMode, LayoutTransactionAuthority,
    LayoutTransactionRejection, LayoutUpdateError, LayoutUpdateGuard, SceneTransaction,
    SceneTransactionQueue,
};
use hypercolor_core::spatial::SpatialEngine;
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Debug)]
pub(crate) struct PortableRenderHold {
    active: AtomicBool,
}

impl PortableRenderHold {
    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub(crate) struct BeginPortableRestore {
    hold: Arc<PortableRenderHold>,
    acknowledgment: oneshot::Sender<Result<(), LayoutTransactionRejection>>,
}

/// Acquire before any store writers. Drop releases frames, including when a
/// caller is cancelled before the global restore intent has been admitted.
pub(crate) struct PortableRenderBarrier {
    hold: Arc<PortableRenderHold>,
    guard: LayoutUpdateGuard,
}

impl PortableRenderBarrier {
    pub(crate) fn guard(&self) -> LayoutUpdateGuard {
        self.guard.clone()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.hold.is_active()
    }
}

impl Drop for PortableRenderBarrier {
    fn drop(&mut self) {
        self.hold.active.store(false, Ordering::Release);
    }
}

#[derive(Clone)]
pub(crate) struct PortableRendererSnapshot {
    pub(crate) hold: Arc<PortableRenderHold>,
    pub(crate) effects: EffectRegistry,
    pub(crate) displays: HashMap<ZoneId, DisplayDescriptor>,
}

impl std::fmt::Debug for PortableRendererSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PortableRendererSnapshot")
            .field("active", &self.hold.is_active())
            .field("effect_generation", &self.effects.generation())
            .field("display_count", &self.displays.len())
            .finish()
    }
}

impl PortableRendererSnapshot {
    pub(crate) fn new(
        barrier: &PortableRenderBarrier,
        effects: EffectRegistry,
        displays: HashMap<ZoneId, DisplayDescriptor>,
    ) -> Self {
        Self {
            hold: Arc::clone(&barrier.hold),
            effects,
            displays,
        }
    }
}

impl SceneTransactionQueue {
    pub(crate) fn accept_portable_restore(&self, request: BeginPortableRestore) {
        if !request.hold.is_active() || request.acknowledgment.is_closed() {
            return;
        }
        *self.portable_restore.lock().expect("restore hold lock") = Some(request.hold);
        let _ = request.acknowledgment.send(Ok(()));
    }

    pub(crate) fn portable_restore_held(&self) -> bool {
        let mut current = self.portable_restore.lock().expect("restore hold lock");
        if current.as_ref().is_some_and(|hold| hold.is_active()) {
            return true;
        }
        *current = None;
        false
    }

    fn owns_portable_restore(&self, hold: &Arc<PortableRenderHold>) -> bool {
        self.portable_restore
            .lock()
            .expect("restore hold lock")
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, hold) && current.is_active())
    }

    pub(super) fn stop_portable_restore(&self) {
        if let Some(hold) = self
            .portable_restore
            .lock()
            .expect("restore hold lock")
            .take()
        {
            hold.active.store(false, Ordering::Release);
        }
    }
}

impl BeginPortableRestore {
    pub(super) fn reject(self) {
        self.hold.active.store(false, Ordering::Release);
        let _ = self
            .acknowledgment
            .send(Err(LayoutTransactionRejection::RendererStopped));
    }
}

impl LayoutTransactionAuthority {
    pub(crate) async fn begin_portable_restore(
        &self,
    ) -> Result<PortableRenderBarrier, LayoutUpdateError> {
        let guard = self.acquire_layout_update_guard().await;
        let hold = Arc::new(PortableRenderHold {
            active: AtomicBool::new(true),
        });
        let barrier = PortableRenderBarrier {
            hold: Arc::clone(&hold),
            guard,
        };
        let (acknowledgment, receipt) = oneshot::channel();
        self.scene_transactions
            .push(SceneTransaction::BeginPortableRestore(
                BeginPortableRestore {
                    hold,
                    acknowledgment,
                },
            ))?;
        receipt
            .await
            .unwrap_or(Err(LayoutTransactionRejection::RendererStopped))?;
        Ok(barrier)
    }

    pub(crate) async fn activate_portable_renderer(
        &self,
        barrier: &PortableRenderBarrier,
        spatial: SpatialEngine,
        scenes: &hypercolor_core::scene::SceneManager,
        snapshot: Arc<PortableRendererSnapshot>,
    ) -> Result<(), LayoutUpdateError> {
        if !barrier.is_active()
            || !Arc::ptr_eq(&barrier.hold, &snapshot.hold)
            || !self.scene_transactions.owns_portable_restore(&barrier.hold)
        {
            return Err(LayoutTransactionRejection::RendererStopped.into());
        }
        let (zones, revision) = scenes.resolved_zones_for_primary_layout(spatial.layout().as_ref());
        let submission = self.scene_transactions.submit_layout_preparation(
            spatial,
            self.spatial_engine.layout().as_ref().clone(),
            scenes.active_scene_id().copied(),
            zones,
            scenes.resolved_zones_revision(),
            revision,
            scenes
                .active_scene()
                .map(|scene| scene.unassigned_behavior.clone())
                .unwrap_or_default(),
            LayoutPublicationMode::PortableRestore,
            None,
            Some(snapshot),
        )?;
        // An abandoned preparation must not leave the renderer holding staged
        // resources indefinitely. A committed activation checks the hold again.
        struct AbortOnDrop(LayoutActivationControl);
        impl Drop for AbortOnDrop {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _abort = AbortOnDrop(submission.activation.clone());
        submission.preparation.wait().await?;
        if !barrier.is_active() {
            return Err(LayoutTransactionRejection::RendererStopped.into());
        }
        submission.activation.commit();
        submission.completion.wait().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
