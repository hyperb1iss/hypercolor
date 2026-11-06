//! Control surfaces for the pixel controller driver.
//!
//! The driver surface edits the whole `devices` list as structured
//! objects plus the driver-level E1.31 defaults. Device surfaces are
//! read-only diagnostics resolved from the device's discovery metadata;
//! a device is edited through its entry in the driver's `devices` list.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use hypercolor_driver_api::{
    ControlApplyTarget, DriverConfigView, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_support::{control_apply, control_surface};
use hypercolor_pixel_protocols::e131::{
    E131_MAX_PRIORITY, E131_MAX_UNIVERSE, E131_MIN_UNIVERSE, E131_SOURCE_NAME_MAX_BYTES,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::control::{ControlValue, IpText};
use hypercolor_types::controls::{
    ApplyImpact, ControlChange, ControlEnumOption, ControlFieldDescriptor, ControlGroupKind,
    ControlObjectField, ControlSurfaceDocument, ControlValueMap, ControlValueType,
};

use crate::DESCRIPTOR;
use crate::backend::METADATA_DEVICE_CONFIG;
use crate::color::ColorOrder;
use crate::config::{
    MAX_DEVICE_ID_LEN, MAX_DEVICES, MAX_FPS_LIMIT, MAX_NAME_LEN, PixelConfig, PixelDeviceConfig,
    PixelProtocol, PixelSegmentConfig,
};
use crate::plan::{PixelDevicePlan, resolve_config, resolve_device};

/// Driver field: configured devices.
pub const FIELD_DEVICES: &str = "devices";
/// Driver field: default E1.31 source name.
pub const FIELD_SOURCE_NAME: &str = "source_name";
/// Driver field: default E1.31 priority.
pub const FIELD_PRIORITY: &str = "priority";

const DEVICES_DESCRIPTION: &str = "Manually configured controllers. Each entry needs an id, a host, \
     a protocol, and either pixel_count or segments. Changes to a connected device apply when it \
     reconnects.";
const SOURCE_NAME_DESCRIPTION: &str =
    "E1.31 source name receivers display. Devices may override it.";
const PRIORITY_DESCRIPTION: &str =
    "E1.31 priority from 0 to 200; the standard default is 100. Devices may override it.";

// ── Driver surface ──────────────────────────────────────────────────────

/// Build the driver-scoped control surface for `config`.
#[must_use]
pub fn pixel_driver_control_surface(config: &PixelConfig) -> ControlSurfaceDocument {
    let mut document = control_surface::driver_surface(DESCRIPTOR.id);
    document.groups.extend([
        control_surface::group("devices", "Devices", ControlGroupKind::Connection, 0),
        control_surface::group("output", "Output", ControlGroupKind::Output, 10),
    ]);
    document.fields = pixel_driver_control_fields();
    document.values = pixel_config_values(config);
    document.revision = control_surface::value_map_revision(&document.values);
    control_surface::mark_fields_available(&mut document);
    document
}

/// Field descriptors of the driver surface.
#[must_use]
pub fn pixel_driver_control_fields() -> Vec<ControlFieldDescriptor> {
    vec![
        control_surface::driver_field(
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
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_SOURCE_NAME,
            "sACN Source Name",
            Some(SOURCE_NAME_DESCRIPTION),
            Some("output"),
            control_surface::string_value_type(Some(source_name_max_len())),
            ApplyImpact::DiscoveryRescan,
            10,
        ),
        control_surface::driver_field(
            DESCRIPTOR.id,
            FIELD_PRIORITY,
            "sACN Priority",
            Some(PRIORITY_DESCRIPTION),
            Some("output"),
            control_surface::integer_value_type(0, Some(i64::from(E131_MAX_PRIORITY))),
            ApplyImpact::DiscoveryRescan,
            20,
        ),
    ]
}

fn source_name_max_len() -> u16 {
    u16::try_from(E131_SOURCE_NAME_MAX_BYTES).unwrap_or(u16::MAX)
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

fn enum_type(options: impl IntoIterator<Item = (&'static str, &'static str)>) -> ControlValueType {
    ControlValueType::Enum {
        options: options
            .into_iter()
            .map(|(value, label)| ControlEnumOption::new(value, label))
            .collect(),
    }
}

fn protocol_type() -> ControlValueType {
    enum_type(
        PixelProtocol::ALL
            .into_iter()
            .map(|protocol| (protocol.as_str(), protocol.label())),
    )
}

fn color_order_type() -> ControlValueType {
    enum_type(
        ColorOrder::ALL
            .into_iter()
            .map(|order| (order.as_str(), order.as_str())),
    )
}

fn segment_object_type() -> ControlValueType {
    ControlValueType::Object {
        fields: vec![
            object_field(
                "name",
                "Name",
                control_surface::string_value_type(Some(name_max_len())),
                false,
            ),
            object_field(
                "pixel_count",
                "Pixels",
                int_type(1, i64::from(u32::MAX)),
                true,
            ),
            object_field(
                "start_universe",
                "Start Universe",
                int_type(0, i64::from(E131_MAX_UNIVERSE)),
                false,
            ),
            object_field("start_channel", "Start Channel", int_type(1, 512), false),
            object_field(
                "ddp_offset",
                "DDP Offset",
                int_type(0, i64::from(u32::MAX)),
                false,
            ),
        ],
    }
}

fn name_max_len() -> u16 {
    u16::try_from(MAX_NAME_LEN).unwrap_or(u16::MAX)
}

/// Structured value type of one `devices` entry.
#[must_use]
pub fn device_object_type() -> ControlValueType {
    ControlValueType::Object {
        fields: vec![
            object_field(
                "id",
                "ID",
                control_surface::string_value_type(Some(
                    u16::try_from(MAX_DEVICE_ID_LEN).unwrap_or(u16::MAX),
                )),
                true,
            ),
            object_field(
                "name",
                "Name",
                control_surface::string_value_type(Some(name_max_len())),
                false,
            ),
            object_field("host", "Host", ControlValueType::IpAddress, true),
            object_field("protocol", "Protocol", protocol_type(), true),
            object_field("port", "Port", int_type(1024, i64::from(u16::MAX)), false),
            object_field("color_order", "Color Order", color_order_type(), false),
            object_field(
                "channels_per_pixel",
                "Channels per Pixel",
                int_type(3, 512),
                false,
            ),
            object_field(
                "pixel_count",
                "Pixels",
                int_type(1, i64::from(u32::MAX)),
                false,
            ),
            object_field(
                "segments",
                "Segments",
                ControlValueType::List {
                    item_type: Box::new(segment_object_type()),
                    min_items: None,
                    max_items: Some(64),
                },
                false,
            ),
            object_field(
                "start_universe",
                "Start Universe",
                int_type(0, i64::from(E131_MAX_UNIVERSE)),
                false,
            ),
            object_field("start_channel", "Start Channel", int_type(1, 512), false),
            object_field(
                "channels_per_universe",
                "Channels per Universe",
                int_type(1, 512),
                false,
            ),
            object_field(
                "ddp_offset",
                "DDP Offset",
                int_type(0, i64::from(u32::MAX)),
                false,
            ),
            object_field(
                "priority",
                "sACN Priority",
                int_type(0, i64::from(E131_MAX_PRIORITY)),
                false,
            ),
            object_field(
                "source_name",
                "sACN Source Name",
                control_surface::string_value_type(Some(source_name_max_len())),
                false,
            ),
            object_field(
                "sync_universe",
                "sACN Sync Universe",
                int_type(i64::from(E131_MIN_UNIVERSE), i64::from(E131_MAX_UNIVERSE)),
                false,
            ),
            object_field(
                "cid",
                "sACN CID",
                control_surface::string_value_type(Some(36)),
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
pub fn pixel_config_values(config: &PixelConfig) -> ControlValueMap {
    ControlValueMap::from([
        (
            FIELD_DEVICES.to_owned(),
            ControlValue::List(config.devices.iter().map(device_value).collect()),
        ),
        (
            FIELD_SOURCE_NAME.to_owned(),
            ControlValue::Text(config.source_name.clone()),
        ),
        (
            FIELD_PRIORITY.to_owned(),
            ControlValue::Int(i64::from(config.priority)),
        ),
    ])
}

fn device_value(device: &PixelDeviceConfig) -> ControlValue {
    let mut map = BTreeMap::new();
    map.insert("id".to_owned(), ControlValue::Text(device.id.clone()));
    insert_text(&mut map, "name", device.name.as_deref());
    map.insert(
        "host".to_owned(),
        ControlValue::Ip(IpText::from(device.host)),
    );
    map.insert(
        "protocol".to_owned(),
        ControlValue::Enum(device.protocol.as_str().to_owned()),
    );
    insert_int(&mut map, "port", device.port.map(i64::from));
    map.insert(
        "color_order".to_owned(),
        ControlValue::Enum(device.color_order.as_str().to_owned()),
    );
    insert_int(
        &mut map,
        "channels_per_pixel",
        device.channels_per_pixel.map(i64::from),
    );
    insert_int(&mut map, "pixel_count", device.pixel_count.map(i64::from));
    if !device.segments.is_empty() {
        map.insert(
            "segments".to_owned(),
            ControlValue::List(device.segments.iter().map(segment_value).collect()),
        );
    }
    insert_int(
        &mut map,
        "start_universe",
        device.start_universe.map(i64::from),
    );
    insert_int(
        &mut map,
        "start_channel",
        device.start_channel.map(i64::from),
    );
    insert_int(
        &mut map,
        "channels_per_universe",
        device.channels_per_universe.map(i64::from),
    );
    insert_int(&mut map, "ddp_offset", device.ddp_offset.map(i64::from));
    insert_int(&mut map, "priority", device.priority.map(i64::from));
    insert_text(&mut map, "source_name", device.source_name.as_deref());
    insert_int(
        &mut map,
        "sync_universe",
        device.sync_universe.map(i64::from),
    );
    insert_text(
        &mut map,
        "cid",
        device.cid.map(|cid| cid.to_string()).as_deref(),
    );
    insert_int(&mut map, "max_fps", device.max_fps.map(i64::from));
    ControlValue::Map(map)
}

fn segment_value(segment: &PixelSegmentConfig) -> ControlValue {
    let mut map = BTreeMap::new();
    insert_text(&mut map, "name", segment.name.as_deref());
    map.insert(
        "pixel_count".to_owned(),
        ControlValue::Int(i64::from(segment.pixel_count)),
    );
    insert_int(
        &mut map,
        "start_universe",
        segment.start_universe.map(i64::from),
    );
    insert_int(
        &mut map,
        "start_channel",
        segment.start_channel.map(i64::from),
    );
    insert_int(&mut map, "ddp_offset", segment.ddp_offset.map(i64::from));
    ControlValue::Map(map)
}

fn insert_text(map: &mut BTreeMap<String, ControlValue>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), ControlValue::Text(value.to_owned()));
    }
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
pub fn parse_validated_config(entry: &DriverConfigEntry) -> Result<PixelConfig> {
    let config = DriverConfigView {
        driver_id: DESCRIPTOR.id,
        entry,
    }
    .parse_settings::<PixelConfig>()?;
    resolve_config(&config)?;
    Ok(config)
}

/// Validate a batch of control changes for the driver or a device.
///
/// # Errors
///
/// Returns an error for unknown fields, type mismatches, device-surface
/// writes, or a resulting config that fails validation.
pub fn validate_pixel_changes(
    target: &ControlApplyTarget<'_>,
    changes: &[ControlChange],
) -> Result<ValidatedControlChanges> {
    match target {
        ControlApplyTarget::Driver { driver_id, config } => {
            if *driver_id != DESCRIPTOR.id {
                bail!("pixel controls cannot validate driver '{driver_id}'");
            }
            let validated = control_apply::validate_control_changes(
                "pixel",
                pixel_driver_control_fields(),
                changes,
                |_| Ok(()),
            )?;
            let mut values = pixel_config_values(&config.parse_settings::<PixelConfig>()?);
            for change in &validated.changes {
                values.insert(change.field_id.clone(), change.value.clone());
            }
            parse_validated_config(&values_entry(&values)?)
                .context("pixel driver config would be invalid")?;
            Ok(validated)
        }
        ControlApplyTarget::Device { device } => {
            if device.info.driver_id() != DESCRIPTOR.id {
                bail!(
                    "pixel controls cannot validate device owned by '{}'",
                    device.info.driver_id()
                );
            }
            bail!("pixel device fields are read-only; edit the device in the driver's devices list")
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
                .with_context(|| format!("failed to serialize pixel control '{key}'"))
        })
        .collect::<Result<_>>()?;
    Ok(DriverConfigEntry::enabled(settings))
}

// ── Device surface ──────────────────────────────────────────────────────

/// Resolve a tracked device's plan from its discovery metadata.
#[must_use]
pub fn tracked_device_plan(device: &TrackedDeviceCtx<'_>) -> Option<PixelDevicePlan> {
    let raw = device.metadata?.get(METADATA_DEVICE_CONFIG)?;
    let config: PixelDeviceConfig = serde_json::from_str(raw).ok()?;
    resolve_device(&config).ok()
}

/// Build the read-only device surface.
#[must_use]
pub fn pixel_device_control_surface(device: &TrackedDeviceCtx<'_>) -> ControlSurfaceDocument {
    let mut document = control_surface::device_surface(DESCRIPTOR.id, device.device_id);
    document.groups.extend([
        control_surface::group("connection", "Connection", ControlGroupKind::Connection, 0),
        control_surface::group("output", "Output", ControlGroupKind::Output, 10),
    ]);

    if let Some(plan) = tracked_device_plan(device) {
        let text = control_surface::string_value_type(Some(255));
        let readonly = |document: &mut ControlSurfaceDocument,
                        id: &str,
                        label: &str,
                        group: &str,
                        value_type: ControlValueType,
                        value: ControlValue,
                        ordering: i32| {
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
        };
        readonly(
            &mut document,
            "config_id",
            "Config ID",
            "connection",
            text.clone(),
            ControlValue::Text(plan.id.clone()),
            0,
        );
        readonly(
            &mut document,
            "host",
            "Host",
            "connection",
            ControlValueType::IpAddress,
            ControlValue::Ip(IpText::from(plan.target.ip())),
            10,
        );
        readonly(
            &mut document,
            "port",
            "Port",
            "connection",
            control_surface::integer_value_type(0, Some(i64::from(u16::MAX))),
            ControlValue::Int(i64::from(plan.target.port())),
            20,
        );
        readonly(
            &mut document,
            "protocol",
            "Protocol",
            "output",
            text.clone(),
            ControlValue::Text(plan.protocol.label().to_owned()),
            30,
        );
        if let Some(universes) = plan.universe_summary() {
            readonly(
                &mut document,
                "universes",
                "Universes",
                "output",
                text.clone(),
                ControlValue::Text(universes),
                40,
            );
        }
        readonly(
            &mut document,
            "pixel_count",
            "Pixels",
            "output",
            control_surface::integer_value_type(0, None),
            ControlValue::Int(i64::from(plan.total_pixels())),
            50,
        );
        readonly(
            &mut document,
            "color_order",
            "Color Order",
            "output",
            text,
            ControlValue::Text(plan.color_order.as_str().to_owned()),
            60,
        );
    }

    control_surface::mark_fields_available(&mut document);
    let mut payload = device.device_id.to_string().into_bytes();
    control_surface::extend_metadata_revision(&mut payload, device.metadata);
    document.revision = control_surface::revision_hash(&payload);
    document
}
