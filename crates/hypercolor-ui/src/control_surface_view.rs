use hypercolor_types::controls::{
    ControlActionDescriptor, ControlAvailabilityState, ControlFieldDescriptor,
    ControlSurfaceDocument, ControlValue as DynamicControlValue,
};

pub fn visible_control_surfaces(
    surfaces: Vec<ControlSurfaceDocument>,
) -> Vec<ControlSurfaceDocument> {
    surfaces
        .into_iter()
        .filter(surface_has_visible_items)
        .collect()
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
        control_surface_event_matches_device, control_value_summary, visible_control_surfaces,
        visible_field_count,
    };

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
}
