use hypercolor_daemon::api::AppState;

#[test]
fn default_app_state_registers_builtin_network_drivers() {
    let state = AppState::new();
    let ids = state.driver_registry.ids();

    assert!(ids.contains(&"wled".to_owned()));
    #[cfg(feature = "hue")]
    assert!(ids.contains(&"hue".to_owned()));
    #[cfg(feature = "nanoleaf")]
    assert!(ids.contains(&"nanoleaf".to_owned()));
}

#[test]
fn builtin_pairing_drivers_expose_pairing_capabilities() {
    let state = AppState::new();
    #[cfg(not(any(feature = "hue", feature = "nanoleaf")))]
    let _ = &state;

    #[cfg(feature = "hue")]
    assert!(
        state
            .driver_registry
            .get("hue")
            .expect("hue driver should be registered")
            .pairing()
            .is_some()
    );

    #[cfg(feature = "nanoleaf")]
    assert!(
        state
            .driver_registry
            .get("nanoleaf")
            .expect("nanoleaf driver should be registered")
            .pairing()
            .is_some()
    );
}
