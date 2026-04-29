use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hypercolor_tui::action::Action;
use hypercolor_tui::component::Component;
use hypercolor_tui::state::DeviceSummary;
use hypercolor_tui::views::DeviceManagerView;
use hypercolor_types::controls::{
    ActionConfirmation, ActionConfirmationLevel, ApplyImpact, ControlActionDescriptor,
    ControlAvailabilityExpr, ControlObjectField, ControlOwner, ControlSurfaceDocument,
    ControlSurfaceScope, ControlValue, ControlValueType,
};
use hypercolor_types::device::DeviceId;

const DEVICE_ID: &str = "00000000-0000-0000-0000-000000000001";

#[test]
fn confirmed_device_action_requires_second_enter() {
    let mut view = loaded_device_manager(true);

    let first = view
        .handle_key_event(enter_key())
        .expect("first enter should be handled");
    assert!(matches!(first, Some(Action::Render)));

    let second = view
        .handle_key_event(enter_key())
        .expect("second enter should be handled");
    match second {
        Some(Action::InvokeDeviceControlAction {
            device_id,
            surface_id,
            action_id,
            input,
        }) => {
            assert_eq!(device_id, DEVICE_ID);
            assert_eq!(surface_id, format!("driver:wled:device:{DEVICE_ID}"));
            assert_eq!(action_id, "factory_reset");
            assert!(input.is_empty());
        }
        other => panic!("expected confirmed action invocation, got {other:?}"),
    }
}

#[test]
fn unconfirmed_device_action_invokes_on_first_enter() {
    let mut view = loaded_device_manager(false);

    let action = view
        .handle_key_event(enter_key())
        .expect("enter should be handled");
    match action {
        Some(Action::InvokeDeviceControlAction { action_id, .. }) => {
            assert_eq!(action_id, "factory_reset");
        }
        other => panic!("expected action invocation, got {other:?}"),
    }
}

#[test]
fn action_with_default_input_invokes_with_default_values() {
    let mut view = loaded_device_manager_with_surface(control_surface_with_default_input());

    let action = view
        .handle_key_event(enter_key())
        .expect("enter should be handled");
    match action {
        Some(Action::InvokeDeviceControlAction {
            action_id, input, ..
        }) => {
            assert_eq!(action_id, "identify");
            assert_eq!(
                input.get("duration_ms"),
                Some(&ControlValue::DurationMs(3000))
            );
            assert!(!input.contains_key("color"));
        }
        other => panic!("expected action invocation, got {other:?}"),
    }
}

fn loaded_device_manager(requires_confirmation: bool) -> DeviceManagerView {
    loaded_device_manager_with_surface(control_surface(requires_confirmation))
}

fn loaded_device_manager_with_surface(surface: ControlSurfaceDocument) -> DeviceManagerView {
    let mut view = DeviceManagerView::new();
    let devices = Arc::new(vec![DeviceSummary {
        id: DEVICE_ID.to_owned(),
        name: "Desk Grid".to_owned(),
        family: "WLED".to_owned(),
        led_count: 225,
        state: "connected".to_owned(),
        fps: None,
    }]);
    view.update(&Action::DevicesUpdated(devices))
        .expect("devices should update");
    view.update(&Action::DeviceControlSurfacesUpdated {
        device_id: DEVICE_ID.to_owned(),
        surfaces: Arc::new(vec![surface]),
    })
    .expect("control surfaces should update");
    view
}

fn control_surface(requires_confirmation: bool) -> ControlSurfaceDocument {
    let mut surface = ControlSurfaceDocument::empty(
        format!("driver:wled:device:{DEVICE_ID}"),
        ControlSurfaceScope::Device {
            device_id: DEVICE_ID.parse::<DeviceId>().expect("valid device id"),
            driver_id: "wled".to_owned(),
        },
    );
    surface.actions.push(ControlActionDescriptor {
        id: "factory_reset".to_owned(),
        owner: ControlOwner::Driver {
            driver_id: "wled".to_owned(),
        },
        group_id: None,
        label: "Factory Reset".to_owned(),
        description: None,
        input_fields: Vec::new(),
        result_type: None,
        confirmation: requires_confirmation.then_some(ActionConfirmation {
            level: ActionConfirmationLevel::Destructive,
            message: "This resets the device".to_owned(),
        }),
        apply_impact: ApplyImpact::DeviceReconnect,
        availability: ControlAvailabilityExpr::Always,
        ordering: 0,
    });
    surface
}

fn control_surface_with_default_input() -> ControlSurfaceDocument {
    let mut surface = control_surface(false);
    surface.actions.clear();
    surface.actions.push(ControlActionDescriptor {
        id: "identify".to_owned(),
        owner: ControlOwner::Host,
        group_id: None,
        label: "Identify".to_owned(),
        description: None,
        input_fields: vec![
            ControlObjectField {
                id: "duration_ms".to_owned(),
                label: "Duration".to_owned(),
                value_type: ControlValueType::DurationMs {
                    min: Some(1),
                    max: Some(120_000),
                    step: Some(100),
                },
                required: false,
                default_value: Some(ControlValue::DurationMs(3000)),
            },
            ControlObjectField {
                id: "color".to_owned(),
                label: "Color".to_owned(),
                value_type: ControlValueType::ColorRgb,
                required: false,
                default_value: None,
            },
        ],
        result_type: None,
        confirmation: None,
        apply_impact: ApplyImpact::Live,
        availability: ControlAvailabilityExpr::Always,
        ordering: 0,
    });
    surface
}

fn enter_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}
