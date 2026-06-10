//! Device pairing and authentication data vocabulary.
//!
//! These types describe pairing flows on the wire — driver capability
//! traits that *implement* pairing live in `hypercolor-driver-api`
//! (which re-exports everything here for back-compat).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Summary of whether a device needs authentication before it can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAuthState {
    /// Device does not require credentials.
    Open,
    /// Device requires credentials and none are stored.
    Required,
    /// Credentials are present and should be used for connections.
    Configured,
    /// Credentials exist, but the driver knows they are invalid or stale.
    Error,
}

/// How the UI or CLI should present a pairing flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PairingFlowKind {
    /// User must perform a physical action, then confirm.
    PhysicalAction,
    /// User must submit one or more credentials.
    CredentialsForm,
}

/// Descriptor for one pairing form field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PairingFieldDescriptor {
    pub key: String,
    pub label: String,
    pub secret: bool,
    pub optional: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// Backend-provided pairing UI/CLI descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PairingDescriptor {
    pub kind: PairingFlowKind,
    pub title: String,
    pub instructions: Vec<String>,
    pub action_label: String,
    #[serde(default)]
    pub fields: Vec<PairingFieldDescriptor>,
}

/// Driver-owned authentication summary for one tracked device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeviceAuthSummary {
    pub state: DeviceAuthState,
    pub can_pair: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<PairingDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Generic pairing request submitted by the daemon API or CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PairDeviceRequest {
    /// Driver-defined values for credential-based flows.
    #[serde(default)]
    pub values: HashMap<String, String>,
    /// Whether to attempt immediate post-pair activation.
    #[serde(default = "bool_true")]
    pub activate_after_pair: bool,
}

/// High-level result category for a pairing attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairDeviceStatus {
    Paired,
    ActionRequired,
    AlreadyPaired,
    InvalidInput,
}

/// Driver-owned result of a pairing action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairDeviceOutcome {
    pub status: PairDeviceStatus,
    pub message: String,
    pub auth_state: DeviceAuthState,
    pub activated: bool,
}

/// Driver-owned result of clearing pairing credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearPairingOutcome {
    pub message: String,
    pub auth_state: DeviceAuthState,
    pub disconnected: bool,
}

const fn bool_true() -> bool {
    true
}
