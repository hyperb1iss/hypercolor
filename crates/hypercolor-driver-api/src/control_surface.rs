//! Shared helpers for driver-owned control surface documents.

use std::collections::HashMap;
use std::net::IpAddr;

use anyhow::{Context, Result};
use hypercolor_types::controls::{
    ApplyImpact, ControlAccess, ControlAvailability, ControlAvailabilityExpr,
    ControlAvailabilityState, ControlFieldDescriptor, ControlGroupDescriptor, ControlGroupKind,
    ControlOwner, ControlPersistence, ControlSurfaceDocument, ControlSurfaceScope, ControlValue,
    ControlValueMap, ControlValueType, ControlVisibility,
};
use hypercolor_types::device::DeviceId;

use crate::validation::validate_ip;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Build an empty driver-scoped document for a driver module.
#[must_use]
pub fn driver_surface(driver_id: &str) -> ControlSurfaceDocument {
    ControlSurfaceDocument::empty(
        format!("driver:{driver_id}"),
        ControlSurfaceScope::Driver {
            driver_id: driver_id.to_owned(),
        },
    )
}

/// Build an empty device-scoped document for a driver-owned device.
#[must_use]
pub fn device_surface(driver_id: &str, device_id: DeviceId) -> ControlSurfaceDocument {
    ControlSurfaceDocument::empty(
        format!("driver:{driver_id}:device:{device_id}"),
        ControlSurfaceScope::Device {
            device_id,
            driver_id: driver_id.to_owned(),
        },
    )
}

/// Build a semantic control group descriptor.
#[must_use]
pub fn group(
    id: &str,
    label: &str,
    kind: ControlGroupKind,
    ordering: i32,
) -> ControlGroupDescriptor {
    ControlGroupDescriptor {
        id: id.to_owned(),
        label: label.to_owned(),
        description: None,
        kind,
        ordering,
    }
}

/// Build a read-write driver configuration field.
#[must_use]
pub fn driver_field(
    driver_id: &str,
    id: &str,
    label: &str,
    description: Option<&str>,
    group_id: Option<&str>,
    value_type: ControlValueType,
    apply_impact: ApplyImpact,
    ordering: i32,
) -> ControlFieldDescriptor {
    field(
        driver_id,
        id,
        label,
        description,
        group_id,
        value_type,
        ControlAccess::ReadWrite,
        ControlPersistence::DriverConfig,
        apply_impact,
        ControlVisibility::Standard,
        ordering,
    )
}

/// Build a read-write per-device configuration field.
#[must_use]
pub fn device_config_field(
    driver_id: &str,
    id: &str,
    label: &str,
    description: Option<&str>,
    group_id: &str,
    value_type: ControlValueType,
    apply_impact: ApplyImpact,
    ordering: i32,
) -> ControlFieldDescriptor {
    field(
        driver_id,
        id,
        label,
        description,
        Some(group_id),
        value_type,
        ControlAccess::ReadWrite,
        ControlPersistence::DeviceConfig,
        apply_impact,
        ControlVisibility::Standard,
        ordering,
    )
}

/// Build a read-only runtime diagnostics field.
#[must_use]
pub fn readonly_field(
    driver_id: &str,
    id: &str,
    label: &str,
    group_id: &str,
    value_type: ControlValueType,
    ordering: i32,
) -> ControlFieldDescriptor {
    field(
        driver_id,
        id,
        label,
        None,
        Some(group_id),
        value_type,
        ControlAccess::ReadOnly,
        ControlPersistence::RuntimeOnly,
        ApplyImpact::None,
        ControlVisibility::Diagnostics,
        ordering,
    )
}

/// Push a read-only field and its current value into a document.
pub fn push_readonly_value(
    document: &mut ControlSurfaceDocument,
    driver_id: &str,
    id: &str,
    label: &str,
    group_id: &str,
    value_type: ControlValueType,
    value: ControlValue,
    ordering: i32,
) {
    document.fields.push(readonly_field(
        driver_id, id, label, group_id, value_type, ordering,
    ));
    document.values.insert(id.to_owned(), value);
}

/// Push a non-empty metadata value as a read-only field.
pub fn push_metadata_value(
    document: &mut ControlSurfaceDocument,
    driver_id: &str,
    metadata: &HashMap<String, String>,
    id: &str,
    label: &str,
    group_id: &str,
    value_type: ControlValueType,
    value: impl FnOnce(String) -> ControlValue,
    ordering: i32,
) {
    let Some(raw) = metadata.get(id).filter(|value| !value.is_empty()).cloned() else {
        return;
    };
    push_readonly_value(
        document,
        driver_id,
        id,
        label,
        group_id,
        value_type,
        value(raw),
        ordering,
    );
}

/// Resolved availability for an available field or action.
#[must_use]
pub const fn available() -> ControlAvailability {
    ControlAvailability {
        state: ControlAvailabilityState::Available,
        reason: None,
    }
}

/// Mark every field descriptor in the document as currently available.
pub fn mark_fields_available(document: &mut ControlSurfaceDocument) {
    document.availability = document
        .fields
        .iter()
        .map(|field| (field.id.clone(), available()))
        .collect();
}

/// Mark every action descriptor in the document as currently available.
pub fn mark_actions_available(document: &mut ControlSurfaceDocument) {
    document.action_availability = document
        .actions
        .iter()
        .map(|action| (action.id.clone(), available()))
        .collect();
}

/// Build a signed integer value type with a step of one.
#[must_use]
pub const fn integer_value_type(min: i64, max: Option<i64>) -> ControlValueType {
    ControlValueType::Integer {
        min: Some(min),
        max,
        step: Some(1),
    }
}

/// Build a string value type with optional maximum length.
#[must_use]
pub fn string_value_type(max_len: Option<u16>) -> ControlValueType {
    ControlValueType::String {
        min_len: None,
        max_len,
        pattern: None,
    }
}

/// Build a homogeneous IP-address list value type.
#[must_use]
pub fn ip_list_value_type(max_items: u16) -> ControlValueType {
    ControlValueType::List {
        item_type: Box::new(ControlValueType::IpAddress),
        min_items: None,
        max_items: Some(max_items),
    }
}

/// Validate every IP address in a typed control list.
pub fn validate_control_ip_list(label: &str, value: &ControlValue) -> Result<()> {
    let ControlValue::List(values) = value else {
        return Ok(());
    };
    for value in values {
        if let ControlValue::IpAddress(raw) = value {
            let ip = raw
                .parse::<IpAddr>()
                .with_context(|| format!("invalid {label}: {raw}"))?;
            validate_ip(ip).with_context(|| format!("invalid {label}: {ip}"))?;
        }
    }
    Ok(())
}

/// Compute the stable FNV-1a revision hash used by control documents.
#[must_use]
pub fn revision_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// Append sorted metadata entries to a revision payload.
pub fn extend_metadata_revision(payload: &mut Vec<u8>, metadata: Option<&HashMap<String, String>>) {
    if let Some(metadata) = metadata {
        let mut metadata_entries = metadata.iter().collect::<Vec<_>>();
        metadata_entries.sort_by_key(|(key, _)| key.as_str());
        for (key, value) in metadata_entries {
            payload.extend_from_slice(key.as_bytes());
            payload.extend_from_slice(value.as_bytes());
        }
    }
}

/// Append typed control values to a revision payload.
pub fn extend_value_map_revision(payload: &mut Vec<u8>, values: &ControlValueMap) {
    for (key, value) in values {
        payload.extend_from_slice(key.as_bytes());
        payload.extend_from_slice(format!("{value:?}").as_bytes());
    }
}

/// Compute a stable revision from a typed value map.
#[must_use]
pub fn value_map_revision(values: &ControlValueMap) -> u64 {
    let mut payload = Vec::new();
    extend_value_map_revision(&mut payload, values);
    revision_hash(&payload)
}

fn field(
    driver_id: &str,
    id: &str,
    label: &str,
    description: Option<&str>,
    group_id: Option<&str>,
    value_type: ControlValueType,
    access: ControlAccess,
    persistence: ControlPersistence,
    apply_impact: ApplyImpact,
    visibility: ControlVisibility,
    ordering: i32,
) -> ControlFieldDescriptor {
    ControlFieldDescriptor {
        id: id.to_owned(),
        owner: ControlOwner::Driver {
            driver_id: driver_id.to_owned(),
        },
        group_id: group_id.map(str::to_owned),
        label: label.to_owned(),
        description: description.map(str::to_owned),
        value_type,
        default_value: None,
        access,
        persistence,
        apply_impact,
        visibility,
        availability: ControlAvailabilityExpr::Always,
        ordering,
    }
}
