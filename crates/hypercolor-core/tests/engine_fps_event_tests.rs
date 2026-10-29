use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::engine::{FpsTier, RenderLoop, TierTransitionConfig};
use hypercolor_types::event::HypercolorEvent;

#[test]
fn manual_fps_changes_publish_once_even_after_a_ceiling_clamps_the_tier() {
    let bus = Arc::new(HypercolorBus::new());
    let mut events = bus.subscribe_all();
    let mut render = RenderLoop::new(60).with_event_bus(bus);
    assert_eq!(render.fps_revision(), 0);
    assert!(
        events.try_recv().is_err(),
        "hello supplies the initial target"
    );

    render.set_tier(FpsTier::High);
    assert_eq!(render.fps_revision(), 1);
    assert!(matches!(
        events.try_recv().expect("manual transition").event,
        HypercolorEvent::FpsChanged {
            old_target: 60,
            new_target: 45,
            ..
        }
    ));
    render.set_tier(FpsTier::High);
    assert_eq!(render.fps_revision(), 1);
    assert!(events.try_recv().is_err(), "an unchanged tier is silent");

    // Live configuration clamps the ceiling before applying its tier.
    render.fps_controller_mut().set_max_tier(FpsTier::Medium);
    render.set_tier(FpsTier::Medium);
    assert_eq!(render.fps_revision(), 2);
    assert!(matches!(
        events.try_recv().expect("ceiling transition").event,
        HypercolorEvent::FpsChanged {
            old_target: 45,
            new_target: 30,
            ..
        }
    ));
    assert!(events.try_recv().is_err());
}

#[test]
fn adaptive_downshifts_publish_the_new_target_after_frame_completion() {
    let bus = Arc::new(HypercolorBus::new());
    let mut events = bus.subscribe_all();
    let mut render = RenderLoop::with_config(
        FpsTier::Full,
        TierTransitionConfig {
            downshift_miss_threshold: 1,
            ..TierTransitionConfig::default()
        },
    )
    .with_event_bus(bus);
    render.start();
    assert!(render.tick(), "running loop has a frame");
    std::thread::sleep(Duration::from_millis(25));
    render.frame_complete().expect("completed frame");
    assert_eq!(render.stats().tier, FpsTier::High);
    assert_eq!(render.fps_revision(), 1);
    assert!(matches!(
        events.try_recv().expect("adaptive transition").event,
        HypercolorEvent::FpsChanged {
            old_target: 60,
            new_target: 45,
            ..
        }
    ));
    assert!(events.try_recv().is_err());
}
