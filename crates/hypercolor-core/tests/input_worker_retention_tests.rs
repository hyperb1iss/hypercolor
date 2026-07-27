//! Process-owned input worker retention contract tests.

use std::time::Duration;

use hypercolor_core::input::worker_retention::test_reaper_spawn_failure_is_reapable;

#[test]
fn reaper_spawn_failure_keeps_worker_reachable_until_nonblocking_reap() {
    assert!(test_reaper_spawn_failure_is_reapable(Duration::from_secs(
        1
    )));
}

#[test]
fn every_input_backend_delegates_retention_to_the_shared_primitive() {
    for (backend, source) in [
        ("PulseAudio", include_str!("../src/input/audio/mod.rs")),
        ("evdev", include_str!("../src/input/evdev.rs")),
        (
            "fallback host input",
            include_str!("../src/input/interaction/mod.rs"),
        ),
        ("media", include_str!("../src/input/media.rs")),
        (
            "Wayland screen capture",
            include_str!("../src/input/screen/wayland.rs"),
        ),
        (
            "Windows screen capture",
            include_str!("../src/input/screen/windows.rs"),
        ),
    ] {
        assert!(
            source.contains("retain_input_worker("),
            "{backend} does not delegate timed-out worker retention"
        );
        assert!(
            !source.contains("-reaper\".to_owned())"),
            "{backend} still spawns a private reaper"
        );
    }
}
