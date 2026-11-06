//! Control surfaces for the Adalight serial driver.
//!
//! The driver surface edits the `devices` list as structured objects.
//! Device surfaces are read-only diagnostics resolved from the device's
//! discovery metadata, including the frame rate the serial line can carry;
//! a device is edited through its entry in the driver's `devices` list.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use hypercolor_driver_api::{
    ControlApplyTarget, DriverConfigView, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_support::{control_apply, control_surface};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::control::ControlValue;
use hypercolor_types::controls::{
    ApplyImpact, ControlChange, ControlEnumOption, ControlFieldDescriptor, ControlGroupKind,
    ControlObjectField, ControlSurfaceDocument, ControlValueMap, ControlValueType,
};

use crate::DESCRIPTOR;
use crate::backend::METADATA_DEVICE_CONFIG;
use crate::color::ColorOrder;
use crate::config::{
    AdalightConfig, AdalightDeviceConfig, MAX_BAUD_RATE, MAX_DEVICE_ID_LEN, MAX_DEVICES,
    MAX_FPS_LIMIT, MAX_LATCH_MS, MAX_NAME_LEN, MAX_PORT_LEN, MAX_STARTUP_DELAY_MS, MIN_BAUD_RATE,
    WhiteMode,
};
use crate::plan::{AdalightDevicePlan, resolve_config, resolve_device};
use crate::protocol::{ADA_MAX_LED_COUNT, AdalightProtocol, WhiteCalibration};

/// Driver field: configured devices.
pub const FIELD_DEVICES: &str = "devices";

const DEVICES_DESCRIPTION: &str = "Serial LED controllers. Each entry needs an id, a port, and an \
     LED count. Hypercolor opens only the ports listed here. Changes to a connected device apply \
     when it reconnects.";

// ── Driver surface ──────────────────────────────────────────────────────

/// Build the driver-scoped control surface for `config`.
#[must_use]
pub fn adalight_driver_control_surface(config: &AdalightConfig) -> ControlSurfaceDocument {
    let mut document = control_surface::driver_surface(DESCRIPTOR.id);
    document.groups.push(control_surface::group(
        "devices",
        "Devices",
        ControlGroupKind::Connection,
        0,
    ));
    document.fields = adalight_driver_control_fields();
    document.values = adalight_config_values(config);
    document.revision = control_surface::value_map_revision(&document.values);
    control_surface::mark_fields_available(&mut document);
    document
}

/// Field descriptors of the driver surface.
#[must_use]
pub fn adalight_driver_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![control_surface::driver_field(
        DESCRIPTOR.id,
        FIELD_DEVICES,
        "Devices",
        Some(DEVICES_DESCRIPTION),
        Some("devices"),
        ControlValueType::List {
            item_type: Box::new(device_object_type()),
            min_items: None,
            max_items: Some(u16::try_from(MAX_DEVICES).unwrap_or(u16::MAX)),
        },
        ApplyImpact::DiscoveryRescan,
        0,
    )]
}

fn object_field(
    id: &str,
    label: &str,
    value_type: ControlValueType,
    required: bool,
) -> ControlObjectField {
    ControlObjectField {
        id: id.to_owned(),
        label: label.to_owned(),
        value_type,
        required,
        default_value: None,
    }
}

fn int_type(min: i64, max: i64) -> ControlValueType {
    control_surface::integer_value_type(min, Some(max))
}

fn text_type(max_len: usize) -> ControlValueType {
    control_surface::string_value_type(Some(u16::try_from(max_len).unwrap_or(u16::MAX)))
}

fn enum_type(options: impl IntoIterator<Item = (&'static str, &'static str)>) -> ControlValueType {
    ControlValueType::Enum {
        options: options
            .into_iter()
            .map(|(value, label)| ControlEnumOption::new(value, label))
            .collect(),
    }
}

fn calibration_object_type() -> ControlValueType {
    ControlValueType::Object {
        fields: vec![
            object_field("gain", "Gain", int_type(0, 255), false),
            object_field("red", "Red", int_type(0, 255), false),
            object_field("green", "Green", int_type(0, 255), false),
            object_field("blue", "Blue", int_type(0, 255), false),
        ],
    }
}

/// Structured value type of one `devices` entry.
#[must_use]
pub fn device_object_type() -> ControlValueType {
    ControlValueType::Object {
        fields: vec![
            object_field("id", "ID", text_type(MAX_DEVICE_ID_LEN), true),
            object_field("name", "Name", text_type(MAX_NAME_LEN), false),
            object_field("port", "Serial Port", text_type(MAX_PORT_LEN), true),
            object_field(
                "baud_rate",
                "Baud Rate",
                int_type(i64::from(MIN_BAUD_RATE), i64::from(MAX_BAUD_RATE)),
                false,
            ),
            object_field(
                "protocol",
                "Protocol",
                enum_type(
                    AdalightProtocol::ALL
                        .into_iter()
                        .map(|protocol| (protocol.as_str(), protocol.label())),
                ),
                false,
            ),
            object_field(
                "led_count",
                "LEDs",
                int_type(1, i64::from(ADA_MAX_LED_COUNT)),
                true,
            ),
            object_field(
                "color_order",
                "Color Order",
                enum_type(
                    ColorOrder::ALL
                        .into_iter()
                        .map(|order| (order.as_str(), order.as_str())),
                ),
                false,
            ),
            object_field(
                "white",
                "White Channel",
                enum_type(
                    WhiteMode::ALL
                        .into_iter()
                        .map(|mode| (mode.as_str(), mode.label())),
                ),
                false,
            ),
            object_field(
                "white_calibration",
                "White Calibration",
                calibration_object_type(),
                false,
            ),
            object_field(
                "startup_delay_ms",
                "Startup Delay (ms)",
                int_type(0, i64::from(MAX_STARTUP_DELAY_MS)),
                false,
            ),
            object_field(
                "latch_ms",
                "Latch Gap (ms)",
                int_type(0, i64::from(MAX_LATCH_MS)),
                false,
            ),
            object_field(
                "max_fps",
                "Max FPS",
                int_type(0, i64::from(MAX_FPS_LIMIT)),
                false,
            ),
        ],
    }
}

/// Current driver values as control values.
#[must_use]
pub fn adalight_config_values(config: &AdalightConfig) -> ControlValueMap {
    ControlValueMap::from([(
        FIELD_DEVICES.to_owned(),
        ControlValue::List(config.devices.iter().map(device_value).collect()),
    )])
}

fn device_value(device: &AdalightDeviceConfig) -> ControlValue {
    let mut map = BTreeMap::new();
    map.insert("id".to_owned(), ControlValue::Text(device.id.clone()));
    if let Some(name) = &device.name {
        map.insert("name".to_owned(), ControlValue::Text(name.clone()));
    }
    map.insert("port".to_owned(), ControlValue::Text(device.port.clone()));
    insert_int(&mut map, "baud_rate", device.baud_rate.map(i64::from));
    map.insert(
        "protocol".to_owned(),
        ControlValue::Enum(device.protocol.as_str().to_owned()),
    );
    map.insert(
        "led_count".to_owned(),
        ControlValue::Int(i64::from(device.led_count)),
    );
    map.insert(
        "color_order".to_owned(),
        ControlValue::Enum(device.color_order.as_str().to_owned()),
    );
    map.insert(
        "white".to_owned(),
        ControlValue::Enum(device.white.as_str().to_owned()),
    );
    if let Some(calibration) = device.white_calibration {
        map.insert(
            "white_calibration".to_owned(),
            calibration_value(calibration),
        );
    }
    insert_int(
        &mut map,
        "startup_delay_ms",
        device.startup_delay_ms.map(i64::from),
    );
    insert_int(&mut map, "latch_ms", device.latch_ms.map(i64::from));
    insert_int(&mut map, "max_fps", device.max_fps.map(i64::from));
    ControlValue::Map(map)
}

fn calibration_value(calibration: WhiteCalibration) -> ControlValue {
    ControlValue::Map(BTreeMap::from([
        (
            "gain".to_owned(),
            ControlValue::Int(i64::from(calibration.gain)),
        ),
        (
            "red".to_owned(),
            ControlValue::Int(i64::from(calibration.red)),
        ),
        (
            "green".to_owned(),
            ControlValue::Int(i64::from(calibration.green)),
        ),
        (
            "blue".to_owned(),
            ControlValue::Int(i64::from(calibration.blue)),
        ),
    ]))
}

fn insert_int(map: &mut BTreeMap<String, ControlValue>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), ControlValue::Int(value));
    }
}

/// Parse driver settings into a validated config.
///
/// # Errors
///
/// Returns an error when the settings do not deserialize or fail validation.
pub fn parse_validated_config(entry: &DriverConfigEntry) -> Result<AdalightConfig> {
    let config = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry,
    }
    .parse_settings::<AdalightConfig>()?;
    resolve_config(&config)?;
    Ok(config)
}

/// Validate a batch of control changes for the driver or a device.
///
/// # Errors
///
/// Returns an error for unknown fields, type mismatches, device-surface
/// writes, or a resulting config that fails validation.
pub fn validate_adalight_changes(
    target: &ControlApplyTarget<'_>,
    changes: &[ControlChange],
) -> Result<ValidatedControlChanges> {
    match target {
        ControlApplyTarget::Driver { driver_id, config } => {
            if *driver_id != DESCRIPTOR.id {
                bail!("adalight controls cannot validate driver '{driver_id}'");
            }
            let validated = control_apply::validate_control_changes(
                DESCRIPTOR.id,
                adalight_driver_control_fields(),
                changes,
                |_| Ok(()),
            )?;
            let mut values = adalight_config_values(&config.parse_settings::<AdalightConfig>()?);
            for change in &validated.changes {
                values.insert(change.field_id.clone(), change.value.clone());
            }
            parse_validated_config(&values_entry(&values)?)
                .context("adalight driver config would be invalid")?;
            Ok(validated)
        }
        ControlApplyTarget::Device { device } => {
            if device.info.driver_id() != DESCRIPTOR.id {
                bail!(
                    "adalight controls cannot validate device owned by '{}'",
                    device.info.driver_id()
                );
            }
            bail!(
                "serial device fields are read-only; edit the device in the driver's devices list"
            )
        }
    }
}

/// Convert control values into a driver config entry.
///
/// # Errors
///
/// Returns an error if a value cannot be serialized.
pub fn values_entry(values: &ControlValueMap) -> Result<DriverConfigEntry> {
    let settings = values
        .iter()
        .map(|(key, value)| {
            serde_json::to_value(value)
                .map(|value| (key.clone(), value))
                .with_context(|| format!("failed to serialize adalight control '{key}'"))
        })
        .collect::<Result<_>>()?;
    Ok(DriverConfigEntry::enabled(settings))
}

// ── Device surface ──────────────────────────────────────────────────────

/// Resolve a tracked device's plan from its discovery metadata.
#[must_use]
pub fn tracked_device_plan(device: &TrackedDeviceCtx<'_>) -> Option<AdalightDevicePlan> {
    let raw = device.metadata?.get(METADATA_DEVICE_CONFIG)?;
    let config: AdalightDeviceConfig = serde_json::from_str(raw).ok()?;
    resolve_device(&config).ok()
}

/// Build the read-only device surface, including the serial line budget.
#[must_use]
pub fn adalight_device_control_surface(device: &TrackedDeviceCtx<'_>) -> ControlSurfaceDocument {
    let mut document = control_surface::device_surface(DESCRIPTOR.id, device.device_id);
    document.groups.extend([
        control_surface::group("connection", "Connection", ControlGroupKind::Connection, 0),
        control_surface::group("output", "Output", ControlGroupKind::Output, 10),
    ]);

    if let Some(plan) = tracked_device_plan(device) {
        push_plan_values(&mut document, &plan);
    }

    control_surface::mark_fields_available(&mut document);
    let mut payload = device.device_id.to_string().into_bytes();
    control_surface::extend_metadata_revision(&mut payload, device.metadata);
    document.revision = control_surface::revision_hash(&payload);
    document
}

fn push_plan_values(document: &mut ControlSurfaceDocument, plan: &AdalightDevicePlan) {
    let text = control_surface::string_value_type(Some(255));
    let count = control_surface::integer_value_type(0, None);
    let rows = [
        (
            "config_id",
            "Config ID",
            "connection",
            text.clone(),
            ControlValue::Text(plan.id.clone()),
        ),
        (
            "port",
            "Serial Port",
            "connection",
            text.clone(),
            ControlValue::Text(plan.port.clone()),
        ),
        (
            "baud_rate",
            "Baud Rate",
            "connection",
            count.clone(),
            ControlValue::Int(i64::from(plan.baud_rate)),
        ),
        (
            "protocol",
            "Protocol",
            "output",
            text.clone(),
            ControlValue::Text(plan.protocol().label().to_owned()),
        ),
        (
            "led_count",
            "LEDs",
            "output",
            count.clone(),
            ControlValue::Int(i64::from(plan.led_count())),
        ),
        (
            "color_order",
            "Color Order",
            "output",
            text.clone(),
            ControlValue::Text(plan.color_order.as_str().to_owned()),
        ),
        (
            "white",
            "White Channel",
            "output",
            text,
            ControlValue::Text(plan.white.label().to_owned()),
        ),
        (
            "frame_bytes",
            "Frame Size (bytes)",
            "output",
            count.clone(),
            ControlValue::Int(i64::try_from(plan.frame_bytes()).unwrap_or(i64::MAX)),
        ),
        (
            "line_max_fps",
            "Serial Line Max FPS",
            "output",
            count.clone(),
            ControlValue::Int(i64::from(plan.line_max_fps())),
        ),
        (
            "output_fps",
            "Output FPS Cap",
            "output",
            count,
            ControlValue::Int(i64::from(plan.effective_max_fps())),
        ),
    ];
    for (ordering, (id, label, group, value_type, value)) in (0_i32..).step_by(10).zip(rows) {
        control_surface::push_readonly_value(
            document,
            DESCRIPTOR.id,
            id,
            label,
            group,
            value_type,
            value,
            ordering,
        );
    }
}
