#![cfg(feature = "servo")]

use dpi::PhysicalSize;
use hypercolor_core::effect::bootstrap_software_rendering_context;
use servo::RenderingContext;

#[test]
fn software_rendering_context_bootstraps_at_target_size() {
    let context = bootstrap_software_rendering_context(320, 200)
        .expect("Servo software rendering context should initialize");

    assert_eq!(context.size(), PhysicalSize::new(320, 200));
}
