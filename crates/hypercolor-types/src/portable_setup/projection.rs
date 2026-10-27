//! Conversion at the boundary between portable authoring and local runtime IDs.

use super::{PortableDisplayTarget, PortableLayout, PortableOutput, PortableScene, PortableZone};
use crate::device::DeviceId;
use crate::scene::{DisplayFaceTarget, Scene, SceneId, Zone};
use crate::spatial::{Output, SpatialLayout};
use std::collections::HashMap;
use uuid::Uuid;

/// Local identity associations, kept outside the portable document.
///
/// Export refuses unassociated references so a path cannot silently leak into
/// the archive. Restore keeps unresolved references as inert portable IDs until
/// discovery or the user supplies an unambiguous association.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SetupBindings {
    pub outputs: HashMap<String, Uuid>,
    pub devices: HashMap<DeviceId, Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no portable association for {0}")]
pub struct UnmappedSetupReference(pub String);

impl SetupBindings {
    fn portable_output(&self, local: &str) -> Result<Uuid, UnmappedSetupReference> {
        self.outputs
            .get(local)
            .copied()
            .or_else(|| {
                local
                    .strip_prefix("portable:")
                    .and_then(|id| Uuid::parse_str(id).ok())
            })
            .ok_or_else(|| UnmappedSetupReference(local.to_owned()))
    }

    fn portable_device(&self, local: DeviceId) -> Result<Uuid, UnmappedSetupReference> {
        self.devices
            .get(&local)
            .copied()
            .ok_or_else(|| UnmappedSetupReference(local.to_string()))
    }

    /// Destination output address, retaining an offline reference when unknown.
    #[must_use]
    pub fn local_output(&self, portable: Uuid) -> String {
        let mut matches = self.outputs.iter().filter(|(_, id)| **id == portable);
        match (matches.next(), matches.next()) {
            (Some((local, _)), None) => local.clone(),
            _ => format!("portable:{portable}"),
        }
    }

    /// Resolve only an explicit, unambiguous physical association.
    /// Unresolved display definitions stay in the durable portable manifest and
    /// are omitted from runtime targets until hardware is associated.
    #[must_use]
    pub fn local_device(&self, portable: Uuid) -> Option<DeviceId> {
        let mut matches = self.devices.iter().filter(|(_, id)| **id == portable);
        match (matches.next(), matches.next()) {
            (Some((local, _)), None) => Some(*local),
            _ => None,
        }
    }
}

impl PortableLayout {
    pub fn project(
        layout: &SpatialLayout,
        bindings: &SetupBindings,
    ) -> Result<Self, UnmappedSetupReference> {
        Ok(Self {
            id: layout.id.clone(),
            name: layout.name.clone(),
            description: layout.description.clone(),
            default_sampling_mode: layout.default_sampling_mode.clone(),
            default_edge_behavior: layout.default_edge_behavior,
            version: layout.version,
            zones: layout
                .zones
                .iter()
                .map(|output| PortableOutput::project(output, bindings))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Materialize normalized coordinates against the destination's resolution.
    #[must_use]
    pub fn materialize(&self, bindings: &SetupBindings, canvas: (u32, u32)) -> SpatialLayout {
        SpatialLayout {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            default_sampling_mode: self.default_sampling_mode.clone(),
            default_edge_behavior: self.default_edge_behavior,
            version: self.version,
            canvas_width: canvas.0,
            canvas_height: canvas.1,
            zones: self
                .zones
                .iter()
                .map(|output| output.materialize(bindings))
                .collect(),
        }
    }
}

impl PortableOutput {
    fn project(output: &Output, bindings: &SetupBindings) -> Result<Self, UnmappedSetupReference> {
        Ok(Self {
            id: output.id.clone(),
            name: output.name.clone(),
            zone_name: output.zone_name.clone(),
            position: output.position,
            size: output.size,
            rotation: output.rotation,
            scale: output.scale,
            display_order: output.display_order,
            orientation: output.orientation,
            topology: output.topology.clone(),
            led_mapping: output.led_mapping.clone(),
            sampling_mode: output.sampling_mode.clone(),
            edge_behavior: output.edge_behavior,
            shape: output.shape.clone(),
            shape_preset: output.shape_preset.clone(),
            attachment: output.attachment.clone(),
            brightness: output.brightness,
            device: bindings.portable_output(&output.device_id)?,
        })
    }

    fn materialize(&self, bindings: &SetupBindings) -> Output {
        Output {
            id: self.id.clone(),
            name: self.name.clone(),
            zone_name: self.zone_name.clone(),
            position: self.position,
            size: self.size,
            rotation: self.rotation,
            scale: self.scale,
            display_order: self.display_order,
            orientation: self.orientation,
            topology: self.topology.clone(),
            led_mapping: self.led_mapping.clone(),
            sampling_mode: self.sampling_mode.clone(),
            edge_behavior: self.edge_behavior,
            shape: self.shape.clone(),
            shape_preset: self.shape_preset.clone(),
            attachment: self.attachment.clone(),
            brightness: self.brightness,
            device_id: bindings.local_output(self.device),
            led_positions: Vec::new(),
        }
    }
}

impl PortableScene {
    pub fn project(
        scene: &Scene,
        bindings: &SetupBindings,
    ) -> Result<Self, UnmappedSetupReference> {
        Ok(Self {
            name: scene.name.clone(),
            description: scene.description.clone(),
            transition: scene.transition.clone(),
            priority: scene.priority,
            enabled: scene.enabled,
            metadata: scene.metadata.clone(),
            unassigned_behavior: scene.unassigned_behavior.clone(),
            layout_id: scene.layout_id.clone(),
            activation_brightness: scene.activation_brightness,
            kind: scene.kind,
            mutation_mode: scene.mutation_mode,
            zones: scene
                .zones
                .iter()
                .map(|zone| PortableZone::project(zone, bindings))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Restore authored identities; destination domain assigns mutation counters.
    #[must_use]
    pub fn materialize(&self, id: SceneId, bindings: &SetupBindings, canvas: (u32, u32)) -> Scene {
        Scene {
            id,
            name: self.name.clone(),
            description: self.description.clone(),
            transition: self.transition.clone(),
            priority: self.priority,
            enabled: self.enabled,
            metadata: self.metadata.clone(),
            unassigned_behavior: self.unassigned_behavior.clone(),
            layout_id: self.layout_id.clone(),
            activation_brightness: self.activation_brightness,
            kind: self.kind,
            mutation_mode: self.mutation_mode,
            zones_revision: 0,
            zones: self
                .zones
                .iter()
                .map(|zone| zone.materialize(bindings, canvas))
                .collect(),
        }
    }
}

impl PortableZone {
    fn project(zone: &Zone, bindings: &SetupBindings) -> Result<Self, UnmappedSetupReference> {
        Ok(Self {
            id: zone.id,
            name: zone.name.clone(),
            description: zone.description.clone(),
            layers: zone.layers.clone(),
            brightness: zone.brightness,
            enabled: zone.enabled,
            color: zone.color.clone(),
            role: zone.role,
            layout: PortableLayout::project(&zone.layout, bindings)?,
            display_target: zone
                .display_target
                .as_ref()
                .map(|target| {
                    Ok(PortableDisplayTarget {
                        device: bindings.portable_device(target.device_id)?,
                        blend_mode: target.blend_mode,
                        opacity: target.opacity,
                    })
                })
                .transpose()?,
        })
    }

    fn materialize(&self, bindings: &SetupBindings, canvas: (u32, u32)) -> Zone {
        Zone {
            id: self.id,
            name: self.name.clone(),
            description: self.description.clone(),
            layers: self.layers.clone(),
            brightness: self.brightness,
            enabled: self.enabled,
            color: self.color.clone(),
            role: self.role,
            layout: self.layout.materialize(bindings, canvas),
            display_target: self.display_target.as_ref().and_then(|target| {
                bindings
                    .local_device(target.device)
                    .map(|device_id| DisplayFaceTarget {
                        device_id,
                        blend_mode: target.blend_mode,
                        opacity: target.opacity,
                    })
            }),
            controls_version: 0,
            layers_version: 0,
        }
    }
}
