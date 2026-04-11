use std::time::SystemTime;

use hypercolor_core::overlay::{
    OverlayBuffer, OverlayInput, OverlayRenderer, OverlaySize, TextRenderer,
};
use hypercolor_types::overlay::{TextAlign, TextOverlayConfig};
use hypercolor_types::sensor::SystemSnapshot;

fn overlay_input(sensors: &SystemSnapshot, elapsed_secs: f32) -> OverlayInput<'_> {
    OverlayInput {
        now: SystemTime::UNIX_EPOCH,
        display_width: 240,
        display_height: 120,
        circular: false,
        sensors,
        elapsed_secs,
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
fn text_renderer_draws_visible_pixels() {
    let mut renderer = TextRenderer::new(TextOverlayConfig {
        text: "Hypercolor".to_owned(),
        font_family: None,
        font_size: 28.0,
        color: "#ffffff".to_owned(),
        align: TextAlign::Center,
        scroll: false,
        scroll_speed: 30.0,
    })
    .expect("renderer should build");
    let size = OverlaySize::new(240, 120);
    renderer.init(size).expect("renderer should init");
    let sensors = SystemSnapshot::empty();
    let mut buffer = OverlayBuffer::new(size);

    renderer
        .render_into(&overlay_input(&sensors, 0.0), &mut buffer)
        .expect("render should succeed");

    assert!(
        alpha_sum(&buffer) > 0,
        "text overlay should draw non-transparent pixels"
    );
    assert!(
        !renderer.content_changed(&overlay_input(&sensors, 0.0)),
        "static text should stay cached after the first render"
    );
}

#[test]
fn text_renderer_replaces_sensor_tokens() {
    let mut renderer = TextRenderer::new(TextOverlayConfig {
        text: "CPU {sensor:cpu_temp}".to_owned(),
        font_family: None,
        font_size: 24.0,
        color: "#ffffff".to_owned(),
        align: TextAlign::Center,
        scroll: false,
        scroll_speed: 30.0,
    })
    .expect("renderer should build");
    let size = OverlaySize::new(240, 120);
    renderer.init(size).expect("renderer should init");
    let mut cool_buffer = OverlayBuffer::new(size);
    let mut hot_buffer = OverlayBuffer::new(size);
    let cool = SystemSnapshot {
        cpu_temp_celsius: Some(72.0),
        ..SystemSnapshot::empty()
    };
    let hot = SystemSnapshot {
        cpu_temp_celsius: Some(88.0),
        ..SystemSnapshot::empty()
    };

    renderer
        .render_into(&overlay_input(&cool, 0.0), &mut cool_buffer)
        .expect("cool render should succeed");
    renderer
        .render_into(&overlay_input(&hot, 0.0), &mut hot_buffer)
        .expect("hot render should succeed");

    assert_ne!(
        cool_buffer.pixels, hot_buffer.pixels,
        "sensor interpolation should change the rendered glyphs"
    );
}

#[test]
fn scrolling_text_marks_content_dirty_when_offset_advances() {
    let mut renderer = TextRenderer::new(TextOverlayConfig {
        text: "this text is intentionally too wide for the viewport".to_owned(),
        font_family: None,
        font_size: 24.0,
        color: "#ffffff".to_owned(),
        align: TextAlign::Left,
        scroll: true,
        scroll_speed: 60.0,
    })
    .expect("renderer should build");
    let size = OverlaySize::new(120, 48);
    renderer.init(size).expect("renderer should init");
    let sensors = SystemSnapshot::empty();
    let mut first = OverlayBuffer::new(size);
    let mut later = OverlayBuffer::new(size);

    renderer
        .render_into(&overlay_input(&sensors, 0.0), &mut first)
        .expect("initial render should succeed");

    assert!(
        renderer.content_changed(&overlay_input(&sensors, 1.0)),
        "scrolling text should request a rerender once the marquee offset advances"
    );

    renderer
        .render_into(&overlay_input(&sensors, 1.0), &mut later)
        .expect("later render should succeed");

    assert_ne!(
        first.pixels, later.pixels,
        "scrolling text should move across frames"
    );
}
