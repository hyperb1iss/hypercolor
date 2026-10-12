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
        persisted: true,
        persistence_error: None,
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
            "path": "/state/hypercolor/config.toml",
            "persisted": true
        })
    );
    assert_eq!(
        serde_json::from_value::<ConfigMutationResponse>(value)
            .expect("config mutation response deserializes"),
        response
    );
}

#[test]
fn an_unsaved_config_mutation_carries_its_persistence_error() {
    let response = ConfigMutationResponse {
        key: Some("audio.device".to_owned()),
        value: Some(serde_json::json!("microphone")),
        live: false,
        requires_restart: false,
        pending_restart: Vec::new(),
        path: "/state/hypercolor/config.toml".to_owned(),
        persisted: false,
        persistence_error: Some("permission denied".to_owned()),
    };

    let value = serde_json::to_value(&response).expect("config mutation response serializes");
    assert_eq!(value["persisted"], false);
    assert_eq!(value["persistence_error"], "permission denied");
    assert_eq!(
        serde_json::from_value::<ConfigMutationResponse>(value)
            .expect("config mutation response deserializes"),
        response
    );
}

#[test]
fn a_config_mutation_without_persistence_fields_reads_as_persisted() {
    let response = serde_json::from_value::<ConfigMutationResponse>(serde_json::json!({
        "key": "daemon.target_fps",
        "value": 60,
        "live": true,
        "requires_restart": false,
        "pending_restart": [],
        "path": "/state/hypercolor/config.toml"
    }))
    .expect("a response from an older daemon deserializes");

    assert!(response.persisted);
    assert_eq!(response.persistence_error, None);
}
