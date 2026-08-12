use hypercolor_types::api::output::{
    OutputPowerMode, OutputPowerResponse, OutputPowerStatus, SetOutputPowerRequest,
};

#[test]
fn output_power_request_accepts_only_desired_states() {
    let paused: SetOutputPowerRequest =
        serde_json::from_str(r#"{"state":"paused"}"#).expect("paused request should decode");
    assert_eq!(paused.state, OutputPowerMode::Paused);
    assert!(serde_json::from_str::<SetOutputPowerRequest>(r#"{"state":"stopped"}"#).is_err());
}

#[test]
fn output_power_response_distinguishes_destructive_stop() {
    let response = OutputPowerResponse {
        state: OutputPowerStatus::Stopped,
    };
    assert_eq!(
        serde_json::to_value(response).expect("output power response should encode"),
        serde_json::json!({ "state": "stopped" })
    );
}
