use hypercolor_driver_api::{ValidatedControlChanges, control_apply, control_surface};
use hypercolor_types::controls::{ApplyImpact, ControlChange, ControlValue, ControlValueMap};

#[test]
fn validate_control_changes_dedupes_impacts() {
    let fields = vec![
        control_surface::driver_field(
            "wled",
            "known_ips",
            "Known IPs",
            None,
            Some("connection"),
            control_surface::ip_list_value_type(64),
            ApplyImpact::DiscoveryRescan,
            0,
        ),
        control_surface::driver_field(
            "wled",
            "realtime_http_enabled",
            "Realtime HTTP",
            None,
            Some("output"),
            hypercolor_types::controls::ControlValueType::Bool,
            ApplyImpact::DiscoveryRescan,
            10,
        ),
    ];
    let changes = vec![
        ControlChange {
            field_id: "known_ips".to_owned(),
            value: ControlValue::List(Vec::new()),
        },
        ControlChange {
            field_id: "realtime_http_enabled".to_owned(),
            value: ControlValue::Bool(true),
        },
    ];

    let validated = control_apply::validate_control_changes("WLED", fields, &changes, |_| Ok(()))
        .expect("valid changes");

    assert_eq!(validated.changes, changes);
    assert_eq!(validated.impacts, vec![ApplyImpact::DiscoveryRescan]);
}

#[test]
fn validate_control_changes_rejects_duplicates() {
    let fields = vec![control_surface::driver_field(
        "hue",
        "use_cie_xy",
        "CIE xy Streaming",
        None,
        Some("output"),
        hypercolor_types::controls::ControlValueType::Bool,
        ApplyImpact::BackendRebind,
        0,
    )];
    let changes = vec![
        ControlChange {
            field_id: "use_cie_xy".to_owned(),
            value: ControlValue::Bool(true),
        },
        ControlChange {
            field_id: "use_cie_xy".to_owned(),
            value: ControlValue::Bool(false),
        },
    ];

    let error = control_apply::validate_control_changes("Hue", fields, &changes, |_| Ok(()))
        .expect_err("duplicate field should fail");

    assert!(error.to_string().contains("duplicate Hue control field"));
}

#[test]
fn apply_value_changes_returns_old_and_new_revisions() {
    let values = ControlValueMap::from([("use_cie_xy".to_owned(), ControlValue::Bool(true))]);
    let changes = vec![ControlChange {
        field_id: "use_cie_xy".to_owned(),
        value: ControlValue::Bool(false),
    }];

    let (values, previous_revision, revision) = control_apply::apply_value_changes(
        values,
        &changes,
        control_surface::value_map_revision,
        |change| change.value.clone(),
    );

    assert_ne!(previous_revision, revision);
    assert_eq!(values["use_cie_xy"], ControlValue::Bool(false));
}

#[test]
fn apply_response_marks_changes_accepted() {
    let changes = ValidatedControlChanges {
        changes: vec![ControlChange {
            field_id: "transition_time".to_owned(),
            value: ControlValue::Integer(12),
        }],
        impacts: vec![ApplyImpact::BackendRebind],
    };
    let values = ControlValueMap::from([("transition_time".to_owned(), ControlValue::Integer(12))]);

    let response = control_apply::apply_response("driver:nanoleaf", 1, 2, changes, values);

    assert_eq!(response.surface_id, "driver:nanoleaf");
    assert_eq!(response.previous_revision, 1);
    assert_eq!(response.revision, 2);
    assert_eq!(response.accepted.len(), 1);
    assert!(response.rejected.is_empty());
    assert_eq!(response.impacts, vec![ApplyImpact::BackendRebind]);
}
