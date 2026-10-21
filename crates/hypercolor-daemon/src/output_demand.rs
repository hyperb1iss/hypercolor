//! Output demand: what consumes the frames the render loop produces.
//!
//! Rendering costs CPU and GPU every frame whether or not anything receives
//! the result. The render loop therefore asks, before each frame, whether
//! any consumer exists, and parks while none does (see
//! `render_thread::output_gate`). This module owns the definition of a
//! consumer so the render thread and the status surfaces count the same
//! things:
//!
//! - a connected device, LED or display, that receives frames: one the
//!   device registry reports renderable, or one the backend manager routes
//!   frames to. Simulated displays are excluded here: they consume frames
//!   only while previewed, and display output holds demand for them then;
//! - a live receiver on any latest-value output stream of the bus: frame
//!   data, spectrum, the canvas and scene-canvas previews (WebSocket
//!   streams, display backdrops, and canvas captures all subscribe one),
//!   screen canvas, screen zones, web viewport, zone previews, and the
//!   per-zone canvas streams;
//! - a stream or diagnostic consumer of host input events, which the
//!   render loop republishes from the interaction input it routes;
//! - an explicit [`hypercolor_core::bus::OutputDemandHold`], taken by
//!   consumers that read published frames without a receiver.

use std::time::Instant;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::DeviceRegistry;
use hypercolor_types::api::system::RenderOutputDemandStatus;
use hypercolor_types::device::DeviceInfo;

use crate::simulators::SIMULATED_DISPLAY_BACKEND_ID;

/// Why a parked render loop is producing no frames.
pub const IDLE_REASON_NO_OUTPUT_CONSUMERS: &str = "no_output_consumers";

/// Consumers of rendered output observed at one instant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutputConsumers {
    /// Connected devices that receive frames.
    pub output_devices: u32,
    /// Live receivers on the bus output streams.
    pub stream_receivers: u32,
    /// Stream and diagnostic consumers of republished host input events.
    pub input_event_streams: u32,
    /// Explicit output-demand holds.
    pub demand_holds: u32,
}

impl OutputConsumers {
    /// Whether anything consumes rendered frames.
    #[must_use]
    pub const fn any(self) -> bool {
        self.output_devices > 0
            || self.stream_receivers > 0
            || self.input_event_streams > 0
            || self.demand_holds > 0
    }

    /// Combine the bus-side consumers with counts the caller observed.
    #[must_use]
    pub fn from_bus(bus: &HypercolorBus, output_devices: u32, input_event_streams: u32) -> Self {
        Self {
            output_devices,
            stream_receivers: saturating_u32(bus.output_receiver_count()),
            input_event_streams,
            demand_holds: saturating_u32(bus.output_demand().hold_count()),
        }
    }
}

/// Whether frames routed to `backend_id` reach a consumer on their own,
/// without anyone watching them.
#[must_use]
pub fn is_output_backend(backend_id: &str) -> bool {
    backend_id != SIMULATED_DISPLAY_BACKEND_ID
}

/// Whether a registry device in `renderable` state consumes frames on its
/// own, without anyone watching it.
#[must_use]
pub fn is_output_device(info: &DeviceInfo, renderable: bool) -> bool {
    renderable && is_output_backend(info.output_backend_id())
}

/// Count the connected devices that receive frames.
pub async fn output_device_count(registry: &DeviceRegistry) -> u32 {
    let count = registry
        .list()
        .await
        .iter()
        .filter(|tracked| is_output_device(&tracked.info, tracked.state.is_renderable()))
        .count();
    saturating_u32(count)
}

/// Park bookkeeping the render thread records for status surfaces.
#[derive(Debug, Clone, Copy, Default)]
pub struct OutputDemandTelemetry {
    /// When the current park began; `None` while the loop renders.
    pub parked_since: Option<Instant>,
    /// Parks since the daemon started.
    pub parks_total: u64,
    /// What last woke a parked loop.
    pub last_wake_reason: Option<&'static str>,
    /// The consumers the render loop counted at its latest evaluation:
    /// every frame while it renders, every wake signal while it is parked.
    pub consumers: OutputConsumers,
}

impl OutputDemandTelemetry {
    /// Project the recorded state onto the status contract.
    #[must_use]
    pub fn status(self) -> RenderOutputDemandStatus {
        let consumers = self.consumers;
        let parked_ms = self
            .parked_since
            .map(|since| u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX));
        RenderOutputDemandStatus {
            parked: self.parked_since.is_some(),
            idle_reason: self
                .parked_since
                .map(|_| IDLE_REASON_NO_OUTPUT_CONSUMERS.to_owned()),
            parked_ms,
            output_devices: consumers.output_devices,
            stream_receivers: consumers.stream_receivers,
            input_event_streams: consumers.input_event_streams,
            demand_holds: consumers.demand_holds,
            parks_total: self.parks_total,
            last_wake_reason: self.last_wake_reason.map(str::to_owned),
        }
    }
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
