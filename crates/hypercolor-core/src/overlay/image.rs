use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::codecs::gif::GifDecoder;
use image::imageops::{FilterType, overlay as overlay_image, resize};
use image::{AnimationDecoder, Delay, ImageFormat, ImageReader, RgbaImage};

use hypercolor_types::overlay::{ImageFit, ImageOverlayConfig};

use super::{OverlayBuffer, OverlayError, OverlayInput, OverlayRenderer, OverlaySize};

pub struct ImageRenderer {
    resolved_path: PathBuf,
    fit: ImageFit,
    speed: f32,
    target_size: OverlaySize,
    prepared: PreparedImage,
    current_frame_index: usize,
    next_refresh_after: Option<Duration>,
}

enum PreparedImage {
    Static(OverlayBuffer),
    Animated(AnimatedImage),
}

struct AnimatedImage {
    frames: Vec<AnimatedFrame>,
    total_duration: Duration,
}

struct AnimatedFrame {
    buffer: OverlayBuffer,
    duration: Duration,
}

impl ImageRenderer {
    pub fn new(config: ImageOverlayConfig) -> Result<Self> {
        let resolved_path = resolve_image_overlay_path(Path::new(&config.path))
            .with_context(|| format!("failed to resolve image overlay source '{}'", config.path))?;

        Ok(Self {
            resolved_path,
            fit: config.fit,
            speed: config.speed,
            target_size: OverlaySize::new(1, 1),
            prepared: PreparedImage::Static(OverlayBuffer::new(OverlaySize::new(1, 1))),
            current_frame_index: 0,
            next_refresh_after: None,
        })
    }

    fn reload(&mut self, target_size: OverlaySize) -> Result<()> {
        self.target_size = target_size;
        self.prepared =
            load_prepared_image(&self.resolved_path, target_size, self.fit, self.speed)?;
        self.current_frame_index = 0;
        self.next_refresh_after = None;
        Ok(())
    }
}

impl OverlayRenderer for ImageRenderer {
    fn init(&mut self, target_size: OverlaySize) -> Result<()> {
        self.reload(target_size)
    }

    fn resize(&mut self, target_size: OverlaySize) -> Result<()> {
        self.reload(target_size)
    }

    fn render_into(
        &mut self,
        input: &OverlayInput<'_>,
        target: &mut OverlayBuffer,
    ) -> std::result::Result<(), OverlayError> {
        match &self.prepared {
            PreparedImage::Static(buffer) => {
                copy_prepared_buffer(target, buffer)?;
                self.current_frame_index = 0;
                self.next_refresh_after = None;
            }
            PreparedImage::Animated(animated) => {
                let (frame_index, refresh_after) = animated.frame_at(input.elapsed_secs);
                copy_prepared_buffer(target, &animated.frames[frame_index].buffer)?;
                self.current_frame_index = frame_index;
                self.next_refresh_after = Some(refresh_after);
            }
        }

        Ok(())
    }

    fn content_changed(&self, input: &OverlayInput<'_>) -> bool {
        match &self.prepared {
            PreparedImage::Static(_) => false,
            PreparedImage::Animated(animated) => {
                let (frame_index, _) = animated.frame_at(input.elapsed_secs);
                frame_index != self.current_frame_index
            }
        }
    }

    fn next_refresh_after(&self) -> Option<Duration> {
        self.next_refresh_after
    }
}

impl AnimatedImage {
    fn frame_at(&self, elapsed_secs: f32) -> (usize, Duration) {
        if self.frames.len() <= 1 || self.total_duration.is_zero() {
            return (0, self.frames[0].duration);
        }

        let total_cycle_secs = self.total_duration.as_secs_f64();
        let mut cycle_position = f64::from(elapsed_secs.max(0.0)).rem_euclid(total_cycle_secs);
        for (index, frame) in self.frames.iter().enumerate() {
            let frame_secs = frame.duration.as_secs_f64();
            if cycle_position < frame_secs || index + 1 == self.frames.len() {
                return (
                    index,
                    Duration::from_secs_f64((frame_secs - cycle_position).max(0.001)),
                );
            }
            cycle_position -= frame_secs;
        }

        (0, self.frames[0].duration)
    }
}

fn load_prepared_image(
    path: &Path,
    target_size: OverlaySize,
    fit: ImageFit,
    speed: f32,
) -> Result<PreparedImage> {
    let reader = ImageReader::open(path)
        .with_context(|| format!("failed to open image overlay '{}'", path.display()))?
        .with_guessed_format()
        .with_context(|| format!("failed to inspect image overlay '{}'", path.display()))?;
    let format = reader.format();

    if matches!(format, Some(ImageFormat::Gif)) {
        drop(reader);
        return load_gif_frames(path, target_size, fit, speed);
    }

    let image = reader
        .decode()
        .with_context(|| format!("failed to decode image overlay '{}'", path.display()))?
        .into_rgba8();
    Ok(PreparedImage::Static(render_image_frame(
        &image,
        target_size,
        fit,
    )))
}

fn load_gif_frames(
    path: &Path,
    target_size: OverlaySize,
    fit: ImageFit,
    speed: f32,
) -> Result<PreparedImage> {
    let file = File::open(path)
        .with_context(|| format!("failed to open GIF overlay '{}'", path.display()))?;
    let decoder = GifDecoder::new(BufReader::new(file))
        .with_context(|| format!("failed to decode GIF overlay '{}'", path.display()))?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .with_context(|| format!("failed to read GIF frames from '{}'", path.display()))?;
    if frames.is_empty() {
        bail!("GIF overlay '{}' contains no frames", path.display());
    }

    let mut prepared_frames = frames
        .into_iter()
        .map(|frame| {
            let delay = frame.delay();
            let buffer = frame.into_buffer();
            Ok(AnimatedFrame {
                buffer: render_image_frame(&buffer, target_size, fit),
                duration: scaled_delay(delay, speed),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if prepared_frames.len() == 1 {
        let frame = prepared_frames
            .pop()
            .expect("single-frame GIF should contain one prepared frame");
        return Ok(PreparedImage::Static(frame.buffer));
    }

    let total_duration = prepared_frames.iter().fold(Duration::ZERO, |total, frame| {
        total.saturating_add(frame.duration)
    });
    Ok(PreparedImage::Animated(AnimatedImage {
        frames: prepared_frames,
        total_duration,
    }))
}

fn render_image_frame(
    source: &RgbaImage,
    target_size: OverlaySize,
    fit: ImageFit,
) -> OverlayBuffer {
    let target_width = target_size.width.max(1);
    let target_height = target_size.height.max(1);
    let mut composed = RgbaImage::new(target_width, target_height);
    let (resized, x, y) = fitted_image(source, target_width, target_height, fit);
    overlay_image(&mut composed, &resized, x, y);
    rgba_image_to_overlay_buffer(&composed)
}

fn fitted_image(
    source: &RgbaImage,
    target_width: u32,
    target_height: u32,
    fit: ImageFit,
) -> (RgbaImage, i64, i64) {
    match fit {
        ImageFit::Stretch => (
            resize(source, target_width, target_height, FilterType::Triangle),
            0,
            0,
        ),
        ImageFit::Original => {
            let x = centered_offset(target_width, source.width());
            let y = centered_offset(target_height, source.height());
            (source.clone(), x, y)
        }
        ImageFit::Contain | ImageFit::Cover => {
            let scale_x = f64::from(target_width) / f64::from(source.width().max(1));
            let scale_y = f64::from(target_height) / f64::from(source.height().max(1));
            let scale = match fit {
                ImageFit::Contain => scale_x.min(scale_y),
                ImageFit::Cover => scale_x.max(scale_y),
                ImageFit::Stretch | ImageFit::Original => unreachable!(),
            };
            let width = scaled_dimension(source.width(), scale);
            let height = scaled_dimension(source.height(), scale);
            let resized = resize(source, width, height, FilterType::Triangle);
            let x = centered_offset(target_width, width);
            let y = centered_offset(target_height, height);
            (resized, x, y)
        }
    }
}

fn rgba_image_to_overlay_buffer(image: &RgbaImage) -> OverlayBuffer {
    let mut buffer = OverlayBuffer::new(OverlaySize::new(image.width(), image.height()));
    for (premul, straight) in buffer
        .pixels
        .chunks_exact_mut(4)
        .zip(image.as_raw().chunks_exact(4))
    {
        let alpha = straight[3];
        premul[3] = alpha;
        if alpha == 0 {
            premul[0] = 0;
            premul[1] = 0;
            premul[2] = 0;
            continue;
        }
        if alpha == u8::MAX {
            premul[0] = straight[0];
            premul[1] = straight[1];
            premul[2] = straight[2];
            continue;
        }
        premul[0] = premultiply_channel(straight[0], alpha);
        premul[1] = premultiply_channel(straight[1], alpha);
        premul[2] = premultiply_channel(straight[2], alpha);
    }
    buffer
}

fn copy_prepared_buffer(
    target: &mut OverlayBuffer,
    prepared: &OverlayBuffer,
) -> Result<(), OverlayError> {
    if target.width != prepared.width || target.height != prepared.height {
        return Err(OverlayError::Fatal(format!(
            "image overlay buffer size mismatch: renderer prepared {}x{}, target was {}x{}",
            prepared.width, prepared.height, target.width, target.height
        )));
    }

    target.pixels.copy_from_slice(&prepared.pixels);
    Ok(())
}

fn resolve_image_overlay_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        bail!(
            "absolute image overlay path does not exist: {}",
            path.display()
        );
    }

    let mut candidates = Vec::new();
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join(path));
    }
    candidates.push(path.to_path_buf());

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    bail!(
        "could not resolve image overlay source '{}'; searched current and raw relative paths",
        path.display()
    );
}

fn scaled_delay(delay: Delay, speed: f32) -> Duration {
    let (numerator, denominator) = delay.numer_denom_ms();
    let millis = if denominator == 0 {
        0.0
    } else {
        f64::from(numerator) / f64::from(denominator)
    };
    Duration::from_secs_f64((millis / f64::from(speed)).max(16.0) / 1_000.0)
}

fn centered_offset(target: u32, content: u32) -> i64 {
    (i64::from(target) - i64::from(content)) / 2
}

fn scaled_dimension(source: u32, scale: f64) -> u32 {
    ((f64::from(source.max(1)) * scale).round().max(1.0)) as u32
}

fn premultiply_channel(channel: u8, alpha: u8) -> u8 {
    let scaled = u16::from(channel)
        .saturating_mul(u16::from(alpha))
        .saturating_add(127)
        / u16::from(u8::MAX);
    u8::try_from(scaled).unwrap_or(u8::MAX)
}
