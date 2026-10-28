//! Historical authored addresses are projection evidence, never live bindings.

use std::collections::{BTreeMap, HashMap};

use hypercolor_types::device::DeviceId;
use hypercolor_types::portable_setup::{PortableDevice, PortableSetupV1, SetupBindings};
use hypercolor_types::spatial::SpatialLayout;
use uuid::Uuid;

use super::PortableSetupService;
use crate::logical_devices::LogicalDeviceKind;

#[derive(Default)]
pub(super) struct AuthoredReferences {
    outputs: BTreeMap<String, ReferenceDescription>,
    devices: HashMap<DeviceId, ReferenceDescription>,
}

#[derive(Clone)]
struct ReferenceDescription {
    name: String,
    authored_name: bool,
    led_count: u32,
}

impl ReferenceDescription {
    fn new(name: &str, led_count: u32) -> Self {
        Self {
            authored_name: !name.trim().is_empty(),
            name: if name.trim().is_empty() {
                "Disconnected output".into()
            } else {
                name.to_owned()
            },
            led_count,
        }
    }

    fn merge(&mut self, other: Self) {
        if (other.authored_name && !self.authored_name)
            || (other.authored_name == self.authored_name && other.name < self.name)
        {
            self.name = other.name;
            self.authored_name = other.authored_name;
        }
        self.led_count = self.led_count.max(other.led_count);
    }

    fn unresolved_device(&self) -> PortableDevice {
        PortableDevice {
            name: self.name.clone(),
            portable_key: None,
            driver_id: "unknown".into(),
            vendor_id: None,
            product_id: None,
            model: None,
            led_count: self.led_count,
            binding_hint: None,
        }
    }
}

impl AuthoredReferences {
    fn observe_output(&mut self, local: &str, name: &str, led_count: u32) {
        let description = ReferenceDescription::new(name, led_count);
        match self.outputs.entry(local.to_owned()) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().merge(description);
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(description);
            }
        }
    }

    fn observe_device(&mut self, local: DeviceId, name: &str, led_count: u32) {
        let description = ReferenceDescription::new(name, led_count);
        match self.devices.entry(local) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().merge(description);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(description);
            }
        }
    }

    fn observe_layout(&mut self, layout: &SpatialLayout) {
        for output in &layout.zones {
            self.observe_output(&output.device_id, &output.name, output.topology.led_count());
        }
    }

    /// Add only local projection aliases. The caller returns live associations
    /// separately, so absent hardware cannot become a runtime target here.
    pub(super) fn assign_aliases(&self, durable: &mut SetupBindings) {
        for device in self.devices.keys() {
            let existing = durable.outputs.get(&device.to_string()).copied();
            durable
                .devices
                .entry(*device)
                .or_insert_with(|| existing.unwrap_or_else(Uuid::now_v7));
        }
        for local in self.outputs.keys() {
            let physical = local.parse::<DeviceId>().ok().map(|id| {
                let existing = durable.outputs.get(local).copied();
                *durable
                    .devices
                    .entry(id)
                    .or_insert_with(|| existing.unwrap_or_else(Uuid::now_v7))
            });
            let inert = local
                .strip_prefix("portable:")
                .and_then(|id| Uuid::parse_str(id).ok());
            durable
                .outputs
                .entry(local.clone())
                .or_insert_with(|| physical.or(inert).unwrap_or_else(Uuid::now_v7));
        }
    }

    /// Fill the graph for addresses that predate any portable restore receipt.
    /// Accepted portable definitions retain their evidence and metadata.
    pub(super) fn retain_definitions(
        &self,
        manifest: &mut PortableSetupV1,
        prior: Option<&PortableSetupV1>,
        aliases: &SetupBindings,
    ) {
        let mut descriptions = BTreeMap::<Uuid, ReferenceDescription>::new();
        let mut observe = |id: Uuid, description: &ReferenceDescription| {
            descriptions
                .entry(id)
                .and_modify(|current| current.merge(description.clone()))
                .or_insert_with(|| description.clone());
        };
        for (local, description) in &self.devices {
            if let Some(id) = aliases.devices.get(local) {
                observe(*id, description);
            }
        }
        for (local, description) in &self.outputs {
            if let Some(id) = aliases.outputs.get(local)
                && !manifest.logical_devices.contains_key(id)
                && prior.is_none_or(|prior| !prior.logical_devices.contains_key(id))
            {
                observe(*id, description);
            }
        }
        for (id, description) in descriptions {
            manifest.devices.entry(id).or_insert_with(|| {
                prior
                    .and_then(|prior| prior.devices.get(&id))
                    .cloned()
                    .unwrap_or_else(|| description.unresolved_device())
            });
        }
    }
}

impl PortableSetupService {
    pub(super) async fn authored_references(&self) -> AuthoredReferences {
        let mut references = AuthoredReferences::default();
        references.observe_layout(&self.state.domains.layout.current());
        for layout in self.state.domains.layout.portable_snapshot().await.values() {
            references.observe_layout(layout);
        }
        let scenes = self.state.scene_manager.snapshot().await;
        for scene in scenes.list() {
            for zone in &scene.zones {
                references.observe_layout(&zone.layout);
                if let Some(target) = &zone.display_target {
                    references.observe_device(target.device_id, &zone.name, 0);
                }
            }
        }
        drop(scenes);
        for logical in self.state.logical_devices.read().await.values() {
            if logical.kind != LogicalDeviceKind::Default {
                references.observe_device(
                    logical.physical_device_id,
                    &logical.name,
                    logical.led_start.saturating_add(logical.led_count),
                );
            }
        }
        for (local, _) in self
            .state
            .attachment_profiles
            .read()
            .await
            .portable_snapshot()
        {
            if let Ok(device) = local.parse() {
                references.observe_device(device, "", 0);
            } else {
                references.observe_output(&local, "", 0);
            }
        }
        for (device, _) in self.state.domains.display.preferences().read().await.iter() {
            references.observe_device(device, "", 0);
        }
        references
    }
}

#[cfg(test)]
mod tests {
    use super::AuthoredReferences;
    use hypercolor_types::device::DeviceId;
    use hypercolor_types::portable_setup::{PortableOutputState, PortableSetupV1, SetupBindings};
    use uuid::Uuid;

    #[test]
    fn device_reference_reuses_output_alias_and_retains_authored_label() {
        let local = DeviceId::new();
        let portable = Uuid::now_v7();
        let mut aliases = SetupBindings::default();
        aliases.outputs.insert(local.to_string(), portable);
        let mut references = AuthoredReferences::default();
        references.observe_device(local, "", 0);
        references.observe_output(&local.to_string(), "Zebra desk", 30);
        references.assign_aliases(&mut aliases);
        assert_eq!(aliases.devices.get(&local), Some(&portable));
        assert_eq!(aliases.outputs.get(&local.to_string()), Some(&portable));

        let mut manifest = PortableSetupV1::new(
            Uuid::now_v7(),
            "Desk".into(),
            PortableOutputState {
                canvas_width: 640,
                canvas_height: 480,
                brightness: 1.0,
                enabled: true,
                paused: false,
            },
        );
        references.retain_definitions(&mut manifest, None, &aliases);
        assert_eq!(manifest.devices.len(), 1);
        let device = &manifest.devices[&portable];
        assert_eq!(device.name, "Zebra desk");
        assert_eq!(device.led_count, 30);
        assert!(device.portable_key.is_none());
        assert!(device.binding_hint.is_none());
    }
}
