//! Windows capture adapter-boundary fixture contracts.
//!
//! The deterministic fixture exercises the production demand, epoch, and
//! session fencing without Desktop Duplication. Frames reach consumers only
//! through the exact publication hub, so these contracts cover activation
//! and fencing rather than pixels.

#![cfg(feature = "windows-capture-fixtures")]

use hypercolor_core::input::screen::consumer::{
    CaptureConfig, CaptureEpoch, CaptureSourceId, ScreenCaptureDemand,
};
use hypercolor_core::input::screen::implementer::WindowsScreenCaptureInput;
use hypercolor_core::input::{InputData, InputSource, ScreenSource};
use hypercolor_windows_capture::CaptureError;

fn fixture_epoch() -> CaptureEpoch {
    CaptureEpoch {
        source_id: CaptureSourceId::new("windows:fixture-display")
            .expect("fixture source id is valid"),
        topology_generation: 1,
        session_generation: 1,
    }
}

#[test]
fn deterministic_fixture_activates_only_for_live_demand() {
    let (mut source, fixture) = WindowsScreenCaptureInput::new_deterministic_fixture(
        CaptureConfig::default(),
        fixture_epoch(),
    )
    .expect("deterministic Windows source is valid");

    source.start().expect("deterministic source starts idle");
    assert!(!fixture.is_active());
    assert!(!fixture.epoch_is_current());
    assert!(matches!(source.sample(), Ok(InputData::None)));

    source
        .set_screen_capture_demand(ScreenCaptureDemand::active())
        .expect("deterministic demand is admitted");
    assert!(fixture.is_active());
    assert!(fixture.epoch_is_current());
    assert!(matches!(source.sample(), Ok(InputData::None)));

    source
        .set_screen_capture_demand(ScreenCaptureDemand::Inactive)
        .expect("deterministic demand releases");
    assert!(!fixture.is_active());
    assert!(!fixture.epoch_is_current());
}

#[test]
fn resource_frame_failures_keep_the_session_epoch_current() {
    let (mut source, fixture) = WindowsScreenCaptureInput::new_deterministic_fixture(
        CaptureConfig::default(),
        fixture_epoch(),
    )
    .expect("deterministic Windows source is valid");

    source.start().expect("deterministic source starts idle");
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active())
        .expect("deterministic demand is admitted");
    assert!(fixture.epoch_is_current());

    fixture.inject_frame_failure(&CaptureError::ResourceExhausted {
        operation: "inject frame pressure",
        requested_bytes: usize::MAX,
    });

    assert!(fixture.is_active());
    assert!(fixture.epoch_is_current());
}

#[test]
fn rebuild_resource_failures_fence_the_invalidated_session() {
    let (mut source, fixture) = WindowsScreenCaptureInput::new_deterministic_fixture(
        CaptureConfig::default(),
        fixture_epoch(),
    )
    .expect("deterministic Windows source is valid");

    source.start().expect("deterministic source starts idle");
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active())
        .expect("deterministic demand is admitted");
    assert!(fixture.epoch_is_current());

    fixture.inject_frame_failure(&CaptureError::SessionResourceExhausted {
        operation: "inject rebuild pressure",
        requested_bytes: usize::MAX,
    });

    assert!(fixture.is_active());
    assert!(!fixture.epoch_is_current());
}

#[test]
fn stopping_the_source_retires_the_fixture_epoch() {
    let (mut source, fixture) = WindowsScreenCaptureInput::new_deterministic_fixture(
        CaptureConfig::default(),
        fixture_epoch(),
    )
    .expect("deterministic Windows source is valid");

    source.start().expect("deterministic source starts idle");
    source
        .set_screen_capture_demand(ScreenCaptureDemand::active())
        .expect("deterministic demand is admitted");
    assert!(fixture.epoch_is_current());

    source.stop();

    assert!(!fixture.is_active());
    assert!(!fixture.epoch_is_current());
}
