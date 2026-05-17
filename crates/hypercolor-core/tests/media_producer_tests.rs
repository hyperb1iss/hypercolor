use std::path::Path;

use gif::{Encoder, Frame, Repeat};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::effect::builtin::register_builtin_effects;
use hypercolor_core::effect::media::MediaProducer;
use hypercolor_types::canvas::Rgba;
use hypercolor_types::effect::ControlType;
use hypercolor_types::layer::{LoopMode, MediaPlayback};
use image::{ImageFormat, RgbaImage};

fn animated_gif_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder =
            Encoder::new(&mut bytes, 1, 1, &[]).expect("test GIF encoder should initialize");
        encoder
            .set_repeat(Repeat::Infinite)
            .expect("test GIF repeat should set");
        write_gif_frame(&mut encoder, [255, 0, 0, 255]);
        write_gif_frame(&mut encoder, [0, 255, 0, 255]);
        write_gif_frame(&mut encoder, [0, 0, 255, 255]);
    }
    bytes
}

fn write_gif_frame(encoder: &mut Encoder<&mut Vec<u8>>, rgba: [u8; 4]) {
    let mut pixels = rgba.to_vec();
    let mut frame = Frame::from_rgba_speed(1, 1, &mut pixels, 10);
    frame.delay = 10;
    encoder
        .write_frame(&frame)
        .expect("test GIF frame should encode");
}

fn pixel_at(producer: &MediaProducer, playback: &MediaPlayback, elapsed_ms: u32) -> Rgba {
    producer
        .render_frame(playback, elapsed_ms, 1, 1)
        .get_pixel(0, 0)
}

#[test]
fn gif_loop_timing_is_deterministic() {
    let bytes = animated_gif_bytes();
    let producer = MediaProducer::from_bytes(&bytes, "image/gif").expect("test GIF should decode");
    let playback = MediaPlayback::default();

    assert_eq!(producer.frame_count(), 3);
    assert_eq!(producer.total_duration_us(), 300_000);
    assert_eq!(pixel_at(&producer, &playback, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(
        pixel_at(&producer, &playback, 100),
        Rgba::new(0, 255, 0, 255)
    );
    assert_eq!(
        pixel_at(&producer, &playback, 200),
        Rgba::new(0, 0, 255, 255)
    );
    assert_eq!(
        pixel_at(&producer, &playback, 300),
        Rgba::new(255, 0, 0, 255)
    );
}

#[test]
fn gif_ping_pong_timing_is_deterministic() {
    let bytes = animated_gif_bytes();
    let producer = MediaProducer::from_bytes(&bytes, "image/gif").expect("test GIF should decode");
    let playback = MediaPlayback {
        loop_mode: LoopMode::PingPong,
        ..MediaPlayback::default()
    };

    assert_eq!(pixel_at(&producer, &playback, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(
        pixel_at(&producer, &playback, 100),
        Rgba::new(0, 255, 0, 255)
    );
    assert_eq!(
        pixel_at(&producer, &playback, 200),
        Rgba::new(0, 0, 255, 255)
    );
    assert_eq!(
        pixel_at(&producer, &playback, 400),
        Rgba::new(0, 255, 0, 255)
    );
    assert_eq!(
        pixel_at(&producer, &playback, 500),
        Rgba::new(255, 0, 0, 255)
    );
    assert_eq!(
        pixel_at(&producer, &playback, 600),
        Rgba::new(255, 0, 0, 255)
    );
}

#[test]
fn png_sequence_directory_uses_lexical_frame_order() {
    let tempdir = tempfile::tempdir().expect("test tempdir should be created");
    write_png(tempdir.path().join("002.png").as_path(), [0, 0, 255, 255]);
    write_png(tempdir.path().join("001.png").as_path(), [255, 0, 0, 255]);

    let producer = MediaProducer::from_png_sequence_dir(tempdir.path(), 100_000)
        .expect("test PNG sequence should decode");
    let playback = MediaPlayback::default();

    assert_eq!(producer.frame_count(), 2);
    assert_eq!(pixel_at(&producer, &playback, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(
        pixel_at(&producer, &playback, 100),
        Rgba::new(0, 0, 255, 255)
    );
}

#[test]
fn media_player_registers_asset_control() {
    let mut registry = EffectRegistry::new(Vec::new());
    register_builtin_effects(&mut registry);

    let media_player = registry
        .iter()
        .find_map(|(_, entry)| {
            (entry.metadata.source.source_stem() == Some("media_player")).then_some(entry)
        })
        .expect("media player builtin should be registered");
    let asset = media_player
        .metadata
        .controls
        .iter()
        .find(|control| control.id == "asset")
        .expect("media player should declare an asset control");

    assert_eq!(asset.control_type, ControlType::Asset);
}

fn write_png(path: &Path, rgba: [u8; 4]) {
    let image = RgbaImage::from_pixel(1, 1, image::Rgba(rgba));
    image
        .save_with_format(path, ImageFormat::Png)
        .expect("test PNG should write");
}
