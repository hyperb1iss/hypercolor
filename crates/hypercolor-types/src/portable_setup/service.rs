//! Shared setup projection and restore contracts for native adapters.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{PortableSetupV1, SetupBindings};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupIdentity {
    pub setup_id: Uuid,
    pub name: String,
}

/// The revision fingerprints authored state, not runtime counters or host paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableSetupSnapshot {
    pub manifest: PortableSetupV1,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RestoreMode {
    /// Upsert supplied members and leave missing members untouched.
    Merge,
    /// Delete only members explicitly removed from the previously accepted setup.
    Synchronize { previous: Box<PortableSetupV1> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreOptions {
    pub operation_id: Uuid,
    pub expected_revision: String,
    pub mode: RestoreMode,
    pub apply_selection: bool,
}

/// Referenced content that must be installed or imported on this computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct MissingSetupResources {
    #[serde(default)]
    pub effects: Vec<crate::effect::EffectId>,
    #[serde(default)]
    pub media: Vec<crate::asset::AssetId>,
    #[serde(default)]
    pub presets: Vec<crate::library::PresetId>,
}

impl MissingSetupResources {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty() && self.media.is_empty() && self.presets.is_empty()
    }
}

/// Local choices captured when a manual backup confirmation is prepared.
#[derive(Debug, Clone, Default)]
pub struct BackupRestoreExpectation {
    pub preferences: Option<crate::portable_setup::PortablePreferences>,
    pub tui_preferences: Option<crate::client_preferences::TuiPreferences>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreReport {
    pub applied: PortableSetupSnapshot,
    pub unresolved_bindings: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "MissingSetupResources::is_empty")]
    pub missing_resources: MissingSetupResources,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreReceipt {
    pub operation_id: Uuid,
    pub setup_id: Uuid,
    pub requested_revision: String,
    pub applied: PortableSetupSnapshot,
    pub unresolved_bindings: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "MissingSetupResources::is_empty")]
    pub missing_resources: MissingSetupResources,
    pub materialized_bindings: SetupBindings,
}
