use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};

use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frame, ImageError};
use thiserror::Error;

use crate::spatial::sample_viewport;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::layer::{LoopMode, MediaPlayback};
use hypercolor_types::viewport::{FitMode, ViewportRect};

const DEFAULT_FRAME_DURATION_US: u64 = 100_000;

#[derive(Debug, Error)]
pub enum MediaProducerError {
    #[error("failed to decode media: {0}")]
    Decode(#[from] ImageError),
    #[error("failed to read media file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("PNG sequence is empty")]
    EmptySequence,
    #[error("unsupported media type: {0}")]
    UnsupportedMime(String),
}

#[derive(Debug, Clone)]
pub struct MediaProducer {
    frames: Vec<DecodedMediaFrame>,
    total_duration_us: u64,
}

#[derive(Debug, Clone)]
struct DecodedMediaFrame {
    canvas: Canvas,
    duration_us: u64,
}

impl MediaProducer {
    pub fn from_path(path: &Path, mime_type: &str) -> Result<Self, MediaProducerError> {
        if path.is_dir() {
            return Self::from_png_sequence_dir(path, DEFAULT_FRAME_DURATION_US);
        }

        let bytes = std::fs::read(path).map_err(|source| MediaProducerError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_bytes(&bytes, mime_type)
    }

    pub fn from_bytes(bytes: &[u8], mime_type: &str) -> Result<Self, MediaProducerError> {
        match mime_type {
            "image/gif" => Self::from_animation_frames(decode_gif_frames(bytes)?),
            "image/apng" => {
                let frames = decode_apng_frames(bytes)?;
                if frames.is_empty() {
                    Self::from_static_bytes(bytes)
                } else {
                    Self::from_animation_frames(frames)
                }
            }
            "image/webp" => {
                let frames = decode_webp_frames(bytes)?;
                if frames.is_empty() {
                    Self::from_static_bytes(bytes)
                } else {
                    Self::from_animation_frames(frames)
                }
            }
            "image/png" | "image/jpeg" => Self::from_static_bytes(bytes),
            #[cfg(not(feature = "media-lottie"))]
            "application/json" => Err(MediaProducerError::UnsupportedMime(
                "application/json (enable the media-lottie feature to decode Lottie assets)"
                    .to_owned(),
            )),
            #[cfg(feature = "media-lottie")]
            "application/json" => Err(MediaProducerError::UnsupportedMime(
                "application/json Lottie decoding is not available in this build".to_owned(),
            )),
            #[cfg(not(feature = "media-video"))]
            video_mime @ ("video/mp4" | "video/webm") => Err(MediaProducerError::UnsupportedMime(
                format!("{video_mime} (enable the media-video feature to decode video assets)"),
            )),
            #[cfg(feature = "media-video")]
            video_mime @ ("video/mp4" | "video/webm") => Err(MediaProducerError::UnsupportedMime(
                format!("{video_mime} video decoding is not available in this build"),
            )),
            other => Err(MediaProducerError::UnsupportedMime(other.to_owned())),
        }
    }

    pub fn from_png_sequence_dir(
        path: &Path,
        frame_duration_us: u64,
    ) -> Result<Self, MediaProducerError> {
        let mut paths = std::fs::read_dir(path)
            .map_err(|source| MediaProducerError::Read {
                path: path.to_path_buf(),
                source,
            })?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        Self::from_png_sequence_paths(&paths, frame_duration_us)
    }

    pub fn from_png_sequence_paths(
        paths: &[PathBuf],
        frame_duration_us: u64,
    ) -> Result<Self, MediaProducerError> {
        if paths.is_empty() {
            return Err(MediaProducerError::EmptySequence);
        }

        let mut frames = Vec::with_capacity(paths.len());
        for path in paths {
            let bytes = std::fs::read(path).map_err(|source| MediaProducerError::Read {
                path: path.clone(),
                source,
            })?;
            let image = image::load_from_memory(&bytes)?.to_rgba8();
            frames.push(DecodedMediaFrame {
                canvas: canvas_from_rgba_image(image),
                duration_us: frame_duration_us.max(1),
            });
        }
        Ok(Self::from_decoded_frames(frames))
    }

    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    #[must_use]
    pub const fn total_duration_us(&self) -> u64 {
        self.total_duration_us
    }

    #[must_use]
    pub fn frame_index_at(&self, playback: &MediaPlayback, elapsed_ms: u32) -> usize {
        if self.frames.len() <= 1 || self.total_duration_us == 0 {
            return 0;
        }

        let offset_us = seconds_to_us(playback.start_offset_secs);
        let speed = if playback.speed.is_finite() {
            playback.speed.max(0.0)
        } else {
            1.0
        };
        let advanced_us = if playback.auto_play {
            millis_to_scaled_us(elapsed_ms, speed)
        } else {
            0
        };
        let playback_us = offset_us.saturating_add(advanced_us);

        match playback.loop_mode {
            LoopMode::None => {
                self.index_for_elapsed_us(playback_us.min(self.total_duration_us.saturating_sub(1)))
            }
            LoopMode::Loop => self.index_for_elapsed_us(playback_us % self.total_duration_us),
            LoopMode::PingPong => {
                let cycle_us = self.total_duration_us.saturating_mul(2);
                if cycle_us == 0 {
                    return 0;
                }
                let phase_us = playback_us % cycle_us;
                if phase_us < self.total_duration_us {
                    self.index_for_elapsed_us(phase_us)
                } else {
                    let reverse_us = self
                        .total_duration_us
                        .saturating_sub(1)
                        .saturating_sub(phase_us - self.total_duration_us);
                    self.index_for_elapsed_us(reverse_us)
                }
            }
        }
    }

    #[must_use]
    pub fn intrinsic_frame(&self, playback: &MediaPlayback, elapsed_ms: u32) -> Canvas {
        self.frames[self.frame_index_at(playback, elapsed_ms)]
            .canvas
            .clone()
    }

    #[must_use]
    pub fn render_frame(
        &self,
        playback: &MediaPlayback,
        elapsed_ms: u32,
        width: u32,
        height: u32,
    ) -> Canvas {
        self.render_frame_with_fit(playback, elapsed_ms, width, height, FitMode::Stretch)
    }

    #[must_use]
    pub fn render_frame_with_fit(
        &self,
        playback: &MediaPlayback,
        elapsed_ms: u32,
        width: u32,
        height: u32,
        fit_mode: FitMode,
    ) -> Canvas {
        let source = self.intrinsic_frame(playback, elapsed_ms);
        if source.width() == width && source.height() == height {
            return source;
        }

        let mut target = Canvas::new(width, height);
        sample_viewport(&mut target, &source, ViewportRect::full(), fit_mode, 1.0);
        target
    }

    fn from_static_bytes(bytes: &[u8]) -> Result<Self, MediaProducerError> {
        let image = image::load_from_memory(bytes)?.to_rgba8();
        Ok(Self::from_decoded_frames(vec![DecodedMediaFrame {
            canvas: canvas_from_rgba_image(image),
            duration_us: DEFAULT_FRAME_DURATION_US,
        }]))
    }

    fn from_animation_frames(frames: Vec<Frame>) -> Result<Self, MediaProducerError> {
        if frames.is_empty() {
            return Err(MediaProducerError::EmptySequence);
        }

        Ok(Self::from_decoded_frames(
            frames
                .into_iter()
                .map(|frame| {
                    let duration_us = delay_us(frame.delay());
                    DecodedMediaFrame {
                        canvas: canvas_from_rgba_image(frame.into_buffer()),
                        duration_us,
                    }
                })
                .collect(),
        ))
    }

    fn from_decoded_frames(frames: Vec<DecodedMediaFrame>) -> Self {
        let total_duration_us = frames
            .iter()
            .map(|frame| frame.duration_us.max(1))
            .fold(0_u64, u64::saturating_add);
        Self {
            frames,
            total_duration_us,
        }
    }

    fn index_for_elapsed_us(&self, elapsed_us: u64) -> usize {
        let mut cursor_us = 0_u64;
        for (index, frame) in self.frames.iter().enumerate() {
            cursor_us = cursor_us.saturating_add(frame.duration_us.max(1));
            if elapsed_us < cursor_us {
                return index;
            }
        }
        self.frames.len().saturating_sub(1)
    }
}

fn decode_gif_frames(bytes: &[u8]) -> Result<Vec<Frame>, ImageError> {
    let reader = BufReader::new(Cursor::new(bytes));
    let decoder = GifDecoder::new(reader)?;
    decoder.into_frames().collect_frames()
}

fn decode_apng_frames(bytes: &[u8]) -> Result<Vec<Frame>, ImageError> {
    let reader = BufReader::new(Cursor::new(bytes));
    let decoder = PngDecoder::new(reader)?;
    decoder.apng()?.into_frames().collect_frames()
}

fn decode_webp_frames(bytes: &[u8]) -> Result<Vec<Frame>, ImageError> {
    let reader = BufReader::new(Cursor::new(bytes));
    let decoder = WebPDecoder::new(reader)?;
    decoder.into_frames().collect_frames()
}

fn canvas_from_rgba_image(image: image::RgbaImage) -> Canvas {
    let (width, height) = image.dimensions();
    Canvas::from_vec(image.into_raw(), width, height)
}

fn delay_us(delay: image::Delay) -> u64 {
    let (numerator, denominator) = delay.numer_denom_ms();
    if denominator == 0 || numerator == 0 {
        return DEFAULT_FRAME_DURATION_US;
    }
    u64::from(numerator)
        .saturating_mul(1_000)
        .saturating_div(u64::from(denominator))
        .max(1)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "media playback maps bounded frame time into integer microseconds"
)]
fn seconds_to_us(seconds: f32) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * 1_000_000.0).round().max(0.0) as u64
    } else {
        0
    }
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "media playback maps bounded frame time into integer microseconds"
)]
fn millis_to_scaled_us(elapsed_ms: u32, speed: f32) -> u64 {
    (elapsed_ms as f32 * speed * 1_000.0).round().max(0.0) as u64
}
