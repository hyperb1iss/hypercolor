use hypercolor_types::controls::{
    ControlAccess, ControlActionDescriptor, ControlAvailabilityState, ControlFieldDescriptor,
    ControlOwner, ControlSurfaceDocument, ControlSurfaceScope, ControlValue as DynamicControlValue,
    ControlValueType,
};

pub fn visible_control_surfaces(
    surfaces: Vec<ControlSurfaceDocument>,
) -> Vec<ControlSurfaceDocument> {
    surfaces
        .into_iter()
        .filter(surface_has_visible_items)
        .collect()
}

pub fn driver_owned_device_control_surfaces(
    surfaces: Vec<ControlSurfaceDocument>,
    device_id: &str,
) -> Vec<ControlSurfaceDocument> {
    surfaces
        .into_iter()
        .filter_map(|surface| driver_owned_device_control_surface(surface, device_id))
        .collect()
}

pub fn driver_owned_device_control_surface(
    mut surface: ControlSurfaceDocument,
    device_id: &str,
) -> Option<ControlSurfaceDocument> {
    match &surface.scope {
        ControlSurfaceScope::Device {
            device_id: surface_device_id,
            ..
        } if surface_device_id.to_string() == device_id => {}
        _ => return None,
    }

    let availability = surface.availability.clone();
    let action_availability = surface.action_availability.clone();

    surface.fields.retain(|field| {
        matches!(field.owner, ControlOwner::Driver { .. })
            && !availability
                .get(&field.id)
                .is_some_and(|availability| availability.state == ControlAvailabilityState::Hidden)
    });
    surface.actions.retain(|action| {
        matches!(action.owner, ControlOwner::Driver { .. })
            && !action_availability.get(&action.id).is_some_and(|availability| {
                availability.state == ControlAvailabilityState::Hidden
            })
    });

    surface_has_visible_items(&surface).then_some(surface)
}

pub fn actionable_control_surfaces(
    surfaces: Vec<ControlSurfaceDocument>,
) -> Vec<ControlSurfaceDocument> {
    surfaces
        .into_iter()
        .filter_map(actionable_control_surface)
        .collect()
}

pub fn actionable_control_surface(
    mut surface: ControlSurfaceDocument,
) -> Option<ControlSurfaceDocument> {
    surface
        .fields
        .retain(|field| field.access != ControlAccess::ReadOnly && field_has_actionable_editor(field));
    surface_has_visible_items(&surface).then_some(surface)
}

fn field_has_actionable_editor(field: &ControlFieldDescriptor) -> bool {
    matches!(
        field.value_type,
        ControlValueType::Bool
            | ControlValueType::Integer { .. }
            | ControlValueType::Float { .. }
            | ControlValueType::String { .. }
            | ControlValueType::Secret
            | ControlValueType::ColorRgb
            | ControlValueType::ColorRgba
            | ControlValueType::IpAddress
            | ControlValueType::MacAddress
            | ControlValueType::DurationMs { .. }
            | ControlValueType::Enum { .. }
    )
}

pub fn surface_has_visible_items(surface: &ControlSurfaceDocument) -> bool {
    visible_field_count(surface) > 0 || visible_action_count(surface) > 0
}

pub fn visible_field_count(surface: &ControlSurfaceDocument) -> usize {
    surface
        .fields
        .iter()
        .filter(|field| !field_is_hidden(surface, field))
        .count()
}

pub fn visible_action_count(surface: &ControlSurfaceDocument) -> usize {
    surface
        .actions
        .iter()
        .filter(|action| !action_is_hidden(surface, action))
        .count()
}

pub fn field_is_hidden(surface: &ControlSurfaceDocument, field: &ControlFieldDescriptor) -> bool {
    surface
        .availability
        .get(&field.id)
        .is_some_and(|availability| availability.state == ControlAvailabilityState::Hidden)
}

pub fn action_is_hidden(
    surface: &ControlSurfaceDocument,
    action: &ControlActionDescriptor,
) -> bool {
    surface
        .action_availability
        .get(&action.id)
        .is_some_and(|availability| availability.state == ControlAvailabilityState::Hidden)
}

pub fn control_value_summary(value: Option<&DynamicControlValue>) -> String {
    match value {
        Some(DynamicControlValue::String(value))
        | Some(DynamicControlValue::IpAddress(value))
        | Some(DynamicControlValue::MacAddress(value)) => value.clone(),
        Some(DynamicControlValue::SecretRef(_)) => "Configured".to_string(),
        Some(DynamicControlValue::ColorRgb(value)) => {
            format!("#{:02x}{:02x}{:02x}", value[0], value[1], value[2])
        }
        Some(DynamicControlValue::ColorRgba(value)) => {
            format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                value[0], value[1], value[2], value[3]
            )
        }
        Some(DynamicControlValue::Bool(value)) => value.to_string(),
        Some(DynamicControlValue::Integer(value)) => value.to_string(),
        Some(DynamicControlValue::Float(value)) => value.to_string(),
        Some(DynamicControlValue::DurationMs(value)) => value.to_string(),
        Some(DynamicControlValue::Enum(value)) => value.clone(),
        Some(DynamicControlValue::Flags(values)) => values.join(", "),
        Some(DynamicControlValue::List(_)) => "list".to_string(),
        Some(DynamicControlValue::Object(_)) => "object".to_string(),
        Some(DynamicControlValue::Unknown) => "unsupported value".to_string(),
        Some(DynamicControlValue::Null) | None => String::new(),
    }
}

pub fn control_surface_event_matches_device(surface_id: &str, device_id: &str) -> bool {
    surface_id == format!("device:{device_id}")
        || surface_id.ends_with(&format!(":device:{device_id}"))
        || !surface_id.contains(":device:")
}

#[cfg(test)]
mod tests {
    use hypercolor_types::controls::{
        ControlSurfaceDocument, ControlSurfaceScope, ControlValue, ControlValueType,
    };

    use super::{
        actionable_control_surfaces, control_surface_event_matches_device, control_value_summary,
        driver_owned_device_control_surfaces, visible_control_surfaces, visible_field_count,
    };

    const DEVICE_ID: &str = "00000000-0000-0000-0000-000000000001";

    #[test]
    fn empty_control_surfaces_are_hidden_from_device_cards() {
        let surface = ControlSurfaceDocument::empty(
            "driver:future",
            ControlSurfaceScope::Driver {
                driver_id: "future".to_string(),
            },
        );

        assert!(visible_control_surfaces(vec![surface]).is_empty());
    }

    #[test]
    fn unknown_future_value_types_stay_visible_and_summarizable() {
        let surface: ControlSurfaceDocument = serde_json::from_value(serde_json::json!({
            "surface_id": "driver:future",
            "scope": { "driver": { "driver_id": "future" } },
            "schema_version": 1,
            "revision": 42,
            "groups": [],
            "fields": [{
                "id": "tone_curve",
                "owner": { "driver": { "driver_id": "future" } },
                "label": "Tone Curve",
                "value_type": { "kind": "spline_curve" },
                "access": "read_write",
                "persistence": "driver_config",
                "apply_impact": "live",
                "visibility": "standard",
                "availability": { "kind": "always" },
                "ordering": 0
            }],
            "actions": [],
            "values": {
                "tone_curve": { "kind": "binary_blob", "value": "opaque-driver-data" }
            },
            "availability": {},
            "action_availability": {}
        }))
        .expect("surface document should tolerate future control kinds");

        assert_eq!(surface.fields[0].value_type, ControlValueType::Unknown);
        assert_eq!(surface.values["tone_curve"], ControlValue::Unknown);
        assert_eq!(visible_field_count(&surface), 1);
        assert_eq!(
            control_value_summary(surface.values.get("tone_curve")),
            "unsupported value"
        );
        assert_eq!(visible_control_surfaces(vec![surface]).len(), 1);
    }

    #[test]
    fn device_control_surface_events_match_only_their_device() {
        assert!(control_surface_event_matches_device(
            "device:desk-strip",
            "desk-strip"
        ));
        assert!(control_surface_event_matches_device(
            "driver:alpha:device:desk-strip",
            "desk-strip"
        ));
        assert!(!control_surface_event_matches_device(
            "driver:alpha:device:desk-strip",
            "shelf-strip"
        ));
    }

    #[test]
    fn driver_control_surface_events_match_all_device_pages() {
        assert!(control_surface_event_matches_device(
            "driver:alpha",
            "desk-strip"
        ));
        assert!(control_surface_event_matches_device(
            "driver:beta",
            "panel-wall"
        ));
    }

    #[test]
    fn driver_owned_device_surfaces_drop_host_controls() {
        let surfaces = driver_owned_device_control_surfaces(control_surface_fixture(), DEVICE_ID);

        assert_eq!(surfaces.len(), 1);
        assert_eq!(
            surfaces[0].surface_id,
            "driver:wled:device:00000000-0000-0000-0000-000000000001"
        );
        assert_eq!(
            field_ids(&surfaces[0]),
            vec!["protocol", "firmware_version", "channel_mask"]
        );
    }

    #[test]
    fn actionable_control_surfaces_drop_read_only_fields() {
        let surfaces = driver_owned_device_control_surfaces(control_surface_fixture(), DEVICE_ID);

        let surfaces = actionable_control_surfaces(surfaces);

        assert_eq!(surfaces.len(), 1);
        assert_eq!(field_ids(&surfaces[0]), vec!["protocol"]);
    }

    #[test]
    fn driver_owned_device_surfaces_ignore_other_devices() {
        let surface = ControlSurfaceDocument::empty(
            "driver:wled:device:00000000-0000-0000-0000-000000000001",
            ControlSurfaceScope::Device {
                device_id: DEVICE_ID.parse().expect("device id fixture should parse"),
                driver_id: "wled".to_string(),
            },
        );

        assert!(
            driver_owned_device_control_surfaces(
                vec![surface],
                "00000000-0000-0000-0000-000000000002",
            )
            .is_empty()
        );
    }

    fn field_ids(surface: &ControlSurfaceDocument) -> Vec<&str> {
        surface
            .fields
            .iter()
            .map(|field| field.id.as_str())
            .collect()
    }

    fn control_surface_fixture() -> Vec<ControlSurfaceDocument> {
        serde_json::from_value(serde_json::json!([
            {
                "surface_id": "device:00000000-0000-0000-0000-000000000001",
                "scope": {
                    "device": {
                        "device_id": DEVICE_ID,
                        "driver_id": "wled"
                    }
                },
                "schema_version": 1,
                "revision": 1,
                "groups": [],
                "fields": [{
                    "id": "brightness",
                    "owner": "host",
                    "label": "Brightness",
                    "value_type": { "kind": "integer", "min": 0, "max": 100, "step": 1 },
                    "access": "read_write",
                    "persistence": "runtime_only",
                    "apply_impact": "live",
                    "visibility": "standard",
                    "availability": { "kind": "always" },
                    "ordering": 0
                }],
                "actions": [],
                "values": { "brightness": { "kind": "integer", "value": 100 } },
                "availability": {},
                "action_availability": {}
            },
            {
                "surface_id": "driver:wled:device:00000000-0000-0000-0000-000000000001",
                "scope": {
                    "device": {
                        "device_id": DEVICE_ID,
                        "driver_id": "wled"
                    }
                },
                "schema_version": 1,
                "revision": 7,
                "groups": [],
                "fields": [
                    {
                        "id": "protocol",
                        "owner": { "driver": { "driver_id": "wled" } },
                        "label": "Protocol",
                        "value_type": {
                            "kind": "enum",
                            "options": [
                                { "value": "ddp", "label": "DDP", "description": null, "deprecated": false }
                            ]
                        },
                        "access": "read_write",
                        "persistence": "device_config",
                        "apply_impact": "device_reconnect",
                        "visibility": "advanced",
                        "availability": { "kind": "always" },
                        "ordering": 0
                    },
                    {
                        "id": "firmware_version",
                        "owner": { "driver": { "driver_id": "wled" } },
                        "label": "Firmware",
                        "value_type": { "kind": "string", "min_len": null, "max_len": null, "pattern": null },
                        "access": "read_only",
                        "persistence": "runtime_only",
                        "apply_impact": "none",
                        "visibility": "diagnostics",
                        "availability": { "kind": "always" },
                        "ordering": 1
                    },
                    {
                        "id": "channel_mask",
                        "owner": { "driver": { "driver_id": "wled" } },
                        "label": "Channel Mask",
                        "value_type": {
                            "kind": "flags",
                            "options": [
                                { "value": "main", "label": "Main", "description": null, "deprecated": false }
                            ]
                        },
                        "access": "read_write",
                        "persistence": "device_config",
                        "apply_impact": "device_reconnect",
                        "visibility": "advanced",
                        "availability": { "kind": "always" },
                        "ordering": 2
                    },
                    {
                        "id": "name",
                        "owner": "host",
                        "label": "Name",
                        "value_type": { "kind": "string", "min_len": null, "max_len": null, "pattern": null },
                        "access": "read_write",
                        "persistence": "runtime_only",
                        "apply_impact": "live",
                        "visibility": "standard",
                        "availability": { "kind": "always" },
                        "ordering": 2
                    }
                ],
                "actions": [],
                "values": {
                    "protocol": { "kind": "enum", "value": "ddp" },
                    "firmware_version": { "kind": "string", "value": "0.15.3" },
                    "channel_mask": { "kind": "flags", "value": ["main"] },
                    "name": { "kind": "string", "value": "Desk Strip" }
                },
                "availability": {},
                "action_availability": {}
            }
        ]))
        .expect("surface fixture should deserialize")
    }
}
