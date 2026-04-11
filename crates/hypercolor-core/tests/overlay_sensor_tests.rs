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

fn clear_normalized_rect(
    buffer: &mut OverlayBuffer,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
) {
    let x_start = ((buffer.width as f32) * left).floor().max(0.0) as usize;
    let y_start = ((buffer.height as f32) * top).floor().max(0.0) as usize;
    let x_end = ((buffer.width as f32) * (left + width))
        .ceil()
        .min(buffer.width as f32) as usize;
    let y_end = ((buffer.height as f32) * (top + height))
        .ceil()
        .min(buffer.height as f32) as usize;
    let stride = buffer.width as usize * 4;

    for y in y_start..y_end {
        for x in x_start..x_end {
            let offset = y * stride + x * 4;
            buffer.pixels[offset..offset + 4].fill(0);
        }
    }
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

#[test]
fn sensor_renderer_resolves_bundled_svg_template() {
    let mut renderer = SensorRenderer::new(SensorOverlayConfig {
        sensor: "cpu_temp".to_owned(),
        style: SensorDisplayStyle::Minimal,
        unit_label: None,
        range_min: 0.0,
        range_max: 100.0,
        color_min: "#00000000".to_owned(),
        color_max: "#00000000".to_owned(),
        font_family: None,
        template: Some("gauges/radial-default.svg".to_owned()),
    })
    .expect("renderer should resolve bundled template");
    let size = OverlaySize::new(160, 160);
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
        "bundled gauge template should contribute visible pixels"
    );
}

#[test]
fn sensor_gauge_visual_signatures_match_reference_tiles() {
    let size = OverlaySize::new(240, 240);
    let mut renderer = SensorRenderer::new(SensorOverlayConfig {
        sensor: "cpu_temp".to_owned(),
        style: SensorDisplayStyle::Gauge,
        unit_label: None,
        range_min: 20.0,
        range_max: 100.0,
        color_min: "#80ffea".to_owned(),
        color_max: "#ff6ac1".to_owned(),
        font_family: None,
        template: Some("gauges/radial-default.svg".to_owned()),
    })
    .expect("renderer should build");
    renderer.init(size).expect("renderer should init");

    for (value, expected) in [
        (
            32.0,
            vec![
                0, 0, 11, 11, 0, 0, 0, 44, 69, 69, 44, 0, 12, 16, 1, 1, 16, 12, 37, 35, 0, 0,
                17, 13, 5, 86, 28, 28, 17, 0, 0, 0, 11, 11, 0, 0,
            ],
        ),
        (
            68.0,
            vec![
                0, 0, 11, 11, 0, 0, 0, 86, 135, 134, 48, 0, 25, 45, 1, 1, 16, 12, 52, 45, 0, 0,
                17, 13, 5, 86, 28, 28, 17, 0, 0, 0, 11, 11, 0, 0,
            ],
        ),
        (
            92.0,
            vec![
                0, 0, 11, 11, 0, 0, 0, 86, 135, 135, 86, 0, 25, 45, 1, 1, 45, 25, 52, 45, 0, 0,
                45, 52, 5, 86, 28, 28, 23, 3, 0, 0, 11, 11, 0, 0,
            ],
        ),
    ] {
        let sensors = SystemSnapshot {
            cpu_temp_celsius: Some(value),
            ..SystemSnapshot::empty()
        };
        let mut buffer = OverlayBuffer::new(size);
        renderer
            .render_into(&overlay_input(&sensors), &mut buffer)
            .expect("render should succeed");
        clear_normalized_rect(&mut buffer, 0.2, 0.34, 0.6, 0.4);

        let signature = alpha_tile_signature(&buffer, 6, 6);
        assert_eq!(
            signature, expected,
            "sensor gauge tile signature should stay stable for value {value}"
        );
    }
}

#[test]
fn sensor_bar_visual_signatures_track_fill_extent() {
    let size = OverlaySize::new(240, 96);
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
    renderer.init(size).expect("renderer should init");

    for (value, expected) in [
        (18.0, vec![0, 0, 0, 0, 0, 0, 74, 39, 25, 25, 25, 20]),
        (82.0, vec![0, 0, 0, 0, 0, 0, 74, 92, 92, 92, 78, 20]),
    ] {
        let sensors = SystemSnapshot {
            gpu_load_percent: Some(value),
            ..SystemSnapshot::empty()
        };
        let mut buffer = OverlayBuffer::new(size);
        renderer
            .render_into(&overlay_input(&sensors), &mut buffer)
            .expect("render should succeed");
        clear_normalized_rect(&mut buffer, 0.0, 0.0, 1.0, 0.52);

        let signature = alpha_tile_signature(&buffer, 6, 2);
        assert_eq!(
            signature, expected,
            "sensor bar tile signature should stay stable for value {value}"
        );
    }
}
