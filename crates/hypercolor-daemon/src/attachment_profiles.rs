//! Persisted device attachment profile store.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

use hypercolor_types::attachment::{ComponentSlot, DeviceComponentProfile};
use hypercolor_types::device::DeviceInfo;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use crate::domain::device_binding::{DeviceBindingRemaps, MigrationPersistence};
use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteReservation, serialize_json_pretty,
    write_atomic,
};

/// Persistent attachment profile store keyed by physical device ID.
#[derive(Debug, Clone)]
pub struct ComponentProfileStore {
    profiles: HashMap<String, DeviceComponentProfile>,
    path: PathBuf,
}

pub(crate) struct ComponentProfileBindingMigration {
    source: HashMap<String, DeviceComponentProfile>,
    candidate: HashMap<String, DeviceComponentProfile>,
    write: AtomicWriteReservation,
    payload: Vec<u8>,
    migrated: usize,
}

pub(crate) struct AdmittedComponentProfileBindingMigration {
    source: HashMap<String, DeviceComponentProfile>,
    candidate: HashMap<String, DeviceComponentProfile>,
    write: AdmittedAtomicWrite,
    migrated: usize,
}

pub(crate) struct PersistedComponentProfileBindingMigration {
    source: HashMap<String, DeviceComponentProfile>,
    candidate: HashMap<String, DeviceComponentProfile>,
    migrated: usize,
}

pub(crate) struct ComponentProfileBindingPublication {
    store: OwnedRwLockWriteGuard<ComponentProfileStore>,
    candidate: Option<HashMap<String, DeviceComponentProfile>>,
    migrated: usize,
}

impl ComponentProfileStore {
    /// Create an empty attachment profile store for the given file path.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            profiles: HashMap::new(),
            path,
        }
    }

    /// Load persisted attachment profiles from disk.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }

        let raw = fs::read_to_string(path).with_context(|| {
            format!(
                "failed to read attachment profile store at {}",
                path.display()
            )
        })?;
        let profiles: HashMap<String, DeviceComponentProfile> = serde_json::from_str(&raw)
            .with_context(|| {
                format!(
                    "failed to parse attachment profile store at {}",
                    path.display()
                )
            })?;

        Ok(Self {
            profiles,
            path: path.to_path_buf(),
        })
    }

    /// Persist attachment profiles with atomic replace semantics.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create attachment profile store directory {}",
                    parent.display()
                )
            })?;
        }

        let payload = serialize_profiles(&self.profiles)?;

        write_atomic(&self.path, &payload).context("failed to persist attachment profile store")?;

        Ok(())
    }

    /// Get a stored profile by physical device ID.
    #[must_use]
    pub fn get(&self, device_id: &str) -> Option<&DeviceComponentProfile> {
        self.profiles.get(device_id)
    }

    /// Get the stored profile for a device, or derive a default one from current zones.
    #[must_use]
    pub fn get_or_default(&self, device: &DeviceInfo) -> DeviceComponentProfile {
        let device_id = device.id.to_string();

        if let Some(profile) = self.profiles.get(&device_id) {
            let slots = device.default_attachment_profile().slots;
            let mut profile = profile.clone();
            profile.slots = merge_slots_preserving_ids(&profile.slots, &slots);
            return profile;
        }

        device.default_attachment_profile()
    }

    /// Insert or replace a stored profile.
    pub fn update(&mut self, device_id: &str, profile: DeviceComponentProfile) {
        self.profiles.insert(device_id.to_owned(), profile);
    }

    /// Remove a stored profile.
    pub fn remove(&mut self, device_id: &str) -> Option<DeviceComponentProfile> {
        self.profiles.remove(device_id)
    }

    /// Whether any stored profile binds the given template ID.
    #[must_use]
    pub fn uses_template(&self, template_id: &str) -> bool {
        self.profiles.values().any(|profile| {
            profile
                .bindings
                .iter()
                .any(|binding| binding.template_id == template_id)
        })
    }

    pub(crate) fn prepare_binding_migration(
        &self,
        remaps: &DeviceBindingRemaps,
    ) -> anyhow::Result<Option<ComponentProfileBindingMigration>> {
        let source = self.profiles.clone();
        let mut candidate = source.clone();
        let migrated = remap_string_keys(
            &mut candidate,
            remaps
                .physical_device_ids
                .iter()
                .map(|(legacy, canonical)| (legacy.to_string(), canonical.to_string())),
        );
        if migrated == 0 {
            return Ok(None);
        }
        let payload = serialize_profiles(&candidate)?;
        let writer = AtomicFileWriter::new(&self.path)?;
        Ok(Some(ComponentProfileBindingMigration {
            source,
            candidate,
            write: writer.reserve(),
            payload,
            migrated,
        }))
    }
}

impl ComponentProfileBindingMigration {
    pub(crate) fn admit(self) -> AdmittedComponentProfileBindingMigration {
        AdmittedComponentProfileBindingMigration {
            source: self.source,
            candidate: self.candidate,
            write: self.write.admit(self.payload),
            migrated: self.migrated,
        }
    }
}

impl AdmittedComponentProfileBindingMigration {
    pub(crate) fn persist(
        self,
    ) -> (
        PersistedComponentProfileBindingMigration,
        MigrationPersistence,
    ) {
        let persistence = MigrationPersistence::from_commit(self.write.commit_stage_aware());
        (
            PersistedComponentProfileBindingMigration {
                source: self.source,
                candidate: self.candidate,
                migrated: self.migrated,
            },
            persistence,
        )
    }
}

impl ComponentProfileBindingPublication {
    pub(crate) async fn prepare(
        store: Arc<RwLock<ComponentProfileStore>>,
        migration: PersistedComponentProfileBindingMigration,
    ) -> anyhow::Result<Self> {
        let store = store.write_owned().await;
        anyhow::ensure!(
            store.profiles == migration.source,
            "device binding migration was superseded by newer attachment profiles"
        );
        Ok(Self {
            store,
            candidate: Some(migration.candidate),
            migrated: migration.migrated,
        })
    }

    pub(crate) fn publish(&mut self) -> usize {
        self.store.profiles = self
            .candidate
            .take()
            .expect("attachment binding migration must publish exactly once");
        self.migrated
    }
}

fn serialize_profiles(
    profiles: &HashMap<String, DeviceComponentProfile>,
) -> anyhow::Result<Vec<u8>> {
    let ordered = profiles
        .iter()
        .map(|(device_id, profile)| (device_id.clone(), profile.clone()))
        .collect::<BTreeMap<_, _>>();
    serialize_json_pretty(&ordered).context("failed to serialize attachment profile store")
}

fn remap_string_keys<T>(
    values: &mut HashMap<String, T>,
    remaps: impl IntoIterator<Item = (String, String)>,
) -> usize {
    let mut migrated = 0;
    for (legacy, canonical) in remaps {
        let Some(value) = values.remove(&legacy) else {
            continue;
        };
        values.entry(canonical).or_insert(value);
        migrated += 1;
    }
    migrated
}

/// Carry stored slot ids onto freshly derived slots so bindings survive a
/// segment rename.
///
/// A slot is matched by its LED range when that range is unique among the
/// stored slots. Zero-LED segments all share one `(start, 0)` range, so
/// they are matched by name instead; an id that would collide with one
/// already handed out keeps the derived id rather than merging two slots.
fn merge_slots_preserving_ids(
    stored_slots: &[ComponentSlot],
    current_slots: &[ComponentSlot],
) -> Vec<ComponentSlot> {
    let mut stored_by_range: HashMap<(u32, u32), Vec<&ComponentSlot>> = HashMap::new();
    for slot in stored_slots {
        stored_by_range
            .entry((slot.led_start, slot.led_count))
            .or_default()
            .push(slot);
    }

    let mut taken_ids: HashSet<String> = HashSet::new();
    current_slots
        .iter()
        .map(|slot| {
            let previous_slot = match stored_by_range
                .get(&(slot.led_start, slot.led_count))
                .map(Vec::as_slice)
            {
                Some([only]) => Some(*only),
                Some(candidates) => candidates
                    .iter()
                    .copied()
                    .find(|candidate| candidate.name == slot.name)
                    .or_else(|| {
                        candidates
                            .iter()
                            .copied()
                            .find(|candidate| candidate.id == slot.id)
                    }),
                None => None,
            };

            let mut merged = slot.clone();
            if let Some(previous_slot) = previous_slot
                && !taken_ids.contains(&previous_slot.id)
            {
                merged.id.clone_from(&previous_slot.id);
            }
            taken_ids.insert(merged.id.clone());
            merged
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use hypercolor_types::attachment::ComponentBinding;
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
        DeviceOrigin, DeviceTopologyHint, SegmentInfo,
    };

    use super::ComponentProfileStore;

    #[test]
    fn get_or_default_preserves_slot_ids_when_zone_names_change() {
        let original = DeviceInfo {
            id: DeviceId::new(),
            name: "Network Area".to_owned(),
            vendor: "Network Vendor".to_owned(),
            family: DeviceFamily::new_static("network-driver", "Network Driver"),
            model: Some("Network Bridge".to_owned()),
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native(
                "network-driver",
                "network-backend",
                ConnectionType::Network,
            ),
            segments: vec![
                SegmentInfo {
                    name: "Channel 0".to_owned(),
                    led_count: 1,
                    topology: DeviceTopologyHint::Point,
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                },
                SegmentInfo {
                    name: "Channel 1".to_owned(),
                    led_count: 1,
                    topology: DeviceTopologyHint::Point,
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                },
            ],
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        };
        let renamed = DeviceInfo {
            segments: vec![
                SegmentInfo {
                    name: "Left Zone".to_owned(),
                    led_count: 1,
                    topology: DeviceTopologyHint::Point,
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                },
                SegmentInfo {
                    name: "Right Zone".to_owned(),
                    led_count: 1,
                    topology: DeviceTopologyHint::Point,
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                },
            ],
            ..original.clone()
        };
        let mut store = ComponentProfileStore::new(PathBuf::from("attachment-profiles-test.json"));
        let mut profile = original.default_attachment_profile();
        let original_slot_ids = profile
            .slots
            .iter()
            .map(|slot| slot.id.clone())
            .collect::<Vec<_>>();
        profile.bindings = vec![ComponentBinding {
            slot_id: original_slot_ids[0].clone(),
            template_id: "dummy-template".to_owned(),
            name: None,
            enabled: true,
            instances: 1,
            led_offset: 0,
        }];
        store.update(&original.id.to_string(), profile);

        let resolved = store.get_or_default(&renamed);

        assert_eq!(resolved.slots[0].name, "Left Zone");
        assert_eq!(resolved.slots[1].name, "Right Zone");
        assert_eq!(resolved.slots[0].id, original_slot_ids[0]);
        assert_eq!(resolved.slots[1].id, original_slot_ids[1]);
        assert!(
            resolved
                .slots
                .iter()
                .any(|slot| slot.id == resolved.bindings[0].slot_id)
        );
    }

    fn channel_device(led_counts: &[u32]) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(),
            name: "Nollie 32".to_owned(),
            vendor: "Nollie".to_owned(),
            family: DeviceFamily::new_static("nollie", "Nollie"),
            model: None,
            connection_type: ConnectionType::Usb,
            origin: DeviceOrigin::native("nollie", "usb", ConnectionType::Usb),
            segments: led_counts
                .iter()
                .enumerate()
                .map(|(index, led_count)| SegmentInfo {
                    name: format!("Channel {}", index + 1),
                    led_count: *led_count,
                    topology: DeviceTopologyHint::Strip,
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                })
                .collect(),
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        }
    }

    /// Several zero-LED channels share one `(led_start, 0)` range; merging a
    /// stored profile back onto them must not collapse their ids onto the
    /// last stored slot.
    #[test]
    fn get_or_default_keeps_zero_led_slot_ids_unique() {
        let device = channel_device(&[30, 30, 0, 0, 0, 0]);
        let default_ids = device
            .default_attachment_profile()
            .slots
            .iter()
            .map(|slot| slot.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            default_ids,
            vec![
                "channel-1",
                "channel-2",
                "channel-3",
                "channel-4",
                "channel-5",
                "channel-6"
            ]
        );

        let mut store = ComponentProfileStore::new(PathBuf::from("/tmp/unused.json"));
        store.update(&device.id.to_string(), device.default_attachment_profile());

        let merged_ids = store
            .get_or_default(&device)
            .slots
            .iter()
            .map(|slot| slot.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(merged_ids, default_ids, "zero-LED slots keep their own ids");

        let unique: std::collections::HashSet<_> = merged_ids.iter().collect();
        assert_eq!(unique.len(), merged_ids.len(), "slot ids must be unique");
    }

    /// The legitimate case the range merge exists for: a renamed zero-LED
    /// segment still maps to its stored slot when the range is unique.
    #[test]
    fn merge_preserves_a_unique_zero_led_slot_id_across_a_rename() {
        let stored = channel_device(&[30, 0]);
        let mut renamed = stored.clone();
        renamed.segments[1].name = "Rear Fans".to_owned();

        let mut store = ComponentProfileStore::new(PathBuf::from("/tmp/unused.json"));
        store.update(&stored.id.to_string(), stored.default_attachment_profile());
        renamed.id = stored.id;

        let merged = store.get_or_default(&renamed);
        assert_eq!(merged.slots[1].id, "channel-2");
        assert_eq!(merged.slots[1].name, "Rear Fans");
    }
}
