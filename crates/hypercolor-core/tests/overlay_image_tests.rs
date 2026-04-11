use std::fs::File;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use gif::{Encoder, Frame, Repeat};
use image::{ImageBuffer, Rgba};
use tempfile::tempdir;

use hypercolor_core::overlay::{
    ImageRenderer, OverlayBuffer, OverlayInput, OverlayRenderer, OverlaySize,
};
use hypercolor_types::overlay::{ImageFit, ImageOverlayConfig};
use hypercolor_types::sensor::SystemSnapshot;

fn overlay_input(sensors: &SystemSnapshot, elapsed_secs: f32) -> OverlayInput<'_> {
    OverlayInput {
        now: SystemTime::now(),
        display_width: 4,
        display_height: 4,
        circular: false,
        sensors,
        elapsed_secs,
        frame_number: 1,
    }
}

fn assert_duration_close(actual: Option<Duration>, expected: Duration) {
    let actual = actual.expect("duration should exist");
    let delta = actual.abs_diff(expected);
    assert!(
        delta <= Duration::from_millis(1),
        "expected {expected:?}, got {actual:?}"
    );
}

#[test]
fn image_renderer_premultiplies_static_pixels() {
    let temp = tempdir().expect("tempdir should exist");
    let image_path = temp.path().join("static.png");
    let png = ImageBuffer::from_pixel(1, 1, Rgba([200_u8, 100, 50, 128]));
    png.save(&image_path).expect("png should save");

    let mut renderer = ImageRenderer::new(ImageOverlayConfig {
        path: image_path.to_string_lossy().into_owned(),
        speed: 1.0,
        fit: ImageFit::Stretch,
    })
    .expect("renderer should build");
    renderer
        .init(OverlaySize::new(1, 1))
        .expect("renderer should initialize");

    let sensors = SystemSnapshot::empty();
    let input = overlay_input(&sensors, 0.0);
    let mut target = OverlayBuffer::new(OverlaySize::new(1, 1));
    renderer
        .render_into(&input, &mut target)
        .expect("render should succeed");

    assert_eq!(target.pixels, vec![100, 50, 25, 128]);
    assert!(!renderer.content_changed(&overlay_input(&sensors, 1.0)));
    assert_eq!(renderer.next_refresh_after(), None);
}

#[test]
fn image_renderer_contain_fit_centers_source() {
    let temp = tempdir().expect("tempdir should exist");
    let image_path = temp.path().join("contain.png");
    let png = ImageBuffer::from_pixel(2, 1, Rgba([255_u8, 0, 0, 255]));
    png.save(&image_path).expect("png should save");

    let mut renderer = ImageRenderer::new(ImageOverlayConfig {
        path: image_path.to_string_lossy().into_owned(),
        speed: 1.0,
        fit: ImageFit::Contain,
    })
    .expect("renderer should build");
    renderer
        .init(OverlaySize::new(4, 4))
        .expect("renderer should initialize");

    let sensors = SystemSnapshot::empty();
    let mut target = OverlayBuffer::new(OverlaySize::new(4, 4));
    renderer
        .render_into(&overlay_input(&sensors, 0.0), &mut target)
        .expect("render should succeed");

    assert_eq!(&target.pixels[0..4], &[0, 0, 0, 0]);
    let centered_offset = ((1 * 4) + 1) * 4;
    assert_eq!(
        &target.pixels[centered_offset..centered_offset + 4],
        &[255, 0, 0, 255]
    );
}

#[test]
fn image_renderer_cycles_gif_frames_and_updates_refresh_hint() -> Result<()> {
    let temp = tempdir().expect("tempdir should exist");
    let gif_path = temp.path().join("animated.gif");
    let mut file = File::create(&gif_path)?;
    let mut encoder = Encoder::new(&mut file, 1, 1, &[])?;
    encoder.set_repeat(Repeat::Infinite)?;

    let mut red = vec![255, 0, 0, 255];
    let mut red_frame = Frame::from_rgba_speed(1, 1, &mut red, 1);
    red_frame.delay = 10;
    encoder.write_frame(&red_frame)?;

    let mut blue = vec![0, 0, 255, 255];
    let mut blue_frame = Frame::from_rgba_speed(1, 1, &mut blue, 1);
    blue_frame.delay = 20;
    encoder.write_frame(&blue_frame)?;
    drop(encoder);
    drop(file);

    let mut renderer = ImageRenderer::new(ImageOverlayConfig {
        path: gif_path.to_string_lossy().into_owned(),
        speed: 1.0,
        fit: ImageFit::Original,
    })?;
    renderer.init(OverlaySize::new(1, 1))?;

    let sensors = SystemSnapshot::empty();
    let mut target = OverlayBuffer::new(OverlaySize::new(1, 1));

    renderer.render_into(&overlay_input(&sensors, 0.0), &mut target)?;
    assert_eq!(target.pixels, vec![255, 0, 0, 255]);
    assert_duration_close(renderer.next_refresh_after(), Duration::from_millis(100));
    assert!(!renderer.content_changed(&overlay_input(&sensors, 0.05)));
    assert!(renderer.content_changed(&overlay_input(&sensors, 0.15)));

    renderer.render_into(&overlay_input(&sensors, 0.15), &mut target)?;
    assert_eq!(target.pixels, vec![0, 0, 255, 255]);
    assert_duration_close(renderer.next_refresh_after(), Duration::from_millis(150));
    assert!(renderer.content_changed(&overlay_input(&sensors, 0.35)));

    Ok(())
}
