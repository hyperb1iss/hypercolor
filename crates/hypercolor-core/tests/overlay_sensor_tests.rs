use std::time::SystemTime;

use hypercolor_core::overlay::{
    OverlayBuffer, OverlayInput, OverlayRenderer, OverlaySize, SensorRenderer,
};
use hypercolor_types::overlay::{SensorDisplayStyle, SensorOverlayConfig};
use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};

fn overlay_input(sensors: &SystemSnapshot) -> OverlayInput<'_> {
    OverlayInput {
        now: SystemTime::UNIX_EPOCH,
        display_width: 240,
        display_height: 160,
        circular: false,
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

#[test]
fn numeric_sensor_renderer_draws_visible_pixels() {
    let mut renderer = SensorRenderer::new(SensorOverlayConfig {
        sensor: "cpu_temp".to_owned(),
        style: SensorDisplayStyle::Numeric,
        unit_label: None,
        range_min: 20.0,
        range_max: 100.0,
        color_min: "#80ffea".to_owned(),
        color_max: "#ff6ac1".to_owned(),
        font_family: None,
        template: None,
    })
    .expect("renderer should build");
    let size = OverlaySize::new(240, 120);
    renderer.init(size).expect("renderer should init");
    let sensors = SystemSnapshot {
        cpu_temp_celsius: Some(72.0),
        ..SystemSnapshot::empty()
    };
    let mut buffer = OverlayBuffer::new(size);

    renderer
        .render_into(&overlay_input(&sensors), &mut buffer)
        .expect("render should succeed");

    assert!(
        alpha_sum(&buffer) > 0,
        "numeric sensor overlay should draw non-transparent pixels"
    );
    assert!(
        !renderer.content_changed(&overlay_input(&sensors)),
        "sensor overlays should rely on the fixed daemon cadence"
    );
}

#[test]
fn sensor_renderer_changes_output_when_sensor_value_changes() {
    let mut renderer = SensorRenderer::new(SensorOverlayConfig {
        sensor: "cpu_temp".to_owned(),
        style: SensorDisplayStyle::Gauge,
        unit_label: None,
        range_min: 20.0,
        range_max: 100.0,
        color_min: "#80ffea".to_owned(),
        color_max: "#ff6ac1".to_owned(),
        font_family: None,
        template: None,
    })
    .expect("renderer should build");
    let size = OverlaySize::new(180, 180);
    renderer.init(size).expect("renderer should init");
    let cool = SystemSnapshot {
        cpu_temp_celsius: Some(42.0),
        ..SystemSnapshot::empty()
    };
    let hot = SystemSnapshot {
        cpu_temp_celsius: Some(88.0),
        ..SystemSnapshot::empty()
    };
    let mut cool_buffer = OverlayBuffer::new(size);
    let mut hot_buffer = OverlayBuffer::new(size);

    renderer
        .render_into(&overlay_input(&cool), &mut cool_buffer)
        .expect("cool render should succeed");
    renderer
        .render_into(&overlay_input(&hot), &mut hot_buffer)
        .expect("hot render should succeed");

    assert_ne!(
        cool_buffer.pixels, hot_buffer.pixels,
        "sensor value changes should alter the rendered gauge"
    );
}

#[test]
fn bar_sensor_renderer_extends_fill_for_higher_values() {
    let mut renderer = SensorRenderer::new(SensorOverlayConfig {
        sensor: "gpu_load".to_owned(),
        style: SensorDisplayStyle::Bar,
        unit_label: None,
        range_min: 0.0,
        range_max: 100.0,
        color_min: "#80ffea".to_owned(),
        color_max: "#ff6ac1".to_owned(),
        font_family: None,
        template: None,
    })
    .expect("renderer should build");
    let size = OverlaySize::new(240, 96);
    renderer.init(size).expect("renderer should init");
    let low = SystemSnapshot {
        gpu_load_percent: Some(18.0),
        ..SystemSnapshot::empty()
    };
    let high = SystemSnapshot {
        gpu_load_percent: Some(82.0),
        ..SystemSnapshot::empty()
    };
    let mut low_buffer = OverlayBuffer::new(size);
    let mut high_buffer = OverlayBuffer::new(size);

    renderer
        .render_into(&overlay_input(&low), &mut low_buffer)
        .expect("low render should succeed");
    renderer
        .render_into(&overlay_input(&high), &mut high_buffer)
        .expect("high render should succeed");

    assert_ne!(
        low_buffer.pixels, high_buffer.pixels,
        "higher sensor values should expand the filled bar"
    );
}

#[test]
fn sensor_renderer_renders_svg_template_background() {
    let temp_dir = tempfile::tempdir().expect("tempdir should be created");
    let template_path = temp_dir.path().join("sensor-template.svg");
    std::fs::write(
        &template_path,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="120" viewBox="0 0 120 120">
<rect x="12" y="12" width="96" height="96" rx="12" fill="#80ffea" fill-opacity="0.55" />
</svg>"##,
    )
    .expect("template should be written");

    let mut renderer = SensorRenderer::new(SensorOverlayConfig {
        sensor: "chipset_temp".to_owned(),
        style: SensorDisplayStyle::Minimal,
        unit_label: None,
        range_min: 0.0,
        range_max: 100.0,
        color_min: "#ffffff".to_owned(),
        color_max: "#ffffff".to_owned(),
        font_family: None,
        template: Some(template_path.to_string_lossy().into_owned()),
    })
    .expect("renderer should build");
    let size = OverlaySize::new(160, 160);
    renderer.init(size).expect("renderer should init");
    let sensors = SystemSnapshot {
        components: vec![SensorReading::new(
            "chipset_temp",
            56.0,
            SensorUnit::Celsius,
            Some(0.0),
            Some(100.0),
            None,
        )],
        ..SystemSnapshot::empty()
    };
    let mut buffer = OverlayBuffer::new(size);

    renderer
        .render_into(&overlay_input(&sensors), &mut buffer)
        .expect("render should succeed");

    assert!(
        alpha_sum(&buffer) > 0,
        "svg template should contribute visible pixels"
    );
}
