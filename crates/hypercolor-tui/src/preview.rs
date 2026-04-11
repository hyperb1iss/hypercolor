//! Preview transport manager for the live canvas overlay.

mod halfblocks_fast;
mod kitty_fast;

use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self as std_mpsc, Receiver as StdReceiver, Sender as StdSender};
use std::thread;
use std::time::{Duration, Instant};

use image::imageops::FilterType;
use image::DynamicImage;
use image::GenericImageView;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, ResizeEncodeRender};

use crate::state::CanvasFrame;

static NEXT_KITTY_IMAGE_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewTransport {
    Primary,
    Halfblocks,
}

#[derive(Debug, Clone, Copy)]
struct PreviewPolicy {
    windowed: PreviewTransport,
    fullscreen: PreviewTransport,
    max_primary_rgba_bytes: usize,
    max_primary_scale_tenths: u16,
    medium_primary_rgba_bytes: usize,
    large_primary_rgba_bytes: usize,
    default_frame_interval: Duration,
    medium_primary_frame_interval: Duration,
    large_primary_frame_interval: Duration,
}

impl PreviewPolicy {
    fn transport_for(
        self,
        fullscreen: bool,
        primary_protocol: ProtocolType,
        font_size: (u16, u16),
        frame: Option<&CanvasFrame>,
        area: Option<Rect>,
    ) -> PreviewTransport {
        let base = if fullscreen {
            self.fullscreen
        } else {
            self.windowed
        };

        if base != PreviewTransport::Primary {
            return base;
        }

        let (Some(frame), Some(area)) = (frame, area) else {
            return base;
        };

        if primary_protocol == ProtocolType::Kitty {
            return base;
        }

        let target_rgba_bytes = Self::target_rgba_bytes(font_size, area);

        if target_rgba_bytes > self.max_primary_rgba_bytes {
            return PreviewTransport::Halfblocks;
        }

        let desired_width = u32::from(frame.width).div_ceil(u32::from(font_size.0.max(1)));
        let desired_height = u32::from(frame.height).div_ceil(u32::from(font_size.1.max(1)));

        if u32::from(area.width) * 10 > desired_width * u32::from(self.max_primary_scale_tenths)
            || u32::from(area.height) * 10
                > desired_height * u32::from(self.max_primary_scale_tenths)
        {
            return PreviewTransport::Halfblocks;
        }

        base
    }

    fn frame_interval_for(
        self,
        transport: PreviewTransport,
        primary_protocol: ProtocolType,
        font_size: (u16, u16),
        area: Option<Rect>,
    ) -> Duration {
        if transport != PreviewTransport::Primary {
            return self.default_frame_interval;
        }

        if primary_protocol == ProtocolType::Kitty {
            return self.default_frame_interval;
        }

        let Some(area) = area else {
            return self.default_frame_interval;
        };

        let target_rgba_bytes = Self::target_rgba_bytes(font_size, area);
        if target_rgba_bytes >= self.large_primary_rgba_bytes {
            self.large_primary_frame_interval
        } else if target_rgba_bytes >= self.medium_primary_rgba_bytes {
            self.medium_primary_frame_interval
        } else {
            self.default_frame_interval
        }
    }

    fn target_rgba_bytes(font_size: (u16, u16), area: Rect) -> usize {
        let char_width = usize::from(font_size.0.max(1));
        let char_height = usize::from(font_size.1.max(1));
        usize::from(area.width) * usize::from(area.height) * char_width * char_height * 4
    }
}

impl Default for PreviewPolicy {
    fn default() -> Self {
        Self {
            windowed: PreviewTransport::Primary,
            fullscreen: PreviewTransport::Primary,
            max_primary_rgba_bytes: 768 * 1024,
            max_primary_scale_tenths: 15,
            medium_primary_rgba_bytes: 256 * 1024,
            large_primary_rgba_bytes: 512 * 1024,
            default_frame_interval: Duration::from_millis(33),
            medium_primary_frame_interval: Duration::from_millis(167),
            large_primary_frame_interval: Duration::from_millis(250),
        }
    }
}

struct PreviewBuildRequest {
    request_id: u64,
    frame: Arc<CanvasFrame>,
    transport: PreviewTransport,
    area: Rect,
    fullscreen: bool,
}

struct PreviewBuildResponse {
    request_id: u64,
    frame_number: u32,
    transport: PreviewTransport,
    area: Rect,
    build_duration: Duration,
    surface: PreviewSurface,
}

enum PreviewBuildResult {
    Ready(PreviewBuildResponse),
    Failed {
        request_id: u64,
        frame_number: u32,
        transport: PreviewTransport,
        area: Rect,
        build_duration: Duration,
        error: String,
    },
}

enum PreviewSurface {
    Stateful(StatefulSurface),
    Halfblocks(halfblocks_fast::HalfblocksFrame),
    Kitty(kitty_fast::KittyFrame),
}

impl PreviewSurface {
    fn kind(&self) -> &'static str {
        match self {
            Self::Stateful(_) => "stateful",
            Self::Halfblocks(_) => "halfblocks",
            Self::Kitty(_) => "kitty_fast",
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        match self {
            Self::Stateful(protocol) => protocol.render(area, buf),
            Self::Halfblocks(frame) => frame.render(area, buf),
            Self::Kitty(frame) => frame.render(area, buf),
        }
    }

    fn matches_area(&self, resize_area: Rect) -> bool {
        match self {
            Self::Stateful(surface) => surface.matches_area(resize_area),
            Self::Halfblocks(frame) => frame.area() == resize_area,
            Self::Kitty(frame) => frame.area() == resize_area,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatefulResizeMode {
    Scale,
    Cover,
}

impl StatefulResizeMode {
    fn resize(self) -> Resize {
        match self {
            Self::Scale => Resize::Scale(None),
            Self::Cover => Resize::Crop(None),
        }
    }
}

struct StatefulSurface {
    protocol: StatefulProtocol,
    resize_mode: StatefulResizeMode,
}

impl StatefulSurface {
    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.protocol.render(area, buf);
    }

    fn matches_area(&self, resize_area: Rect) -> bool {
        self.protocol
            .needs_resize(&self.resize_mode.resize(), resize_area)
            .is_none()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DrawBackpressure {
    pressure_level: u8,
    fast_draw_streak: u8,
}

impl DrawBackpressure {
    fn observe_draw(&mut self, elapsed: Duration) {
        let next_level = if elapsed >= Duration::from_millis(250) {
            3
        } else if elapsed >= Duration::from_millis(125) {
            2
        } else if elapsed >= Duration::from_millis(60) {
            1
        } else {
            0
        };

        if next_level > 0 {
            self.pressure_level = self.pressure_level.max(next_level);
            self.fast_draw_streak = 0;
            return;
        }

        if self.pressure_level == 0 {
            return;
        }

        self.fast_draw_streak = self.fast_draw_streak.saturating_add(1);
        if self.fast_draw_streak >= 2 {
            self.pressure_level = self.pressure_level.saturating_sub(1);
            self.fast_draw_streak = 0;
        }
    }

    fn frame_interval(self) -> Duration {
        match self.pressure_level {
            0 => Duration::ZERO,
            1 => Duration::from_millis(167),
            2 => Duration::from_millis(250),
            _ => Duration::from_millis(500),
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct BuildBackpressure {
    pressure_level: u8,
    fast_build_streak: u8,
}

impl BuildBackpressure {
    fn observe_build(&mut self, elapsed: Duration) {
        let next_level = if elapsed >= Duration::from_millis(120) {
            3
        } else if elapsed >= Duration::from_millis(80) {
            2
        } else if elapsed >= Duration::from_millis(40) {
            1
        } else {
            0
        };

        if next_level > 0 {
            self.pressure_level = self.pressure_level.max(next_level);
            self.fast_build_streak = 0;
            return;
        }

        if self.pressure_level == 0 {
            return;
        }

        self.fast_build_streak = self.fast_build_streak.saturating_add(1);
        if self.fast_build_streak >= 2 {
            self.pressure_level = self.pressure_level.saturating_sub(1);
            self.fast_build_streak = 0;
        }
    }

    fn frame_interval(self) -> Duration {
        match self.pressure_level {
            0 => Duration::ZERO,
            1 => Duration::from_millis(50),
            2 => Duration::from_millis(83),
            _ => Duration::from_millis(125),
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug)]
struct PreviewTelemetry {
    enabled: bool,
    last_log_at: Instant,
    received_frames: u32,
    queued_builds: u32,
    completed_builds: u32,
    build_failures: u32,
    stale_results: u32,
    busy_skips: u32,
    interval_skips: u32,
    presented_frames: u32,
    draw_calls: u32,
    last_received_frame: Option<u32>,
    last_completed_frame: Option<u32>,
    last_presented_frame: Option<u32>,
    last_transport: Option<PreviewTransport>,
    last_area: Option<Rect>,
    last_frame_interval: Duration,
    last_build_duration: Duration,
    max_build_duration: Duration,
    last_draw_duration: Duration,
    max_draw_duration: Duration,
}

impl PreviewTelemetry {
    fn new() -> Self {
        Self {
            enabled: env::var("HYPERCOLOR_TUI_PREVIEW_TELEMETRY").is_ok_and(|value| value != "0"),
            last_log_at: Instant::now(),
            received_frames: 0,
            queued_builds: 0,
            completed_builds: 0,
            build_failures: 0,
            stale_results: 0,
            busy_skips: 0,
            interval_skips: 0,
            presented_frames: 0,
            draw_calls: 0,
            last_received_frame: None,
            last_completed_frame: None,
            last_presented_frame: None,
            last_transport: None,
            last_area: None,
            last_frame_interval: Duration::ZERO,
            last_build_duration: Duration::ZERO,
            max_build_duration: Duration::ZERO,
            last_draw_duration: Duration::ZERO,
            max_draw_duration: Duration::ZERO,
        }
    }

    fn note_frame_received(&mut self, frame_number: u32) {
        if !self.enabled {
            return;
        }

        self.received_frames = self.received_frames.saturating_add(1);
        self.last_received_frame = Some(frame_number);
    }

    fn note_build_queued(
        &mut self,
        transport: PreviewTransport,
        area: Rect,
        frame_interval: Duration,
    ) {
        if !self.enabled {
            return;
        }

        self.queued_builds = self.queued_builds.saturating_add(1);
        self.last_transport = Some(transport);
        self.last_area = Some(area);
        self.last_frame_interval = frame_interval;
    }

    fn note_build_completed(
        &mut self,
        frame_number: u32,
        transport: PreviewTransport,
        area: Rect,
        build_duration: Duration,
    ) {
        if !self.enabled {
            return;
        }

        self.completed_builds = self.completed_builds.saturating_add(1);
        self.last_completed_frame = Some(frame_number);
        self.last_transport = Some(transport);
        self.last_area = Some(area);
        self.last_build_duration = build_duration;
        self.max_build_duration = self.max_build_duration.max(build_duration);
    }

    fn note_build_failed(
        &mut self,
        transport: PreviewTransport,
        area: Rect,
        build_duration: Duration,
    ) {
        if !self.enabled {
            return;
        }

        self.build_failures = self.build_failures.saturating_add(1);
        self.last_transport = Some(transport);
        self.last_area = Some(area);
        self.last_build_duration = build_duration;
        self.max_build_duration = self.max_build_duration.max(build_duration);
    }

    fn note_stale_result(&mut self) {
        if !self.enabled {
            return;
        }

        self.stale_results = self.stale_results.saturating_add(1);
    }

    fn note_busy_skip(&mut self) {
        if !self.enabled {
            return;
        }

        self.busy_skips = self.busy_skips.saturating_add(1);
    }

    fn note_interval_skip(&mut self) {
        if !self.enabled {
            return;
        }

        self.interval_skips = self.interval_skips.saturating_add(1);
    }

    fn note_presented(&mut self, frame_number: u32, draw_duration: Duration) {
        if !self.enabled {
            return;
        }

        self.draw_calls = self.draw_calls.saturating_add(1);
        self.last_draw_duration = draw_duration;
        self.max_draw_duration = self.max_draw_duration.max(draw_duration);

        if self.last_presented_frame != Some(frame_number) {
            self.presented_frames = self.presented_frames.saturating_add(1);
            self.last_presented_frame = Some(frame_number);
        }
    }

    fn reset_window(&mut self) {
        self.received_frames = 0;
        self.queued_builds = 0;
        self.completed_builds = 0;
        self.build_failures = 0;
        self.stale_results = 0;
        self.busy_skips = 0;
        self.interval_skips = 0;
        self.presented_frames = 0;
        self.draw_calls = 0;
        self.max_build_duration = self.last_build_duration;
        self.max_draw_duration = self.last_draw_duration;
    }
}

pub(crate) struct PreviewManager {
    primary_picker: Picker,
    primary_protocol: ProtocolType,
    policy: PreviewPolicy,
    build_tx: Option<StdSender<PreviewBuildRequest>>,
    build_rx: StdReceiver<PreviewBuildResult>,
    build_worker: Option<thread::JoinHandle<()>>,
    current: Option<PreviewSurface>,
    current_frame_number: Option<u32>,
    current_transport: Option<PreviewTransport>,
    preview_area: Option<Rect>,
    latest_frame: Option<Arc<CanvasFrame>>,
    fullscreen: bool,
    selected_transport: PreviewTransport,
    next_request_id: u64,
    in_flight_request_id: Option<u64>,
    last_build_started: Option<Instant>,
    build_backpressure: BuildBackpressure,
    draw_backpressure: DrawBackpressure,
    primary_cooloff_until: Option<Instant>,
    fullscreen_primary_locked_out: bool,
    telemetry: PreviewTelemetry,
}

impl PreviewManager {
    pub(crate) fn new(primary_picker: Picker) -> Self {
        let (build_tx, build_requests) = std_mpsc::channel::<PreviewBuildRequest>();
        let (build_results_tx, build_rx) = std_mpsc::channel::<PreviewBuildResult>();

        let halfblocks_picker = Picker::halfblocks();
        let build_primary_protocol = primary_picker.protocol_type();
        let build_primary_picker = primary_picker.clone();
        let build_halfblocks_picker = halfblocks_picker.clone();
        let build_use_kitty_fast = build_primary_protocol == ProtocolType::Kitty;
        let build_use_kitty_fast_fullscreen =
            env::var("HYPERCOLOR_TUI_KITTY_FAST_FULLSCREEN").is_ok_and(|value| value == "1");
        let build_use_fast_halfblocks = build_primary_protocol == ProtocolType::Halfblocks;
        let build_is_tmux = env::var("TERM").is_ok_and(|term| term.starts_with("tmux"))
            || env::var("TERM_PROGRAM").is_ok_and(|term_program| term_program == "tmux");
        let kitty_image_id = NEXT_KITTY_IMAGE_ID.fetch_add(1, Ordering::Relaxed).max(1);
        let build_worker = thread::Builder::new()
            .name("hypercolor-tui-preview".to_string())
            .spawn(move || {
                while let Ok(request) = build_requests.recv() {
                    let build_started = Instant::now();

                    if request.transport == PreviewTransport::Halfblocks
                        || build_use_fast_halfblocks
                    {
                        match halfblocks_fast::HalfblocksFrame::new(
                            request.frame.as_ref(),
                            request.area,
                        ) {
                            Ok(frame) => {
                                if build_results_tx
                                    .send(PreviewBuildResult::Ready(PreviewBuildResponse {
                                        request_id: request.request_id,
                                        frame_number: request.frame.frame_number,
                                        transport: request.transport,
                                        area: request.area,
                                        build_duration: build_started.elapsed(),
                                        surface: PreviewSurface::Halfblocks(frame),
                                    }))
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(error) => {
                                if build_results_tx
                                    .send(PreviewBuildResult::Failed {
                                        request_id: request.request_id,
                                        frame_number: request.frame.frame_number,
                                        transport: request.transport,
                                        area: request.area,
                                        build_duration: build_started.elapsed(),
                                        error,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                        continue;
                    }

                    if request.transport == PreviewTransport::Primary
                        && build_use_kitty_fast
                        && (!request.fullscreen || build_use_kitty_fast_fullscreen)
                    {
                        let img = match build_preview_image(
                            request.frame.as_ref(),
                            request.area,
                            build_primary_picker.font_size(),
                            StatefulResizeMode::Cover,
                            request.fullscreen,
                        ) {
                            Ok(img) => img,
                            Err(error) => {
                                if build_results_tx
                                    .send(PreviewBuildResult::Failed {
                                        request_id: request.request_id,
                                        frame_number: request.frame.frame_number,
                                        transport: request.transport,
                                        area: request.area,
                                        build_duration: build_started.elapsed(),
                                        error,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        };
                        match kitty_fast::KittyFrame::new(
                            img.into_rgb8(),
                            request.area,
                            kitty_image_id,
                            build_is_tmux,
                        ) {
                            Ok(frame) => {
                                if build_results_tx
                                    .send(PreviewBuildResult::Ready(PreviewBuildResponse {
                                        request_id: request.request_id,
                                        frame_number: request.frame.frame_number,
                                        transport: request.transport,
                                        area: request.area,
                                        build_duration: build_started.elapsed(),
                                        surface: PreviewSurface::Kitty(frame),
                                    }))
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(error) => {
                                if build_results_tx
                                    .send(PreviewBuildResult::Failed {
                                        request_id: request.request_id,
                                        frame_number: request.frame.frame_number,
                                        transport: request.transport,
                                        area: request.area,
                                        build_duration: build_started.elapsed(),
                                        error,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                        continue;
                    }

                    let picker = match request.transport {
                        PreviewTransport::Primary => &build_primary_picker,
                        PreviewTransport::Halfblocks => &build_halfblocks_picker,
                    };
                    let resize_mode =
                        primary_resize_mode(request.transport, build_primary_protocol);
                    let img = match build_preview_image(
                        request.frame.as_ref(),
                        request.area,
                        picker.font_size(),
                        resize_mode,
                        false,
                    ) {
                        Ok(img) => img,
                        Err(error) => {
                            if build_results_tx
                                .send(PreviewBuildResult::Failed {
                                    request_id: request.request_id,
                                    frame_number: request.frame.frame_number,
                                    transport: request.transport,
                                    area: request.area,
                                    build_duration: build_started.elapsed(),
                                    error,
                                })
                                .is_err()
                            {
                                break;
                            }
                            continue;
                        }
                    };

                    let mut protocol = picker.new_resize_protocol(img);
                    let resize = resize_mode.resize();

                    if let Some(target_rect) = protocol.needs_resize(&resize, request.area) {
                        protocol.resize_encode(&resize, target_rect);
                        if let Some(Err(error)) = protocol.last_encoding_result() {
                            if build_results_tx
                                .send(PreviewBuildResult::Failed {
                                    request_id: request.request_id,
                                    frame_number: request.frame.frame_number,
                                    transport: request.transport,
                                    area: request.area,
                                    build_duration: build_started.elapsed(),
                                    error: error.to_string(),
                                })
                                .is_err()
                            {
                                break;
                            }
                            continue;
                        }
                    }

                    if build_results_tx
                        .send(PreviewBuildResult::Ready(PreviewBuildResponse {
                            request_id: request.request_id,
                            frame_number: request.frame.frame_number,
                            transport: request.transport,
                            area: request.area,
                            build_duration: build_started.elapsed(),
                            surface: PreviewSurface::Stateful(StatefulSurface {
                                protocol,
                                resize_mode,
                            }),
                        }))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .ok();

        let policy = PreviewPolicy::default();

        Self {
            primary_picker,
            primary_protocol: build_primary_protocol,
            policy,
            build_tx: Some(build_tx),
            build_rx,
            build_worker,
            current: None,
            current_frame_number: None,
            current_transport: None,
            preview_area: None,
            latest_frame: None,
            fullscreen: false,
            selected_transport: PreviewTransport::Primary,
            next_request_id: 0,
            in_flight_request_id: None,
            last_build_started: None,
            build_backpressure: BuildBackpressure::default(),
            draw_backpressure: DrawBackpressure::default(),
            primary_cooloff_until: None,
            fullscreen_primary_locked_out: false,
            telemetry: PreviewTelemetry::new(),
        }
    }

    pub(crate) fn on_frame(&mut self, frame: Arc<CanvasFrame>, fullscreen: bool) {
        self.fullscreen = fullscreen;
        self.telemetry.note_frame_received(frame.frame_number);
        let next_transport = self.transport_for(Some(frame.as_ref()), self.preview_area);
        if next_transport != self.selected_transport {
            self.selected_transport = next_transport;
            self.invalidate_current();
        }
        self.latest_frame = Some(frame);
        self.maybe_queue_build();
        self.maybe_log_telemetry();
    }

    pub(crate) fn set_fullscreen(&mut self, fullscreen: bool) {
        if self.fullscreen != fullscreen {
            self.fullscreen_primary_locked_out = false;
        }
        self.fullscreen = fullscreen;
        let next_transport = self.transport_for(self.latest_frame.as_deref(), self.preview_area);
        if next_transport != self.selected_transport {
            self.selected_transport = next_transport;
            self.invalidate_current();
        }
        self.maybe_queue_build();
    }

    pub(crate) fn clear(&mut self) {
        self.latest_frame = None;
        self.reset_protocols();
    }

    pub(crate) fn shutdown(&mut self) {
        self.clear();
        self.build_tx = None;

        if let Some(worker) = self.build_worker.take()
            && let Err(error) = worker.join()
        {
            tracing::warn!("preview worker panicked during shutdown: {error:?}");
        }
    }

    pub(crate) fn drain_resize_results(&mut self) -> bool {
        let mut dirty = false;

        while let Ok(result) = self.build_rx.try_recv() {
            match result {
                PreviewBuildResult::Ready(completed) => {
                    if self.in_flight_request_id == Some(completed.request_id) {
                        dirty = true;
                        self.telemetry.note_build_completed(
                            completed.frame_number,
                            completed.transport,
                            completed.area,
                            completed.build_duration,
                        );
                        if completed.transport == PreviewTransport::Primary {
                            self.build_backpressure.observe_build(completed.build_duration);
                        }
                        self.current = Some(completed.surface);
                        self.current_frame_number = Some(completed.frame_number);
                        self.current_transport = Some(completed.transport);
                        self.in_flight_request_id = None;
                    } else {
                        self.telemetry.note_stale_result();
                    }
                }
                PreviewBuildResult::Failed {
                    request_id,
                    frame_number,
                    transport,
                    area,
                    build_duration,
                    error,
                } => {
                    if self.in_flight_request_id == Some(request_id) {
                        self.in_flight_request_id = None;
                        self.telemetry
                            .note_build_failed(transport, area, build_duration);
                        if transport == PreviewTransport::Primary {
                            self.build_backpressure.observe_build(build_duration);
                        }
                    } else {
                        self.telemetry.note_stale_result();
                    }
                    tracing::debug!("preview build failed for frame {frame_number}: {error}");
                }
            }
        }

        if self.in_flight_request_id.is_none() {
            self.maybe_queue_build();
        }

        self.maybe_log_telemetry();
        dirty
    }

    pub(crate) fn render(&mut self, area: Option<Rect>, buf: &mut Buffer) {
        let Some(area) = area.filter(|area| area.width > 0 && area.height > 0) else {
            self.preview_area = None;
            return;
        };

        if let Some(frame) = self.latest_frame.clone() {
            let next_transport = self.transport_for(Some(frame.as_ref()), Some(area));
            if next_transport != self.selected_transport {
                self.selected_transport = next_transport;
                self.invalidate_current();
            }
        }

        self.preview_area = Some(area);
        self.maybe_queue_build();

        if let Some(surface) = self.current.as_mut() {
            surface.render(area, buf);
        }
    }

    #[must_use]
    pub(crate) fn has_current_frame(&self) -> bool {
        self.current.is_some()
    }

    pub(crate) fn note_draw_duration(&mut self, elapsed: Duration) {
        if let Some(frame_number) = self.current_frame_number {
            self.telemetry.note_presented(frame_number, elapsed);
        }

        if self.latest_frame.is_none() || self.selected_transport != PreviewTransport::Primary {
            self.build_backpressure.reset();
            self.draw_backpressure.reset();
            self.maybe_log_telemetry();
            return;
        }

        self.draw_backpressure.observe_draw(elapsed);
        if self.draw_backpressure.pressure_level >= 3 {
            self.primary_cooloff_until = Some(Instant::now() + Duration::from_secs(2));
            if self.fullscreen {
                self.fullscreen_primary_locked_out = true;
                self.selected_transport = PreviewTransport::Halfblocks;
                self.invalidate_current();
            }
        }
        self.maybe_log_telemetry();
    }

    fn maybe_queue_build(&mut self) {
        if self.in_flight_request_id.is_some() {
            self.telemetry.note_busy_skip();
            return;
        }

        let Some(frame) = self.latest_frame.clone() else {
            return;
        };
        let Some(area) = self
            .preview_area
            .filter(|area| area.width > 0 && area.height > 0)
        else {
            return;
        };
        let resize_area = Self::resize_area(area);
        let transport = self.transport_for(Some(frame.as_ref()), Some(area));
        self.selected_transport = transport;
        let frame_interval = std::cmp::max(
            self.policy.frame_interval_for(
                transport,
                self.primary_protocol,
                self.primary_picker.font_size(),
                Some(area),
            ),
            self.build_backpressure.frame_interval(),
        );
        let frame_interval = std::cmp::max(
            frame_interval,
            self.draw_backpressure.frame_interval(),
        );

        if self.current_frame_number == Some(frame.frame_number)
            && self.current_transport == Some(transport)
            && self
                .current
                .as_ref()
                .is_some_and(|surface| surface.matches_area(resize_area))
        {
            return;
        }

        if self.current.is_some()
            && self
                .last_build_started
                .is_some_and(|started| started.elapsed() < frame_interval)
        {
            self.telemetry.note_interval_skip();
            return;
        }

        let Some(build_tx) = self.build_tx.as_ref() else {
            return;
        };

        self.next_request_id = self.next_request_id.wrapping_add(1);
        let request_id = self.next_request_id;
        if build_tx
            .send(PreviewBuildRequest {
                request_id,
                frame,
                transport,
                area: resize_area,
                fullscreen: self.fullscreen,
            })
            .is_ok()
        {
            self.telemetry
                .note_build_queued(transport, resize_area, frame_interval);
            self.in_flight_request_id = Some(request_id);
            self.last_build_started = Some(Instant::now());
        }
    }

    fn transport_for(&self, frame: Option<&CanvasFrame>, area: Option<Rect>) -> PreviewTransport {
        let transport = self.policy.transport_for(
            self.fullscreen,
            self.primary_protocol,
            self.primary_picker.font_size(),
            frame,
            area,
        );

        if self.fullscreen && self.fullscreen_primary_locked_out {
            return PreviewTransport::Halfblocks;
        }

        if transport == PreviewTransport::Primary
            && self
                .primary_cooloff_until
                .is_some_and(|until| until > Instant::now())
        {
            PreviewTransport::Halfblocks
        } else {
            transport
        }
    }

    fn maybe_log_telemetry(&mut self) {
        if !self.telemetry.enabled || self.telemetry.last_log_at.elapsed() < Duration::from_secs(1)
        {
            return;
        }

        let area = self.telemetry.last_area.or(self.preview_area);
        let latest_frame = self
            .latest_frame
            .as_ref()
            .map_or(0, |frame| frame.frame_number);
        let current_frame = self.current_frame_number.unwrap_or(0);
        let frame_lag = latest_frame.saturating_sub(current_frame);
        let inflight_ms = if self.in_flight_request_id.is_some() {
            self.last_build_started
                .map_or(0, |started| started.elapsed().as_millis())
        } else {
            0
        };
        let cooloff_ms = self.primary_cooloff_until.map_or(0, |until| {
            until.saturating_duration_since(Instant::now()).as_millis()
        });

        tracing::info!(
            target: "hypercolor_tui::preview",
            protocol = ?self.primary_protocol,
            selected_transport = ?self.selected_transport,
            current_transport = ?self.current_transport,
            current_surface = self.current.as_ref().map_or("none", PreviewSurface::kind),
            fullscreen = self.fullscreen,
            fullscreen_primary_locked_out = self.fullscreen_primary_locked_out,
            area_width = area.map_or(0, |rect| rect.width),
            area_height = area.map_or(0, |rect| rect.height),
            recv_fps = self.telemetry.received_frames,
            queued_fps = self.telemetry.queued_builds,
            build_fps = self.telemetry.completed_builds,
            present_fps = self.telemetry.presented_frames,
            draws_per_sec = self.telemetry.draw_calls,
            busy_skips = self.telemetry.busy_skips,
            interval_skips = self.telemetry.interval_skips,
            stale_results = self.telemetry.stale_results,
            build_failures = self.telemetry.build_failures,
            last_received = self.telemetry.last_received_frame.unwrap_or(0),
            last_completed = self.telemetry.last_completed_frame.unwrap_or(0),
            last_presented = self.telemetry.last_presented_frame.unwrap_or(0),
            latest_frame,
            current_frame,
            frame_lag,
            inflight_ms,
            draw_pressure = self.draw_backpressure.pressure_level,
            build_pressure = self.build_backpressure.pressure_level,
            cooloff_ms,
            frame_interval_ms = self.telemetry.last_frame_interval.as_millis(),
            last_build_ms = self.telemetry.last_build_duration.as_millis(),
            max_build_ms = self.telemetry.max_build_duration.as_millis(),
            last_draw_ms = self.telemetry.last_draw_duration.as_millis(),
            max_draw_ms = self.telemetry.max_draw_duration.as_millis(),
            "preview telemetry"
        );

        self.telemetry.last_log_at = Instant::now();
        self.telemetry.reset_window();
    }

    fn invalidate_current(&mut self) {
        self.current = None;
        self.current_frame_number = None;
        self.current_transport = None;
        self.in_flight_request_id = None;
        self.last_build_started = None;
    }

    fn reset_protocols(&mut self) {
        self.invalidate_current();
        self.preview_area = None;
        self.build_backpressure.reset();
        self.draw_backpressure.reset();
        self.primary_cooloff_until = None;
        self.fullscreen_primary_locked_out = false;
    }

    fn resize_area(area: Rect) -> Rect {
        Rect::new(0, 0, area.width, area.height)
    }
}

fn primary_resize_mode(
    transport: PreviewTransport,
    primary_protocol: ProtocolType,
) -> StatefulResizeMode {
    if transport == PreviewTransport::Primary && primary_protocol == ProtocolType::Kitty {
        StatefulResizeMode::Cover
    } else {
        StatefulResizeMode::Scale
    }
}

fn build_preview_image(
    frame: &CanvasFrame,
    area: Rect,
    font_size: (u16, u16),
    resize_mode: StatefulResizeMode,
    fullscreen: bool,
) -> Result<DynamicImage, String> {
    let Some(img) = image::RgbImage::from_raw(
        u32::from(frame.width),
        u32::from(frame.height),
        frame.pixels.as_ref().clone(),
    ) else {
        return Err("invalid preview frame length".to_string());
    };

    if resize_mode != StatefulResizeMode::Cover || area.width == 0 || area.height == 0 {
        return Ok(DynamicImage::ImageRgb8(img));
    }

    let target_width = u32::from(area.width) * u32::from(font_size.0.max(1));
    let target_height = u32::from(area.height) * u32::from(font_size.1.max(1));
    if target_width == 0 || target_height == 0 {
        return Ok(DynamicImage::ImageRgb8(img));
    }

    let image = DynamicImage::ImageRgb8(img);
    let (source_width, source_height) = image.dimensions();
    if source_width == target_width && source_height == target_height {
        return Ok(image);
    }

    let filter = if fullscreen {
        FilterType::Nearest
    } else {
        FilterType::Triangle
    };
    Ok(image.resize_to_fill(target_width, target_height, filter))
}

#[cfg(test)]
mod tests {
    use super::{
        BuildBackpressure, DrawBackpressure, PreviewPolicy, PreviewTransport,
        StatefulResizeMode, build_preview_image, primary_resize_mode,
    };
    use crate::state::CanvasFrame;
    use ratatui::layout::Rect;
    use ratatui_image::picker::ProtocolType;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn default_policy_uses_primary_windowed_and_halfblocks_fullscreen() {
        let policy = PreviewPolicy::default();

        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 320,
            height: 200,
            pixels: Arc::new(vec![0; 320 * 200 * 3]),
        };

        assert_eq!(
            policy.transport_for(
                false,
                ProtocolType::Halfblocks,
                (9, 18),
                Some(&frame),
                Some(Rect::new(0, 0, 30, 12)),
            ),
            PreviewTransport::Primary
        );
        assert_eq!(
            policy.transport_for(
                true,
                ProtocolType::Halfblocks,
                (9, 18),
                Some(&frame),
                Some(Rect::new(0, 0, 30, 12)),
            ),
            PreviewTransport::Primary
        );
    }

    #[test]
    fn large_windowed_preview_falls_back_to_halfblocks() {
        let policy = PreviewPolicy::default();
        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 320,
            height: 200,
            pixels: Arc::new(vec![0; 320 * 200 * 3]),
        };

        assert_eq!(
            policy.transport_for(
                false,
                ProtocolType::Halfblocks,
                (9, 18),
                Some(&frame),
                Some(Rect::new(0, 0, 52, 24)),
            ),
            PreviewTransport::Halfblocks
        );
    }

    #[test]
    fn aggressive_upscale_falls_back_even_below_byte_budget() {
        let policy = PreviewPolicy::default();
        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 320,
            height: 200,
            pixels: Arc::new(vec![0; 320 * 200 * 3]),
        };

        assert_eq!(
            policy.transport_for(
                false,
                ProtocolType::Halfblocks,
                (9, 18),
                Some(&frame),
                Some(Rect::new(0, 0, 55, 16)),
            ),
            PreviewTransport::Halfblocks
        );
    }

    #[test]
    fn kitty_primary_uses_default_frame_interval_even_for_large_area() {
        let policy = PreviewPolicy::default();

        assert_eq!(
            policy.frame_interval_for(
                PreviewTransport::Primary,
                ProtocolType::Kitty,
                (14, 34),
                Some(Rect::new(0, 0, 37, 14)),
            ),
            Duration::from_millis(33)
        );
    }

    #[test]
    fn kitty_primary_uses_cover_resize_mode() {
        assert_eq!(
            primary_resize_mode(PreviewTransport::Primary, ProtocolType::Kitty),
            StatefulResizeMode::Cover
        );
        assert_eq!(
            primary_resize_mode(PreviewTransport::Primary, ProtocolType::Halfblocks),
            StatefulResizeMode::Scale
        );
    }

    #[test]
    fn cover_resize_prep_scales_to_exact_target_pixels() {
        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 4,
            height: 2,
            pixels: Arc::new(vec![255; 4 * 2 * 3]),
        };

        let image = build_preview_image(
            &frame,
            Rect::new(0, 0, 2, 2),
            (2, 2),
            StatefulResizeMode::Cover,
            false,
        )
        .expect("cover image should build");

        assert_eq!(image.width(), 4);
        assert_eq!(image.height(), 4);
    }

    #[test]
    fn draw_backpressure_escalates_after_slow_draws() {
        let mut backpressure = DrawBackpressure::default();
        backpressure.observe_draw(Duration::from_millis(280));
        assert_eq!(backpressure.frame_interval(), Duration::from_millis(500));
    }

    #[test]
    fn draw_backpressure_relaxes_after_fast_draws() {
        let mut backpressure = DrawBackpressure::default();
        backpressure.observe_draw(Duration::from_millis(150));
        for _ in 0..4 {
            backpressure.observe_draw(Duration::from_millis(20));
        }
        assert_eq!(backpressure.frame_interval(), Duration::ZERO);
    }

    #[test]
    fn build_backpressure_escalates_after_slow_builds() {
        let mut backpressure = BuildBackpressure::default();
        backpressure.observe_build(Duration::from_millis(130));
        assert_eq!(backpressure.frame_interval(), Duration::from_millis(125));
    }

    #[test]
    fn build_backpressure_relaxes_after_fast_builds() {
        let mut backpressure = BuildBackpressure::default();
        backpressure.observe_build(Duration::from_millis(90));
        for _ in 0..2 {
            backpressure.observe_build(Duration::from_millis(15));
        }
        assert_eq!(backpressure.frame_interval(), Duration::ZERO);
    }
}
