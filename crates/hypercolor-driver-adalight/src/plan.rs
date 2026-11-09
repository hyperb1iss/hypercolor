//! Resolved device plans: a validated config entry plus its line budget.
//!
//! Serial bandwidth is a physical limit. A frame of `B` bytes at `R` baud
//! occupies the line for `10 * B / R` seconds with 8N1 framing, so the
//! plan computes the fastest cadence the line can carry and the backend
//! paces to it. When the render loop is faster, frames coalesce to the
//! newest one instead of queueing behind the UART.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::color::ColorOrder;
use crate::config::{
    AdalightConfig, AdalightConfigError, AdalightDeviceConfig, MAX_BAUD_RATE, MAX_DEVICES,
    MAX_FPS_LIMIT, MAX_LATCH_MS, MAX_NAME_LEN, MAX_PORT_LEN, MAX_STARTUP_DELAY_MS, MIN_BAUD_RATE,
    WhiteMode, is_valid_device_id,
};
use crate::protocol::{
    AdalightProtocol, AwaWhite, Bandwidth, FrameLayout, Framing, WhiteCalibration,
};

/// Longest a single frame may occupy the line. A layout slower than one
/// frame per second cannot animate, so config validation rejects it.
pub const MAX_FRAME_TIME: Duration = Duration::from_secs(1);

/// Most black frames sent on disconnect. HyperSerial never blanks on idle
/// and drops corrupt frames, and Adalight-FastLED drops frames that arrive
/// while it latches, so any one surviving copy turns the strip off.
pub const MAX_BLACKOUT_FRAMES: usize = 3;

/// Line time the blackout frames may occupy together, so a disconnect
/// drains well inside the daemon's two-second disconnect timeout.
pub const BLACKOUT_LINE_BUDGET: Duration = Duration::from_millis(500);

/// Fully resolved plan for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdalightDevicePlan {
    /// Stable device id from config.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Serial port path.
    pub port: String,
    /// Line rate in baud.
    pub baud_rate: u32,
    /// Frame shape.
    pub layout: FrameLayout,
    /// Wire byte order of each LED.
    pub color_order: ColorOrder,
    /// White channel handling.
    pub white: WhiteMode,
    /// Wait after opening the port before the first frame.
    pub startup_delay: Duration,
    /// Idle gap after each frame.
    pub latch: Duration,
    /// User frame rate cap; 0 means none.
    pub max_fps: u32,
}

impl AdalightDevicePlan {
    /// Wire protocol variant.
    #[must_use]
    pub const fn protocol(&self) -> AdalightProtocol {
        self.layout.framing().protocol()
    }

    /// LEDs on the strip.
    #[must_use]
    pub const fn led_count(&self) -> u32 {
        self.layout.led_count()
    }

    /// Bytes per frame on the wire.
    #[must_use]
    pub const fn frame_bytes(&self) -> usize {
        self.layout.frame_len()
    }

    /// Serial line budget for this layout.
    #[must_use]
    pub const fn bandwidth(&self) -> Bandwidth {
        self.layout.bandwidth(self.baud_rate)
    }

    /// Time one frame occupies the line.
    #[must_use]
    pub const fn frame_time(&self) -> Duration {
        self.bandwidth().frame_time()
    }

    /// Shortest interval the line allows between frame starts: the frame
    /// time plus the configured latch gap.
    #[must_use]
    pub fn line_interval(&self) -> Duration {
        self.frame_time().saturating_add(self.latch)
    }

    /// Fastest frame rate the serial line carries, ignoring any user cap.
    #[must_use]
    pub fn line_max_fps(&self) -> u32 {
        fps_for_interval(self.line_interval())
    }

    /// Interval the backend paces frames at: the line interval, or the
    /// user cap's interval when that is longer.
    #[must_use]
    pub fn frame_interval(&self) -> Duration {
        let line = self.line_interval();
        if self.max_fps == 0 {
            return line;
        }
        line.max(Duration::from_secs(1) / self.max_fps)
    }

    /// Black frames sent on disconnect: as many as fit in
    /// [`BLACKOUT_LINE_BUDGET`] of line time, from one to
    /// [`MAX_BLACKOUT_FRAMES`].
    #[must_use]
    pub fn blackout_frames(&self) -> usize {
        let frame = self.frame_time().as_micros().max(1);
        usize::try_from(BLACKOUT_LINE_BUDGET.as_micros() / frame)
            .unwrap_or(MAX_BLACKOUT_FRAMES)
            .clamp(1, MAX_BLACKOUT_FRAMES)
    }

    /// Frame rate the device is driven at: the line limit, or the user cap
    /// when that is lower.
    #[must_use]
    pub fn effective_max_fps(&self) -> u32 {
        let line = self.line_max_fps();
        if self.max_fps == 0 {
            line
        } else {
            line.min(self.max_fps)
        }
    }
}

fn fps_for_interval(interval: Duration) -> u32 {
    let micros = interval.as_micros();
    if micros == 0 {
        return u32::MAX;
    }
    u32::try_from(1_000_000 / micros).unwrap_or(u32::MAX)
}

/// Validate one device entry and resolve its plan.
///
/// # Errors
///
/// Returns [`AdalightConfigError::Device`] describing the first problem.
pub fn resolve_device(
    device: &AdalightDeviceConfig,
) -> Result<AdalightDevicePlan, AdalightConfigError> {
    let id = device.id.as_str();
    if !is_valid_device_id(id) {
        return Err(AdalightConfigError::device(
            id,
            "id must be 1-64 ASCII letters, digits, '-', or '_'",
        ));
    }
    if device
        .name
        .as_deref()
        .is_some_and(|name| name.chars().count() > MAX_NAME_LEN)
    {
        return Err(AdalightConfigError::device(
            id,
            format!("name is longer than {MAX_NAME_LEN} characters"),
        ));
    }

    let port = validate_port(id, &device.port)?;
    let baud_rate = device.effective_baud_rate();
    if !(MIN_BAUD_RATE..=MAX_BAUD_RATE).contains(&baud_rate) {
        return Err(AdalightConfigError::device(
            id,
            format!("baud_rate {baud_rate} is outside {MIN_BAUD_RATE}-{MAX_BAUD_RATE}"),
        ));
    }

    let framing = framing_for(device)?;
    let layout = FrameLayout::new(framing, device.led_count)
        .map_err(|error| AdalightConfigError::device(id, error.to_string()))?;

    let startup_delay_ms = device.startup_delay_ms.unwrap_or(0);
    if startup_delay_ms > MAX_STARTUP_DELAY_MS {
        return Err(AdalightConfigError::device(
            id,
            format!("startup_delay_ms {startup_delay_ms} is above {MAX_STARTUP_DELAY_MS}"),
        ));
    }
    let latch_ms = device.latch_ms.unwrap_or(0);
    if latch_ms > MAX_LATCH_MS {
        return Err(AdalightConfigError::device(
            id,
            format!("latch_ms {latch_ms} is above {MAX_LATCH_MS}"),
        ));
    }
    let max_fps = device.max_fps.unwrap_or(0);
    if max_fps > MAX_FPS_LIMIT {
        return Err(AdalightConfigError::device(
            id,
            format!("max_fps {max_fps} is above {MAX_FPS_LIMIT}"),
        ));
    }

    let plan = AdalightDevicePlan {
        id: id.to_owned(),
        name: device.display_name().to_owned(),
        port,
        baud_rate,
        layout,
        color_order: device.color_order,
        white: device.white,
        startup_delay: Duration::from_millis(u64::from(startup_delay_ms)),
        latch: Duration::from_millis(u64::from(latch_ms)),
        max_fps,
    };

    let frame_time = plan.frame_time();
    if frame_time > MAX_FRAME_TIME {
        return Err(AdalightConfigError::device(
            id,
            format!(
                "one {}-byte frame takes {} ms at {baud_rate} baud, so the line cannot carry \
                 one frame per second; raise baud_rate or lower led_count",
                plan.frame_bytes(),
                frame_time.as_millis()
            ),
        ));
    }
    Ok(plan)
}

fn framing_for(device: &AdalightDeviceConfig) -> Result<Framing, AdalightConfigError> {
    match (device.protocol, device.white) {
        (AdalightProtocol::Ada, WhiteMode::Off) => Ok(Framing::Ada),
        (AdalightProtocol::Ada, WhiteMode::Firmware | WhiteMode::Host) => {
            Err(AdalightConfigError::device(
                &device.id,
                "white channel modes need protocol = \"awa\"; Adalight firmware reads three \
                 bytes per LED",
            ))
        }
        (AdalightProtocol::Awa, WhiteMode::Off) => Ok(Framing::Awa(AwaWhite::Off)),
        (AdalightProtocol::Awa, WhiteMode::Firmware) => Ok(Framing::Awa(AwaWhite::Firmware(
            device
                .white_calibration
                .unwrap_or(WhiteCalibration::COLD_WHITE),
        ))),
        (AdalightProtocol::Awa, WhiteMode::Host) => Ok(Framing::Awa(AwaWhite::Host)),
    }
}

fn validate_port(id: &str, port: &str) -> Result<String, AdalightConfigError> {
    let trimmed = port.trim();
    if trimmed.is_empty() {
        return Err(AdalightConfigError::device(id, "port must not be empty"));
    }
    if trimmed.len() > MAX_PORT_LEN {
        return Err(AdalightConfigError::device(
            id,
            format!("port is longer than {MAX_PORT_LEN} bytes"),
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(AdalightConfigError::device(
            id,
            "port must not contain control characters",
        ));
    }
    Ok(trimmed.to_owned())
}

/// Validate a whole driver config and resolve every device plan, in order.
///
/// # Errors
///
/// Returns the first device error, a duplicate id, a duplicate port, or a
/// device count above [`MAX_DEVICES`].
pub fn resolve_config(
    config: &AdalightConfig,
) -> Result<Vec<AdalightDevicePlan>, AdalightConfigError> {
    if config.devices.len() > MAX_DEVICES {
        return Err(AdalightConfigError::TooManyDevices {
            count: config.devices.len(),
        });
    }
    let mut ids = HashSet::new();
    let mut ports: HashMap<String, String> = HashMap::new();
    let mut plans = Vec::with_capacity(config.devices.len());
    for device in &config.devices {
        let plan = resolve_device(device)?;
        if !ids.insert(plan.id.clone()) {
            return Err(AdalightConfigError::DuplicateId { id: plan.id });
        }
        if let Some(first) = ports.insert(plan.port.clone(), plan.id.clone()) {
            return Err(AdalightConfigError::DuplicatePort {
                first,
                second: plan.id,
                port: plan.port,
            });
        }
        plans.push(plan);
    }
    Ok(plans)
}
