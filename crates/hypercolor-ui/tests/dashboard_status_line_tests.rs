//! The dashboard status line's health dot and value labels.

use hypercolor_ui::pages::dashboard::header::{
    DaemonHealth, HealthTone, StatusValue, daemon_health, device_count_label, fps_label,
    separator_class,
};
use hypercolor_ui::ws::messages::ConnectionState;

#[test]
fn a_connected_daemon_with_its_render_loop_up_is_running() {
    let health = daemon_health(ConnectionState::Connected, Some(true), false);
    assert_eq!(health, DaemonHealth::Running);
    assert_eq!(health.label(), "Running");
    assert_eq!(health.tone(), HealthTone::Success);
    // Before the first status snapshot lands, a live socket reads as up.
    assert_eq!(
        daemon_health(ConnectionState::Connected, None, false),
        DaemonHealth::Running
    );
}

#[test]
fn paused_output_warns() {
    let health = daemon_health(ConnectionState::Connected, Some(true), true);
    assert_eq!(health, DaemonHealth::Paused);
    assert_eq!(health.tone(), HealthTone::Warning);
}

#[test]
fn a_stopped_render_loop_outranks_a_pause() {
    let health = daemon_health(ConnectionState::Connected, Some(false), true);
    assert_eq!(health, DaemonHealth::Stopped);
    assert_eq!(health.tone(), HealthTone::Error);
}

#[test]
fn the_socket_decides_before_anything_the_daemon_reported() {
    assert_eq!(
        daemon_health(ConnectionState::Connecting, Some(true), false),
        DaemonHealth::Connecting
    );
    assert_eq!(DaemonHealth::Connecting.tone(), HealthTone::Warning);
    for state in [ConnectionState::Disconnected, ConnectionState::Error] {
        let health = daemon_health(state, Some(true), false);
        assert_eq!(health, DaemonHealth::Offline);
        assert_eq!(health.tone(), HealthTone::Error);
    }
}

#[test]
fn values_read_as_plain_words() {
    assert_eq!(device_count_label(1), "1 device");
    assert_eq!(device_count_label(23), "23 devices");
    assert_eq!(device_count_label(0), "0 devices");
    assert_eq!(fps_label(60.0), "60 fps");
    assert_eq!(fps_label(59.6), "60 fps");
}

#[test]
fn the_first_value_shown_drops_its_separator_with_the_word_on_phones() {
    let all = [StatusValue::Uptime, StatusValue::Devices, StatusValue::Fps];
    assert!(separator_class(StatusValue::Uptime, &all).contains("max-md:hidden"));
    assert!(!separator_class(StatusValue::Devices, &all).contains("max-md:hidden"));
    assert!(!separator_class(StatusValue::Fps, &all).contains("max-md:hidden"));

    // Before the uptime lands, the device count leads the line.
    let no_uptime = [StatusValue::Devices, StatusValue::Fps];
    assert!(separator_class(StatusValue::Devices, &no_uptime).contains("max-md:hidden"));
    assert!(!separator_class(StatusValue::Fps, &no_uptime).contains("max-md:hidden"));
}

#[test]
fn wider_screens_keep_every_separator() {
    // Only the phone breakpoint hides a separator; the health word shows
    // above it, so each value keeps the dot that sets it apart.
    let all = [StatusValue::Uptime, StatusValue::Devices, StatusValue::Fps];
    for value in all {
        let class = separator_class(value, &all);
        assert!(
            class
                .split_whitespace()
                .all(|token| token == "text-fg-tertiary/50" || token.starts_with("max-md:")),
            "{class}"
        );
    }
}
