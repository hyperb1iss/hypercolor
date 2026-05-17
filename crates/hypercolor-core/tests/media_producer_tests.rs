use std::path::Path;
#[cfg(feature = "media-video")]
use std::process::Command;
#[cfg(feature = "media-video")]
use std::time::{Duration, Instant};

use gif::{Encoder, Frame, Repeat};
#[cfg(feature = "media-video")]
use hypercolor_core::asset::StreamUrlPolicy;
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::effect::builtin::register_builtin_effects;
use hypercolor_core::effect::media::MediaProducer;
use hypercolor_types::canvas::Rgba;
use hypercolor_types::effect::ControlType;
use hypercolor_types::layer::{LoopMode, MediaPlayback};
use image::codecs::webp::WebPEncoder;
use image::{ImageEncoder, ImageFormat, RgbaImage};

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

fn animated_webp_bytes() -> Vec<u8> {
    let frames = [
        encoded_still_webp([255, 0, 0, 255]),
        encoded_still_webp([0, 255, 0, 255]),
        encoded_still_webp([0, 0, 255, 255]),
    ];
    let mut body = Vec::new();
    push_chunk(
        &mut body,
        b"VP8X",
        &[
            0x12, 0, 0, 0, // animation + alpha
            0, 0, 0, // canvas width minus one
            0, 0, 0, // canvas height minus one
        ],
    );
    push_chunk(&mut body, b"ANIM", &[0, 0, 0, 0, 0, 0]);
    for frame in frames {
        let mut payload = Vec::new();
        push_u24_le(&mut payload, 0);
        push_u24_le(&mut payload, 0);
        push_u24_le(&mut payload, 0);
        push_u24_le(&mut payload, 0);
        push_u24_le(&mut payload, 100);
        payload.push(0);
        payload.extend_from_slice(webp_payload_chunks(&frame));
        push_chunk(&mut body, b"ANMF", &payload);
    }

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    let riff_size = u32::try_from(4 + body.len()).expect("test WebP should fit in u32");
    bytes.extend_from_slice(&riff_size.to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&body);
    bytes
}

#[cfg(feature = "media-lottie")]
fn empty_lottie_bytes() -> &'static [u8] {
    br#"{
        "v": "5.7.6",
        "fr": 30,
        "ip": 0,
        "op": 2,
        "w": 1,
        "h": 1,
        "nm": "hypercolor-test",
        "ddd": 0,
        "assets": [],
        "layers": []
    }"#
}

#[cfg(feature = "media-video")]
fn write_test_webm(path: &Path) -> bool {
    Command::new("gst-launch-1.0")
        .args([
            "-q",
            "videotestsrc",
            "num-buffers=2",
            "pattern=white",
            "!",
            "video/x-raw,width=16,height=16,framerate=1/1",
            "!",
            "videoconvert",
            "!",
            "vp8enc",
            "!",
            "webmmux",
            "!",
            "filesink",
            &format!("location={}", path.display()),
        ])
        .status()
        .is_ok_and(|status| status.success())
}

fn encoded_still_webp(rgba: [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::new();
    WebPEncoder::new_lossless(&mut bytes)
        .write_image(&rgba, 1, 1, image::ColorType::Rgba8.into())
        .expect("test WebP frame should encode");
    bytes
}

fn webp_payload_chunks(bytes: &[u8]) -> &[u8] {
    assert!(
        bytes.len() >= 12,
        "test WebP frame should include RIFF header"
    );
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WEBP");
    &bytes[12..]
}

fn push_chunk(bytes: &mut Vec<u8>, fourcc: &[u8; 4], payload: &[u8]) {
    bytes.extend_from_slice(fourcc);
    let payload_len = u32::try_from(payload.len()).expect("test chunk should fit in u32");
    bytes.extend_from_slice(&payload_len.to_le_bytes());
    bytes.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        bytes.push(0);
    }
}

fn push_u24_le(bytes: &mut Vec<u8>, value: u32) {
    bytes.push((value & 0xff) as u8);
    bytes.push(((value >> 8) & 0xff) as u8);
    bytes.push(((value >> 16) & 0xff) as u8);
}

fn pixel_at(producer: &MediaProducer, playback: &MediaPlayback, elapsed_ms: u32) -> Rgba {
    producer
        .render_frame(playback, elapsed_ms, 1, 1)
        .get_pixel(0, 0)
}

fn assert_pixel_near(actual: Rgba, expected: Rgba) {
    assert!(
        actual.r.abs_diff(expected.r) <= 1
            && actual.g.abs_diff(expected.g) <= 1
            && actual.b.abs_diff(expected.b) <= 1
            && actual.a == expected.a,
        "actual {actual:?} should be within one RGB step of {expected:?}",
    );
}

#[test]
fn animated_webp_loop_timing_is_deterministic() {
    let bytes = animated_webp_bytes();
    let producer =
        MediaProducer::from_bytes(&bytes, "image/webp").expect("test WebP should decode");
    let playback = MediaPlayback::default();

    assert_eq!(producer.frame_count(), 3);
    assert_eq!(producer.total_duration_us(), 300_000);
    assert_pixel_near(pixel_at(&producer, &playback, 0), Rgba::new(255, 0, 0, 255));
    assert_pixel_near(
        pixel_at(&producer, &playback, 100),
        Rgba::new(0, 255, 0, 255),
    );
    assert_pixel_near(
        pixel_at(&producer, &playback, 200),
        Rgba::new(0, 0, 255, 255),
    );
    assert_pixel_near(
        pixel_at(&producer, &playback, 300),
        Rgba::new(255, 0, 0, 255),
    );
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

#[cfg(feature = "media-lottie")]
#[test]
fn lottie_frames_decode_when_feature_is_enabled() {
    let producer = MediaProducer::from_bytes(empty_lottie_bytes(), "application/json")
        .expect("test Lottie should decode");
    let playback = MediaPlayback::default();

    assert_eq!(producer.frame_count(), 2);
    assert_eq!(producer.total_duration_us(), 66_666);
    assert_eq!(pixel_at(&producer, &playback, 0), Rgba::new(0, 0, 0, 0));
    assert_eq!(pixel_at(&producer, &playback, 34), Rgba::new(0, 0, 0, 0));
}

#[cfg(feature = "media-video")]
#[test]
fn webm_video_frames_decode_when_feature_is_enabled() {
    let tempdir = tempfile::tempdir().expect("test tempdir should be created");
    let path = tempdir.path().join("sample.webm");
    if !write_test_webm(&path) {
        eprintln!("skipping media-video test because GStreamer VP8 plugins are unavailable");
        return;
    }

    let producer = MediaProducer::from_path(&path, "video/webm")
        .expect("test WebM should decode through GStreamer");
    let playback = MediaPlayback::default();

    assert_eq!(producer.frame_count(), 2);
    assert_eq!(producer.total_duration_us(), 2_000_000);
    assert_eq!(producer.render_frame(&playback, 0, 16, 16).width(), 16,);
    let pixel = pixel_at(&producer, &playback, 0);
    assert!(pixel.r >= 250 && pixel.g >= 250 && pixel.b >= 250 && pixel.a == 255);
}

#[cfg(feature = "media-video")]
#[test]
fn stream_url_producer_returns_before_first_live_frame() {
    let started = Instant::now();
    let producer = MediaProducer::from_bytes_with_stream_policy(
        b"http://1.1.1.1/hypercolor-missing-live.m3u8\n",
        "application/vnd.hypercolor.stream-url",
        &StreamUrlPolicy::default(),
    )
    .expect("stream URL producer should start");

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "stream URL producer should not preroll frames before returning"
    );
    assert_eq!(producer.frame_count(), 0);
    assert!(!producer.has_renderable_frame());
    let playback = MediaPlayback::default();
    assert_eq!(pixel_at(&producer, &playback, 0), Rgba::new(0, 0, 0, 255));
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
