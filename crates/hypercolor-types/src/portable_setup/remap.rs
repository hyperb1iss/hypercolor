//! Simultaneous device identity translation for an authored setup graph.

use std::collections::{BTreeMap, btree_map::Entry};

use uuid::Uuid;

use super::{PortableLayout, PortableSetupError, PortableSetupV1};

impl PortableSetupV1 {
    /// Translate physical and logical device identities and their references.
    ///
    /// Each mapping is applied once, so swaps are valid and mappings never
    /// cascade. Unmatched identities and all other authored identities remain
    /// unchanged. The original document is never mutated.
    ///
    /// # Errors
    ///
    /// Rejects invalid input graphs, nil destinations, and destinations that
    /// merge distinct physical or logical devices, including unchanged ones.
    pub fn remap_devices_and_outputs(
        &self,
        identities: &BTreeMap<Uuid, Uuid>,
    ) -> Result<Self, PortableSetupError> {
        self.validate()?;
        let mut result = self.clone();
        result.devices = remap_keys(result.devices, identities, "devices")?;
        result.logical_devices = remap_keys(result.logical_devices, identities, "logical_devices")?;
        for logical in result.logical_devices.values_mut() {
            logical.device = mapped(logical.device, identities);
        }
        result.attachments = remap_keys(result.attachments, identities, "attachments")?;
        result.display_faces = remap_keys(result.display_faces, identities, "display_faces")?;
        result.device_output = remap_keys(result.device_output, identities, "device_output")?;
        for layout in result.layouts.values_mut() {
            remap_layout(layout, identities);
        }
        for scene in result.scenes.values_mut() {
            for zone in &mut scene.zones {
                remap_layout(&mut zone.layout, identities);
                if let Some(target) = &mut zone.display_target {
                    target.device = mapped(target.device, identities);
                }
            }
        }
        result.validate()?;
        Ok(result)
    }
}

fn mapped(id: Uuid, identities: &BTreeMap<Uuid, Uuid>) -> Uuid {
    identities.get(&id).copied().unwrap_or(id)
}

fn remap_layout(layout: &mut PortableLayout, identities: &BTreeMap<Uuid, Uuid>) {
    for output in &mut layout.zones {
        output.device = mapped(output.device, identities);
    }
}

fn remap_keys<T>(
    source: BTreeMap<Uuid, T>,
    identities: &BTreeMap<Uuid, Uuid>,
    path: &str,
) -> Result<BTreeMap<Uuid, T>, PortableSetupError> {
    let mut result = BTreeMap::new();
    for (id, value) in source {
        let destination = mapped(id, identities);
        match result.entry(destination) {
            Entry::Vacant(entry) => {
                entry.insert(value);
            }
            Entry::Occupied(_) => {
                return Err(PortableSetupError {
                    path: format!("{path}.{destination}"),
                    message: "identity remapping would overwrite another record".into(),
                });
            }
        }
    }
    Ok(result)
}
