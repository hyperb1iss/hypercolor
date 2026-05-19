//! Security and resolution tests for the builtin `media_player` effect.

use std::io::Cursor;
use std::sync::Arc;

use hypercolor_core::asset::{AssetLibrary, AssetUploadOptions};
use hypercolor_core::effect::FrameInput;
use hypercolor_core::effect::builtin::create_builtin_renderer;
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas};
use hypercolor_types::effect::ControlValue;
use hypercolor_types::sensor::SystemSnapshot;
use image::{ImageBuffer, ImageFormat, Rgba};
use tempfile::TempDir;
use tokio::sync::RwLock;

const W: u32 = 16;
const H: u32 = 16;

fn png_bytes(color: [u8; 4]) -> Vec<u8> {
    let image = ImageBuffer::from_pixel(2, 2, Rgba(color));
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode test png");
    bytes.into_inner()
}

fn render_media_player(asset_value: &str, library: Arc<RwLock<AssetLibrary>>) -> Canvas {
    let mut renderer = create_builtin_renderer("media_player").expect("media_player renderer");
    renderer.bind_asset_library(library);
    renderer.set_control("asset", &ControlValue::Text(asset_value.to_owned()));

    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let input = FrameInput {
        time_secs: 0.0,
        delta_secs: 1.0 / 60.0,
        frame_number: 0,
        audio: &audio,
        interaction: &interaction,
        screen: None,
        sensors: &sensors,
        canvas_width: W,
        canvas_height: H,
    };
    let mut canvas = Canvas::new(W, H);
    renderer
        .render_into(&input, &mut canvas)
        .expect("media_player render");
    canvas
}

/// True when any pixel has a non-black color channel. A cleared canvas is
/// opaque black, so this distinguishes rendered media from "no producer".
fn canvas_has_content(canvas: &Canvas) -> bool {
    canvas
        .as_rgba_bytes()
        .chunks_exact(BYTES_PER_PIXEL)
        .any(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0)
}

#[test]
fn media_player_renders_a_library_asset_resolved_by_id() {
    let tempdir = TempDir::new().expect("tempdir");
    let mut library = AssetLibrary::open(tempdir.path().join("assets")).expect("open library");
    let upsert = library
        .add_bytes(
            &png_bytes([255, 0, 200, 255]),
            AssetUploadOptions::new("clip.png"),
        )
        .expect("add asset");
    let asset_id = upsert.record.id;
    let library = Arc::new(RwLock::new(library));

    let canvas = render_media_player(&asset_id.to_string(), library);
    assert!(
        canvas_has_content(&canvas),
        "an asset resolved by id through the library should render visible media"
    );
}

#[test]
fn media_player_rejects_filesystem_paths() {
    let tempdir = TempDir::new().expect("tempdir");
    let library = AssetLibrary::open(tempdir.path().join("assets")).expect("open library");
    let library = Arc::new(RwLock::new(library));

    // A real, valid PNG that exists on disk but was never added to the library
    // must not be loadable by passing its path as the asset control value.
    let stray_png = tempdir.path().join("stray.png");
    std::fs::write(&stray_png, png_bytes([0, 255, 0, 255])).expect("write stray png");

    let canvas = render_media_player(&stray_png.display().to_string(), library);
    assert!(
        !canvas_has_content(&canvas),
        "a filesystem path must not resolve to media even when the file is a valid image"
    );
}

#[test]
fn media_player_rejects_unknown_asset_ids() {
    let tempdir = TempDir::new().expect("tempdir");
    let library = AssetLibrary::open(tempdir.path().join("assets")).expect("open library");
    let library = Arc::new(RwLock::new(library));

    let canvas = render_media_player("00000000-0000-0000-0000-000000000000", library);
    assert!(
        !canvas_has_content(&canvas),
        "an asset id absent from the library must not resolve to media"
    );
}
