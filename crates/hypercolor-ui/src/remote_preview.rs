//! The live canvas preview under a Remote bridge.
//!
//! A bridged page never streams canvas frames over its socket. A contract 2
//! bridge carries the canvas as a video track: every preview surface plays
//! that track and reports how much of it the page needs, and this module
//! folds the reports into the one demand the bridge receives. A contract 1
//! bridge carries no video, so every surface shows a still.

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;

use crate::color::CanvasFrameAnalysis;
use crate::remote_bridge::{PREVIEW_STATE_EVENT, PreviewChannel, PreviewDemand, PreviewState};

/// Minimum spacing of ambient samples taken from the video, matching the
/// cadence of the socket path's frame analysis.
pub const VIDEO_ANALYSIS_INTERVAL_MS: f64 = 500.0;

/// Largest edge a surface may ask for, in device pixels.
const MAX_PREVIEW_EDGE_PX: u32 = 16_384;

/// One preview surface's need for the video.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SurfaceDemand {
    /// Whether any part of the surface intersects the viewport.
    pub visible: bool,
    /// The surface's longest edge in device pixels.
    pub longest_edge_px: u32,
}

/// The longest edge of a `width` by `height` CSS pixel box in device pixels,
/// rounded up. A missing or invalid ratio counts as 1.
#[must_use]
pub fn device_pixel_extent(width: f64, height: f64, device_pixel_ratio: f64) -> u32 {
    let ratio = if device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0 {
        device_pixel_ratio
    } else {
        1.0
    };
    let edge = width.max(height) * ratio;
    if !edge.is_finite() || edge <= 0.0 {
        return 0;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the edge is positive and clamped below u32::MAX"
    )]
    let edge = edge.ceil().min(f64::from(MAX_PREVIEW_EDGE_PX)) as u32;
    edge
}

/// Every mounted surface's demand, folded into the page's.
#[derive(Debug, Default)]
pub struct PreviewDemandRegistry {
    next_surface: u64,
    surfaces: BTreeMap<u64, SurfaceDemand>,
}

impl PreviewDemandRegistry {
    /// A new surface, off screen until it reports otherwise.
    pub fn register(&mut self) -> u64 {
        self.next_surface += 1;
        self.surfaces
            .insert(self.next_surface, SurfaceDemand::default());
        self.next_surface
    }

    pub fn update(&mut self, surface: u64, demand: SurfaceDemand) {
        if let Some(slot) = self.surfaces.get_mut(&surface) {
            *slot = demand;
        }
    }

    pub fn release(&mut self, surface: u64) {
        self.surfaces.remove(&surface);
    }

    /// The page's demand: on while a surface with a real extent is on screen
    /// in a visible page, sized to the largest such surface. The size holds
    /// while the page is hidden, so the preview resumes at the same width.
    #[must_use]
    pub fn demand(&self, page_visible: bool) -> PreviewDemand {
        let max_width = self
            .surfaces
            .values()
            .filter(|surface| surface.visible)
            .map(|surface| surface.longest_edge_px)
            .max()
            .unwrap_or(0);
        PreviewDemand {
            enabled: page_visible && max_width > 0,
            max_width,
        }
    }
}

/// Shared state of the bridged preview: the bridge's state and stream, and
/// the page's demand. Provided once per bridged page.
#[derive(Clone, Copy)]
pub struct RemotePreviewContext {
    state: RwSignal<PreviewState>,
    stream: RwSignal<Option<web_sys::MediaStream>, LocalStorage>,
    channel: StoredValue<Option<PreviewChannel>, LocalStorage>,
    registry: StoredValue<PreviewDemandRegistry>,
    page_visible: StoredValue<bool>,
    sent: StoredValue<PreviewDemand>,
    analysis: Option<WriteSignal<Option<CanvasFrameAnalysis>>>,
    last_analysis_at: StoredValue<Option<f64>>,
}

impl RemotePreviewContext {
    /// The context for a bridged page. `channel` holds the bridge's contract
    /// 2 preview members and is `None` under contract 1. Ambient samples of
    /// the video go to `analysis`.
    ///
    /// Listens for [`PREVIEW_STATE_EVENT`] and page visibility until the
    /// current reactive owner is disposed.
    #[must_use]
    pub fn new(
        channel: Option<PreviewChannel>,
        analysis: Option<WriteSignal<Option<CanvasFrameAnalysis>>>,
    ) -> Self {
        let state = channel
            .as_ref()
            .map_or(PreviewState::Unsupported, PreviewChannel::state);
        let stream = channel.as_ref().and_then(PreviewChannel::stream);
        let listens = channel.is_some();
        let context = Self {
            state: RwSignal::new(state),
            stream: RwSignal::new_local(stream),
            channel: StoredValue::new_local(channel),
            registry: StoredValue::new(PreviewDemandRegistry::default()),
            page_visible: StoredValue::new(page_is_visible()),
            sent: StoredValue::new(PreviewDemand::default()),
            analysis,
            last_analysis_at: StoredValue::new(None),
        };
        if listens {
            context.listen();
        }
        context
    }

    fn listen(self) {
        use hypercolor_leptos_ext::events::{document, document_event_target, on, window};
        if let Some(window) = window() {
            let handle = on(window.as_ref(), PREVIEW_STATE_EVENT, move |_| {
                self.refresh()
            });
            let _ = StoredValue::new_local(handle);
        }
        if let Some(document) = document() {
            let target = document.clone();
            let handle = on(
                document_event_target(&document),
                "visibilitychange",
                move |_| self.set_page_visible(!target.hidden()),
            );
            let _ = StoredValue::new_local(handle);
        }
    }

    /// Read the bridge's state and stream again, and offer the page's demand
    /// again if the bridge refused it earlier.
    pub fn refresh(self) {
        let Some(Some(channel)) = self.channel.try_get_value() else {
            return;
        };
        let state = channel.state();
        let stream = channel.stream();
        let changed = self
            .stream
            .with_untracked(|current| match (current, &stream) {
                (Some(current), Some(next)) => !js_sys::Object::is(current, next),
                (None, None) => false,
                _ => true,
            });
        if changed {
            self.stream.set(stream);
        }
        if self.state.get_untracked() != state {
            self.state.set(state);
        }
        self.flush();
    }

    /// The bridge's preview state. Reactive.
    #[must_use]
    pub fn state(self) -> PreviewState {
        self.state.get()
    }

    /// The stream holding the video track. Reactive.
    #[must_use]
    pub fn stream(self) -> Option<web_sys::MediaStream> {
        self.stream.get()
    }

    /// The demand the bridge last accepted.
    #[must_use]
    pub fn sent_demand(self) -> PreviewDemand {
        self.sent.try_get_value().unwrap_or_default()
    }

    pub fn register_surface(self) -> u64 {
        self.registry
            .try_update_value(PreviewDemandRegistry::register)
            .unwrap_or_default()
    }

    pub fn update_surface(self, surface: u64, demand: SurfaceDemand) {
        let _ = self
            .registry
            .try_update_value(|registry| registry.update(surface, demand));
        self.flush();
    }

    pub fn release_surface(self, surface: u64) {
        let _ = self
            .registry
            .try_update_value(|registry| registry.release(surface));
        self.flush();
    }

    pub fn set_page_visible(self, visible: bool) {
        let _ = self.page_visible.try_set_value(visible);
        self.flush();
    }

    fn flush(self) {
        let Some(page_visible) = self.page_visible.try_get_value() else {
            return;
        };
        let Some(demand) = self
            .registry
            .try_with_value(|registry| registry.demand(page_visible))
        else {
            return;
        };
        let Some(previous) = self.sent.try_get_value() else {
            return;
        };
        if previous == demand {
            return;
        }
        // Recorded before the call, so a state event the bridge dispatches
        // from inside `setPreview` finds this demand already sent.
        let _ = self.sent.try_set_value(demand);
        let accepted = match self.channel.try_get_value() {
            Some(Some(channel)) => channel.set_preview(demand),
            _ => true,
        };
        // A refused demand goes back to unsent, so the next surface report
        // or bridge state change offers it again, unless a newer one was
        // sent in the meantime.
        if !accepted && self.sent.try_get_value() == Some(demand) {
            let _ = self.sent.try_set_value(previous);
        }
    }

    /// Whether a surface should sample the video for the ambient palette at
    /// `now_ms`. Claims the slot when it answers `true`, so surfaces showing
    /// the same video share one sample per interval.
    pub fn claim_analysis(self, now_ms: f64) -> bool {
        if self.analysis.is_none() {
            return false;
        }
        let due = self
            .last_analysis_at
            .try_get_value()
            .flatten()
            .is_none_or(|last| now_ms - last >= VIDEO_ANALYSIS_INTERVAL_MS);
        if due {
            let _ = self.last_analysis_at.try_set_value(Some(now_ms));
        }
        due
    }

    /// Publish an ambient sample taken from the video.
    pub fn publish_analysis(self, analysis: CanvasFrameAnalysis) {
        if let Some(sink) = self.analysis {
            let _ = sink.try_set(Some(analysis));
        }
    }
}

fn page_is_visible() -> bool {
    hypercolor_leptos_ext::events::document().is_none_or(|document| !document.hidden())
}
