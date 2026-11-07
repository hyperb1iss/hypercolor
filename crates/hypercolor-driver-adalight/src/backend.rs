//! Adalight backend: implements [`DeviceBackend`] over a serial port.
//!
//! Each connected device owns a session holding its resolved plan, the
//! open port, and a reusable frame buffer whose header is built once. The
//! backend never enumerates ports, and only the live driver config decides
//! what opens: every adoption and every open resolves the device from the
//! config at that moment, and discovery metadata never names a port here.
//!
//! Pacing follows the render loop up to what the line can carry. The
//! backend advertises the line interval as the output cadence, so the
//! core's latest-frame queue coalesces faster frames to the newest one. As
//! a backstop, a frame is skipped while the OS driver still holds a whole
//! frame of unsent bytes, and a backlog that stops draining fails the
//! delivery so the lifecycle reconnects. Frames never queue behind the
//! UART.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceFrameSink, DeviceLifecyclePolicy,
    DeviceWriteOutcome, DiscoveredDevice, DriverConfigSource, DriverConfigView, DriverError,
    OutputCadence,
};
use hypercolor_driver_support::{
    ConfiguredDevices, DeviceSession, FrameDelivery, Session, SessionTable,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{
    ClearBuffer, DataBits, FlowControl, Parity, SerialPort, SerialPortBuilderExt, SerialStream,
    StopBits,
};
use tracing::{debug, info, warn};

use crate::config::AdalightConfig;
use crate::drain::{DrainVerdict, DrainWatch};
use crate::plan::{AdalightDevicePlan, resolve_config};
use crate::protocol::FrameEncoder;

/// Discovery metadata key describing the device's config entry as JSON.
pub const METADATA_DEVICE_CONFIG: &str = "adalight_device";

/// Longest silence before the last frame is resent.
///
/// Adalight sketches blank the strip after 15 to 60 s without data and
/// WLED's serial input falls back to its own effects after 2.5 s, so a
/// one-second refresh keeps every known firmware showing the last frame.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);

/// Render loop ceiling, used to report when a line is the bottleneck.
pub const RENDER_LOOP_MAX_FPS: u32 = 60;

/// Time a disconnect may spend on its blackout frames and port close
/// together, inside the daemon's two-second disconnect timeout.
pub const DISCONNECT_BUDGET: Duration = Duration::from_millis(1_800);

/// Shortest bound on one frame write before the port counts as stalled.
const MIN_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Frames' worth of line time a write may take before it counts as stalled.
const WRITE_TIMEOUT_FRAMES: u32 = 4;

/// Time the blackout frames may take to reach the OS driver.
const BLACKOUT_WRITE_BOUND: Duration = Duration::from_millis(600);

/// Bytes of firmware greeting read after the startup delay.
const GREETING_MAX_BYTES: usize = 256;

/// Quiet period that ends the greeting read.
const GREETING_READ_IDLE: Duration = Duration::from_millis(25);

const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);

/// Backend for manually configured Adalight and AWA serial controllers.
pub struct AdalightBackend {
    /// The live config, the only authority on what may open.
    devices: ConfiguredDevices<AdalightDevicePlan>,
    /// Connected sessions.
    sessions: SessionTable<AdalightSession>,
}

/// Every configured device with its plan, for [`ConfiguredDevices`].
fn configured_plans(
    config: DriverConfigView<'_>,
) -> Result<Vec<(DeviceId, AdalightDevicePlan)>, DriverError> {
    let config = config.parse_settings::<AdalightConfig>()?;
    let plans = resolve_config(&config).map_err(|error| DriverError::Configuration {
        message: error.to_string(),
    })?;
    Ok(plans
        .into_iter()
        .map(|plan| (crate::device_id_for(&plan.id), plan))
        .collect())
}

impl AdalightBackend {
    /// Create a backend authorized by `config`.
    #[must_use]
    pub fn new(config: Arc<dyn DriverConfigSource>) -> Self {
        Self {
            devices: ConfiguredDevices::new(crate::DESCRIPTOR.id, config, configured_plans),
            sessions: SessionTable::new(),
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for AdalightBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: crate::DESCRIPTOR.id.to_owned(),
            name: "Adalight Serial (Ada / AWA)".to_owned(),
            description: "Manually configured Adalight and HyperSerial AWA controllers on serial \
                          ports"
                .to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        self.devices.current(&discovered.info.id).map(|_| ())
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    fn lifecycle_policy(&self, info: &DeviceInfo) -> DeviceLifecyclePolicy {
        let policy = DeviceLifecyclePolicy::default();
        let startup_delay = self
            .devices
            .current(&info.id)
            .map(|device| device.plan().startup_delay)
            .unwrap_or_default();
        policy.with_connect_timeout(policy.connect_timeout().saturating_add(startup_delay))
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        self.sessions.ensure_accepting(id)?;
        let device = self.devices.current(id)?;
        let plan = Arc::clone(device.plan());

        if let Some(open) = self.sessions.facts(id) {
            if *open == plan {
                return Ok(());
            }
            // The config changed under a live session: release the old port
            // before reopening, since the open is exclusive.
            if let Err(error) = self.sessions.close(id).await {
                debug!(device_id = %id, error = %error, "closing a moved serial session failed");
            }
        }

        // Closing an old session can take a while; check again right
        // before the open.
        self.devices.confirm(id, &device)?;

        let mut port = open_port(*id, &plan).await?;
        if !plan.startup_delay.is_zero() {
            tokio::time::sleep(plan.startup_delay).await;
            log_greeting(*id, &plan, &mut port).await;
        }
        // The config may have moved while the port opened and the board
        // booted; a session only ever goes live under the config it opened.
        if let Err(error) = self.devices.confirm(id, &device) {
            close_port(port).await;
            return Err(error);
        }

        let line_max_fps = plan.line_max_fps();
        info!(
            device_id = %id,
            adalight_device = %plan.id,
            port = %plan.port,
            baud_rate = plan.baud_rate,
            protocol = plan.protocol().as_str(),
            leds = plan.led_count(),
            frame_bytes = plan.frame_bytes(),
            line_max_fps,
            "Connected serial LED controller"
        );
        if line_max_fps < RENDER_LOOP_MAX_FPS {
            warn!(
                device_id = %id,
                adalight_device = %plan.id,
                baud_rate = plan.baud_rate,
                frame_bytes = plan.frame_bytes(),
                line_max_fps,
                "serial line carries at most {line_max_fps} frames per second for this strip; \
                 faster frames coalesce to the newest one"
            );
        }

        let session = AdalightSession::new(*id, Arc::clone(&plan), port);
        self.sessions
            .install(Session::new(*id, plan, session))
            .await
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        self.sessions.close(id).await?;
        info!(device_id = %id, "Disconnected serial LED controller");
        Ok(())
    }

    async fn retire(&self) -> Result<(), DeviceError> {
        self.sessions.retire().await
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        self.sessions.require(id)?.write(colors).await.map(|_| ())
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.sessions.facts(id).map(|plan| plan.effective_max_fps())
    }

    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        self.sessions.facts(id).map(|plan| output_cadence_for(&plan))
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sessions.frame_sink(id)
    }
}

/// Output cadence for a plan: the line interval as the minimum spacing,
/// with a keepalive resend after [`KEEPALIVE_INTERVAL`] of silence.
#[must_use]
pub fn output_cadence_for(plan: &AdalightDevicePlan) -> OutputCadence {
    OutputCadence::from_min_interval(plan.frame_interval(), plan.effective_max_fps())
        .with_max_frame_silence(KEEPALIVE_INTERVAL)
}

/// Longest a backlog or a single write may go without progress before the
/// port counts as stalled: four frames of line time, at least one second.
#[must_use]
pub fn stall_budget(plan: &AdalightDevicePlan) -> Duration {
    plan.frame_time()
        .saturating_mul(WRITE_TIMEOUT_FRAMES)
        .max(MIN_WRITE_TIMEOUT)
}

async fn open_port(id: DeviceId, plan: &AdalightDevicePlan) -> Result<SerialStream, DeviceError> {
    let builder = tokio_serial::new(plan.port.clone(), plan.baud_rate)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None);
    let port = plan.port.clone();
    let handle = tokio::runtime::Handle::current();
    // Opening a tty runs the USB-serial driver's open and line setup;
    // keep that off the async workers.
    let opened = tokio::task::spawn_blocking(move || {
        let _runtime = handle.enter();
        builder.open_native_async()
    })
    .await
    .map_err(|error| DeviceError::connection(id, format!("serial open task failed: {error}")))?;
    opened.map_err(|error| map_open_error(id, &port, &error))
}

/// Close a port that never became a session, off the async workers.
async fn close_port(port: SerialStream) {
    let handle = tokio::runtime::Handle::current();
    let close = handle.clone().spawn_blocking(move || {
        let _runtime = handle.enter();
        drop(port);
    });
    let _ = tokio::time::timeout(DISCONNECT_BUDGET, close).await;
}

fn map_open_error(id: DeviceId, port: &str, error: &tokio_serial::Error) -> DeviceError {
    let detail = format!("cannot open serial port {port}: {error}");
    match error.kind() {
        tokio_serial::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => {
            DeviceError::PermissionDenied {
                device: id.to_string(),
                detail,
            }
        }
        _ => DeviceError::connection(id, detail),
    }
}

/// Read whatever the firmware printed while it booted and log it. Classic
/// Adalight sketches repeat `Ada` once a second while idle and HyperSerial
/// prints `Awa driver` at boot, so this confirms the baud rate and firmware
/// without sending anything.
async fn log_greeting(id: DeviceId, plan: &AdalightDevicePlan, port: &mut SerialStream) {
    let mut greeting = Vec::with_capacity(GREETING_MAX_BYTES);
    let mut chunk = [0_u8; 64];
    while greeting.len() < GREETING_MAX_BYTES {
        match tokio::time::timeout(GREETING_READ_IDLE, port.read(&mut chunk)).await {
            Ok(Ok(read)) if read > 0 => {
                let take = read.min(GREETING_MAX_BYTES - greeting.len());
                greeting.extend_from_slice(&chunk[..take]);
            }
            _ => break,
        }
    }
    if greeting.is_empty() {
        debug!(device_id = %id, port = %plan.port, "serial firmware sent no greeting");
        return;
    }
    let text: String = String::from_utf8_lossy(&greeting)
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    info!(
        device_id = %id,
        port = %plan.port,
        greeting = %text.split_whitespace().collect::<Vec<_>>().join(" "),
        "serial firmware greeting"
    );
}

// ── Session ─────────────────────────────────────────────────────────────

struct AdalightSession {
    device_id: DeviceId,
    plan: Arc<AdalightDevicePlan>,
    port: Option<SerialStream>,
    encoder: FrameEncoder,
    stall_budget: Duration,
    /// Whether any frame has gone out, so a blackout frame is worth sending.
    has_sent: bool,
    /// Whether the OS driver reports its output queue depth; once a query
    /// fails the backstop is skipped for the rest of the session.
    queue_depth_supported: bool,
    drain: DrainWatch,
    last_size_mismatch_warn_at: Option<Instant>,
}

impl AdalightSession {
    fn new(device_id: DeviceId, plan: Arc<AdalightDevicePlan>, port: SerialStream) -> Self {
        let encoder = FrameEncoder::new(plan.layout, plan.color_order);
        let stall_budget = stall_budget(&plan);
        Self {
            device_id,
            plan,
            port: Some(port),
            encoder,
            stall_budget,
            has_sent: false,
            queue_depth_supported: true,
            drain: DrainWatch::new(),
            last_size_mismatch_warn_at: None,
        }
    }

    async fn write_colors(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<DeviceWriteOutcome> {
        self.warn_on_size_mismatch(colors.len());
        match self.drain_verdict() {
            DrainVerdict::Clear => {}
            DrainVerdict::Backlogged => return Ok(DeviceWriteOutcome::SuppressedCadence),
            DrainVerdict::Stalled {
                queued,
                stalled_for,
            } => bail!(
                "serial driver for {} has held {queued} unsent bytes without draining for {} ms",
                self.plan.port,
                stalled_for.as_millis()
            ),
        }
        self.encoder.encode(colors);
        if let Some(delivery) = delivery {
            delivery.transport_started();
        }
        self.send_frame(self.stall_budget).await?;
        self.has_sent = true;
        Ok(DeviceWriteOutcome::Sent)
    }

    /// Send black frames so the strip goes dark when output stops. Frames
    /// still waiting in the OS driver are discarded first, so the blackout
    /// goes out at once and the close drains only the blackout.
    async fn black_out(&mut self) -> Result<()> {
        if !self.has_sent {
            return Ok(());
        }
        let Some(port) = self.port.as_ref() else {
            return Ok(());
        };
        if let Err(error) = port.clear(ClearBuffer::Output) {
            debug!(
                device_id = %self.device_id,
                port = %self.plan.port,
                error = %error,
                "serial driver could not discard queued output before blackout"
            );
        }
        self.drain.reset();
        self.encoder.encode_black();
        for _ in 0..self.plan.blackout_frames() {
            self.send_frame(BLACKOUT_WRITE_BOUND).await?;
        }
        Ok(())
    }

    /// Close the port on a blocking thread, because closing a tty waits for
    /// the driver to drain its output and must not stall an async worker.
    /// The returned task finishes once the descriptor (and its exclusive
    /// lock) is released.
    fn close(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        let port = self.port.take()?;
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            drop(port);
            return None;
        };
        Some(handle.clone().spawn_blocking(move || {
            let _runtime = handle.enter();
            drop(port);
        }))
    }

    /// Close the port and wait up to `bound` for it to be released, so a
    /// reconnect does not race the old descriptor's exclusive lock. A
    /// wedged driver can hold a tty close for its full closing wait; past
    /// the bound the close finishes in the background.
    async fn close_and_wait(&mut self, bound: Duration) {
        let Some(task) = self.close() else {
            return;
        };
        if tokio::time::timeout(bound, task).await.is_err() {
            debug!(
                device_id = %self.device_id,
                port = %self.plan.port,
                bound_ms = bound.as_millis(),
                "serial port close is still draining; finishing in the background"
            );
        }
    }

    async fn send_frame(&mut self, bound: Duration) -> Result<()> {
        let port = self
            .port
            .as_mut()
            .ok_or_else(|| anyhow!("serial port is closed"))?;
        tokio::time::timeout(bound, port.write_all(self.encoder.as_bytes()))
            .await
            .with_context(|| {
                format!(
                    "serial write to {} stalled for {} ms",
                    self.plan.port,
                    bound.as_millis()
                )
            })?
            .with_context(|| format!("serial write to {} failed", self.plan.port))
    }

    /// Whether the next frame may go out, given the OS driver's queue.
    fn drain_verdict(&mut self) -> DrainVerdict {
        if !self.queue_depth_supported {
            return DrainVerdict::Clear;
        }
        let Some(port) = self.port.as_ref() else {
            return DrainVerdict::Clear;
        };
        match port.bytes_to_write() {
            Ok(queued) => self.drain.observe(
                queued,
                self.plan.frame_bytes(),
                self.stall_budget,
                Instant::now(),
            ),
            Err(error) => {
                debug!(
                    device_id = %self.device_id,
                    port = %self.plan.port,
                    error = %error,
                    "serial driver does not report its output queue; relying on cadence alone"
                );
                self.queue_depth_supported = false;
                DrainVerdict::Clear
            }
        }
    }

    fn warn_on_size_mismatch(&mut self, actual: usize) {
        let expected = self.plan.led_count() as usize;
        if actual == expected {
            return;
        }
        let should_warn = self
            .last_size_mismatch_warn_at
            .is_none_or(|at| at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
        if should_warn {
            warn!(
                device_id = %self.device_id,
                expected_led_count = expected,
                actual_led_count = actual,
                "serial LED frame size mismatch; truncating or padding with black"
            );
            self.last_size_mismatch_warn_at = Some(Instant::now());
        }
    }
}

impl DeviceSession for AdalightSession {
    type Facts = Arc<AdalightDevicePlan>;

    const CLEANUP: Duration = DISCONNECT_BUDGET.saturating_add(Duration::from_millis(100));

    async fn write_frame(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        self.write_colors(colors, delivery)
            .await
            .map_err(|error| DeviceError::write(self.device_id, format!("{error:#}")))
    }

    /// Acknowledge encoded frame bytes, which is what the line carries.
    async fn deliver_frame(
        &mut self,
        delivery: FrameDelivery,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = self.plan.frame_bytes();
        let started_at = Instant::now();
        let result = self.write_frame(colors.as_slice(), Some(&delivery)).await;
        DeviceDeliveryAck::from_write_result(
            delivery.id(),
            payload_bytes,
            started_at.elapsed(),
            result,
        )
    }

    /// Black out the strip, then release the port and its exclusive lock.
    async fn finish(mut self) -> Result<(), DeviceError> {
        let started = Instant::now();
        if let Err(error) = self.black_out().await {
            debug!(
                device_id = %self.device_id,
                port = %self.plan.port,
                error = %format!("{error:#}"),
                "best-effort serial blackout frames failed"
            );
        }
        self.close_and_wait(DISCONNECT_BUDGET.saturating_sub(started.elapsed()))
            .await;
        Ok(())
    }
}

/// Closing a tty can block for its full closing wait, so a session that ends
/// by any path still hands its port to a blocking thread.
impl Drop for AdalightSession {
    fn drop(&mut self) {
        drop(self.close());
    }
}
