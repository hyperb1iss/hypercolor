use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hypercolor_core::overlay::{
    ClockRenderer, OverlayBuffer, OverlayInput, OverlayRenderer, OverlaySize,
};
use hypercolor_types::overlay::{ClockConfig, ClockStyle, HourFormat};
use hypercolor_types::sensor::SystemSnapshot;

fn overlay_input(now: SystemTime) -> OverlayInput<'static> {
    OverlayInput {
        now,
        display_width: 240,
        display_height: 120,
        circular: false,
        sensors: Box::leak(Box::new(SystemSnapshot::empty())),
        elapsed_secs: 0.0,
        frame_number: 1,
    }
}

fn alpha_sum(buffer: &OverlayBuffer) -> u64 {
    buffer
        .pixels
        .chunks_exact(4)
        .map(|pixel| u64::from(pixel[3]))
        .sum()
}

#[test]
fn digital_clock_renderer_draws_visible_pixels_and_tracks_seconds() {
    let mut renderer = ClockRenderer::new(ClockConfig {
        style: ClockStyle::Digital,
        hour_format: HourFormat::TwentyFour,
        show_seconds: true,
        show_date: true,
        date_format: Some("%Y-%m-%d".to_owned()),
        font_family: None,
        color: "#ffffff".to_owned(),
        secondary_color: Some("#80ffea".to_owned()),
        template: None,
    })
    .expect("renderer should build");
    let size = OverlaySize::new(240, 120);
    renderer.init(size).expect("renderer should init");
    let mut buffer = OverlayBuffer::new(size);
    let now = UNIX_EPOCH + Duration::from_secs(13 * 3_600 + 5 * 60 + 42);

    renderer
        .render_into(&overlay_input(now), &mut buffer)
        .expect("render should succeed");

    assert!(
        alpha_sum(&buffer) > 0,
        "digital clock should draw non-transparent pixels"
    );
    assert!(
        !renderer.content_changed(&overlay_input(now)),
        "same second should stay cached after the first render"
    );
    assert!(
        renderer.content_changed(&overlay_input(now + Duration::from_secs(1))),
        "next second should request a rerender"
    );
}

#[test]
fn digital_clock_renderer_changes_output_for_twelve_hour_mode() {
    let size = OverlaySize::new(240, 120);
    let now = UNIX_EPOCH + Duration::from_secs(13 * 3_600 + 5 * 60);
    let mut twenty_four = ClockRenderer::new(ClockConfig {
        style: ClockStyle::Digital,
        hour_format: HourFormat::TwentyFour,
        show_seconds: false,
        show_date: false,
        date_format: None,
        font_family: None,
        color: "#ffffff".to_owned(),
        secondary_color: None,
        template: None,
    })
    .expect("24h renderer should build");
    let mut twelve = ClockRenderer::new(ClockConfig {
        style: ClockStyle::Digital,
        hour_format: HourFormat::Twelve,
        show_seconds: false,
        show_date: false,
        date_format: None,
        font_family: None,
        color: "#ffffff".to_owned(),
        secondary_color: None,
        template: None,
    })
    .expect("12h renderer should build");
    twenty_four.init(size).expect("24h renderer should init");
    twelve.init(size).expect("12h renderer should init");
    let mut twenty_four_buffer = OverlayBuffer::new(size);
    let mut twelve_buffer = OverlayBuffer::new(size);

    twenty_four
        .render_into(&overlay_input(now), &mut twenty_four_buffer)
        .expect("24h render should succeed");
    twelve
        .render_into(&overlay_input(now), &mut twelve_buffer)
        .expect("12h render should succeed");

    assert_ne!(
        twenty_four_buffer.pixels, twelve_buffer.pixels,
        "12h and 24h clocks should not rasterize identically"
    );
}

#[test]
fn analog_clock_renderer_advances_half_second_second_hand() {
    let mut renderer = ClockRenderer::new(ClockConfig {
        style: ClockStyle::Analog,
        hour_format: HourFormat::TwentyFour,
        show_seconds: true,
        show_date: false,
        date_format: None,
        font_family: None,
        color: "#ffffff".to_owned(),
        secondary_color: Some("#ff6ac1".to_owned()),
        template: None,
    })
    .expect("renderer should build");
    let size = OverlaySize::new(160, 160);
    renderer.init(size).expect("renderer should init");
    let base = UNIX_EPOCH + Duration::from_secs(8 * 3_600 + 15 * 60 + 12);
    let later = base + Duration::from_millis(500);
    let mut first = OverlayBuffer::new(size);
    let mut second = OverlayBuffer::new(size);

    renderer
        .render_into(&overlay_input(base), &mut first)
        .expect("first render should succeed");
    assert!(
        renderer.content_changed(&overlay_input(later)),
        "analog seconds hand should request a half-second refresh"
    );
    renderer
        .render_into(&overlay_input(later), &mut second)
        .expect("second render should succeed");

    assert_ne!(
        first.pixels, second.pixels,
        "half-second analog refresh should move the second hand"
    );
}

#[test]
fn clock_renderer_renders_svg_template_background() {
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let template_path = temp_dir.path().join("clock-template.svg");
    std::fs::write(
        &template_path,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100">
<circle cx="50" cy="50" r="42" fill="#80ffea" fill-opacity="0.65" />
</svg>"##,
    )
    .expect("template should be written");

    let mut renderer = ClockRenderer::new(ClockConfig {
        style: ClockStyle::Analog,
        hour_format: HourFormat::TwentyFour,
        show_seconds: false,
        show_date: false,
        date_format: None,
        font_family: None,
        color: "#00000000".to_owned(),
        secondary_color: Some("#00000000".to_owned()),
        template: Some(template_path.to_string_lossy().into_owned()),
    })
    .expect("renderer should build");
    let size = OverlaySize::new(160, 160);
    renderer.init(size).expect("renderer should init");
    let mut buffer = OverlayBuffer::new(size);

    renderer
        .render_into(&overlay_input(UNIX_EPOCH), &mut buffer)
        .expect("render should succeed");

    assert!(
        alpha_sum(&buffer) > 0,
        "svg template should contribute visible pixels"
    );
}

#[test]
fn clock_renderer_resolves_bundled_svg_template() {
    let mut renderer = ClockRenderer::new(ClockConfig {
        style: ClockStyle::Digital,
        hour_format: HourFormat::TwentyFour,
        show_seconds: false,
        show_date: false,
        date_format: None,
        font_family: None,
        color: "#00000000".to_owned(),
        secondary_color: Some("#00000000".to_owned()),
        template: Some("clocks/digital-default.svg".to_owned()),
    })
    .expect("renderer should resolve bundled template");
    let size = OverlaySize::new(240, 120);
    renderer.init(size).expect("renderer should init");
    let mut buffer = OverlayBuffer::new(size);

    renderer
        .render_into(&overlay_input(UNIX_EPOCH), &mut buffer)
        .expect("render should succeed");

    assert!(
        alpha_sum(&buffer) > 0,
        "bundled svg template should contribute visible pixels"
    );
}
