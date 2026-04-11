//! Preview transport manager for the live canvas overlay.

use std::sync::Arc;
use std::sync::mpsc::{self as std_mpsc, Receiver as StdReceiver, Sender as StdSender};
use std::thread;

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui_image::errors::Errors as ImageProtocolError;
use ratatui_image::picker::Picker;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};
use ratatui_image::{Resize, ResizeEncodeRender};

use crate::state::CanvasFrame;

#[allow(
    dead_code,
    reason = "Preview transport manager is staged for future TUI integration"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewTransport {
    Primary,
    Halfblocks,
}

#[allow(
    dead_code,
    reason = "Preview transport manager is staged for future TUI integration"
)]
#[derive(Debug, Clone, Copy)]
struct PreviewPolicy {
    windowed: PreviewTransport,
    fullscreen: PreviewTransport,
}

#[allow(
    dead_code,
    reason = "Preview transport manager is staged for future TUI integration"
)]
impl PreviewPolicy {
    fn transport_for(self, fullscreen: bool) -> PreviewTransport {
        if fullscreen {
            self.fullscreen
        } else {
            self.windowed
        }
    }
}

impl Default for PreviewPolicy {
    fn default() -> Self {
        Self {
            windowed: PreviewTransport::Primary,
            fullscreen: PreviewTransport::Halfblocks,
        }
    }
}

#[allow(
    dead_code,
    reason = "Preview transport manager is staged for future TUI integration"
)]
pub(crate) struct PreviewManager {
    primary_picker: Picker,
    halfblocks_picker: Picker,
    policy: PreviewPolicy,
    resize_tx: Option<StdSender<ResizeRequest>>,
    resize_rx: StdReceiver<Result<ResizeResponse, ImageProtocolError>>,
    resize_worker: Option<thread::JoinHandle<()>>,
    current: Option<ThreadProtocol>,
    pending: Option<ThreadProtocol>,
    preview_area: Option<Rect>,
    latest_frame: Option<Arc<CanvasFrame>>,
    selected_transport: PreviewTransport,
}

#[allow(
    dead_code,
    reason = "Preview transport manager is staged for future TUI integration"
)]
impl PreviewManager {
    pub(crate) fn new(primary_picker: Picker) -> Self {
        let (resize_tx, resize_requests) = std_mpsc::channel::<ResizeRequest>();
        let (resize_results_tx, resize_rx) =
            std_mpsc::channel::<Result<ResizeResponse, ImageProtocolError>>();

        let resize_worker = thread::Builder::new()
            .name("hypercolor-tui-preview".to_string())
            .spawn(move || {
                while let Ok(request) = resize_requests.recv() {
                    if resize_results_tx.send(request.resize_encode()).is_err() {
                        break;
                    }
                }
            })
            .ok();

        let policy = PreviewPolicy::default();

        Self {
            primary_picker,
            halfblocks_picker: Picker::halfblocks(),
            policy,
            resize_tx: Some(resize_tx),
            resize_rx,
            resize_worker,
            current: None,
            pending: None,
            preview_area: None,
            latest_frame: None,
            selected_transport: policy.transport_for(false),
        }
    }

    pub(crate) fn on_frame(&mut self, frame: Arc<CanvasFrame>, fullscreen: bool) {
        self.selected_transport = self.policy.transport_for(fullscreen);
        self.queue_protocol(frame.as_ref());
        self.latest_frame = Some(frame);
    }

    pub(crate) fn set_fullscreen(&mut self, fullscreen: bool) {
        let next_transport = self.policy.transport_for(fullscreen);
        self.selected_transport = next_transport;
        self.reset_protocols();

        if let Some(frame) = self.latest_frame.clone() {
            self.queue_protocol(frame.as_ref());
        }
    }

    pub(crate) fn clear(&mut self) {
        self.latest_frame = None;
        self.reset_protocols();
    }

    pub(crate) fn shutdown(&mut self) {
        self.clear();
        self.resize_tx = None;

        if let Some(worker) = self.resize_worker.take()
            && let Err(error) = worker.join()
        {
            tracing::warn!("preview resize worker panicked during shutdown: {error:?}");
        }
    }

    pub(crate) fn drain_resize_results(&mut self) -> bool {
        let mut dirty = false;

        while let Ok(result) = self.resize_rx.try_recv() {
            match result {
                Ok(completed) => {
                    if let Some(protocol) = self.pending.as_mut()
                        && protocol.update_resized_protocol(completed)
                    {
                        dirty = true;
                        let ready_for_current_area = self
                            .preview_area
                            .map(Self::resize_area)
                            .is_some_and(|area| {
                                protocol.needs_resize(&Resize::Scale(None), area).is_none()
                            });

                        if ready_for_current_area || self.current.is_none() {
                            self.current = self.pending.take();
                        }
                    }
                }
                Err(error) => {
                    tracing::debug!("preview resize/encode failed: {error}");
                }
            }
        }

        dirty
    }

    pub(crate) fn render(&mut self, area: Option<Rect>, buf: &mut Buffer) {
        let Some(area) = area.filter(|area| area.width > 0 && area.height > 0) else {
            self.preview_area = None;
            return;
        };

        self.preview_area = Some(area);
        let resize_area = Self::resize_area(area);

        if let Some(protocol) = self.pending.as_mut()
            && let Some(target_rect) = protocol.needs_resize(&Resize::Scale(None), resize_area)
        {
            protocol.resize_encode(&Resize::Scale(None), target_rect);
        }

        if let Some(protocol) = self.current.as_mut() {
            protocol.render(area, buf);
        }
    }

    #[must_use]
    pub(crate) fn has_current_frame(&self) -> bool {
        self.current.is_some()
    }

    fn queue_protocol(&mut self, frame: &CanvasFrame) {
        let Some(img) = image::RgbImage::from_raw(
            u32::from(frame.width),
            u32::from(frame.height),
            frame.pixels.clone(),
        ) else {
            return;
        };

        let next_protocol = self
            .picker_for(self.selected_transport)
            .new_resize_protocol(DynamicImage::ImageRgb8(img));

        if let Some(protocol) = self.pending.as_mut() {
            if protocol.protocol_type().is_some() {
                protocol.replace_protocol(next_protocol);
            }
        } else if let Some(resize_tx) = self.resize_tx.as_ref() {
            self.pending = Some(ThreadProtocol::new(resize_tx.clone(), Some(next_protocol)));
        }
    }

    fn picker_for(&self, transport: PreviewTransport) -> &Picker {
        match transport {
            PreviewTransport::Primary => &self.primary_picker,
            PreviewTransport::Halfblocks => &self.halfblocks_picker,
        }
    }

    fn reset_protocols(&mut self) {
        self.current = None;
        self.pending = None;
        self.preview_area = None;
    }

    fn resize_area(area: Rect) -> Rect {
        Rect::new(0, 0, area.width, area.height)
    }
}

#[cfg(test)]
mod tests {
    use super::{PreviewPolicy, PreviewTransport};

    #[test]
    fn default_policy_uses_primary_windowed_and_halfblocks_fullscreen() {
        let policy = PreviewPolicy::default();

        assert_eq!(policy.transport_for(false), PreviewTransport::Primary);
        assert_eq!(policy.transport_for(true), PreviewTransport::Halfblocks);
    }
}
