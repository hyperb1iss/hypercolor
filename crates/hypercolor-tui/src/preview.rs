//! Preview transport manager for the live canvas overlay.

use std::sync::Arc;
use std::sync::mpsc::{self as std_mpsc, Receiver as StdReceiver, Sender as StdSender};
use std::thread;
use std::time::{Duration, Instant};

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, ResizeEncodeRender};

use crate::state::CanvasFrame;

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
        font_size: (u16, u16),
        area: Option<Rect>,
    ) -> Duration {
        if transport != PreviewTransport::Primary {
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
            fullscreen: PreviewTransport::Halfblocks,
            max_primary_rgba_bytes: 768 * 1024,
            max_primary_scale_tenths: 15,
            medium_primary_rgba_bytes: 256 * 1024,
            large_primary_rgba_bytes: 512 * 1024,
            default_frame_interval: Duration::from_millis(100),
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
}

struct PreviewBuildResponse {
    request_id: u64,
    frame_number: u32,
    transport: PreviewTransport,
    protocol: StatefulProtocol,
}

enum PreviewBuildResult {
    Ready(PreviewBuildResponse),
    Failed {
        request_id: u64,
        frame_number: u32,
        error: String,
    },
}

pub(crate) struct PreviewManager {
    primary_picker: Picker,
    policy: PreviewPolicy,
    build_tx: Option<StdSender<PreviewBuildRequest>>,
    build_rx: StdReceiver<PreviewBuildResult>,
    build_worker: Option<thread::JoinHandle<()>>,
    current: Option<StatefulProtocol>,
    current_frame_number: Option<u32>,
    current_transport: Option<PreviewTransport>,
    preview_area: Option<Rect>,
    latest_frame: Option<Arc<CanvasFrame>>,
    fullscreen: bool,
    selected_transport: PreviewTransport,
    next_request_id: u64,
    in_flight_request_id: Option<u64>,
    last_build_started: Option<Instant>,
}

impl PreviewManager {
    pub(crate) fn new(primary_picker: Picker) -> Self {
        let (build_tx, build_requests) = std_mpsc::channel::<PreviewBuildRequest>();
        let (build_results_tx, build_rx) = std_mpsc::channel::<PreviewBuildResult>();

        let halfblocks_picker = Picker::halfblocks();
        let build_primary_picker = primary_picker.clone();
        let build_halfblocks_picker = halfblocks_picker.clone();
        let build_worker = thread::Builder::new()
            .name("hypercolor-tui-preview".to_string())
            .spawn(move || {
                while let Ok(request) = build_requests.recv() {
                    let picker = match request.transport {
                        PreviewTransport::Primary => &build_primary_picker,
                        PreviewTransport::Halfblocks => &build_halfblocks_picker,
                    };

                    let Some(img) = image::RgbImage::from_raw(
                        u32::from(request.frame.width),
                        u32::from(request.frame.height),
                        request.frame.pixels.clone(),
                    ) else {
                        if build_results_tx
                            .send(PreviewBuildResult::Failed {
                                request_id: request.request_id,
                                frame_number: request.frame.frame_number,
                                error: "invalid preview frame length".to_string(),
                            })
                            .is_err()
                        {
                            break;
                        }
                        continue;
                    };

                    let mut protocol = picker.new_resize_protocol(DynamicImage::ImageRgb8(img));

                    if let Some(target_rect) =
                        protocol.needs_resize(&Resize::Scale(None), request.area)
                    {
                        protocol.resize_encode(&Resize::Scale(None), target_rect);
                        if let Some(Err(error)) = protocol.last_encoding_result() {
                            if build_results_tx
                                .send(PreviewBuildResult::Failed {
                                    request_id: request.request_id,
                                    frame_number: request.frame.frame_number,
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
                            protocol,
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
        }
    }

    pub(crate) fn on_frame(&mut self, frame: Arc<CanvasFrame>, fullscreen: bool) {
        self.fullscreen = fullscreen;
        let next_transport = self.transport_for(Some(frame.as_ref()), self.preview_area);
        if next_transport != self.selected_transport {
            self.selected_transport = next_transport;
            self.invalidate_current();
        }
        self.latest_frame = Some(frame);
        self.maybe_queue_build();
    }

    pub(crate) fn set_fullscreen(&mut self, fullscreen: bool) {
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
                        self.current = Some(completed.protocol);
                        self.current_frame_number = Some(completed.frame_number);
                        self.current_transport = Some(completed.transport);
                        self.in_flight_request_id = None;
                    }
                }
                PreviewBuildResult::Failed {
                    request_id,
                    frame_number,
                    error,
                } => {
                    if self.in_flight_request_id == Some(request_id) {
                        self.in_flight_request_id = None;
                    }
                    tracing::debug!("preview build failed for frame {frame_number}: {error}");
                }
            }
        }

        if self.in_flight_request_id.is_none() {
            self.maybe_queue_build();
        }

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

        if let Some(protocol) = self.current.as_mut() {
            protocol.render(area, buf);
        }
    }

    #[must_use]
    pub(crate) fn has_current_frame(&self) -> bool {
        self.current.is_some()
    }

    fn maybe_queue_build(&mut self) {
        if self.in_flight_request_id.is_some() {
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
        let frame_interval =
            self.policy
                .frame_interval_for(transport, self.primary_picker.font_size(), Some(area));

        if self.current_frame_number == Some(frame.frame_number)
            && self.current_transport == Some(transport)
            && self.current.as_ref().is_some_and(|protocol| {
                protocol
                    .needs_resize(&Resize::Scale(None), resize_area)
                    .is_none()
            })
        {
            return;
        }

        if self.current.is_some()
            && self
                .last_build_started
                .is_some_and(|started| started.elapsed() < frame_interval)
        {
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
            })
            .is_ok()
        {
            self.in_flight_request_id = Some(request_id);
            self.last_build_started = Some(Instant::now());
        }
    }

    fn transport_for(&self, frame: Option<&CanvasFrame>, area: Option<Rect>) -> PreviewTransport {
        self.policy.transport_for(
            self.fullscreen,
            self.primary_picker.font_size(),
            frame,
            area,
        )
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
    }

    fn resize_area(area: Rect) -> Rect {
        Rect::new(0, 0, area.width, area.height)
    }
}

#[cfg(test)]
mod tests {
    use super::{PreviewPolicy, PreviewTransport};
    use crate::state::CanvasFrame;
    use ratatui::layout::Rect;

    #[test]
    fn default_policy_uses_primary_windowed_and_halfblocks_fullscreen() {
        let policy = PreviewPolicy::default();

        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 320,
            height: 200,
            pixels: vec![0; 320 * 200 * 3],
        };

        assert_eq!(
            policy.transport_for(false, (9, 18), Some(&frame), Some(Rect::new(0, 0, 30, 12))),
            PreviewTransport::Primary
        );
        assert_eq!(
            policy.transport_for(true, (9, 18), Some(&frame), Some(Rect::new(0, 0, 30, 12))),
            PreviewTransport::Halfblocks
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
            pixels: vec![0; 320 * 200 * 3],
        };

        assert_eq!(
            policy.transport_for(false, (9, 18), Some(&frame), Some(Rect::new(0, 0, 52, 24))),
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
            pixels: vec![0; 320 * 200 * 3],
        };

        assert_eq!(
            policy.transport_for(false, (9, 18), Some(&frame), Some(Rect::new(0, 0, 55, 16))),
            PreviewTransport::Halfblocks
        );
    }
}
