use hypercolor_openrgb_host::configured_endpoint;
use hypercolor_types::config::DriverConfigEntry;
use serde_json::json;

#[test]
fn configured_endpoint_respects_custom_port_and_rejects_remote_or_empty() {
    let mut config = DriverConfigEntry::default();
    assert_eq!(
        configured_endpoint(&config).expect("default").to_string(),
        "127.0.0.1:6742"
    );
    config
        .settings
        .insert("endpoints".to_owned(), json!(["127.0.0.1:6789"]));
    assert_eq!(configured_endpoint(&config).expect("custom").port(), 6789);
    for value in [json!([]), json!(["192.0.2.1:6742"]), json!(["malformed"])] {
        config.settings.insert("endpoints".to_owned(), value);
        assert!(configured_endpoint(&config).is_err());
    }
}
