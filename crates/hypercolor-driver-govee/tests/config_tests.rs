use hypercolor_driver_govee::GoveeConfig;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn govee_defaults_match_driver_contract() {
    let config = GoveeConfig::default();

    assert!(config.known_ips.is_empty());
    assert!(!config.power_off_on_disconnect);
    assert_eq!(config.lan_state_fps, 10);
    assert_eq!(config.razer_fps, 25);
}

#[test]
fn partial_govee_settings_receive_driver_owned_defaults() -> TestResult {
    let config = serde_json::from_value::<GoveeConfig>(serde_json::json!({
        "power_off_on_disconnect": true
    }))?;

    assert!(config.known_ips.is_empty());
    assert!(config.power_off_on_disconnect);
    assert_eq!(config.lan_state_fps, 10);
    assert_eq!(config.razer_fps, 25);
    Ok(())
}
