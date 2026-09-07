use hypercolor_types::api::config::ConfigMutationResponse;

#[test]
fn config_mutation_response_round_trips_the_wire_contract() {
    let response = ConfigMutationResponse {
        key: Some("daemon.target_fps".to_owned()),
        value: Some(serde_json::json!(60)),
        live: true,
        requires_restart: false,
        pending_restart: vec!["server".to_owned()],
        path: "/state/hypercolor/config.toml".to_owned(),
    };

    let value = serde_json::to_value(&response).expect("config mutation response serializes");
    assert_eq!(
        value,
        serde_json::json!({
            "key": "daemon.target_fps",
            "value": 60,
            "live": true,
            "requires_restart": false,
            "pending_restart": ["server"],
            "path": "/state/hypercolor/config.toml"
        })
    );
    assert_eq!(
        serde_json::from_value::<ConfigMutationResponse>(value)
            .expect("config mutation response deserializes"),
        response
    );
}
