//! The dashboard status line's health dot and value labels.

use hypercolor_ui::pages::dashboard::header::{
    DaemonHealth, HealthTone, daemon_health, device_count_label, fps_label,
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
