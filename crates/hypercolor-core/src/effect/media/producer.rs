use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
#[cfg(feature = "media-video")]
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
#[cfg(feature = "media-video")]
use std::thread::{self, JoinHandle};
#[cfg(feature = "media-video")]
use std::time::{Duration, Instant};

#[cfg(feature = "media-video")]
use gst::prelude::*;
#[cfg(feature = "media-video")]
use gstreamer as gst;
#[cfg(feature = "media-video")]
use gstreamer_app as gst_app;
#[cfg(feature = "media-video")]
use gstreamer_video as gst_video;
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frame, ImageError};
use thiserror::Error;

use crate::asset::library::StreamUrlPolicy;
#[cfg(feature = "media-video")]
use crate::asset::library::stream_url_from_bytes_with_policy;
use crate::spatial::sample_viewport;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::layer::{LoopMode, MediaPlayback};
use hypercolor_types::viewport::{FitMode, ViewportRect};

const DEFAULT_FRAME_DURATION_US: u64 = 100_000;
#[cfg(feature = "media-lottie")]
const DEFAULT_LOTTIE_CACHE_KEY: &str = "hypercolor-inline-lottie";
#[cfg(feature = "media-video")]
const GST_FRAME_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(5);
#[cfg(feature = "media-video")]
const LIVE_STREAM_FRAME_TIMEOUT: gst::ClockTime = gst::ClockTime::from_mseconds(250);
#[cfg(feature = "media-video")]
const LIVE_STREAM_STALL_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(feature = "media-video")]
const LIVE_STREAM_SHUTDOWN_POLL: Duration = Duration::from_millis(100);
#[cfg(feature = "media-video")]
const LIVE_STREAM_RECONNECT_BACKOFF_MS: [u64; 5] = [1_000, 2_000, 5_000, 10_000, 30_000];
#[cfg(feature = "media-video")]
const STREAM_URL_MIME: &str = "application/vnd.hypercolor.stream-url";
const STATIC_MEDIA_ESTIMATED_COST_US: u64 = 0;
const ANIMATED_MEDIA_ESTIMATED_COST_US: u64 = 400;
const MAX_ANIMATION_FRAMES: usize = 16_384;
const MAX_ANIMATION_DECODED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ANIMATION_DIMENSION: u32 = 8_192;
#[cfg(feature = "media-lottie")]
const LOTTIE_MEDIA_ESTIMATED_COST_US: u64 = 8_000;
#[cfg(feature = "media-lottie")]
const MAX_LOTTIE_DIMENSION: usize = 8_192;
#[cfg(feature = "media-lottie")]
const MAX_LOTTIE_FRAME_COUNT: usize = 600;
#[cfg(feature = "media-lottie")]
const MAX_LOTTIE_DECODED_BYTES: usize = 128 * 1024 * 1024;
#[cfg(feature = "media-video")]
const VIDEO_MEDIA_ESTIMATED_COST_US: u64 = 20_000;
#[cfg(feature = "media-video")]
const STREAM_MEDIA_ESTIMATED_COST_US: u64 = 25_000;

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
    #[error("decoded animation exceeds safety limits")]
    AnimationTooLarge,
    #[cfg(feature = "media-lottie")]
    #[error("failed to decode Lottie animation")]
    LottieDecode,
    #[cfg(feature = "media-lottie")]
    #[error("Lottie animation data contains an embedded NUL byte")]
    LottieContainsNul,
    #[cfg(feature = "media-lottie")]
    #[error("Lottie animation has invalid dimensions {width}x{height}")]
    InvalidLottieSize { width: usize, height: usize },
    #[cfg(feature = "media-lottie")]
    #[error("Lottie animation has too many frames: {frame_count} (max {max})")]
    LottieFrameCountExceeded { frame_count: usize, max: usize },
    #[cfg(feature = "media-lottie")]
    #[error("Lottie animation decoded size {decoded_bytes} bytes exceeds max {max_bytes} bytes")]
    LottieDecodedBudgetExceeded { decoded_bytes: usize, max_bytes: usize },
    #[cfg(feature = "media-video")]
    #[error("failed to decode video: {0}")]
    VideoDecode(String),
    #[cfg(feature = "media-video")]
    #[error("invalid stream URL asset")]
    InvalidStreamUrl,
}

#[derive(Debug, Clone)]
pub struct MediaProducer {
    frames: Vec<DecodedMediaFrame>,
    total_duration_us: u64,
    estimated_cost_us: u64,
    #[cfg(feature = "media-video")]
    live_stream: Option<LiveStreamProducer>,
}

#[derive(Debug, Clone)]
struct DecodedMediaFrame {
    canvas: Canvas,
    duration_us: u64,
}

#[cfg(feature = "media-video")]
#[derive(Debug, Clone)]
struct LiveStreamProducer {
    inner: Arc<LiveStreamInner>,
}

#[cfg(feature = "media-video")]
#[derive(Debug)]
struct LiveStreamInner {
    state: Arc<Mutex<LiveStreamState>>,
    shutdown: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[cfg(feature = "media-video")]
#[derive(Debug, Default)]
struct LiveStreamState {
    latest_frame: Option<DecodedMediaFrame>,
    frames_seen: usize,
    last_error: Option<String>,
}

impl MediaProducer {
    pub fn from_path(path: &Path, mime_type: &str) -> Result<Self, MediaProducerError> {
        Self::from_path_with_stream_policy(path, mime_type, &StreamUrlPolicy::default())
    }

    pub fn from_path_with_stream_policy(
        path: &Path,
        mime_type: &str,
        stream_url_policy: &StreamUrlPolicy,
    ) -> Result<Self, MediaProducerError> {
        if path.is_dir() {
            return Self::from_png_sequence_dir(path, DEFAULT_FRAME_DURATION_US);
        }

        #[cfg(feature = "media-video")]
        if is_video_mime(mime_type) {
            return Self::from_video_path(path);
        }

        let bytes = std::fs::read(path).map_err(|source| MediaProducerError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_bytes_with_stream_policy(&bytes, mime_type, stream_url_policy)
    }

    pub fn from_bytes(bytes: &[u8], mime_type: &str) -> Result<Self, MediaProducerError> {
        Self::from_bytes_with_stream_policy(bytes, mime_type, &StreamUrlPolicy::default())
    }

    pub fn from_bytes_with_stream_policy(
        bytes: &[u8],
        mime_type: &str,
        stream_url_policy: &StreamUrlPolicy,
    ) -> Result<Self, MediaProducerError> {
        let _ = stream_url_policy;
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
            "application/json" => Self::from_lottie_bytes(bytes),
            #[cfg(not(feature = "media-video"))]
            video_mime @ ("video/mp4" | "video/webm") => Err(MediaProducerError::UnsupportedMime(
                format!("{video_mime} (enable the media-video feature to decode video assets)"),
            )),
            #[cfg(feature = "media-video")]
            video_mime @ ("video/mp4" | "video/webm") => Err(MediaProducerError::UnsupportedMime(
                format!("{video_mime} video decoding requires a file-backed asset"),
            )),
            #[cfg(feature = "media-video")]
            STREAM_URL_MIME => Self::from_stream_url_bytes(bytes, stream_url_policy),
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
        #[cfg(feature = "media-video")]
        if let Some(live_stream) = &self.live_stream {
            return live_stream.frame_count();
        }

        self.frames.len()
    }

    #[must_use]
    pub fn has_renderable_frame(&self) -> bool {
        #[cfg(feature = "media-video")]
        if let Some(live_stream) = &self.live_stream {
            return live_stream.has_frame();
        }

        !self.frames.is_empty()
    }

    #[must_use]
    pub fn live_stream_error(&self) -> Option<String> {
        #[cfg(feature = "media-video")]
        if let Some(live_stream) = &self.live_stream {
            return live_stream.last_error();
        }

        None
    }

    #[must_use]
    pub const fn total_duration_us(&self) -> u64 {
        self.total_duration_us
    }

    #[must_use]
    pub const fn estimated_cost_us(&self) -> u64 {
        self.estimated_cost_us
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
        #[cfg(feature = "media-video")]
        if let Some(live_stream) = &self.live_stream {
            return live_stream
                .latest_frame()
                .unwrap_or_else(|| Canvas::new(1, 1));
        }

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
        Ok(Self::from_decoded_frames_with_cost(
            vec![DecodedMediaFrame {
                canvas: canvas_from_rgba_image(image),
                duration_us: DEFAULT_FRAME_DURATION_US,
            }],
            STATIC_MEDIA_ESTIMATED_COST_US,
        ))
    }

    #[cfg(feature = "media-lottie")]
    fn from_lottie_bytes(bytes: &[u8]) -> Result<Self, MediaProducerError> {
        if bytes.contains(&0) {
            return Err(MediaProducerError::LottieContainsNul);
        }

        let cache_key = lottie_cache_key(bytes);
        let mut animation =
            rlottie::Animation::from_data(bytes.to_vec(), cache_key.as_str(), Path::new("."))
                .ok_or(MediaProducerError::LottieDecode)?;
        let size = animation.size();
        validate_lottie_size(size)?;

        let frame_count = animation.totalframe().max(1);
        validate_lottie_decode_budget(size, frame_count)?;
        let duration_us = lottie_frame_duration_us(&animation, frame_count);
        let mut surface = rlottie::Surface::new(size);
        let mut frames = Vec::with_capacity(frame_count);
        for frame_index in 0..frame_count {
            animation.render(frame_index, &mut surface);
            frames.push(DecodedMediaFrame {
                canvas: canvas_from_lottie_surface(&surface),
                duration_us,
            });
        }

        Ok(Self::from_decoded_frames_with_cost(
            frames,
            LOTTIE_MEDIA_ESTIMATED_COST_US,
        ))
    }

    #[cfg(feature = "media-video")]
    fn from_video_path(path: &Path) -> Result<Self, MediaProducerError> {
        ensure_gstreamer()?;
        let uri = gst::glib::filename_to_uri(path, None)
            .map_err(|error| MediaProducerError::VideoDecode(error.to_string()))?;
        let frames = decode_video_uri(uri.as_str(), None)?;
        Ok(Self::from_decoded_frames_with_cost(
            frames,
            VIDEO_MEDIA_ESTIMATED_COST_US,
        ))
    }

    #[cfg(feature = "media-video")]
    fn from_stream_url_bytes(
        bytes: &[u8],
        stream_url_policy: &StreamUrlPolicy,
    ) -> Result<Self, MediaProducerError> {
        ensure_gstreamer()?;
        let url = stream_url_from_bytes_with_policy(bytes, stream_url_policy)
            .ok_or(MediaProducerError::InvalidStreamUrl)?;
        Ok(Self {
            frames: Vec::new(),
            total_duration_us: DEFAULT_FRAME_DURATION_US,
            estimated_cost_us: STREAM_MEDIA_ESTIMATED_COST_US,
            live_stream: Some(LiveStreamProducer::spawn(url)?),
        })
    }

    fn from_animation_frames(frames: Vec<Frame>) -> Result<Self, MediaProducerError> {
        if frames.is_empty() {
            return Err(MediaProducerError::EmptySequence);
        }

        Ok(Self::from_decoded_frames_with_cost(
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
            ANIMATED_MEDIA_ESTIMATED_COST_US,
        ))
    }

    fn from_decoded_frames(frames: Vec<DecodedMediaFrame>) -> Self {
        Self::from_decoded_frames_with_cost(frames, ANIMATED_MEDIA_ESTIMATED_COST_US)
    }

    fn from_decoded_frames_with_cost(
        frames: Vec<DecodedMediaFrame>,
        estimated_cost_us: u64,
    ) -> Self {
        let total_duration_us = frames
            .iter()
            .map(|frame| frame.duration_us.max(1))
            .fold(0_u64, u64::saturating_add);
        Self {
            frames,
            total_duration_us,
            estimated_cost_us,
            #[cfg(feature = "media-video")]
            live_stream: None,
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

#[cfg(feature = "media-video")]
impl LiveStreamProducer {
    fn spawn(uri: String) -> Result<Self, MediaProducerError> {
        let state = Arc::new(Mutex::new(LiveStreamState::default()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&state);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = thread::Builder::new()
            .name("hypercolor-media-stream".to_owned())
            .spawn(move || run_live_stream_worker(uri, worker_state, worker_shutdown))
            .map_err(|error| MediaProducerError::VideoDecode(error.to_string()))?;

        Ok(Self {
            inner: Arc::new(LiveStreamInner {
                state,
                shutdown,
                worker: Mutex::new(Some(worker)),
            }),
        })
    }

    fn latest_frame(&self) -> Option<Canvas> {
        self.inner.state.lock().ok().and_then(|state| {
            state
                .latest_frame
                .as_ref()
                .map(|frame| frame.canvas.clone())
        })
    }

    fn frame_count(&self) -> usize {
        self.inner.state.lock().map_or(0, |state| state.frames_seen)
    }

    fn has_frame(&self) -> bool {
        self.inner
            .state
            .lock()
            .is_ok_and(|state| state.latest_frame.is_some())
    }

    fn last_error(&self) -> Option<String> {
        self.inner
            .state
            .lock()
            .ok()
            .and_then(|state| state.last_error.clone())
    }
}

#[cfg(feature = "media-video")]
impl Drop for LiveStreamInner {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

fn decode_gif_frames(bytes: &[u8]) -> Result<Vec<Frame>, MediaProducerError> {
    let reader = BufReader::new(Cursor::new(bytes));
    let decoder = GifDecoder::new(reader)?;
    collect_limited_frames(decoder.into_frames())
}

fn decode_apng_frames(bytes: &[u8]) -> Result<Vec<Frame>, MediaProducerError> {
    let reader = BufReader::new(Cursor::new(bytes));
    let decoder = PngDecoder::new(reader)?;
    collect_limited_frames(decoder.apng()?.into_frames())
}

fn decode_webp_frames(bytes: &[u8]) -> Result<Vec<Frame>, MediaProducerError> {
    let reader = BufReader::new(Cursor::new(bytes));
    let decoder = WebPDecoder::new(reader)?;
    collect_limited_frames(decoder.into_frames())
}

fn collect_limited_frames<I>(frames: I) -> Result<Vec<Frame>, MediaProducerError>
where
    I: IntoIterator<Item = Result<Frame, ImageError>>,
{
    let mut decoded = Vec::new();
    let mut total_bytes = 0_u64;

    for frame_result in frames {
        let frame = frame_result?;
        let buffer = frame.buffer();
        if buffer.width() > MAX_ANIMATION_DIMENSION || buffer.height() > MAX_ANIMATION_DIMENSION {
            return Err(MediaProducerError::AnimationTooLarge);
        }

        let frame_bytes = u64::from(buffer.width())
            .saturating_mul(u64::from(buffer.height()))
            .saturating_mul(4);
        total_bytes = total_bytes.saturating_add(frame_bytes);
        if total_bytes > MAX_ANIMATION_DECODED_BYTES || decoded.len() >= MAX_ANIMATION_FRAMES {
            return Err(MediaProducerError::AnimationTooLarge);
        }

        decoded.push(frame);
    }

    Ok(decoded)
}

fn canvas_from_rgba_image(image: image::RgbaImage) -> Canvas {
    let (width, height) = image.dimensions();
    Canvas::from_vec(image.into_raw(), width, height)
}

/// rlottie caches parsed animations by this key, so it must be derived from
/// the content — a fixed key makes every distinct inline Lottie collide.
#[cfg(feature = "media-lottie")]
fn lottie_cache_key(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{DEFAULT_LOTTIE_CACHE_KEY}-{:016x}", hasher.finish())
}

#[cfg(feature = "media-lottie")]
fn validate_lottie_size(size: rlottie::Size) -> Result<(), MediaProducerError> {
    if size.width == 0
        || size.height == 0
        || size.width > MAX_LOTTIE_DIMENSION
        || size.height > MAX_LOTTIE_DIMENSION
    {
        return Err(MediaProducerError::InvalidLottieSize {
            width: size.width,
            height: size.height,
        });
    }

    Ok(())
}

#[cfg(feature = "media-lottie")]
fn validate_lottie_decode_budget(
    size: rlottie::Size,
    frame_count: usize,
) -> Result<(), MediaProducerError> {
    if frame_count > MAX_LOTTIE_FRAME_COUNT {
        return Err(MediaProducerError::LottieFrameCountExceeded {
            frame_count,
            max: MAX_LOTTIE_FRAME_COUNT,
        });
    }

    let decoded_bytes = size
        .width
        .checked_mul(size.height)
        .and_then(|pixels_per_frame| pixels_per_frame.checked_mul(4))
        .and_then(|bytes_per_frame| bytes_per_frame.checked_mul(frame_count))
        .ok_or(MediaProducerError::LottieDecodedBudgetExceeded {
            decoded_bytes: usize::MAX,
            max_bytes: MAX_LOTTIE_DECODED_BYTES,
        })?;

    if decoded_bytes > MAX_LOTTIE_DECODED_BYTES {
        return Err(MediaProducerError::LottieDecodedBudgetExceeded {
            decoded_bytes,
            max_bytes: MAX_LOTTIE_DECODED_BYTES,
        });
    }

    Ok(())
}

#[cfg(feature = "media-lottie")]
fn lottie_frame_duration_us(animation: &rlottie::Animation, frame_count: usize) -> u64 {
    let framerate = animation.framerate();
    if framerate.is_finite() && framerate > 0.0 {
        return (1_000_000.0 / framerate).round().max(1.0) as u64;
    }

    let duration = animation.duration();
    if duration.is_finite() && duration > 0.0 && frame_count > 0 {
        return (duration * 1_000_000.0 / frame_count as f64)
            .round()
            .max(1.0) as u64;
    }

    DEFAULT_FRAME_DURATION_US
}

#[cfg(feature = "media-lottie")]
fn canvas_from_lottie_surface(surface: &rlottie::Surface) -> Canvas {
    let mut rgba = Vec::with_capacity(surface.width() * surface.height() * 4);
    for pixel in surface.data() {
        rgba.extend_from_slice(&[pixel.r, pixel.g, pixel.b, pixel.a]);
    }
    Canvas::from_vec(rgba, surface.width() as u32, surface.height() as u32)
}

#[cfg(feature = "media-video")]
fn is_video_mime(mime_type: &str) -> bool {
    matches!(mime_type, "video/mp4" | "video/webm")
}

#[cfg(feature = "media-video")]
fn ensure_gstreamer() -> Result<(), MediaProducerError> {
    static GST_INIT: OnceLock<Result<(), String>> = OnceLock::new();
    GST_INIT
        .get_or_init(|| gst::init().map_err(|error| error.to_string()))
        .clone()
        .map_err(MediaProducerError::VideoDecode)
}

#[cfg(feature = "media-video")]
fn decode_video_uri(
    uri: &str,
    frame_limit: Option<usize>,
) -> Result<Vec<DecodedMediaFrame>, MediaProducerError> {
    let video = build_rgba_video_pipeline(uri)?;
    video
        .pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| MediaProducerError::VideoDecode(format!("{error:?}")))?;
    let result = pull_video_frames(&video.pipeline, &video.sink, frame_limit);
    let _ = video.pipeline.set_state(gst::State::Null);
    result
}

#[cfg(feature = "media-video")]
struct RgbaVideoPipeline {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
}

#[cfg(feature = "media-video")]
fn build_rgba_video_pipeline(uri: &str) -> Result<RgbaVideoPipeline, MediaProducerError> {
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .build();
    let pipeline = gst::Pipeline::new();
    let source = make_gst_element("uridecodebin")?;
    source.set_property("uri", uri);
    let convert = make_gst_element("videoconvert")?;
    let capsfilter = make_gst_element("capsfilter")?;
    capsfilter.set_property("caps", &caps);
    let sink = gst_app::AppSink::builder()
        .caps(&caps)
        .sync(false)
        .async_(false)
        .enable_last_sample(false)
        .wait_on_eos(false)
        .build();
    let sink_element = sink.upcast_ref::<gst::Element>();

    pipeline
        .add_many([&source, &convert, &capsfilter, sink_element])
        .map_err(|error| MediaProducerError::VideoDecode(error.to_string()))?;
    gst::Element::link_many([&convert, &capsfilter, sink_element])
        .map_err(|error| MediaProducerError::VideoDecode(error.to_string()))?;
    link_uridecodebin_to_convert(&source, &convert)?;

    Ok(RgbaVideoPipeline { pipeline, sink })
}

#[cfg(feature = "media-video")]
fn make_gst_element(factory_name: &str) -> Result<gst::Element, MediaProducerError> {
    gst::ElementFactory::make(factory_name)
        .build()
        .map_err(|error| MediaProducerError::VideoDecode(error.to_string()))
}

#[cfg(feature = "media-video")]
fn link_uridecodebin_to_convert(
    source: &gst::Element,
    convert: &gst::Element,
) -> Result<(), MediaProducerError> {
    let convert_sink_pad = convert
        .static_pad("sink")
        .ok_or_else(|| MediaProducerError::VideoDecode("videoconvert has no sink pad".into()))?;
    source.connect_pad_added(move |_source, src_pad| {
        if convert_sink_pad.is_linked() {
            return;
        }
        let Some(caps) = src_pad.current_caps() else {
            return;
        };
        let Some(structure) = caps.structure(0) else {
            return;
        };
        if !structure.name().starts_with("video/") {
            return;
        }
        let _ = src_pad.link(&convert_sink_pad);
    });
    Ok(())
}

#[cfg(feature = "media-video")]
fn pull_video_frames(
    pipeline: &gst::Pipeline,
    sink: &gst_app::AppSink,
    frame_limit: Option<usize>,
) -> Result<Vec<DecodedMediaFrame>, MediaProducerError> {
    let mut frames = Vec::new();
    loop {
        if let Some(sample) = sink.try_pull_sample(GST_FRAME_TIMEOUT) {
            frames.push(decoded_frame_from_sample(&sample)?);
            if frame_limit.is_some_and(|limit| frames.len() >= limit) {
                break;
            }
            continue;
        }
        if sink.is_eos() {
            break;
        }
        if let Some(error) = pipeline_error(pipeline) {
            return Err(MediaProducerError::VideoDecode(error));
        }
        return Err(MediaProducerError::VideoDecode(
            "timed out waiting for a decoded frame".to_owned(),
        ));
    }

    if frames.is_empty() {
        return Err(MediaProducerError::EmptySequence);
    }
    Ok(frames)
}

#[cfg(feature = "media-video")]
fn run_live_stream_worker(
    uri: String,
    state: Arc<Mutex<LiveStreamState>>,
    shutdown: Arc<AtomicBool>,
) {
    let mut backoff = LiveStreamBackoff::default();
    while !shutdown.load(Ordering::Relaxed) {
        let frames_before = live_stream_frames_seen(&state);
        match pump_live_stream_once(&uri, &state, &shutdown) {
            Ok(LiveStreamPumpExit::Shutdown) => break,
            Ok(LiveStreamPumpExit::Retry) => {
                backoff.reset_if_frames_advanced(frames_before, &state);
                record_live_stream_error(&state, "live stream ended");
                if sleep_live_stream_backoff(backoff.next_delay(), &shutdown) {
                    break;
                }
            }
            Err(error) => {
                backoff.reset_if_frames_advanced(frames_before, &state);
                record_live_stream_error(&state, error.to_string());
                tracing::warn!(%uri, %error, "live media stream reconnecting");
                if sleep_live_stream_backoff(backoff.next_delay(), &shutdown) {
                    break;
                }
            }
        }
    }
}

#[cfg(feature = "media-video")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveStreamPumpExit {
    Shutdown,
    Retry,
}

#[cfg(feature = "media-video")]
fn pump_live_stream_once(
    uri: &str,
    state: &Arc<Mutex<LiveStreamState>>,
    shutdown: &AtomicBool,
) -> Result<LiveStreamPumpExit, MediaProducerError> {
    let video = build_rgba_video_pipeline(uri)?;
    video
        .pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| MediaProducerError::VideoDecode(format!("{error:?}")))?;
    let result = pump_live_stream_samples(&video.pipeline, &video.sink, state, shutdown);
    let _ = video.pipeline.set_state(gst::State::Null);
    result
}

#[cfg(feature = "media-video")]
fn pump_live_stream_samples(
    pipeline: &gst::Pipeline,
    sink: &gst_app::AppSink,
    state: &Arc<Mutex<LiveStreamState>>,
    shutdown: &AtomicBool,
) -> Result<LiveStreamPumpExit, MediaProducerError> {
    let mut last_sample_at = Instant::now();
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(LiveStreamPumpExit::Shutdown);
        }
        if let Some(sample) = sink.try_pull_sample(LIVE_STREAM_FRAME_TIMEOUT) {
            record_live_stream_frame(state, decoded_frame_from_sample(&sample)?);
            last_sample_at = Instant::now();
            continue;
        }
        if sink.is_eos() {
            return Ok(LiveStreamPumpExit::Retry);
        }
        if let Some(error) = pipeline_error(pipeline) {
            return Err(MediaProducerError::VideoDecode(error));
        }
        if last_sample_at.elapsed() >= LIVE_STREAM_STALL_TIMEOUT {
            return Err(MediaProducerError::VideoDecode(
                "timed out waiting for a live stream frame".to_owned(),
            ));
        }
    }
}

#[cfg(feature = "media-video")]
fn record_live_stream_frame(state: &Arc<Mutex<LiveStreamState>>, frame: DecodedMediaFrame) {
    if let Ok(mut state) = state.lock() {
        state.latest_frame = Some(frame);
        state.frames_seen = state.frames_seen.saturating_add(1);
        state.last_error = None;
    }
}

#[cfg(feature = "media-video")]
fn record_live_stream_error(state: &Arc<Mutex<LiveStreamState>>, reason: impl Into<String>) {
    if let Ok(mut state) = state.lock() {
        state.last_error = Some(reason.into());
    }
}

#[cfg(feature = "media-video")]
fn live_stream_frames_seen(state: &Arc<Mutex<LiveStreamState>>) -> usize {
    state.lock().map_or(0, |state| state.frames_seen)
}

#[cfg(feature = "media-video")]
fn sleep_live_stream_backoff(delay: Duration, shutdown: &AtomicBool) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep((deadline - now).min(LIVE_STREAM_SHUTDOWN_POLL));
    }
}

#[cfg(feature = "media-video")]
#[derive(Debug, Default)]
struct LiveStreamBackoff {
    failures: usize,
}

#[cfg(feature = "media-video")]
impl LiveStreamBackoff {
    fn reset_if_frames_advanced(
        &mut self,
        frames_before: usize,
        state: &Arc<Mutex<LiveStreamState>>,
    ) {
        if live_stream_frames_seen(state) > frames_before {
            self.failures = 0;
        }
    }

    fn next_delay(&mut self) -> Duration {
        let index = self
            .failures
            .min(LIVE_STREAM_RECONNECT_BACKOFF_MS.len().saturating_sub(1));
        self.failures = self.failures.saturating_add(1);
        Duration::from_millis(LIVE_STREAM_RECONNECT_BACKOFF_MS[index])
    }
}

#[cfg(feature = "media-video")]
fn pipeline_error(pipeline: &gst::Pipeline) -> Option<String> {
    let bus = pipeline.bus()?;
    while let Some(message) = bus.timed_pop(gst::ClockTime::ZERO) {
        if let gst::MessageView::Error(error) = message.view() {
            return Some(error.error().to_string());
        }
    }
    None
}

#[cfg(feature = "media-video")]
fn decoded_frame_from_sample(
    sample: &gst::Sample,
) -> Result<DecodedMediaFrame, MediaProducerError> {
    let caps = sample
        .caps()
        .ok_or_else(|| MediaProducerError::VideoDecode("decoded sample has no caps".into()))?;
    let info = gst_video::VideoInfo::from_caps(caps)
        .map_err(|error| MediaProducerError::VideoDecode(error.to_string()))?;
    let buffer = sample
        .buffer()
        .ok_or_else(|| MediaProducerError::VideoDecode("decoded sample has no buffer".into()))?;
    let map = buffer
        .map_readable()
        .map_err(|error| MediaProducerError::VideoDecode(format!("{error:?}")))?;

    let canvas = canvas_from_rgba_sample(map.as_slice(), &info)?;
    let duration_us = buffer
        .duration()
        .map(clock_time_to_us)
        .unwrap_or(DEFAULT_FRAME_DURATION_US);
    Ok(DecodedMediaFrame {
        canvas,
        duration_us,
    })
}

#[cfg(feature = "media-video")]
fn canvas_from_rgba_sample(
    sample: &[u8],
    info: &gst_video::VideoInfo,
) -> Result<Canvas, MediaProducerError> {
    let width = usize::try_from(info.width())
        .map_err(|error| MediaProducerError::VideoDecode(error.to_string()))?;
    let height = usize::try_from(info.height())
        .map_err(|error| MediaProducerError::VideoDecode(error.to_string()))?;
    let stride =
        info.stride().first().copied().ok_or_else(|| {
            MediaProducerError::VideoDecode("decoded sample has no stride".into())
        })?;
    let stride = usize::try_from(stride)
        .map_err(|error| MediaProducerError::VideoDecode(error.to_string()))?;
    let row_len = width
        .checked_mul(4)
        .ok_or_else(|| MediaProducerError::VideoDecode("decoded frame row is too wide".into()))?;
    let required_len = stride.checked_mul(height).ok_or_else(|| {
        MediaProducerError::VideoDecode("decoded frame buffer is too large".into())
    })?;
    if sample.len() < required_len {
        return Err(MediaProducerError::VideoDecode(
            "decoded frame buffer is shorter than its caps".to_owned(),
        ));
    }

    let mut rgba = Vec::with_capacity(row_len * height);
    for row in sample.chunks(stride).take(height) {
        rgba.extend_from_slice(&row[..row_len]);
    }
    Ok(Canvas::from_vec(rgba, info.width(), info.height()))
}

#[cfg(feature = "media-video")]
fn clock_time_to_us(time: gst::ClockTime) -> u64 {
    time.nseconds().saturating_div(1_000).max(1)
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
