//! Conservative reconciliation for persisted, machine-local device bindings.

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use hypercolor_core::device::{DeviceLifecycleManager, DeviceRegistry};
use hypercolor_types::device::{DeviceFingerprint, DeviceId, DeviceInfo, DriverTransportKind};
use hypercolor_types::spatial::{Output, SpatialLayout};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tracing::info;

use crate::attachment_profiles::{
    ComponentProfileBindingMigration, ComponentProfileBindingPublication, ComponentProfileStore,
    PersistedComponentProfileBindingMigration,
};
use crate::device_settings::{
    DeviceSettingsAccess, DeviceSettingsBindingMigration, DeviceSettingsBindingPublication,
    PersistedDeviceSettingsBindingMigration,
};
use crate::display_preferences::{
    DisplayPreferencesDeviceBindingMigration, DisplayPreferencesDeviceBindingPublication,
    DisplayPreferencesStore, PersistedDisplayPreferencesDeviceBindingMigration,
};
use crate::domain::layout::{
    LayoutContext, LayoutDeviceBindingMigration, LayoutDeviceBindingPublication,
    PersistedLayoutDeviceBindingMigration,
};
use crate::logical_devices::{
    LogicalDevice, LogicalDeviceBindingMigration, LogicalDeviceBindingPublication,
    LogicalDeviceStoreAuthority, PersistedLogicalDeviceBindingMigration,
};
use crate::persistence::{AtomicFileWriter, AtomicWriteCommitResult, serialize_json_pretty};

const MAX_BINDING_MIGRATION_ATTEMPTS: usize = 3;
const DEVICE_BINDING_JOURNAL_SCHEMA_VERSION: u32 = 1;
pub(crate) const DEVICE_BINDING_MIGRATION_JOURNAL_FILE: &str = "device-binding-migration.json";

#[derive(Clone)]
#[doc(hidden)]
pub struct DeviceBindingMigrationContext {
    layout: LayoutContext,
    logical_devices: LogicalDeviceStoreAuthority,
    attachment_profiles: Arc<RwLock<ComponentProfileStore>>,
    device_settings: DeviceSettingsAccess,
    display_preferences: Arc<RwLock<DisplayPreferencesStore>>,
    journal: DeviceBindingMigrationJournal,
}

struct PreparedDeviceBindingMigration {
    layout: LayoutDeviceBindingMigration,
    logical_devices: Option<LogicalDeviceBindingMigration>,
    attachment_profiles: Option<ComponentProfileBindingMigration>,
    device_settings: Option<DeviceSettingsBindingMigration>,
    display_preferences: Option<DisplayPreferencesDeviceBindingMigration>,
}

struct PersistedDeviceBindingMigration {
    layout: PersistedLayoutDeviceBindingMigration,
    logical_devices: Option<PersistedLogicalDeviceBindingMigration>,
    attachment_profiles: Option<PersistedComponentProfileBindingMigration>,
    device_settings: Option<PersistedDeviceSettingsBindingMigration>,
    display_preferences: Option<PersistedDisplayPreferencesDeviceBindingMigration>,
}

struct DeviceBindingPublication {
    layout: LayoutDeviceBindingPublication,
    logical_devices: Option<LogicalDeviceBindingPublication>,
    attachment_profiles: Option<ComponentProfileBindingPublication>,
    device_settings: Option<DeviceSettingsBindingPublication>,
    display_preferences: Option<DisplayPreferencesDeviceBindingPublication>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DeviceBindingMigrationReport {
    pub(crate) mappings: usize,
    pub(crate) references: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceBindingRemaps {
    pub(crate) layout_device_ids: HashMap<String, String>,
    pub(crate) physical_device_ids: HashMap<DeviceId, DeviceId>,
    pub(crate) persisted_setting_keys: HashMap<String, String>,
}

#[derive(Clone)]
struct DeviceBindingMigrationJournal {
    path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceBindingMigrationJournalDocument {
    schema_version: u32,
    remaps: Option<DeviceBindingRemaps>,
}

#[derive(Debug)]
pub(crate) enum MigrationPersistence {
    Durable,
    Superseded,
    Failed(String),
}

impl MigrationPersistence {
    pub(crate) fn from_commit(outcome: AtomicWriteCommitResult) -> Self {
        match outcome {
            AtomicWriteCommitResult::DurableWritten => Self::Durable,
            AtomicWriteCommitResult::Superseded => Self::Superseded,
            AtomicWriteCommitResult::FailedBeforeReplacement(error)
            | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                Self::Failed(error.to_string())
            }
        }
    }
}

impl DeviceBindingMigrationJournal {
    const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn load(&self) -> anyhow::Result<Option<DeviceBindingRemaps>> {
        let payload = match std::fs::read(&self.path) {
            Ok(payload) => payload,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to read device binding migration journal at {}",
                        self.path.display()
                    )
                });
            }
        };
        let document: DeviceBindingMigrationJournalDocument = serde_json::from_slice(&payload)
            .with_context(|| {
                format!(
                    "failed to parse device binding migration journal at {}",
                    self.path.display()
                )
            })?;
        anyhow::ensure!(
            document.schema_version == DEVICE_BINDING_JOURNAL_SCHEMA_VERSION,
            "device binding migration journal at {} uses unsupported schema version {}; expected {}",
            self.path.display(),
            document.schema_version,
            DEVICE_BINDING_JOURNAL_SCHEMA_VERSION
        );
        Ok(document.remaps)
    }

    fn persist_active(&self, remaps: &DeviceBindingRemaps) -> anyhow::Result<()> {
        self.persist(Some(remaps.clone()))
    }

    fn clear(&self) -> anyhow::Result<()> {
        self.persist(None)
    }

    fn persist(&self, remaps: Option<DeviceBindingRemaps>) -> anyhow::Result<()> {
        let payload = serialize_json_pretty(&DeviceBindingMigrationJournalDocument {
            schema_version: DEVICE_BINDING_JOURNAL_SCHEMA_VERSION,
            remaps,
        })
        .context("failed to serialize device binding migration journal")?;
        let outcome = AtomicFileWriter::new(&self.path)?
            .reserve()
            .admit(payload)
            .commit_stage_aware();
        match outcome {
            AtomicWriteCommitResult::DurableWritten => Ok(()),
            AtomicWriteCommitResult::Superseded => {
                anyhow::bail!("device binding migration journal write was superseded")
            }
            AtomicWriteCommitResult::FailedBeforeReplacement(error)
            | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => Err(error)
                .with_context(|| {
                    format!(
                        "failed to persist device binding migration journal at {}",
                        self.path.display()
                    )
                }),
        }
    }
}

impl DeviceBindingRemaps {
    pub(crate) fn remap_layout(&self, layout: &mut SpatialLayout) -> usize {
        layout
            .zones
            .iter_mut()
            .map(|output| {
                self.layout_device_ids
                    .get(&output.device_id)
                    .map_or(0, |canonical| {
                        output.device_id.clone_from(canonical);
                        1
                    })
            })
            .sum()
    }

    pub(crate) fn remap_layout_device_id_set(&self, ids: &mut HashSet<String>) -> usize {
        let replacements = ids
            .iter()
            .filter_map(|legacy| {
                self.layout_device_ids
                    .get(legacy)
                    .map(|canonical| (legacy.clone(), canonical.clone()))
            })
            .collect::<Vec<_>>();
        for (legacy, canonical) in &replacements {
            ids.remove(legacy);
            ids.insert(canonical.clone());
        }
        replacements.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum BindingClass {
    ClaimlessUsb {
        owner: String,
        vendor_id: u16,
        product_id: u16,
    },
    SmBus {
        owner: String,
        protocol_or_model: String,
        address: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct SegmentShape {
    name: String,
    led_count: u32,
}

#[derive(Debug, Clone)]
pub(super) struct CurrentBinding {
    pub layout_device_id: String,
    pub physical_device_id: DeviceId,
    fingerprint: DeviceFingerprint,
    device_info: DeviceInfo,
    class: BindingClass,
    segment_shape: Vec<SegmentShape>,
}

#[derive(Debug, Default)]
pub(crate) struct PersistedBindingEvidence {
    shapes: HashMap<String, Vec<Vec<SegmentShape>>>,
    descriptor_tokens: HashMap<String, Vec<Vec<String>>>,
}

impl PersistedBindingEvidence {
    pub(super) fn observe_layout(&mut self, layout: &SpatialLayout) {
        let mut outputs_by_device = HashMap::<&str, Vec<&Output>>::new();
        for output in &layout.zones {
            outputs_by_device
                .entry(output.device_id.as_str())
                .or_default()
                .push(output);
        }
        for (layout_device_id, outputs) in outputs_by_device {
            let descriptor_tokens = output_descriptor_tokens(&outputs);
            let shape = output_shape(outputs);
            if shape.is_empty() {
                continue;
            }
            let observed = self.shapes.entry(layout_device_id.to_owned()).or_default();
            if !observed.contains(&shape) {
                observed.push(shape);
            }
            if !descriptor_tokens.is_empty() {
                let observed = self
                    .descriptor_tokens
                    .entry(layout_device_id.to_owned())
                    .or_default();
                if !observed.contains(&descriptor_tokens) {
                    observed.push(descriptor_tokens);
                }
            }
        }
    }

    pub(super) fn layout_device_ids(&self) -> impl Iterator<Item = &str> {
        self.shapes.keys().map(String::as_str)
    }
}

impl CurrentBinding {
    pub(super) fn from_discovery(
        info: &DeviceInfo,
        fingerprint: &DeviceFingerprint,
        layout_device_id: String,
        has_portable_claim: bool,
    ) -> Option<Self> {
        let encoded = fingerprint.as_str();
        let class = match &info.origin.transport {
            DriverTransportKind::Usb if !has_portable_claim => {
                let owner = normalized_component(&info.origin.driver_id);
                let (owner, vendor_id, product_id) = parse_usb_fingerprint(encoded, &owner)?;
                BindingClass::ClaimlessUsb {
                    owner,
                    vendor_id,
                    product_id,
                }
            }
            DriverTransportKind::Smbus => {
                let owner = normalized_component(&info.origin.driver_id);
                let (owner, address) = parse_smbus_fingerprint(encoded, &owner)?;
                let protocol_or_model = info
                    .model
                    .as_deref()
                    .or(info.origin.protocol_id.as_deref())?
                    .trim()
                    .to_ascii_lowercase();
                if protocol_or_model.is_empty() {
                    return None;
                }
                BindingClass::SmBus {
                    owner,
                    protocol_or_model,
                    address,
                }
            }
            DriverTransportKind::Network
            | DriverTransportKind::Midi
            | DriverTransportKind::Serial
            | DriverTransportKind::Virtual
            | DriverTransportKind::Bridge
            | DriverTransportKind::Custom(_)
            | DriverTransportKind::Usb => return None,
        };
        let segment_shape = device_shape(info);
        if segment_shape.is_empty() {
            return None;
        }
        Some(Self {
            layout_device_id,
            physical_device_id: info.id,
            fingerprint: fingerprint.clone(),
            device_info: info.clone(),
            class,
            segment_shape,
        })
    }
}

impl DeviceBindingMigrationContext {
    #[doc(hidden)]
    pub fn new(
        layout: LayoutContext,
        logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
        logical_devices_path: PathBuf,
        attachment_profiles: Arc<RwLock<ComponentProfileStore>>,
        device_settings: DeviceSettingsAccess,
        display_preferences: Arc<RwLock<DisplayPreferencesStore>>,
        journal_path: PathBuf,
    ) -> Self {
        Self {
            layout,
            logical_devices: LogicalDeviceStoreAuthority::new(
                logical_devices,
                logical_devices_path,
            ),
            attachment_profiles,
            device_settings,
            display_preferences,
            journal: DeviceBindingMigrationJournal::new(journal_path),
        }
    }

    pub(crate) async fn reconcile_complete_sweep(
        &self,
        registry: &DeviceRegistry,
        lifecycle: &Arc<Mutex<DeviceLifecycleManager>>,
        seen_ids: &HashSet<DeviceId>,
    ) -> anyhow::Result<DeviceBindingMigrationReport> {
        let remaps = if let Some(remaps) = self.journal.load()? {
            anyhow::ensure!(
                !remaps.layout_device_ids.is_empty(),
                "active device binding migration journal contains no layout remaps"
            );
            info!(
                path = %self.journal.path.display(),
                mappings = remaps.layout_device_ids.len(),
                "Replaying durable device binding migration intent"
            );
            remaps
        } else {
            let evidence = self.layout.collect_binding_evidence().await;
            let current = collect_current_bindings(registry, lifecycle, seen_ids).await;
            let persisted_binding_keys = self.device_settings.persisted_binding_keys().await;
            let remaps = build_binding_remaps(&evidence, &current, &persisted_binding_keys);
            if remaps.layout_device_ids.is_empty() {
                return Ok(DeviceBindingMigrationReport::default());
            }
            self.journal.persist_active(&remaps)?;
            remaps
        };

        for attempt in 1..=MAX_BINDING_MIGRATION_ATTEMPTS {
            let prepared = self.prepare(&remaps).await?;
            let (persisted, persistence) = prepared.persist();
            if migration_was_superseded(&persistence) {
                anyhow::ensure!(
                    attempt < MAX_BINDING_MIGRATION_ATTEMPTS,
                    "device binding migration was repeatedly superseded; a later discovery sweep \
                     will retry"
                );
                tokio::task::yield_now().await;
                continue;
            }
            let failures = persistence
                .into_iter()
                .filter_map(|outcome| match outcome {
                    MigrationPersistence::Failed(error) => Some(error),
                    MigrationPersistence::Durable | MigrationPersistence::Superseded => None,
                })
                .collect::<Vec<_>>();
            if !failures.is_empty() {
                anyhow::bail!(
                    "device binding migration was not published because durable persistence \
                     failed: {}",
                    failures.join("; ")
                );
            }
            let Some(persisted) = persisted else {
                anyhow::bail!("device binding migration stopped before the layout evidence commit");
            };

            self.layout
                .converge_persisted_device_binding(&persisted.layout)
                .await?;

            let Ok(mut publication) = self.prepare_publication(persisted).await else {
                anyhow::ensure!(
                    attempt < MAX_BINDING_MIGRATION_ATTEMPTS,
                    "device binding migration publication was repeatedly superseded; a later \
                     discovery sweep will retry"
                );
                tokio::task::yield_now().await;
                continue;
            };
            let references = publication.publish(self);
            self.journal.clear()?;
            return Ok(DeviceBindingMigrationReport {
                mappings: remaps.layout_device_ids.len(),
                references,
            });
        }
        unreachable!("device binding migration attempts return or fail at their bound")
    }

    async fn prepare(
        &self,
        remaps: &DeviceBindingRemaps,
    ) -> anyhow::Result<PreparedDeviceBindingMigration> {
        let layout = self.layout.prepare_device_binding_migration(remaps).await?;
        let logical_devices = self
            .logical_devices
            .prepare_binding_migration(remaps)
            .await?;
        let attachment_profiles = self
            .attachment_profiles
            .read()
            .await
            .prepare_binding_migration(remaps)?;
        let device_settings = self
            .device_settings
            .prepare_binding_migration(remaps)
            .await?;
        let display_preferences = self
            .display_preferences
            .read()
            .await
            .prepare_device_binding_migration(remaps)?;
        Ok(PreparedDeviceBindingMigration {
            layout,
            logical_devices,
            attachment_profiles,
            device_settings,
            display_preferences,
        })
    }

    async fn prepare_publication(
        &self,
        migration: PersistedDeviceBindingMigration,
    ) -> anyhow::Result<DeviceBindingPublication> {
        let layout = self
            .layout
            .prepare_device_binding_publication(migration.layout)
            .await?;
        let logical_devices = match migration.logical_devices {
            Some(migration) => Some(
                self.logical_devices
                    .prepare_binding_publication(migration)
                    .await?,
            ),
            None => None,
        };
        let attachment_profiles = match migration.attachment_profiles {
            Some(migration) => Some(
                ComponentProfileBindingPublication::prepare(
                    Arc::clone(&self.attachment_profiles),
                    migration,
                )
                .await?,
            ),
            None => None,
        };
        let device_settings = match migration.device_settings {
            Some(migration) => Some(
                self.device_settings
                    .prepare_binding_publication(migration)
                    .await?,
            ),
            None => None,
        };
        let display_preferences = match migration.display_preferences {
            Some(migration) => Some(
                DisplayPreferencesDeviceBindingPublication::prepare(
                    Arc::clone(&self.display_preferences),
                    migration,
                )
                .await?,
            ),
            None => None,
        };
        Ok(DeviceBindingPublication {
            layout,
            logical_devices,
            attachment_profiles,
            device_settings,
            display_preferences,
        })
    }
}

fn migration_was_superseded(persistence: &[MigrationPersistence]) -> bool {
    persistence
        .iter()
        .any(|outcome| matches!(outcome, MigrationPersistence::Superseded))
}

impl PreparedDeviceBindingMigration {
    fn persist(
        self,
    ) -> (
        Option<PersistedDeviceBindingMigration>,
        Vec<MigrationPersistence>,
    ) {
        let Self {
            layout,
            logical_devices,
            attachment_profiles,
            device_settings,
            display_preferences,
        } = self;
        let mut persistence = Vec::new();
        let (logical_devices, logical_persistence) = logical_devices.map_or_else(
            || (None, MigrationPersistence::Durable),
            |migration| {
                let (persisted, outcome) = migration.admit().persist();
                (Some(persisted), outcome)
            },
        );
        let stop = !matches!(logical_persistence, MigrationPersistence::Durable);
        persistence.push(logical_persistence);
        if stop {
            return (None, persistence);
        }
        let (attachment_profiles, profile_persistence) = attachment_profiles.map_or_else(
            || (None, MigrationPersistence::Durable),
            |migration| {
                let (persisted, outcome) = migration.admit().persist();
                (Some(persisted), outcome)
            },
        );
        let stop = !matches!(profile_persistence, MigrationPersistence::Durable);
        persistence.push(profile_persistence);
        if stop {
            return (None, persistence);
        }
        let (device_settings, settings_persistence) = device_settings.map_or_else(
            || (None, MigrationPersistence::Durable),
            |migration| {
                let (persisted, outcome) = migration.admit().persist();
                (Some(persisted), outcome)
            },
        );
        let stop = !matches!(settings_persistence, MigrationPersistence::Durable);
        persistence.push(settings_persistence);
        if stop {
            return (None, persistence);
        }
        let (display_preferences, display_persistence) = display_preferences.map_or_else(
            || (None, MigrationPersistence::Durable),
            |migration| {
                let (persisted, outcome) = migration.admit().persist();
                (Some(persisted), outcome)
            },
        );
        let stop = !matches!(display_persistence, MigrationPersistence::Durable);
        persistence.push(display_persistence);
        if stop {
            return (None, persistence);
        }
        let (layout, layout_persistence) = layout.persist();
        persistence.extend(layout_persistence);
        let Some(layout) = layout else {
            return (None, persistence);
        };
        (
            Some(PersistedDeviceBindingMigration {
                layout,
                logical_devices,
                attachment_profiles,
                device_settings,
                display_preferences,
            }),
            persistence,
        )
    }
}

impl DeviceBindingPublication {
    fn publish(&mut self, context: &DeviceBindingMigrationContext) -> usize {
        let mut migrated = self.layout.publish(&context.layout);
        if let Some(logical_devices) = self.logical_devices.as_mut() {
            migrated += logical_devices.publish();
        }
        if let Some(attachment_profiles) = self.attachment_profiles.as_mut() {
            migrated += attachment_profiles.publish();
        }
        if let Some(device_settings) = self.device_settings.as_mut() {
            migrated += device_settings.publish();
        }
        if let Some(display_preferences) = self.display_preferences.as_mut() {
            migrated += display_preferences.publish();
        }
        migrated
    }
}

async fn collect_current_bindings(
    registry: &DeviceRegistry,
    lifecycle: &Arc<Mutex<DeviceLifecycleManager>>,
    seen_ids: &HashSet<DeviceId>,
) -> Vec<CurrentBinding> {
    let tracked = registry.list().await;
    let lifecycle_ids = {
        let lifecycle = lifecycle.lock().await;
        tracked
            .iter()
            .map(|tracked| {
                (
                    tracked.info.id,
                    lifecycle
                        .layout_device_id_for(tracked.info.id)
                        .map(ToOwned::to_owned),
                )
            })
            .collect::<HashMap<_, _>>()
    };
    let mut current = Vec::new();
    for tracked in tracked {
        if !seen_ids.contains(&tracked.info.id) {
            continue;
        }
        let Some(fingerprint) = registry.fingerprint_for_id(&tracked.info.id).await else {
            continue;
        };
        let layout_device_id = lifecycle_ids
            .get(&tracked.info.id)
            .and_then(Clone::clone)
            .unwrap_or_else(|| {
                DeviceLifecycleManager::canonical_layout_device_id(
                    &tracked.info,
                    Some(&fingerprint),
                )
            });
        let has_portable_claim = registry.claim_for_id(&tracked.info.id).await.is_some();
        if let Some(binding) = CurrentBinding::from_discovery(
            &tracked.info,
            &fingerprint,
            layout_device_id,
            has_portable_claim,
        ) {
            current.push(binding);
        }
    }
    current
}

fn build_binding_remaps(
    evidence: &PersistedBindingEvidence,
    current: &[CurrentBinding],
    persisted_binding_keys: &HashSet<String>,
) -> DeviceBindingRemaps {
    let layout_device_ids = plan_layout_device_id_remaps(evidence, current);
    let current_by_layout_id = current
        .iter()
        .map(|binding| (binding.layout_device_id.as_str(), binding))
        .collect::<HashMap<_, _>>();
    let mut physical_device_ids = HashMap::new();
    let mut persisted_setting_keys = HashMap::new();
    for (legacy_layout_id, canonical_layout_id) in &layout_device_ids {
        let Some(binding) = current_by_layout_id.get(canonical_layout_id.as_str()) else {
            continue;
        };
        let candidates = persisted_binding_keys
            .iter()
            .filter(|key| exact_fingerprint_evidence_matches(legacy_layout_id, key, binding))
            .collect::<Vec<_>>();
        let [legacy_fingerprint] = candidates.as_slice() else {
            continue;
        };
        let legacy_physical_id =
            DeviceFingerprint::from_persisted((*legacy_fingerprint).clone()).stable_device_id();
        physical_device_ids.insert(legacy_physical_id, binding.physical_device_id);
        persisted_setting_keys.insert(
            (*legacy_fingerprint).clone(),
            binding.fingerprint.as_str().to_owned(),
        );
        persisted_setting_keys.insert(
            legacy_physical_id.to_string(),
            binding.physical_device_id.to_string(),
        );
    }
    DeviceBindingRemaps {
        layout_device_ids,
        physical_device_ids,
        persisted_setting_keys,
    }
}

fn exact_fingerprint_evidence_matches(
    legacy_layout_id: &str,
    key: &str,
    binding: &CurrentBinding,
) -> bool {
    if !binding_fingerprint_matches_class(key, &binding.class) {
        return false;
    }
    let fingerprint = DeviceFingerprint::from_persisted(key.to_owned());
    DeviceLifecycleManager::canonical_layout_device_id(&binding.device_info, Some(&fingerprint))
        == legacy_layout_id
}

pub(super) fn plan_layout_device_id_remaps(
    evidence: &PersistedBindingEvidence,
    current: &[CurrentBinding],
) -> HashMap<String, String> {
    let exact = current
        .iter()
        .map(|binding| binding.layout_device_id.as_str())
        .collect::<HashSet<_>>();
    let mut claimant_count = current
        .iter()
        .map(|binding| usize::from(evidence.shapes.contains_key(&binding.layout_device_id)))
        .collect::<Vec<_>>();
    let mut candidates = HashMap::<String, Vec<usize>>::new();
    for legacy_id in evidence.layout_device_ids() {
        if exact.contains(legacy_id) {
            continue;
        }
        let Some(shapes) = evidence.shapes.get(legacy_id) else {
            continue;
        };
        let matches = current
            .iter()
            .enumerate()
            .filter(|(_, binding)| {
                legacy_id_matches_class(legacy_id, &binding.class, evidence)
                    && shapes.iter().all(|shape| shape == &binding.segment_shape)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if !matches.is_empty() {
            candidates.insert(legacy_id.to_owned(), matches);
        }
    }

    for matches in candidates.values() {
        if let [index] = matches.as_slice() {
            claimant_count[*index] = claimant_count[*index].saturating_add(1);
        }
    }

    candidates
        .into_iter()
        .filter_map(|(legacy_id, matches)| {
            let [index] = matches.as_slice() else {
                return None;
            };
            (claimant_count[*index] == 1)
                .then(|| (legacy_id, current[*index].layout_device_id.clone()))
        })
        .collect()
}

fn legacy_id_matches_class(
    layout_device_id: &str,
    class: &BindingClass,
    evidence: &PersistedBindingEvidence,
) -> bool {
    match class {
        BindingClass::ClaimlessUsb {
            owner,
            vendor_id,
            product_id,
        } => parse_usb_layout_id(layout_device_id).is_some_and(
            |(legacy_owner, legacy_vendor, legacy_product)| {
                legacy_owner == *owner
                    && legacy_vendor == *vendor_id
                    && legacy_product == *product_id
            },
        ),
        BindingClass::SmBus {
            owner,
            protocol_or_model,
            address,
        } => {
            parse_smbus_layout_id(layout_device_id).is_some_and(|(legacy_owner, legacy_address)| {
                legacy_owner == *owner
                    && legacy_address == *address
                    && persisted_descriptor_matches(layout_device_id, protocol_or_model, evidence)
            })
        }
    }
}

fn persisted_descriptor_matches(
    layout_device_id: &str,
    protocol_or_model: &str,
    evidence: &PersistedBindingEvidence,
) -> bool {
    let required = identity_tokens(protocol_or_model);
    !required.is_empty()
        && evidence
            .descriptor_tokens
            .get(layout_device_id)
            .is_some_and(|observed| {
                !observed.is_empty()
                    && observed
                        .iter()
                        .all(|tokens| required.iter().all(|required| tokens.contains(required)))
            })
}

fn binding_fingerprint_matches_class(value: &str, class: &BindingClass) -> bool {
    match class {
        BindingClass::ClaimlessUsb {
            owner,
            vendor_id,
            product_id,
        } => parse_usb_fingerprint(value, owner).is_some_and(
            |(legacy_owner, legacy_vendor, legacy_product)| {
                legacy_owner == *owner
                    && legacy_vendor == *vendor_id
                    && legacy_product == *product_id
            },
        ),
        BindingClass::SmBus { owner, address, .. } => parse_smbus_fingerprint(value, owner)
            .is_some_and(|(legacy_owner, legacy_address)| {
                legacy_owner == *owner && legacy_address == *address
            }),
    }
}

fn parse_usb_fingerprint(value: &str, expected_owner: &str) -> Option<(String, u16, u16)> {
    let rest = value.strip_prefix("usb:")?;
    let mut parts = rest.split(':');
    let first = parts.next()?;
    let (owner, vendor_id, product_id) = if let Some(vendor_id) = parse_four_digit_hex(first) {
        (
            expected_owner.to_owned(),
            vendor_id,
            parse_four_digit_hex(parts.next()?)?,
        )
    } else {
        (
            normalized_component(first),
            parse_four_digit_hex(parts.next()?)?,
            parse_four_digit_hex(parts.next()?)?,
        )
    };
    parts.next()?;
    Some((owner, vendor_id, product_id))
}

fn parse_usb_layout_id(value: &str) -> Option<(String, u16, u16)> {
    let mut parts = value.split(':');
    let owner = normalized_component(parts.next()?);
    let vendor_id = parse_four_digit_hex(parts.next()?)?;
    let product_id = parse_four_digit_hex(parts.next()?)?;
    parts.next()?;
    Some((owner, vendor_id, product_id))
}

fn parse_smbus_fingerprint(value: &str, expected_owner: &str) -> Option<(String, u16)> {
    let rest = value.strip_prefix("smbus:")?;
    let (identity, address) = rest.rsplit_once(':')?;
    let owner = identity
        .split_once(':')
        .filter(|(owner, _)| normalized_component(owner) == expected_owner)
        .map_or_else(
            || expected_owner.to_owned(),
            |(owner, _)| normalized_component(owner),
        );
    Some((owner, parse_hex(address)?))
}

fn parse_smbus_layout_id(value: &str) -> Option<(String, u16)> {
    let (owner, remainder) = value.split_once(':')?;
    let (_, address) = remainder.rsplit_once(':')?;
    Some((normalized_component(owner), parse_hex(address)?))
}

fn parse_four_digit_hex(value: &str) -> Option<u16> {
    (value.len() == 4).then_some(())?;
    parse_hex(value)
}

fn parse_hex(value: &str) -> Option<u16> {
    u16::from_str_radix(value, 16).ok()
}

fn device_shape(info: &DeviceInfo) -> Vec<SegmentShape> {
    let mut shape = info
        .segments
        .iter()
        .filter(|segment| {
            segment.led_count > 0
                && !matches!(
                    segment.topology,
                    hypercolor_types::device::DeviceTopologyHint::Display { .. }
                )
        })
        .map(|segment| SegmentShape {
            name: normalized_component(&segment.name),
            led_count: segment.led_count,
        })
        .collect::<Vec<_>>();
    shape.sort_unstable();
    shape
}

fn output_shape(outputs: Vec<&Output>) -> Vec<SegmentShape> {
    let mut shape = outputs
        .into_iter()
        .filter_map(|output| {
            let name = normalized_component(output.zone_name.as_deref()?);
            (!name.is_empty()).then(|| SegmentShape {
                name,
                led_count: output.topology.led_count(),
            })
        })
        .collect::<Vec<_>>();
    shape.sort_unstable();
    shape
}

fn output_descriptor_tokens(outputs: &[&Output]) -> Vec<String> {
    let mut tokens = outputs
        .iter()
        .flat_map(|output| identity_tokens(&output.name))
        .collect::<Vec<_>>();
    tokens.sort_unstable();
    tokens.dedup();
    tokens
}

fn identity_tokens(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn normalized_component(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
        DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint, SegmentInfo,
    };

    use super::{
        BindingClass, CurrentBinding, MigrationPersistence, PersistedBindingEvidence, SegmentShape,
        build_binding_remaps, migration_was_superseded, plan_layout_device_id_remaps,
    };

    fn usb(layout_device_id: &str, product_id: u16, led_count: u32) -> CurrentBinding {
        let physical_device_id = DeviceId::new();
        CurrentBinding {
            layout_device_id: layout_device_id.to_owned(),
            physical_device_id,
            fingerprint: hypercolor_types::device::DeviceFingerprint::from_persisted(format!(
                "usb:{layout_device_id}"
            )),
            device_info: device_info(
                physical_device_id,
                "Razer Device",
                None,
                "razer",
                ConnectionType::Usb,
                led_count,
            ),
            class: BindingClass::ClaimlessUsb {
                owner: "razer".to_owned(),
                vendor_id: 0x1532,
                product_id,
            },
            segment_shape: vec![SegmentShape {
                name: "main".to_owned(),
                led_count,
            }],
        }
    }

    fn evidence(entries: &[(&str, u32)]) -> PersistedBindingEvidence {
        PersistedBindingEvidence {
            shapes: entries
                .iter()
                .map(|(id, led_count)| {
                    (
                        (*id).to_owned(),
                        vec![vec![SegmentShape {
                            name: "main".to_owned(),
                            led_count: *led_count,
                        }]],
                    )
                })
                .collect::<HashMap<_, _>>(),
            descriptor_tokens: HashMap::new(),
        }
    }

    fn device_info(
        id: DeviceId,
        name: &str,
        model: Option<&str>,
        owner: &str,
        connection_type: ConnectionType,
        led_count: u32,
    ) -> DeviceInfo {
        let backend_id = match connection_type {
            ConnectionType::Usb => "usb",
            ConnectionType::SmBus => "smbus",
            ConnectionType::Network => "network",
            ConnectionType::Bluetooth => "bluetooth",
            ConnectionType::Bridge => "bridge",
        };
        DeviceInfo {
            id,
            name: name.to_owned(),
            vendor: owner.to_owned(),
            family: DeviceFamily::new(owner, owner),
            model: model.map(ToOwned::to_owned),
            connection_type,
            origin: DeviceOrigin::native(owner, backend_id, connection_type),
            segments: vec![SegmentInfo {
                name: "Main".to_owned(),
                led_count,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count,
                ..DeviceCapabilities::default()
            },
        }
    }

    #[test]
    fn exact_layout_ids_win_without_a_rewrite() {
        let current = vec![usb("razer:1532:0099:pci-root", 0x0099, 16)];
        let evidence = evidence(&[("razer:1532:0099:pci-root", 16)]);

        assert!(plan_layout_device_id_remaps(&evidence, &current).is_empty());
    }

    #[test]
    fn exact_layout_id_prevents_a_fuzzy_duplicate_claim() {
        let current = vec![usb("razer:1532:0099:pci-root", 0x0099, 16)];
        let evidence = evidence(&[
            ("razer:1532:0099:pci-root", 16),
            ("razer:1532:0099:001-6-4-4", 16),
        ]);

        assert!(plan_layout_device_id_remaps(&evidence, &current).is_empty());
    }

    #[test]
    fn unique_claimless_usb_class_reconciles_across_host_paths() {
        let current = vec![usb("razer:1532:0099:pci-root", 0x0099, 16)];
        let evidence = evidence(&[("razer:1532:0099:001-6-4-4", 16)]);

        assert_eq!(
            plan_layout_device_id_remaps(&evidence, &current),
            HashMap::from([(
                "razer:1532:0099:001-6-4-4".to_owned(),
                "razer:1532:0099:pci-root".to_owned(),
            )])
        );

        let legacy_fingerprint = "usb:razer:1532:0099:001-6-4-4";
        let remaps = build_binding_remaps(
            &evidence,
            &current,
            &HashSet::from([legacy_fingerprint.to_owned()]),
        );
        let legacy_physical_id =
            DeviceFingerprint::from_persisted(legacy_fingerprint).stable_device_id();
        assert_eq!(
            remaps.physical_device_ids.get(&legacy_physical_id),
            Some(&current[0].physical_device_id)
        );
        assert_eq!(
            remaps.persisted_setting_keys.get(legacy_fingerprint),
            Some(&current[0].fingerprint.as_str().to_owned())
        );

        let without_settings = build_binding_remaps(&evidence, &current, &HashSet::new());
        assert_eq!(
            without_settings.layout_device_ids,
            HashMap::from([(
                "razer:1532:0099:001-6-4-4".to_owned(),
                "razer:1532:0099:pci-root".to_owned(),
            )])
        );
        assert!(without_settings.physical_device_ids.is_empty());
        assert!(without_settings.persisted_setting_keys.is_empty());

        let ambiguous = build_binding_remaps(
            &evidence,
            &current,
            &HashSet::from([
                legacy_fingerprint.to_owned(),
                "usb:razer:1532:0099:001/6/4/4".to_owned(),
            ]),
        );
        assert_eq!(
            ambiguous.layout_device_ids,
            without_settings.layout_device_ids
        );
        assert!(ambiguous.physical_device_ids.is_empty());
        assert!(ambiguous.persisted_setting_keys.is_empty());
    }

    #[test]
    fn ambiguous_usb_class_is_left_untouched() {
        let current = vec![
            usb("razer:1532:0099:pci-left", 0x0099, 16),
            usb("razer:1532:0099:pci-right", 0x0099, 16),
        ];
        let evidence = evidence(&[("razer:1532:0099:001-6-4-4", 16)]);

        assert!(plan_layout_device_id_remaps(&evidence, &current).is_empty());
    }

    #[test]
    fn incompatible_segment_shape_is_left_untouched() {
        let current = vec![usb("razer:1532:0099:pci-root", 0x0099, 24)];
        let evidence = evidence(&[("razer:1532:0099:001-6-4-4", 16)]);

        assert!(plan_layout_device_id_remaps(&evidence, &current).is_empty());
    }

    #[test]
    fn absent_device_is_left_untouched() {
        let evidence = evidence(&[("razer:1532:0099:001-6-4-4", 16)]);

        assert!(plan_layout_device_id_remaps(&evidence, &[]).is_empty());
    }

    #[test]
    fn superseded_participant_restarts_the_transaction() {
        assert!(migration_was_superseded(&[
            MigrationPersistence::Durable,
            MigrationPersistence::Superseded,
        ]));
        assert!(!migration_was_superseded(&[
            MigrationPersistence::Durable,
            MigrationPersistence::Durable,
        ]));
    }

    #[test]
    fn one_current_device_cannot_claim_two_legacy_bindings() {
        let current = vec![usb("razer:1532:0099:pci-root", 0x0099, 16)];
        let evidence = evidence(&[
            ("razer:1532:0099:001-6-4-4", 16),
            ("razer:1532:0099:001-6-4-5", 16),
        ]);

        assert!(plan_layout_device_id_remaps(&evidence, &current).is_empty());
    }

    #[test]
    fn unique_smbus_address_and_descriptor_reconcile_exact_evidence() {
        let physical_device_id = DeviceId::new();
        let current = vec![CurrentBinding {
            layout_device_id: "asus:pawnio:i801:71".to_owned(),
            physical_device_id,
            fingerprint: hypercolor_types::device::DeviceFingerprint::from_persisted(
                "smbus:asus:pawnio:i801:71",
            ),
            device_info: device_info(
                physical_device_id,
                "ASUS Aura Motherboard (SMBus 0x71)",
                Some("asus_aura_smbus_motherboard"),
                "asus",
                ConnectionType::SmBus,
                16,
            ),
            class: BindingClass::SmBus {
                owner: "asus".to_owned(),
                protocol_or_model: "asus_aura_smbus_motherboard".to_owned(),
                address: 0x71,
            },
            segment_shape: vec![SegmentShape {
                name: "main".to_owned(),
                led_count: 16,
            }],
        }];
        let mut evidence = evidence(&[("asus:dev-i2c-9:71", 16)]);
        evidence.descriptor_tokens.insert(
            "asus:dev-i2c-9:71".to_owned(),
            vec![vec![
                "asus".to_owned(),
                "aura".to_owned(),
                "motherboard".to_owned(),
                "smbus".to_owned(),
            ]],
        );

        assert_eq!(
            plan_layout_device_id_remaps(&evidence, &current),
            HashMap::from([(
                "asus:dev-i2c-9:71".to_owned(),
                "asus:pawnio:i801:71".to_owned(),
            )])
        );

        for legacy_fingerprint in ["smbus:asus:/dev/i2c-9:71", "smbus:/dev/i2c-9:71"] {
            let remaps = build_binding_remaps(
                &evidence,
                &current,
                &HashSet::from([legacy_fingerprint.to_owned()]),
            );
            let legacy_physical_id =
                DeviceFingerprint::from_persisted(legacy_fingerprint).stable_device_id();
            assert_eq!(
                remaps.physical_device_ids.get(&legacy_physical_id),
                Some(&current[0].physical_device_id)
            );
            assert_eq!(
                remaps.persisted_setting_keys.get(legacy_fingerprint),
                Some(&current[0].fingerprint.as_str().to_owned())
            );
        }

        evidence.descriptor_tokens.insert(
            "asus:dev-i2c-9:71".to_owned(),
            vec![vec!["asus".to_owned(), "aura".to_owned(), "gpu".to_owned()]],
        );
        assert!(plan_layout_device_id_remaps(&evidence, &current).is_empty());
    }
}

#[cfg(all(test, feature = "persistence-test-hooks"))]
mod persistence_tests;
