//! Adalight backend: implements [`DeviceBackend`] over a serial port.
//!
//! Each connected device owns a session holding its resolved plan, the
//! open port, and a reusable frame buffer whose header is built once. The
//! backend never enumerates ports and never opens one the config does not
//! name.
//!
//! Pacing follows the render loop up to what the line can carry. The
//! backend advertises the line interval as the output cadence, so the
//! core's latest-frame queue coalesces faster frames to the newest one. As
//! a backstop, a frame is skipped while the driver still holds a whole
//! frame's worth of unsent bytes, and the newest frame goes out once the
//! line catches up. Frames never queue behind the UART.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceLifecyclePolicy, DeviceWriteOutcome, DiscoveredDevice, OutputCadence,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_serial::{
    DataBits, FlowControl, Parity, SerialPort, SerialPortBuilderExt, SerialStream, StopBits,
};
use tracing::{debug, info, warn};

use crate::config::AdalightDeviceConfig;
use crate::plan::{AdalightDevicePlan, resolve_device};
use crate::protocol::FrameEncoder;

/// Discovery metadata key carrying the device's config entry as JSON.
pub const METADATA_DEVICE_CONFIG: &str = "adalight_device";

/// Longest silence before the last frame is resent.
///
/// Adalight sketches blank the strip after 15 to 60 s without data and
/// WLED's serial input falls back to its own effects after 2.5 s, so a
/// one-second refresh keeps every known firmware showing the last frame.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);

/// Black frames sent on disconnect. HyperSerial never blanks on idle and
/// drops corrupt frames, and Adalight-FastLED drops frames that arrive while
/// it latches, so one surviving copy of three turns the strip off.
pub const BLACKOUT_FRAMES: usize = 3;

/// Render loop ceiling, used to report when a line is the bottleneck.
pub const RENDER_LOOP_MAX_FPS: u32 = 60;

/// Shortest bound on one frame write before the port counts as stalled.
const MIN_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Frames' worth of line time a write may take before it counts as stalled.
const WRITE_TIMEOUT_FRAMES: u32 = 4;

/// Bytes of firmware greeting read after the startup delay.
const GREETING_MAX_BYTES: usize = 256;

/// Quiet period that ends the greeting read.
const GREETING_READ_IDLE: Duration = Duration::from_millis(25);

/// Slack on top of the blackout frames' line time while a close drains.
const CLOSE_WAIT_MARGIN: Duration = Duration::from_millis(500);

const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);

/// Backend for manually configured Adalight and AWA serial controllers.
#[derive(Default)]
pub struct AdalightBackend {
    /// Plans for adopted devices, keyed by device id.
    adopted: StdRwLock<HashMap<DeviceId, Arc<AdalightDevicePlan>>>,
    /// Connected sessions.
    sessions: StdRwLock<HashMap<DeviceId, Arc<Mutex<AdalightSession>>>>,
}

impl AdalightBackend {
    /// Create an empty backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolved plan for an adopted device.
    #[must_use]
    pub fn adopted_plan(&self, id: &DeviceId) -> Option<Arc<AdalightDevicePlan>> {
        self.adopted
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    /// Whether a device has an open session.
    #[must_use]
    pub fn is_connected(&self, id: &DeviceId) -> bool {
        self.session(id).is_some()
    }

    fn session(&self, id: &DeviceId) -> Option<Arc<Mutex<AdalightSession>>> {
        self.sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    fn require_session(&self, id: &DeviceId) -> Result<Arc<Mutex<AdalightSession>>, DeviceError> {
        self.session(id).ok_or_else(|| DeviceError::Disconnected {
            device: id.to_string(),
        })
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
        let device_id = discovered.info.id;
        let Some(raw) = discovered.metadata.get(METADATA_DEVICE_CONFIG) else {
            return Err(DeviceError::NotAdopted { device_id });
        };
        let config: AdalightDeviceConfig = serde_json::from_str(raw).map_err(|error| {
            DeviceError::protocol(
                device_id,
                format!("invalid adalight device metadata: {error}"),
            )
        })?;
        let plan = resolve_device(&config)
            .map_err(|error| DeviceError::protocol(device_id, error.to_string()))?;
        self.adopted
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(device_id, Arc::new(plan));
        Ok(())
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    fn lifecycle_policy(&self, info: &DeviceInfo) -> DeviceLifecyclePolicy {
        let policy = DeviceLifecyclePolicy::default();
        match self.adopted_plan(&info.id) {
            Some(plan) => policy
                .with_connect_timeout(policy.connect_timeout().saturating_add(plan.startup_delay)),
            None => policy,
        }
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let plan = self
            .adopted_plan(id)
            .ok_or(DeviceError::NotAdopted { device_id: *id })?;
        if let Some(existing) = self.session(id) {
            let mut existing = existing.lock().await;
            if *existing.plan == *plan {
                return Ok(());
            }
            // The config changed under a live session: release the old port
            // before reopening, since the open is exclusive.
            existing.close_and_wait().await;
            drop(existing);
            self.sessions
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(id);
        }

        let mut port = open_port(*id, &plan).await?;
        if !plan.startup_delay.is_zero() {
            tokio::time::sleep(plan.startup_delay).await;
            log_greeting(*id, &plan, &mut port).await;
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

        let session = AdalightSession::new(*id, plan, port);
        self.sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id, Arc::new(Mutex::new(session)));
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let session = self
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
            .ok_or_else(|| DeviceError::Disconnected {
                device: id.to_string(),
            })?;
        let mut session = session.lock().await;
        if let Err(error) = session.finish().await {
            debug!(
                device_id = %id,
                port = %session.plan.port,
                error = %error,
                "best-effort serial blackout frame failed"
            );
        }
        session.close_and_wait().await;
        info!(device_id = %id, "Disconnected serial LED controller");
        Ok(())
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        let session = self.require_session(id)?;
        let mut session = session.lock().await;
        session
            .write_colors(colors)
            .await
            .map(|_| ())
            .map_err(|error| DeviceError::write(id, format!("{error:#}")))
    }

    async fn write_colors_shared(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<(), DeviceError> {
        self.write_colors(id, colors.as_slice()).await
    }

    async fn write_colors_shared_outcome(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        let session = self.require_session(id)?;
        let mut session = session.lock().await;
        session
            .write_colors(colors.as_slice())
            .await
            .map_err(|error| DeviceError::write(id, format!("{error:#}")))
    }

    async fn deliver_colors_shared_observed(
        &self,
        device_id: &DeviceId,
        delivery_id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        match self.require_session(device_id) {
            Ok(session) => {
                deliver_observed(
                    *device_id,
                    &session,
                    delivery_id,
                    &colors,
                    observer.as_ref(),
                )
                .await
            }
            Err(error) => DeviceDeliveryAck::rejected(delivery_id, error),
        }
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.adopted_plan(id).map(|plan| plan.effective_max_fps())
    }

    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        self.adopted_plan(id).map(|plan| output_cadence_for(&plan))
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.session(id).map(|session| {
            Arc::new(AdalightFrameSink {
                device_id: *id,
                session,
            }) as Arc<dyn DeviceFrameSink>
        })
    }
}

/// Output cadence for a plan: the line interval as the minimum spacing,
/// with a keepalive resend after [`KEEPALIVE_INTERVAL`] of silence.
#[must_use]
pub fn output_cadence_for(plan: &AdalightDevicePlan) -> OutputCadence {
    OutputCadence::from_min_interval(plan.frame_interval(), plan.effective_max_fps())
        .with_max_frame_silence(KEEPALIVE_INTERVAL)
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

async fn deliver_observed(
    device_id: DeviceId,
    session: &Arc<Mutex<AdalightSession>>,
    delivery_id: DeviceDeliveryId,
    colors: &[[u8; 3]],
    observer: &dyn DeviceDeliveryObserver,
) -> DeviceDeliveryAck {
    let started_at = Instant::now();
    let mut session = session.lock().await;
    let payload_bytes = session.plan.frame_bytes();
    observer.transport_started(delivery_id);
    let result = session
        .write_colors(colors)
        .await
        .map_err(|error| DeviceError::write(device_id, format!("{error:#}")));
    DeviceDeliveryAck::from_write_result(delivery_id, payload_bytes, started_at.elapsed(), result)
}

struct AdalightFrameSink {
    device_id: DeviceId,
    session: Arc<Mutex<AdalightSession>>,
}

#[async_trait::async_trait]
impl DeviceFrameSink for AdalightFrameSink {
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        self.write_colors_shared_outcome(colors).await.map(|_| ())
    }

    async fn write_colors_shared_outcome(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        let mut session = self.session.lock().await;
        session
            .write_colors(colors.as_slice())
            .await
            .map_err(|error| DeviceError::write(self.device_id, format!("{error:#}")))
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        deliver_observed(
            self.device_id,
            &self.session,
            id,
            &colors,
            observer.as_ref(),
        )
        .await
    }
}

// ── Session ─────────────────────────────────────────────────────────────

struct AdalightSession {
    device_id: DeviceId,
    plan: Arc<AdalightDevicePlan>,
    port: Option<SerialStream>,
    encoder: FrameEncoder,
    write_timeout: Duration,
    /// Whether any frame has gone out, so a blackout frame is worth sending.
    has_sent: bool,
    /// Whether the driver reported its output queue depth; once a query
    /// fails the backstop is skipped for the rest of the session.
    queue_depth_supported: bool,
    last_size_mismatch_warn_at: Option<Instant>,
}

impl AdalightSession {
    fn new(device_id: DeviceId, plan: Arc<AdalightDevicePlan>, port: SerialStream) -> Self {
        let encoder = FrameEncoder::new(plan.layout, plan.color_order);
        let write_timeout = plan
            .frame_time()
            .saturating_mul(WRITE_TIMEOUT_FRAMES)
            .max(MIN_WRITE_TIMEOUT);
        Self {
            device_id,
            plan,
            port: Some(port),
            encoder,
            write_timeout,
            has_sent: false,
            queue_depth_supported: true,
            last_size_mismatch_warn_at: None,
        }
    }

    async fn write_colors(&mut self, colors: &[[u8; 3]]) -> Result<DeviceWriteOutcome> {
        self.warn_on_size_mismatch(colors.len());
        if self.line_backlogged() {
            return Ok(DeviceWriteOutcome::SuppressedCadence);
        }
        self.encoder.encode(colors);
        self.send_frame().await?;
        self.has_sent = true;
        Ok(DeviceWriteOutcome::Sent)
    }

    /// Send black frames so the strip goes dark when output stops.
    async fn finish(&mut self) -> Result<()> {
        if !self.has_sent || self.port.is_none() {
            return Ok(());
        }
        self.encoder.encode_black();
        for _ in 0..BLACKOUT_FRAMES {
            self.send_frame().await?;
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

    /// Close the port and wait, within a bound, until it is released, so a
    /// reconnect does not race the old descriptor's exclusive lock. A
    /// wedged driver can hold a tty close for its full closing wait; past
    /// the bound the close finishes in the background.
    async fn close_and_wait(&mut self) {
        let bound = self
            .plan
            .frame_time()
            .saturating_mul(u32::try_from(BLACKOUT_FRAMES + 1).unwrap_or(u32::MAX))
            .saturating_add(CLOSE_WAIT_MARGIN);
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

    async fn send_frame(&mut self) -> Result<()> {
        let port = self
            .port
            .as_mut()
            .ok_or_else(|| anyhow!("serial port is closed"))?;
        tokio::time::timeout(self.write_timeout, port.write_all(self.encoder.as_bytes()))
            .await
            .with_context(|| {
                format!(
                    "serial write to {} stalled for {} ms",
                    self.plan.port,
                    self.write_timeout.as_millis()
                )
            })?
            .with_context(|| format!("serial write to {} failed", self.plan.port))
    }

    /// Whether the driver still holds a whole frame of unsent bytes.
    fn line_backlogged(&mut self) -> bool {
        if !self.queue_depth_supported {
            return false;
        }
        let Some(port) = self.port.as_ref() else {
            return false;
        };
        match port.bytes_to_write() {
            Ok(queued) => queued as usize >= self.plan.frame_bytes(),
            Err(error) => {
                debug!(
                    device_id = %self.device_id,
                    port = %self.plan.port,
                    error = %error,
                    "serial driver does not report its output queue; relying on cadence alone"
                );
                self.queue_depth_supported = false;
                false
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

impl Drop for AdalightSession {
    fn drop(&mut self) {
        drop(self.close());
    }
}
