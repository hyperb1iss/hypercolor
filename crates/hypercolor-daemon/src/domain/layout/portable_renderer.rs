//! Renderer resources prepared from already-fenced restore candidates.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::{effect::EffectRegistry, scene::SceneManager, spatial::SpatialEngine};
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::display::{DisplayDescriptor, DisplayPixelFormat};
use hypercolor_types::spatial::SpatialLayout;

use super::{LayoutContext, RenderCanvas};
use crate::domain::{DomainError, spatial::SpatialService};
use crate::scene_transactions::LayoutTransactionAuthority;
use crate::scene_transactions::portable::{PortableRenderBarrier, PortableRendererSnapshot};

pub(crate) struct PreparedPortableRenderer {
    authority: LayoutTransactionAuthority,
    spatial: SpatialService,
    canvas: RenderCanvas,
    candidate: SpatialEngine,
    scenes: SceneManager,
    snapshot: Arc<PortableRendererSnapshot>,
    activated: bool,
}

impl LayoutContext {
    /// Finish the current frame before the restore retains any store writers.
    pub(crate) async fn begin_portable_restore(
        &self,
    ) -> Result<PortableRenderBarrier, DomainError> {
        self.publication
            .transactions
            .begin_portable_restore()
            .await
            .map_err(|error| DomainError::Internal(error.into()))
    }

    /// No source authority is read here: every value comes from a prepared
    /// participant whose ordinary writer the coordinator already owns.
    pub(crate) fn prepare_portable_renderer(
        &self,
        barrier: &PortableRenderBarrier,
        layout: SpatialLayout,
        scenes: &SceneManager,
        effects: &EffectRegistry,
        devices: &HashMap<DeviceId, DeviceInfo>,
        face_fps_cap: u32,
    ) -> Result<PreparedPortableRenderer, DomainError> {
        if !barrier.is_active() {
            return Err(DomainError::conflict(
                "renderer restore boundary has closed",
            ));
        }
        let candidate =
            SpatialEngine::try_new(layout).map_err(|error| DomainError::Internal(error.into()))?;
        let (zones, _) = scenes.resolved_zones_for_primary_layout(candidate.layout().as_ref());
        let displays = zones
            .iter()
            .filter_map(|zone| {
                let target = zone.display_target.as_ref()?;
                let device = devices.get(&target.device_id)?;
                let surface = device.display_surface()?;
                let fps = crate::display_output::capped_zone_direct_display_target_fps(
                    device.capabilities.max_fps,
                    face_fps_cap,
                );
                Some((
                    zone.id,
                    DisplayDescriptor::derive(
                        surface.width,
                        surface.height,
                        surface.circular,
                        None,
                        fps,
                        DisplayPixelFormat::from(surface.format),
                    ),
                ))
            })
            .collect();
        Ok(PreparedPortableRenderer {
            authority: self.publication.transactions.clone(),
            spatial: self.publication.spatial.clone(),
            canvas: self.publication.render_canvas.clone(),
            candidate,
            scenes: scenes.clone(),
            snapshot: Arc::new(PortableRendererSnapshot::new(
                barrier,
                effects.clone(),
                displays,
            )),
            activated: false,
        })
    }
}

impl PreparedPortableRenderer {
    /// Retryable while all store writers and the frame boundary remain owned.
    /// Frames keep their previous picture until the coordinator releases it.
    pub(crate) async fn activate(
        &mut self,
        barrier: &PortableRenderBarrier,
    ) -> Result<(), DomainError> {
        if self.activated {
            return Ok(());
        }
        self.authority
            .activate_portable_renderer(
                barrier,
                self.candidate.clone(),
                &self.scenes,
                Arc::clone(&self.snapshot),
            )
            .await
            .map_err(|error| DomainError::Internal(error.into()))?;
        self.activated = true;
        Ok(())
    }

    /// The renderer has acknowledged the same dimensions before authority moves.
    pub(crate) fn publish(&self) {
        assert!(
            self.activated,
            "portable renderer must acknowledge before publication"
        );
        let layout = self.candidate.layout();
        self.canvas.set(layout.canvas_width, layout.canvas_height);
        self.spatial.replace(self.candidate.clone());
    }
}
