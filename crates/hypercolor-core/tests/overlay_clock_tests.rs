use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hypercolor_core::overlay::{
    ClockRenderer, OverlayBuffer, OverlayInput, OverlayRenderer, OverlaySize,
};
use hypercolor_types::overlay::{ClockConfig, ClockStyle, HourFormat};
use hypercolor_types::sensor::SystemSnapshot;

fn overlay_input(
    now: SystemTime,
    display_width: u32,
    display_height: u32,
    circular: bool,
) -> OverlayInput<'static> {
    OverlayInput {
        now,
        display_width,
        display_height,
        circular,
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

fn alpha_tile_signature(buffer: &OverlayBuffer, columns: usize, rows: usize) -> Vec<u16> {
    let mut signature = Vec::with_capacity(columns * rows);
    let width = buffer.width as usize;
    let height = buffer.height as usize;

    for row in 0..rows {
        let y_start = row * height / rows;
        let y_end = ((row + 1) * height / rows).max(y_start + 1);
        for column in 0..columns {
            let x_start = column * width / columns;
            let x_end = ((column + 1) * width / columns).max(x_start + 1);
            let mut alpha_total = 0_u64;
            let mut pixel_count = 0_u64;

            for y in y_start..y_end {
                for x in x_start..x_end {
                    let offset = (y * width + x) * 4;
                    alpha_total += u64::from(buffer.pixels[offset + 3]);
                    pixel_count += 1;
                }
            }

            signature.push((alpha_total / pixel_count.max(1)) as u16);
        }
    }

    signature
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
        .render_into(&overlay_input(now, 240, 120, false), &mut buffer)
        .expect("render should succeed");

    assert!(
        alpha_sum(&buffer) > 0,
        "digital clock should draw non-transparent pixels"
    );
    assert!(
        !renderer.content_changed(&overlay_input(now, 240, 120, false)),
        "same second should stay cached after the first render"
    );
    assert!(
        renderer.content_changed(&overlay_input(now + Duration::from_secs(1), 240, 120, false)),
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
        .render_into(&overlay_input(now, 240, 120, false), &mut twenty_four_buffer)
        .expect("24h render should succeed");
    twelve
        .render_into(&overlay_input(now, 240, 120, false), &mut twelve_buffer)
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
        .render_into(&overlay_input(base, 160, 160, true), &mut first)
        .expect("first render should succeed");
    assert!(
        renderer.content_changed(&overlay_input(later, 160, 160, true)),
        "analog seconds hand should request a half-second refresh"
    );
    renderer
        .render_into(&overlay_input(later, 160, 160, true), &mut second)
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
        .render_into(&overlay_input(UNIX_EPOCH, 160, 160, true), &mut buffer)
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
        .render_into(&overlay_input(UNIX_EPOCH, 240, 120, false), &mut buffer)
        .expect("render should succeed");

    assert!(
        alpha_sum(&buffer) > 0,
        "bundled svg template should contribute visible pixels"
    );
}

#[test]
fn analog_clock_visual_signatures_match_reference_tiles() {
    let now = UNIX_EPOCH + Duration::from_secs(10 * 3_600 + 8 * 60 + 24);

    for (size, expected) in [
        (
            OverlaySize::new(480, 480),
            vec![
                0, 1, 27, 27, 1, 0, 1, 54, 52, 52, 75, 1, 27, 52, 57, 136, 86, 27, 27, 52, 63,
                69, 52, 27, 1, 54, 52, 59, 62, 1, 0, 1, 27, 27, 1, 0,
            ],
        ),
        (
            OverlaySize::new(240, 240),
            vec![
                0, 1, 27, 27, 1, 0, 1, 54, 52, 52, 75, 1, 27, 52, 57, 136, 86, 27, 27, 52, 63,
                69, 52, 27, 1, 54, 52, 59, 62, 1, 0, 1, 27, 27, 1, 0,
            ],
        ),
        (
            OverlaySize::new(120, 120),
            vec![
                0, 1, 27, 27, 1, 0, 1, 55, 52, 52, 76, 1, 27, 52, 57, 136, 86, 27, 27, 52, 63,
                69, 52, 27, 1, 55, 52, 59, 63, 1, 0, 1, 27, 27, 1, 0,
            ],
        ),
    ] {
        let mut renderer = ClockRenderer::new(ClockConfig {
            style: ClockStyle::Analog,
            hour_format: HourFormat::TwentyFour,
            show_seconds: true,
            show_date: false,
            date_format: None,
            font_family: None,
            color: "#ffffff".to_owned(),
            secondary_color: Some("#ff6ac1".to_owned()),
            template: Some("clocks/analog-minimal.svg".to_owned()),
        })
        .expect("renderer should build");
        renderer.init(size).expect("renderer should init");
        let mut buffer = OverlayBuffer::new(size);

        renderer
            .render_into(&overlay_input(now, size.width, size.height, true), &mut buffer)
            .expect("render should succeed");

        let signature = alpha_tile_signature(&buffer, 6, 6);
        assert_eq!(
            signature, expected,
            "analog clock tile signature should stay stable at {}x{}",
            size.width, size.height
        );
    }
}
