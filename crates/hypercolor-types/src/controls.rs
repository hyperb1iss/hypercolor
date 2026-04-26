//! Typed driver and device control surface contracts.
//!
//! Control surfaces let drivers expose dynamic configuration, per-device
//! controls, actions, and read-only state without teaching clients about each
//! concrete driver.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

use crate::device::DeviceId;

/// Stable identifier for a control surface.
pub type ControlSurfaceId = String;

/// Stable identifier for a field within one control surface.
pub type ControlFieldId = String;

/// Stable identifier for an action within one control surface.
pub type ControlActionId = String;

/// Stable identifier for a semantic group within one control surface.
pub type ControlGroupId = String;

/// Monotonic revision for optimistic concurrency.
pub type ControlSurfaceRevision = u64;

/// Current schema version for control surface documents.
pub const CONTROL_SURFACE_SCHEMA_VERSION: u32 = 1;

/// Scope owned by a control surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlSurfaceScope {
    /// Driver-module level controls.
    Driver {
        /// Stable driver module identifier.
        driver_id: String,
    },

    /// Controls for one physical or virtual device.
    Device {
        /// Stable device identifier.
        device_id: DeviceId,

        /// Driver module that owns the device semantics.
        driver_id: String,
    },
}

/// Complete API document for a driver or device control surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ControlSurfaceDocument {
    /// Stable surface identifier.
    pub surface_id: ControlSurfaceId,

    /// Scope represented by this document.
    pub scope: ControlSurfaceScope,

    /// Control-surface schema version.
    pub schema_version: u32,

    /// Current descriptor/value revision.
    pub revision: ControlSurfaceRevision,

    /// Semantic groups for fields and actions.
    pub groups: Vec<ControlGroupDescriptor>,

    /// Field descriptors.
    pub fields: Vec<ControlFieldDescriptor>,

    /// Action descriptors.
    pub actions: Vec<ControlActionDescriptor>,

    /// Current field values keyed by field ID.
    #[schema(value_type = Object)]
    pub values: ControlValueMap,

    /// Resolved availability keyed by field ID.
    pub availability: ControlAvailabilityMap,
}

impl ControlSurfaceDocument {
    /// Create an empty document for the given surface and scope.
    #[must_use]
    pub fn empty(surface_id: impl Into<String>, scope: ControlSurfaceScope) -> Self {
        Self {
            surface_id: surface_id.into(),
            scope,
            schema_version: CONTROL_SURFACE_SCHEMA_VERSION,
            revision: 0,
            groups: Vec::new(),
            fields: Vec::new(),
            actions: Vec::new(),
            values: ControlValueMap::new(),
            availability: ControlAvailabilityMap::new(),
        }
    }
}

/// Typed value map keyed by control field ID.
pub type ControlValueMap = BTreeMap<ControlFieldId, ControlValue>;

/// Availability map keyed by control field ID.
pub type ControlAvailabilityMap = BTreeMap<ControlFieldId, ControlAvailability>;

/// Closed type vocabulary for control values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(no_recursion)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControlValueType {
    /// Boolean value.
    Bool,

    /// Signed integer with optional bounds.
    Integer {
        /// Inclusive minimum.
        min: Option<i64>,

        /// Inclusive maximum.
        max: Option<i64>,

        /// Suggested step.
        step: Option<i64>,
    },

    /// Floating point value with optional bounds.
    Float {
        /// Inclusive minimum.
        min: Option<f64>,

        /// Inclusive maximum.
        max: Option<f64>,

        /// Suggested step.
        step: Option<f64>,
    },

    /// UTF-8 string value.
    String {
        /// Minimum character count.
        min_len: Option<u16>,

        /// Maximum character count.
        max_len: Option<u16>,

        /// Optional validation pattern owned by the driver/UI.
        pattern: Option<String>,
    },

    /// Secret reference or write-only secret input.
    Secret,

    /// RGB color.
    ColorRgb,

    /// RGBA color.
    ColorRgba,

    /// IP address.
    IpAddress,

    /// MAC address.
    MacAddress,

    /// Duration in milliseconds.
    DurationMs {
        /// Inclusive minimum.
        min: Option<u64>,

        /// Inclusive maximum.
        max: Option<u64>,

        /// Suggested step.
        step: Option<u64>,
    },

    /// Single choice from a stable option set.
    Enum {
        /// Valid options.
        options: Vec<ControlEnumOption>,
    },

    /// Multiple choices from a stable option set.
    Flags {
        /// Valid options.
        options: Vec<ControlEnumOption>,
    },

    /// Homogeneous list.
    List {
        /// Item type.
        item_type: Box<ControlValueType>,

        /// Minimum item count.
        min_items: Option<u16>,

        /// Maximum item count.
        max_items: Option<u16>,
    },

    /// Small structured object.
    Object {
        /// Object field descriptors.
        fields: Vec<ControlObjectField>,
    },
}

impl ControlValueType {
    /// Validate that a value matches this type and its structural constraints.
    pub fn validate_value(&self, value: &ControlValue) -> Result<(), ControlValueValidationError> {
        match (self, value) {
            (Self::Bool, ControlValue::Bool(_))
            | (Self::Secret, ControlValue::SecretRef(_))
            | (Self::ColorRgb, ControlValue::ColorRgb(_))
            | (Self::ColorRgba, ControlValue::ColorRgba(_)) => Ok(()),
            (Self::Integer { min, max, step }, ControlValue::Integer(value)) => {
                validate_i64(*value, *min, *max, *step)
            }
            (Self::Float { min, max, step }, ControlValue::Float(value)) => {
                validate_f64(*value, *min, *max, *step)
            }
            (
                Self::String {
                    min_len, max_len, ..
                },
                ControlValue::String(value),
            ) => validate_string(value, *min_len, *max_len),
            (Self::IpAddress, ControlValue::IpAddress(value)) => validate_ip_address(value),
            (Self::MacAddress, ControlValue::MacAddress(value)) => validate_mac_address(value),
            (Self::DurationMs { min, max, step }, ControlValue::DurationMs(value)) => {
                validate_u64(*value, *min, *max, *step)
            }
            (Self::Enum { options }, ControlValue::Enum(value)) => validate_enum(options, value),
            (Self::Flags { options }, ControlValue::Flags(values)) => {
                validate_flags(options, values)
            }
            (
                Self::List {
                    item_type,
                    min_items,
                    max_items,
                },
                ControlValue::List(values),
            ) => validate_list(item_type, values, *min_items, *max_items),
            (Self::Object { fields }, ControlValue::Object(values)) => {
                validate_object(fields, values)
            }
            _ => Err(ControlValueValidationError::TypeMismatch {
                expected: self.clone(),
                actual: value.kind(),
            }),
        }
    }
}

/// Typed value payload matching a [`ControlValueType`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(no_recursion)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ControlValue {
    /// Empty value.
    Null,

    /// Boolean value.
    Bool(bool),

    /// Signed integer value.
    Integer(i64),

    /// Floating point value.
    Float(f64),

    /// UTF-8 string value.
    String(String),

    /// Reference to a secret in the credential store.
    SecretRef(String),

    /// RGB color.
    ColorRgb([u8; 3]),

    /// RGBA color.
    ColorRgba([u8; 4]),

    /// IP address text.
    IpAddress(String),

    /// MAC address text.
    MacAddress(String),

    /// Duration in milliseconds.
    DurationMs(u64),

    /// Single enum option value.
    Enum(String),

    /// Multiple flag option values.
    Flags(Vec<String>),

    /// Homogeneous list.
    List(Vec<ControlValue>),

    /// Structured object.
    Object(BTreeMap<String, ControlValue>),
}

impl ControlValue {
    /// API-facing kind for this value.
    #[must_use]
    pub fn kind(&self) -> ControlValueKind {
        match self {
            Self::Null => ControlValueKind::Null,
            Self::Bool(_) => ControlValueKind::Bool,
            Self::Integer(_) => ControlValueKind::Integer,
            Self::Float(_) => ControlValueKind::Float,
            Self::String(_) => ControlValueKind::String,
            Self::SecretRef(_) => ControlValueKind::SecretRef,
            Self::ColorRgb(_) => ControlValueKind::ColorRgb,
            Self::ColorRgba(_) => ControlValueKind::ColorRgba,
            Self::IpAddress(_) => ControlValueKind::IpAddress,
            Self::MacAddress(_) => ControlValueKind::MacAddress,
            Self::DurationMs(_) => ControlValueKind::DurationMs,
            Self::Enum(_) => ControlValueKind::Enum,
            Self::Flags(_) => ControlValueKind::Flags,
            Self::List(_) => ControlValueKind::List,
            Self::Object(_) => ControlValueKind::Object,
        }
    }
}

/// Lightweight kind descriptor for validation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlValueKind {
    /// Null value.
    Null,
    /// Boolean value.
    Bool,
    /// Integer value.
    Integer,
    /// Float value.
    Float,
    /// String value.
    String,
    /// Secret reference.
    SecretRef,
    /// RGB color.
    ColorRgb,
    /// RGBA color.
    ColorRgba,
    /// IP address.
    IpAddress,
    /// MAC address.
    MacAddress,
    /// Duration in milliseconds.
    DurationMs,
    /// Enum value.
    Enum,
    /// Flag values.
    Flags,
    /// List value.
    List,
    /// Object value.
    Object,
}

/// Stable enum option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ControlEnumOption {
    /// Stable option value.
    pub value: String,

    /// Human-readable label.
    pub label: String,

    /// Optional help text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Whether this option should remain loadable but discouraged.
    pub deprecated: bool,
}

impl ControlEnumOption {
    /// Create a non-deprecated option with no description.
    #[must_use]
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
            deprecated: false,
        }
    }
}

/// Field inside an object control value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ControlObjectField {
    /// Stable field identifier.
    pub id: String,

    /// Human-readable label.
    pub label: String,

    /// Expected value type.
    #[schema(value_type = Object)]
    pub value_type: ControlValueType,

    /// Whether this field is required.
    pub required: bool,

    /// Optional default value.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub default_value: Option<ControlValue>,
}

/// Field, action, or group owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlOwner {
    /// Host-owned common behavior.
    Host,

    /// Driver-owned behavior.
    Driver {
        /// Stable driver module identifier.
        driver_id: String,
    },
}

/// Field descriptor for one typed control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ControlFieldDescriptor {
    /// Stable field identifier within the surface.
    pub id: ControlFieldId,

    /// Field owner.
    pub owner: ControlOwner,

    /// Optional semantic group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<ControlGroupId>,

    /// Human-readable label.
    pub label: String,

    /// Optional help text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Expected value type.
    #[schema(value_type = Object)]
    pub value_type: ControlValueType,

    /// Optional default value.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub default_value: Option<ControlValue>,

    /// Read/write behavior.
    pub access: ControlAccess,

    /// Persistence target.
    pub persistence: ControlPersistence,

    /// Dynamic impact required when this field changes.
    pub apply_impact: ApplyImpact,

    /// Visibility tier.
    pub visibility: ControlVisibility,

    /// Availability expression before daemon resolution.
    #[schema(value_type = Object)]
    pub availability: ControlAvailabilityExpr,

    /// Stable ordering hint.
    pub ordering: i32,
}

/// Semantic group descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ControlGroupDescriptor {
    /// Stable group identifier.
    pub id: ControlGroupId,

    /// Human-readable label.
    pub label: String,

    /// Optional help text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Semantic group kind.
    pub kind: ControlGroupKind,

    /// Stable ordering hint.
    pub ordering: i32,
}

/// Semantic group kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlGroupKind {
    /// General controls.
    General,
    /// Connection controls.
    Connection,
    /// Output controls.
    Output,
    /// Color controls.
    Color,
    /// Topology controls.
    Topology,
    /// Performance controls.
    Performance,
    /// Diagnostics controls.
    Diagnostics,
    /// Advanced controls.
    Advanced,
    /// Dangerous controls.
    Danger,
    /// Driver-defined custom group.
    Custom,
}

/// Field access mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlAccess {
    /// Client may read but not write.
    ReadOnly,
    /// Client may read and write.
    ReadWrite,
    /// Client may write but not read.
    WriteOnly,
}

/// Persistence target for a control field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlPersistence {
    /// Stored in `drivers.<id>`.
    DriverConfig,
    /// Stored in per-device config.
    DeviceConfig,
    /// Stored as a profile override.
    ProfileOverride,
    /// Stored only in runtime memory.
    RuntimeOnly,
    /// Stored in hardware by a driver action or apply transaction.
    HardwareStored,
}

/// Field visibility tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlVisibility {
    /// Standard user-facing control.
    Standard,
    /// Advanced control.
    Advanced,
    /// Diagnostics control.
    Diagnostics,
    /// Hidden field.
    Hidden,
}

/// Dynamic impact required to apply a control change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApplyImpact {
    /// No operational impact.
    None,
    /// Apply directly to live runtime state.
    Live,
    /// Trigger a discovery rescan.
    DiscoveryRescan,
    /// Reconnect the affected device.
    DeviceReconnect,
    /// Rebind the affected output backend.
    BackendRebind,
    /// Rebuild topology for affected devices.
    TopologyRebuild,
    /// Persist state into physical hardware.
    HardwarePersist,
    /// Driver-defined dynamic impact.
    Custom(String),
}

/// Descriptor-time availability expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControlAvailabilityExpr {
    /// Always available.
    Always,

    /// Never available.
    Never {
        /// Human-readable reason.
        reason: String,
    },

    /// Available when another field equals a value.
    WhenFieldEquals {
        /// Field to inspect.
        field_id: ControlFieldId,

        /// Expected value.
        value: ControlValue,
    },

    /// Available when a named capability exists.
    WhenCapability {
        /// Capability identifier.
        capability: String,
    },

    /// Available when all expressions match.
    All {
        /// Child expressions.
        expressions: Vec<ControlAvailabilityExpr>,
    },

    /// Available when any expression matches.
    Any {
        /// Child expressions.
        expressions: Vec<ControlAvailabilityExpr>,
    },
}

/// Resolved availability for a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ControlAvailability {
    /// Resolved state.
    pub state: ControlAvailabilityState,

    /// Optional reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Resolved availability state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlAvailabilityState {
    /// Control can be edited or invoked.
    Available,
    /// Control is visible but disabled.
    Disabled,
    /// Control is visible but read-only.
    ReadOnly,
    /// Control is unsupported by this target.
    Unsupported,
    /// Control should be hidden by default.
    Hidden,
}

/// Action descriptor for one-shot commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ControlActionDescriptor {
    /// Stable action identifier within the surface.
    pub id: ControlActionId,

    /// Action owner.
    pub owner: ControlOwner,

    /// Optional semantic group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<ControlGroupId>,

    /// Human-readable label.
    pub label: String,

    /// Optional help text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Typed input fields.
    pub input_fields: Vec<ControlObjectField>,

    /// Optional typed result.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub result_type: Option<ControlValueType>,

    /// Optional confirmation metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<ActionConfirmation>,

    /// Dynamic impact required by this action.
    pub apply_impact: ApplyImpact,

    /// Availability expression before daemon resolution.
    #[schema(value_type = Object)]
    pub availability: ControlAvailabilityExpr,

    /// Stable ordering hint.
    pub ordering: i32,
}

/// Action confirmation metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ActionConfirmation {
    /// Confirmation severity.
    pub level: ActionConfirmationLevel,

    /// Human-readable message.
    pub message: String,
}

/// Confirmation severity for actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionConfirmationLevel {
    /// Normal confirmation.
    Normal,
    /// Destructive operation.
    Destructive,
    /// Operation writes persistent state to hardware.
    HardwarePersistent,
}

/// Request to apply one or more control changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApplyControlChangesRequest {
    /// Target surface.
    pub surface_id: ControlSurfaceId,

    /// Optional expected revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<ControlSurfaceRevision>,

    /// Changes to apply atomically.
    pub changes: Vec<ControlChange>,

    /// Validate without mutating state.
    pub dry_run: bool,
}

/// One requested field change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ControlChange {
    /// Field to update.
    pub field_id: ControlFieldId,

    /// Requested value.
    #[schema(value_type = Object)]
    pub value: ControlValue,
}

/// Response from applying control changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApplyControlChangesResponse {
    /// Target surface.
    pub surface_id: ControlSurfaceId,

    /// Previous revision.
    pub previous_revision: ControlSurfaceRevision,

    /// New revision.
    pub revision: ControlSurfaceRevision,

    /// Accepted changes after driver normalization.
    pub accepted: Vec<AppliedControlChange>,

    /// Rejected changes.
    pub rejected: Vec<RejectedControlChange>,

    /// Dynamic impacts produced by the transaction.
    pub impacts: Vec<ApplyImpact>,

    /// Current values after the transaction.
    #[schema(value_type = Object)]
    pub values: ControlValueMap,
}

/// Accepted field change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AppliedControlChange {
    /// Field that changed.
    pub field_id: ControlFieldId,

    /// Applied value.
    #[schema(value_type = Object)]
    pub value: ControlValue,
}

/// Rejected field change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RejectedControlChange {
    /// Field that failed validation or apply.
    pub field_id: ControlFieldId,

    /// Attempted value.
    #[schema(value_type = Object)]
    pub attempted_value: ControlValue,

    /// Typed error.
    pub error: ControlApplyError,
}

/// Typed control apply error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControlApplyError {
    /// Field does not exist.
    UnknownField,
    /// Value has the wrong type.
    TypeMismatch {
        /// Expected type.
        #[schema(value_type = Object)]
        expected: ControlValueType,
    },
    /// Value is outside the allowed range.
    OutOfRange,
    /// Value is semantically invalid.
    InvalidValue {
        /// Human-readable validation message.
        message: String,
    },
    /// Control is unavailable.
    Unavailable {
        /// Human-readable reason.
        reason: String,
    },
    /// Surface revision conflict.
    Conflict {
        /// Current server-side revision.
        current_revision: ControlSurfaceRevision,
    },
    /// Caller is not authorized.
    Unauthorized,
    /// Device is offline.
    DeviceOffline,
    /// Driver has not implemented dynamic apply for this control yet.
    UnsupportedDynamicApply {
        /// Human-readable detail.
        message: String,
    },
    /// Driver-specific error.
    DriverError {
        /// Human-readable detail.
        message: String,
    },
}

/// Result from invoking an action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ControlActionResult {
    /// Target surface.
    pub surface_id: ControlSurfaceId,

    /// Action that ran.
    pub action_id: ControlActionId,

    /// Action status.
    pub status: ControlActionStatus,

    /// Optional typed result.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub result: Option<ControlValue>,

    /// Resulting surface revision.
    pub revision: ControlSurfaceRevision,
}

/// Action execution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlActionStatus {
    /// Action was accepted for async execution.
    Accepted,
    /// Action is running.
    Running,
    /// Action completed.
    Completed,
    /// Action failed.
    Failed,
}

/// WebSocket event for control-surface changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControlSurfaceEvent {
    /// Surface descriptors, availability, or values changed.
    SurfaceChanged {
        /// Surface that changed.
        surface_id: ControlSurfaceId,

        /// New revision.
        revision: ControlSurfaceRevision,
    },

    /// Field values changed.
    ValuesChanged {
        /// Surface that changed.
        surface_id: ControlSurfaceId,

        /// New revision.
        revision: ControlSurfaceRevision,

        /// Changed or current values.
        #[schema(value_type = Object)]
        values: ControlValueMap,
    },

    /// Availability changed.
    AvailabilityChanged {
        /// Surface that changed.
        surface_id: ControlSurfaceId,

        /// New revision.
        revision: ControlSurfaceRevision,

        /// Current availability.
        availability: ControlAvailabilityMap,
    },

    /// Action progress changed.
    ActionProgress {
        /// Surface that owns the action.
        surface_id: ControlSurfaceId,

        /// Action identifier.
        action_id: ControlActionId,

        /// Current status.
        status: ControlActionStatus,

        /// Optional progress from 0.0 to 1.0.
        #[serde(skip_serializing_if = "Option::is_none")]
        progress: Option<f32>,
    },
}

/// Validation error for matching [`ControlValue`] to [`ControlValueType`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ControlValueValidationError {
    /// Value kind does not match expected type.
    #[error("expected {expected:?}, got {actual:?}")]
    TypeMismatch {
        /// Expected type.
        expected: ControlValueType,

        /// Actual value kind.
        actual: ControlValueKind,
    },

    /// Value is below the inclusive minimum.
    #[error("value is below minimum")]
    BelowMinimum,

    /// Value is above the inclusive maximum.
    #[error("value is above maximum")]
    AboveMaximum,

    /// Value does not align with the configured step.
    #[error("value does not align with step")]
    InvalidStep,

    /// String is too short.
    #[error("string is too short")]
    StringTooShort,

    /// String is too long.
    #[error("string is too long")]
    StringTooLong,

    /// IP address is invalid.
    #[error("invalid IP address")]
    InvalidIpAddress,

    /// MAC address is invalid.
    #[error("invalid MAC address")]
    InvalidMacAddress,

    /// Enum or flag option is unknown.
    #[error("unknown option: {0}")]
    UnknownOption(String),

    /// Flag option was repeated.
    #[error("duplicate option: {0}")]
    DuplicateOption(String),

    /// List has too few items.
    #[error("list has too few items")]
    TooFewItems,

    /// List has too many items.
    #[error("list has too many items")]
    TooManyItems,

    /// Required object field is missing.
    #[error("missing required field: {0}")]
    MissingField(String),

    /// Object contains an unknown field.
    #[error("unknown field: {0}")]
    UnknownField(String),

    /// Object field failed validation.
    #[error("invalid field {field}: {source}")]
    InvalidField {
        /// Field identifier.
        field: String,

        /// Field validation error.
        source: Box<ControlValueValidationError>,
    },

    /// List item failed validation.
    #[error("invalid item {index}: {source}")]
    InvalidItem {
        /// Item index.
        index: usize,

        /// Item validation error.
        source: Box<ControlValueValidationError>,
    },
}

fn validate_i64(
    value: i64,
    min: Option<i64>,
    max: Option<i64>,
    step: Option<i64>,
) -> Result<(), ControlValueValidationError> {
    if min.is_some_and(|min| value < min) {
        return Err(ControlValueValidationError::BelowMinimum);
    }
    if max.is_some_and(|max| value > max) {
        return Err(ControlValueValidationError::AboveMaximum);
    }
    if step.is_some_and(|step| step > 0 && value.rem_euclid(step) != 0) {
        return Err(ControlValueValidationError::InvalidStep);
    }
    Ok(())
}

fn validate_u64(
    value: u64,
    min: Option<u64>,
    max: Option<u64>,
    step: Option<u64>,
) -> Result<(), ControlValueValidationError> {
    if min.is_some_and(|min| value < min) {
        return Err(ControlValueValidationError::BelowMinimum);
    }
    if max.is_some_and(|max| value > max) {
        return Err(ControlValueValidationError::AboveMaximum);
    }
    if step.is_some_and(|step| step > 0 && !value.is_multiple_of(step)) {
        return Err(ControlValueValidationError::InvalidStep);
    }
    Ok(())
}

fn validate_f64(
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
) -> Result<(), ControlValueValidationError> {
    if min.is_some_and(|min| value < min) {
        return Err(ControlValueValidationError::BelowMinimum);
    }
    if max.is_some_and(|max| value > max) {
        return Err(ControlValueValidationError::AboveMaximum);
    }
    if step.is_some_and(|step| step > 0.0 && (value / step).fract().abs() > f64::EPSILON) {
        return Err(ControlValueValidationError::InvalidStep);
    }
    Ok(())
}

fn validate_string(
    value: &str,
    min_len: Option<u16>,
    max_len: Option<u16>,
) -> Result<(), ControlValueValidationError> {
    let len = value.chars().count();
    if min_len.is_some_and(|min_len| len < usize::from(min_len)) {
        return Err(ControlValueValidationError::StringTooShort);
    }
    if max_len.is_some_and(|max_len| len > usize::from(max_len)) {
        return Err(ControlValueValidationError::StringTooLong);
    }
    Ok(())
}

fn validate_ip_address(value: &str) -> Result<(), ControlValueValidationError> {
    value
        .parse::<IpAddr>()
        .map(|_| ())
        .map_err(|_| ControlValueValidationError::InvalidIpAddress)
}

fn validate_mac_address(value: &str) -> Result<(), ControlValueValidationError> {
    let mut parts = value.split(':');
    if (0..6).all(|_| parts.next().is_some_and(is_hex_octet)) && parts.next().is_none() {
        Ok(())
    } else {
        Err(ControlValueValidationError::InvalidMacAddress)
    }
}

fn is_hex_octet(value: &str) -> bool {
    value.len() == 2 && value.chars().all(|c| c.is_ascii_hexdigit())
}

fn validate_enum(
    options: &[ControlEnumOption],
    value: &str,
) -> Result<(), ControlValueValidationError> {
    if options.iter().any(|option| option.value == value) {
        Ok(())
    } else {
        Err(ControlValueValidationError::UnknownOption(value.to_owned()))
    }
}

fn validate_flags(
    options: &[ControlEnumOption],
    values: &[String],
) -> Result<(), ControlValueValidationError> {
    let valid_values = options
        .iter()
        .map(|option| option.value.as_str())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();

    for value in values {
        if !valid_values.contains(value.as_str()) {
            return Err(ControlValueValidationError::UnknownOption(value.clone()));
        }
        if !seen.insert(value.as_str()) {
            return Err(ControlValueValidationError::DuplicateOption(value.clone()));
        }
    }

    Ok(())
}

fn validate_list(
    item_type: &ControlValueType,
    values: &[ControlValue],
    min_items: Option<u16>,
    max_items: Option<u16>,
) -> Result<(), ControlValueValidationError> {
    if min_items.is_some_and(|min_items| values.len() < usize::from(min_items)) {
        return Err(ControlValueValidationError::TooFewItems);
    }
    if max_items.is_some_and(|max_items| values.len() > usize::from(max_items)) {
        return Err(ControlValueValidationError::TooManyItems);
    }

    for (index, value) in values.iter().enumerate() {
        item_type.validate_value(value).map_err(|source| {
            ControlValueValidationError::InvalidItem {
                index,
                source: Box::new(source),
            }
        })?;
    }

    Ok(())
}

fn validate_object(
    fields: &[ControlObjectField],
    values: &BTreeMap<String, ControlValue>,
) -> Result<(), ControlValueValidationError> {
    let field_ids = fields
        .iter()
        .map(|field| field.id.as_str())
        .collect::<BTreeSet<_>>();

    for key in values.keys() {
        if !field_ids.contains(key.as_str()) {
            return Err(ControlValueValidationError::UnknownField(key.clone()));
        }
    }

    for field in fields {
        match values.get(&field.id) {
            Some(value) => field.value_type.validate_value(value).map_err(|source| {
                ControlValueValidationError::InvalidField {
                    field: field.id.clone(),
                    source: Box::new(source),
                }
            })?,
            None if field.required => {
                return Err(ControlValueValidationError::MissingField(field.id.clone()));
            }
            None => {}
        }
    }

    Ok(())
}
