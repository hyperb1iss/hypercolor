use hypercolor_types::api::output::{OutputPatchRequest, OutputPowerMode, OutputResource};

#[test]
fn output_resource_carries_power_and_brightness_together() {
    let resource = OutputResource {
        power: OutputPowerMode::Paused,
        brightness: 0.5,
    };
    assert_eq!(
        serde_json::to_value(resource).expect("output resource should encode"),
        serde_json::json!({ "power": "paused", "brightness": 0.5 })
    );
}

#[test]
fn output_patch_accepts_either_field_or_both() {
    let power: OutputPatchRequest =
        serde_json::from_str(r#"{"power":"running"}"#).expect("power-only patch should decode");
    assert_eq!(power.power, Some(OutputPowerMode::Running));
    assert_eq!(power.brightness, None);

    let brightness: OutputPatchRequest = serde_json::from_str(r#"{"brightness":0.25}"#)
        .expect("brightness-only patch should decode");
    assert_eq!(brightness.power, None);
    assert_eq!(brightness.brightness, Some(0.25));

    let both: OutputPatchRequest = serde_json::from_str(r#"{"power":"paused","brightness":0.0}"#)
        .expect("full patch should decode");
    assert_eq!(both.power, Some(OutputPowerMode::Paused));
    assert_eq!(both.brightness, Some(0.0));
}

#[test]
fn output_patch_rejects_the_retired_power_vocabulary() {
    assert!(serde_json::from_str::<OutputPatchRequest>(r#"{"power":"stopped"}"#).is_err());
    assert!(serde_json::from_str::<OutputPatchRequest>(r#"{"state":"paused"}"#).is_err());
}

/// An empty patch decodes — the service, not the decoder, is where a
/// no-op request is refused, so the wire type stays a pure projection.
#[test]
fn output_patch_decodes_an_empty_document() {
    let empty: OutputPatchRequest = serde_json::from_str("{}").expect("empty patch should decode");
    assert_eq!(empty, OutputPatchRequest::default());
}
