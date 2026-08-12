use hypercolor_app::window::{
    SETTINGS_ROUTE, WINDOW_VISIBILITY_EVENT, WINDOW_VISIBILITY_GLOBAL, macos_system_settings_url,
    route_navigation_script, should_open_in_system_browser, system_browser_url,
    visibility_state_script,
};

#[test]
fn visibility_state_script_sets_global_and_dispatches_event() {
    let script = visibility_state_script(false);

    assert!(script.contains(WINDOW_VISIBILITY_GLOBAL));
    assert!(script.contains(WINDOW_VISIBILITY_EVENT));
    assert!(script.contains("const visible = false;"));
    assert!(script.contains("CustomEvent"));
}

#[test]
fn route_navigation_script_pushes_spa_route_and_notifies_router() {
    let script = route_navigation_script(SETTINGS_ROUTE);

    assert!(script.contains("\"/settings\""));
    assert!(script.contains("window.location.pathname"));
    assert!(script.contains("window.history.pushState"));
    assert!(script.contains("PopStateEvent"));
}

#[test]
fn system_browser_handoff_allows_only_web_urls() {
    let https = url::Url::parse("https://github.com/sponsors/hyperb1iss")
        .expect("sponsor URL should parse");
    let http = url::Url::parse("http://127.0.0.1:9420/preview").expect("preview URL should parse");
    let file = url::Url::parse("file:///tmp/hypercolor").expect("file URL should parse");

    assert!(should_open_in_system_browser(&https));
    assert!(should_open_in_system_browser(&http));
    assert!(!should_open_in_system_browser(&file));
}

#[test]
fn system_browser_url_rejects_malformed_and_non_web_urls() {
    assert!(system_browser_url("https://github.com/sponsors/hyperb1iss").is_ok());
    assert!(system_browser_url("file:///tmp/hypercolor").is_err());
    assert!(system_browser_url("x-apple.systempreferences:Privacy_ListenEvent").is_err());
    assert!(system_browser_url("not a url").is_err());
}

#[test]
fn macos_system_settings_allowlist_has_only_the_two_privacy_remedies() {
    assert_eq!(
        macos_system_settings_url("input_monitoring"),
        Ok(
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ListenEvent"
        )
    );
    assert_eq!(
        macos_system_settings_url("screen_recording"),
        Ok(
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ScreenCapture"
        )
    );
    assert!(macos_system_settings_url("privacy_security").is_err());
    assert!(macos_system_settings_url(
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_ListenEvent"
    )
    .is_err());
}
