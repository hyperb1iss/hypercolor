//! Layer writes addressed to a screen's view.
//!
//! A connected screen is listed on the wire as one `Display` zone, its
//! view, whose layers are the layers of the zone that places the screen
//! scoped to it. A write to a view is always about that one screen: it
//! lands in the owning zone's stack, it never changes what another screen
//! shows, and it never touches a layer that is not this screen's. A layer
//! other screens share is split first, so this screen gets a copy of its
//! own and the others keep the original.
//!
//! A stored display zone is not a view. It is a zone of the scene, and
//! the layer routes reach it directly.

use hypercolor_core::scene::LayerMutationError;
use hypercolor_core::scene::screens::{self, ScreenView};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::LayerStackChangeKind;
use hypercolor_types::layer::{LayerScope, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{SceneId, ZoneId};

use crate::domain::scene::SceneMutation;
use crate::domain::scene_tree::layer_error;
use crate::domain::{DomainError, ResourceKind};

/// The screen a view id addresses in the active scene.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ViewTarget {
    pub(crate) scene: SceneId,
    pub(crate) zone: ZoneId,
    pub(crate) device: DeviceId,
}

/// Resolve a zone id that is a connected screen's view.
pub(crate) fn view_target(mutation: &SceneMutation, zone_id: ZoneId) -> Option<ViewTarget> {
    let scenes = mutation.scenes();
    let scene_id = *scenes.active_scene_id()?;
    scenes
        .screen_of_view(&scene_id, zone_id)
        .map(|device| ViewTarget {
            scene: scene_id,
            zone: zone_id,
            device,
        })
}

impl ViewTarget {
    /// The view as the candidate holds it now.
    pub(crate) fn view(self, mutation: &SceneMutation) -> Result<ScreenView, DomainError> {
        mutation
            .scenes()
            .screen_view(&self.scene, self.device)
            .ok_or_else(|| DomainError::not_found(ResourceKind::Zone, self.zone))
    }

    /// The owner of a layer this view holds, or the layer's not-found.
    fn owner_of(
        self,
        mutation: &SceneMutation,
        layer_id: SceneLayerId,
    ) -> Result<(ZoneId, String), DomainError> {
        let view = self.view(mutation)?;
        if !view.zone.layers.iter().any(|layer| layer.id == layer_id) {
            return Err(DomainError::not_found(ResourceKind::Layer, layer_id));
        }
        view.owner
            .ok_or_else(|| DomainError::not_found(ResourceKind::Layer, layer_id))
    }

    /// Create a layer at the top of the owning zone's stack, scoped to the
    /// screen. A screen no zone places has no output to scope a layer to,
    /// so the layer joins the scene's stored display zone for the screen,
    /// which is created under the view's id when there is none.
    pub(crate) fn create_layer(
        self,
        mutation: &mut SceneMutation,
        mut layer: SceneLayer,
    ) -> Result<(), DomainError> {
        if layer.scope.is_some() {
            return Err(DomainError::validation_field(
                "scope",
                "a screen's layers apply to that screen; its view takes no scope",
            ));
        }
        let layer_id = layer.id;
        let Some((owner, output_id)) = self.view(mutation)?.owner else {
            let zone_id = mutation
                .ensure_display_zone(self.scene, self.device)
                .map_err(|_| DomainError::not_found(ResourceKind::Zone, self.zone))?;
            return mutation
                .insert_layer(self.scene, zone_id, layer, None, None)
                .map(|_| ())
                .map_err(|error| layer_error(error, self.zone, Some(layer_id)));
        };
        layer.scope = Some(LayerScope {
            outputs: vec![output_id],
        });
        mutation
            .edit_screen_layers(
                self.scene,
                owner,
                self.device,
                LayerStackChangeKind::Created,
                |zone| {
                    if zone.layers.iter().any(|held| held.id == layer_id) {
                        return Err(LayerMutationError::DuplicateLayer { layer_id });
                    }
                    zone.layers.push(layer);
                    Ok(())
                },
            )
            .map_err(|error| layer_error(error, self.zone, Some(layer_id)))
    }

    /// Replace one of the screen's layers. A layer scoped to this screen
    /// alone is replaced in place; a shared one is split, and the
    /// replacement lands directly above it for this screen only. Either
    /// way the replacement is scoped to the screen.
    pub(crate) fn replace_layer(
        self,
        mutation: &mut SceneMutation,
        layer_id: SceneLayerId,
        mut layer: SceneLayer,
    ) -> Result<(), DomainError> {
        let (owner, output_id) = self.owner_of(mutation, layer_id)?;
        mutation
            .edit_screen_layers(
                self.scene,
                owner,
                self.device,
                LayerStackChangeKind::Updated,
                |zone| {
                    let index = screens::split_for_screen(zone, layer_id, &output_id)
                        .ok_or(LayerMutationError::LayerMissing { layer_id })?;
                    layer.scope.clone_from(&zone.layers[index].scope);
                    zone.layers[index] = layer;
                    Ok(())
                },
            )
            .map_err(|error| layer_error(error, self.zone, Some(layer_id)))
    }

    /// Delete one of the screen's layers, or take the screen out of its
    /// scope when other screens share it.
    pub(crate) fn delete_layer(
        self,
        mutation: &mut SceneMutation,
        layer_id: SceneLayerId,
    ) -> Result<(), DomainError> {
        let (owner, output_id) = self.owner_of(mutation, layer_id)?;
        mutation
            .edit_screen_layers(
                self.scene,
                owner,
                self.device,
                LayerStackChangeKind::Removed,
                |zone| {
                    if screens::remove_screen_from_layer(zone, layer_id, &output_id) {
                        Ok(())
                    } else {
                        Err(LayerMutationError::LayerMissing { layer_id })
                    }
                },
            )
            .map_err(|error| layer_error(error, self.zone, Some(layer_id)))
    }

    /// Rewrite the order of the screen's layers. `order` names every layer
    /// of the view exactly once. The layers are rewritten, in the requested
    /// order, into the positions they occupy in the owning stack, and no
    /// other layer moves. When the order changes, each shared layer is
    /// split first so other screens keep theirs.
    pub(crate) fn reorder_layers(
        self,
        mutation: &mut SceneMutation,
        order: &[SceneLayerId],
    ) -> Result<(), DomainError> {
        let view = self.view(mutation)?;
        let current = view
            .zone
            .layers
            .iter()
            .map(|layer| layer.id)
            .collect::<Vec<_>>();
        let mut sorted_current = current.clone();
        let mut sorted_order = order.to_vec();
        sorted_current.sort_unstable_by_key(SceneLayerId::as_uuid);
        sorted_order.sort_unstable_by_key(SceneLayerId::as_uuid);
        if sorted_current != sorted_order {
            return Err(layer_error(
                LayerMutationError::InvalidOrder,
                self.zone,
                None,
            ));
        }
        if current == order {
            return Ok(());
        }
        let Some((owner, output_id)) = view.owner else {
            return Ok(());
        };
        mutation
            .edit_screen_layers(
                self.scene,
                owner,
                self.device,
                LayerStackChangeKind::Reordered,
                |zone| {
                    let mut own = Vec::with_capacity(order.len());
                    for layer_id in order {
                        let index = screens::split_for_screen(zone, *layer_id, &output_id).ok_or(
                            LayerMutationError::LayerMissing {
                                layer_id: *layer_id,
                            },
                        )?;
                        own.push(zone.layers[index].id);
                    }
                    let mut slots = own
                        .iter()
                        .filter_map(|id| zone.layers.iter().position(|layer| layer.id == *id))
                        .collect::<Vec<_>>();
                    slots.sort_unstable();
                    let ordered = own
                        .iter()
                        .filter_map(|id| zone.layers.iter().find(|layer| layer.id == *id).cloned())
                        .collect::<Vec<_>>();
                    for (slot, layer) in slots.into_iter().zip(ordered) {
                        zone.layers[slot] = layer;
                    }
                    Ok(())
                },
            )
            .map_err(|error| layer_error(error, self.zone, None))
    }

    /// Give the screen its own copy of a layer it shares, so a write that
    /// follows changes this screen alone. Returns the owning zone and the
    /// id the screen's layer now has.
    pub(crate) fn own_layer(
        self,
        mutation: &mut SceneMutation,
        layer_id: SceneLayerId,
        kind: LayerStackChangeKind,
    ) -> Result<(ZoneId, SceneLayerId), DomainError> {
        let (owner, output_id) = self.owner_of(mutation, layer_id)?;
        let own = mutation
            .edit_screen_layers(self.scene, owner, self.device, kind, |zone| {
                let index = screens::split_for_screen(zone, layer_id, &output_id)
                    .ok_or(LayerMutationError::LayerMissing { layer_id })?;
                Ok(zone.layers[index].id)
            })
            .map_err(|error| layer_error(error, self.zone, Some(layer_id)))?;
        Ok((owner, own))
    }
}
