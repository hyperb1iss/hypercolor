#![cfg(feature = "servo")]

use std::path::PathBuf;
use std::sync::LazyLock;
use std::thread;
use std::time::Duration;

use hypercolor_core::effect::builtin::WebViewportRenderer;
use hypercolor_core::effect::{EffectRenderer, FrameInput};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::effect::{
    ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource,
};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::viewport::ViewportRect;
use reqwest::Url;
use tempfile::tempdir;
use uuid::Uuid;

const FRAME_DT_SECONDS: f32 = 1.0 / 30.0;
const OUTPUT_WIDTH: u32 = 32;
const OUTPUT_HEIGHT: u32 = 18;
const PREVIEW_WIDTH: u32 = 640;
const PREVIEW_HEIGHT: u32 = 360;
static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
static INTERACTION: LazyLock<InteractionData> = LazyLock::new(InteractionData::default);
static SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);

fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "Web Viewport".into(),
        author: "hypercolor-tests".into(),
        version: "0.1.0".into(),
        description: "web viewport integration test".into(),
        category: EffectCategory::Source,
        tags: vec!["servo".into(), "web".into(), "viewport".into()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/web_viewport"),
        },
        license: None,
    }
}

fn frame(frame_number: u64) -> FrameInput<'static> {
    FrameInput {
        time_secs: frame_number as f32 * FRAME_DT_SECONDS,
        delta_secs: FRAME_DT_SECONDS,
        frame_number,
        audio: &SILENCE,
        interaction: &INTERACTION,
        screen: None,
        sensors: &SENSORS,
        canvas_width: OUTPUT_WIDTH,
        canvas_height: OUTPUT_HEIGHT,
    }
}

struct FixturePage {
    _temp: tempfile::TempDir,
    url: String,
}

fn write_two_panel_fixture() -> FixturePage {
    let temp = tempdir().expect("tempdir should create");
    let path = temp.path().join("two-panel.html");
    std::fs::write(
        &path,
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <style>
    html, body {
      margin: 0;
      width: 100%;
      height: 100%;
      overflow: hidden;
      background: #000;
    }
    .half {
      position: absolute;
      top: 0;
      bottom: 0;
      width: 50%;
    }
    .left { left: 0; background: rgb(255, 0, 0); }
    .right { right: 0; background: rgb(0, 0, 255); }
  </style>
</head>
<body>
  <div class="half left"></div>
  <div class="half right"></div>
</body>
</html>"#,
    )
    .expect("fixture write should succeed");

    let url = Url::from_file_path(&path).expect("fixture path should convert to file URL");
    FixturePage {
        _temp: temp,
        url: url.to_string(),
    }
}

fn render_until(
    renderer: &mut WebViewportRenderer,
    predicate: impl Fn(&Canvas, Option<&Canvas>) -> bool,
) -> (Canvas, Option<Canvas>) {
    let mut last_preview = None::<Canvas>;

    for frame_number in 0..60 {
        let canvas = renderer
            .tick(&frame(frame_number))
            .expect("tick should succeed");
        let preview = renderer.preview_canvas();
        if predicate(&canvas, preview.as_ref()) {
            return (canvas, preview);
        }
        last_preview = preview;
        thread::sleep(Duration::from_millis(16));
    }

    panic!(
        "web viewport never reached the expected render state; last preview dimensions = {:?}",
        last_preview
            .as_ref()
            .map(|canvas| (canvas.width(), canvas.height()))
    );
}

fn pixel_is_red_dominant(pixel: Rgba) -> bool {
    pixel.r >= 180 && pixel.g <= 80 && pixel.b <= 80
}

fn pixel_is_blue_dominant(pixel: Rgba) -> bool {
    pixel.b >= 180 && pixel.r <= 80 && pixel.g <= 80
}

#[test]
#[ignore = "requires full Servo runtime and local file rendering"]
fn web_viewport_renders_local_fixture_and_exposes_preview_canvas() {
    let fixture = write_two_panel_fixture();
    let mut renderer = WebViewportRenderer::new();
    renderer.set_control("url", &ControlValue::Text(fixture.url));
    renderer.set_control("fit_mode", &ControlValue::Enum("Stretch".into()));
    renderer.set_control("render_width", &ControlValue::Float(PREVIEW_WIDTH as f32));
    renderer.set_control("render_height", &ControlValue::Float(PREVIEW_HEIGHT as f32));
    renderer
        .init(&metadata())
        .expect("web viewport init should succeed");

    let (canvas, preview) = render_until(&mut renderer, |canvas, preview| {
        preview.is_some()
            && pixel_is_red_dominant(canvas.get_pixel(0, OUTPUT_HEIGHT / 2))
            && pixel_is_blue_dominant(canvas.get_pixel(OUTPUT_WIDTH - 1, OUTPUT_HEIGHT / 2))
    });

    let preview = preview.expect("preview canvas should be published");
    assert_eq!(preview.width(), PREVIEW_WIDTH);
    assert_eq!(preview.height(), PREVIEW_HEIGHT);
    assert!(pixel_is_red_dominant(
        canvas.get_pixel(0, OUTPUT_HEIGHT / 2)
    ));
    assert!(pixel_is_blue_dominant(
        canvas.get_pixel(OUTPUT_WIDTH - 1, OUTPUT_HEIGHT / 2)
    ));

    renderer.destroy();
}

#[test]
#[ignore = "requires full Servo runtime and local file rendering"]
fn web_viewport_viewport_control_crops_the_requested_region() {
    let fixture = write_two_panel_fixture();
    let mut renderer = WebViewportRenderer::new();
    renderer.set_control("url", &ControlValue::Text(fixture.url));
    renderer.set_control("fit_mode", &ControlValue::Enum("Stretch".into()));
    renderer.set_control("render_width", &ControlValue::Float(PREVIEW_WIDTH as f32));
    renderer.set_control("render_height", &ControlValue::Float(PREVIEW_HEIGHT as f32));
    renderer.set_control(
        "viewport",
        &ControlValue::Rect(ViewportRect::new(0.55, 0.0, 0.45, 1.0)),
    );
    renderer
        .init(&metadata())
        .expect("web viewport init should succeed");

    let (canvas, _) = render_until(&mut renderer, |canvas, _| {
        pixel_is_blue_dominant(canvas.get_pixel(0, OUTPUT_HEIGHT / 2))
            && pixel_is_blue_dominant(canvas.get_pixel(OUTPUT_WIDTH - 1, OUTPUT_HEIGHT / 2))
    });

    assert!(pixel_is_blue_dominant(
        canvas.get_pixel(0, OUTPUT_HEIGHT / 2)
    ));
    assert!(pixel_is_blue_dominant(
        canvas.get_pixel(OUTPUT_WIDTH - 1, OUTPUT_HEIGHT / 2)
    ));

    renderer.destroy();
}
