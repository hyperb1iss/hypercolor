//! Installation-local portable restore schemas.
//!
//! These records contain recovery proofs and local paths; they are never
//! transported as account setup manifests or exposed as archive contents.

use std::collections::BTreeMap;
use std::path::PathBuf;

use hypercolor_types::portable_setup::{
    PortableBackupV1, PortableBlob, PortablePreferences, PortableSetupV1, RestoreOptions,
    RestoreReceipt, SetupBindings,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RuntimeRestoreProof {
    pub before: Option<crate::runtime_state::RuntimeSessionSnapshot>,
    pub candidate: crate::runtime_state::RuntimeSessionSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BackupAncillary {
    pub expected_preferences: Option<PortablePreferences>,
    pub preferences: Option<PortablePreferences>,
    pub favorites: Vec<hypercolor_types::library::FavoriteEffect>,
    pub tui_preferences: Option<hypercolor_types::client_preferences::TuiPreferences>,
    pub expected_tui_preferences: Option<hypercolor_types::client_preferences::TuiPreferences>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RestoreJournal {
    Pending {
        schema_version: u32,
        before: Box<PortableSetupV1>,
        candidate: Box<PortableSetupV1>,
        requested_revision: String,
        bindings: SetupBindings,
        options: RestoreOptions,
        runtime: RuntimeRestoreProof,
        ancillary: Option<BackupAncillary>,
    },
    Committed {
        schema_version: u32,
        receipt: Box<RestoreReceipt>,
        finalized: bool,
        #[serde(default)]
        reconnect_network_outputs: bool,
        ancillary: Option<BackupAncillary>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActiveProjection {
    pub(crate) schema_version: u32,
    pub(crate) receipt: RestoreReceipt,
    pub(crate) aliases: SetupBindings,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StagedBackup {
    #[serde(deserialize_with = "staged_manifest")]
    pub(crate) backup: PortableBackupV1,
    pub(crate) bindings: SetupBindings,
    pub(crate) expected_revision: String,
    pub(crate) expected_preferences: Option<PortablePreferences>,
    pub(crate) expected_tui_preferences:
        Option<hypercolor_types::client_preferences::TuiPreferences>,
}

/// Decode a staged manifest the way an imported archive's is decoded.
///
/// A staging record outlives the build that wrote it, so the manifest
/// inside may declare an older schema. Reading it through the manifest
/// decoder upgrades it in memory, and refuses one from a newer build by
/// name, instead of leaving validation to reject a version it was never
/// shown how to read.
fn staged_manifest<'de, D>(deserializer: D) -> Result<PortableBackupV1, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let manifest = serde_json::Value::deserialize(deserializer)?;
    let bytes = serde_json::to_vec(&manifest).map_err(serde::de::Error::custom)?;
    PortableBackupV1::from_manifest_json(&bytes).map_err(serde::de::Error::custom)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EffectPlan {
    pub(crate) schema_version: u32,
    pub(crate) operation_id: Uuid,
    pub(crate) input_hash: String,
    pub(crate) retired: BTreeMap<Uuid, RetiredEffect>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetiredEffect {
    pub(crate) root: PathBuf,
    pub(crate) bundled: bool,
    pub(crate) files: BTreeMap<String, PortableBlob>,
}
