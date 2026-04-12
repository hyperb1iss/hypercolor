use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hypercolor_core::overlay::{
    ClockRenderer, OverlayBuffer, OverlayInput, OverlayRenderer, OverlaySize, SensorRenderer,
};
use hypercolor_types::overlay::{
    ClockConfig, ClockStyle, HourFormat, SensorDisplayStyle, SensorOverlayConfig,
};
use hypercolor_types::sensor::SystemSnapshot;

fn overlay_input(
    now: SystemTime,
    display_width: u32,
    display_height: u32,
    circular: bool,
    sensors: &SystemSnapshot,
) -> OverlayInput<'_> {
    OverlayInput {
        now,
        display_width,
        display_height,
        circular,
        sensors,
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

fn render_clock_template(
    template: &str,
    style: ClockStyle,
    size: OverlaySize,
    circular: bool,
) -> OverlayBuffer {
    let mut renderer = ClockRenderer::new(ClockConfig {
        style,
        hour_format: HourFormat::TwentyFour,
        show_seconds: true,
        show_date: true,
        date_format: Some("%Y-%m-%d".to_owned()),
        font_family: None,
        color: "#00000000".to_owned(),
        secondary_color: Some("#00000000".to_owned()),
        template: Some(template.to_owned()),
    })
    .expect("clock renderer should build");
    renderer.init(size).expect("clock renderer should init");

    let sensors = SystemSnapshot::empty();
    let mut buffer = OverlayBuffer::new(size);
    renderer
        .render_into(
            &overlay_input(
                UNIX_EPOCH + Duration::from_secs(9 * 3_600 + 41 * 60 + 18),
                size.width,
                size.height,
                circular,
                &sensors,
            ),
            &mut buffer,
        )
        .expect("clock render should succeed");
    buffer
}

fn render_sensor_template(
    template: &str,
    style: SensorDisplayStyle,
    size: OverlaySize,
    circular: bool,
) -> OverlayBuffer {
    let mut renderer = SensorRenderer::new(SensorOverlayConfig {
        sensor: "cpu_temp".to_owned(),
        style,
        unit_label: None,
        range_min: 20.0,
        range_max: 100.0,
        color_min: "#00000000".to_owned(),
        color_max: "#00000000".to_owned(),
        font_family: None,
        template: Some(template.to_owned()),
    })
    .expect("sensor renderer should build");
    renderer.init(size).expect("sensor renderer should init");

    let sensors = SystemSnapshot {
        cpu_temp_celsius: Some(72.0),
        ..SystemSnapshot::empty()
    };
    let mut buffer = OverlayBuffer::new(size);
    renderer
        .render_into(
            &overlay_input(
                SystemTime::UNIX_EPOCH,
                size.width,
                size.height,
                circular,
                &sensors,
            ),
            &mut buffer,
        )
        .expect("sensor render should succeed");
    buffer
}

#[test]
fn bundled_clock_templates_render_visible_pixels() {
    for (template, style, size, circular) in [
        (
            "clocks/digital-default.svg",
            ClockStyle::Digital,
            OverlaySize::new(240, 120),
            false,
        ),
        (
            "clocks/analog-classic.svg",
            ClockStyle::Analog,
            OverlaySize::new(240, 240),
            true,
        ),
        (
            "clocks/analog-minimal.svg",
            ClockStyle::Analog,
            OverlaySize::new(240, 240),
            true,
        ),
    ] {
        let buffer = render_clock_template(template, style, size, circular);
        assert!(
            alpha_sum(&buffer) > 0,
            "bundled clock template {template} should render visible pixels"
        );
    }
}

#[test]
fn bundled_sensor_templates_render_visible_pixels() {
    for (template, style, size, circular) in [
        (
            "gauges/radial-default.svg",
            SensorDisplayStyle::Gauge,
            OverlaySize::new(240, 240),
            true,
        ),
        (
            "gauges/radial-thin.svg",
            SensorDisplayStyle::Gauge,
            OverlaySize::new(240, 240),
            true,
        ),
        (
            "gauges/bar-horizontal.svg",
            SensorDisplayStyle::Bar,
            OverlaySize::new(240, 96),
            false,
        ),
    ] {
        let buffer = render_sensor_template(template, style, size, circular);
        assert!(
            alpha_sum(&buffer) > 0,
            "bundled sensor template {template} should render visible pixels"
        );
    }
}

#[test]
fn bundled_frame_templates_render_visible_pixels() {
    for (template, size, circular) in [
        ("frames/circle-border.svg", OverlaySize::new(240, 240), true),
        ("frames/rounded-rect.svg", OverlaySize::new(240, 120), false),
    ] {
        let buffer = render_clock_template(template, ClockStyle::Digital, size, circular);
        assert!(
            alpha_sum(&buffer) > 0,
            "bundled frame template {template} should render visible pixels"
        );
    }
}
