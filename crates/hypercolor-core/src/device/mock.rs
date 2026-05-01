//! Mock device backend and transport scanner for integration testing.
//!
//! Provides configurable mock implementations of [`DeviceBackend`],
//! [`TransportScanner`], and [`EffectRenderer`] that simulate realistic
//! device behavior without real hardware. Every call is tracked for
//! test assertions.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Result, bail};

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, Rgba};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::effect::{ControlValue, EffectMetadata};
use hypercolor_types::spatial::LedTopology;

use super::traits::{BackendInfo, DeviceBackend};
use crate::device::{DiscoveredDevice, DiscoveryConnectBehavior, TransportScanner};
use crate::effect::{EffectRenderer, FrameInput};

// ── Call Tracking ───────────────────────────────────────────────────────────

/// A recorded method call on the mock backend, for test assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockCall {
    /// `info()` was called.
    Info,
    /// `discover()` was called.
    Discover,
    /// `connect(id)` was called.
    Connect(DeviceId),
    /// `disconnect(id)` was called.
    Disconnect(DeviceId),
    /// `write_colors(id, count)` — stores the device ID and color count.
    WriteColors {
        device_id: DeviceId,
        led_count: usize,
    },
}

// ── MockDeviceConfig ────────────────────────────────────────────────────────

/// Configuration for a single mock device.
#[derive(Debug, Clone)]
pub struct MockDeviceConfig {
    /// Device display name.
    pub name: String,
    /// Number of LEDs.
    pub led_count: u32,
    /// LED topology for the device zone.
    pub topology: LedTopology,
    /// Pre-assigned device ID (generated if `None`).
    pub id: Option<DeviceId>,
}

// ── MockDeviceBackend ───────────────────────────────────────────────────────

/// A configurable mock [`DeviceBackend`] for testing the full pipeline.
///
/// Tracks connection state, records every call, and stores the last
/// frame written to each device for inspection.
///
/// # Example
///
/// ```rust,ignore
/// let mut backend = MockDeviceBackend::new()
///     .with_device("LED Strip", 60, LedTopology::Strip)
///     .with_device("Matrix", 100, LedTopology::Matrix { rows: 10, cols: 10 });
/// ```
pub struct MockDeviceBackend {
    /// Pre-configured devices this backend will "discover".
    devices: Vec<DeviceInfo>,

    /// Currently connected device IDs.
    connected: HashSet<DeviceId>,

    /// Last colors written per device.
    last_colors: HashMap<DeviceId, Vec<[u8; 3]>>,

    /// Total `write_colors` call count.
    write_count: u64,

    /// Ordered call log for assertions.
    calls: Vec<MockCall>,

    /// If `true`, `connect` calls will fail.
    pub fail_connect: bool,

    /// If `true`, `write_colors` calls will fail.
    pub fail_write: bool,
}

impl MockDeviceBackend {
    /// Create a new empty mock backend with no pre-configured devices.
    #[must_use]
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            connected: HashSet::new(),
            last_colors: HashMap::new(),
            write_count: 0,
            calls: Vec::new(),
            fail_connect: false,
            fail_write: false,
        }
    }

    /// Add a mock device with the given name, LED count, and topology.
    ///
    /// Returns `self` for builder-style chaining.
    #[must_use]
    pub fn with_device(mut self, config: &MockDeviceConfig) -> Self {
        let id = config.id.unwrap_or_default();
        let info = build_device_info(id, &config.name, config.led_count, &config.topology);
        self.devices.push(info);
        self
    }

    /// Returns the ordered call log for test assertions.
    #[must_use]
    pub fn calls(&self) -> &[MockCall] {
        &self.calls
    }

    /// Returns the total number of `write_colors` calls across all devices.
    #[must_use]
    pub fn write_count(&self) -> u64 {
        self.write_count
    }

    /// Returns the last colors written to a specific device.
    #[must_use]
    pub fn last_colors(&self, id: &DeviceId) -> Option<&Vec<[u8; 3]>> {
        self.last_colors.get(id)
    }

    /// Check whether a device is currently connected.
    #[must_use]
    pub fn is_connected(&self, id: &DeviceId) -> bool {
        self.connected.contains(id)
    }

    /// Returns the list of configured device infos (for test setup).
    #[must_use]
    pub fn device_infos(&self) -> &[DeviceInfo] {
        &self.devices
    }
}

impl Default for MockDeviceBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl DeviceBackend for MockDeviceBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "mock".to_owned(),
            name: "Mock Device Backend".to_owned(),
            description: "Simulated devices for testing — no real hardware required".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        self.calls.push(MockCall::Discover);
        Ok(self.devices.clone())
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        self.calls.push(MockCall::Connect(*id));

        if self.fail_connect {
            bail!("mock connect failure for device {id}");
        }
        if self.connected.contains(id) {
            bail!("device {id} is already connected");
        }
        // Verify the device is actually known
        if !self.devices.iter().any(|d| d.id == *id) {
            bail!("device {id} not found in mock backend");
        }
        self.connected.insert(*id);
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        self.calls.push(MockCall::Disconnect(*id));

        if !self.connected.remove(id) {
            bail!("device {id} is not connected");
        }
        self.last_colors.remove(id);
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        self.calls.push(MockCall::WriteColors {
            device_id: *id,
            led_count: colors.len(),
        });

        if self.fail_write {
            bail!("mock write failure for device {id}");
        }
        if !self.connected.contains(id) {
            bail!("cannot write to disconnected device {id}");
        }

        self.write_count += 1;
        self.last_colors.insert(*id, colors.to_vec());
        Ok(())
    }
}

// ── MockTransportScanner ────────────────────────────────────────────────────

/// A configurable mock [`TransportScanner`] for discovery tests.
///
/// Returns a pre-built list of [`DiscoveredDevice`] entries on scan,
/// or fails if configured to do so.
pub struct MockTransportScanner {
    /// Scanner name for logging.
    scanner_name: String,

    /// Devices this scanner will "find".
    devices: Vec<DiscoveredDevice>,

    /// If `true`, `scan()` returns an error.
    pub should_fail: bool,
}

impl MockTransportScanner {
    /// Create a new mock scanner with no devices.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            scanner_name: name.to_owned(),
            devices: Vec::new(),
            should_fail: false,
        }
    }

    /// Add a discovered device to the scanner's result set.
    #[must_use]
    pub fn with_device(mut self, config: &MockDeviceConfig) -> Self {
        let id = config.id.unwrap_or_default();
        let info = build_device_info(id, &config.name, config.led_count, &config.topology);
        let fingerprint_key = format!(
            "mock:{}:{}",
            config.name.to_lowercase().replace(' ', "-"),
            id
        );

        self.devices.push(DiscoveredDevice {
            origin: info.origin.clone(),
            name: config.name.clone(),
            family: DeviceFamily::named("Mock"),
            fingerprint: DeviceFingerprint(fingerprint_key),
            connect_behavior: DiscoveryConnectBehavior::AutoConnect,
            info,
            metadata: HashMap::new(),
        });
        self
    }
}

#[async_trait::async_trait]
impl TransportScanner for MockTransportScanner {
    fn name(&self) -> &str {
        &self.scanner_name
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        if self.should_fail {
            bail!("mock scanner '{}' failed", self.scanner_name);
        }
        Ok(self.devices.clone())
    }
}

// ── MockEffectRenderer ──────────────────────────────────────────────────────

/// Rendering mode for the mock effect renderer.
#[derive(Debug, Clone)]
pub enum MockRenderMode {
    /// Fill the entire canvas with a single solid color.
    Solid([u8; 4]),

    /// Horizontal rainbow gradient (hue varies along x-axis).
    RainbowGradient,

    /// Simulate audio reactivity: brightness scales with `rms_level`.
    AudioReactive {
        /// Base color (modulated by audio level).
        base_color: [u8; 4],
    },
}

/// A configurable mock [`EffectRenderer`] for testing the effect pipeline.
///
/// Produces real canvas data according to the selected [`MockRenderMode`].
/// Tracks lifecycle calls for assertions.
pub struct MockEffectRenderer {
    /// Active render mode.
    mode: MockRenderMode,

    /// Whether `init()` has been called.
    pub initialized: bool,

    /// Whether `destroy()` has been called.
    pub destroyed: bool,

    /// Total number of `tick()` calls.
    pub tick_count: u64,

    /// Current control values.
    pub controls: HashMap<String, ControlValue>,

    /// If set, `init()` returns this error.
    pub init_error: Option<String>,
}

impl MockEffectRenderer {
    /// Create a new mock renderer with the given render mode.
    #[must_use]
    pub fn new(mode: MockRenderMode) -> Self {
        Self {
            mode,
            initialized: false,
            destroyed: false,
            tick_count: 0,
            controls: HashMap::new(),
            init_error: None,
        }
    }

    /// Create a solid-color renderer (convenience).
    #[must_use]
    pub fn solid(r: u8, g: u8, b: u8) -> Self {
        Self::new(MockRenderMode::Solid([r, g, b, 255]))
    }

    /// Create a rainbow gradient renderer (convenience).
    #[must_use]
    pub fn rainbow() -> Self {
        Self::new(MockRenderMode::RainbowGradient)
    }

    /// Create an audio-reactive renderer (convenience).
    #[must_use]
    pub fn audio_reactive(r: u8, g: u8, b: u8) -> Self {
        Self::new(MockRenderMode::AudioReactive {
            base_color: [r, g, b, 255],
        })
    }

    /// Returns a sample [`EffectMetadata`] for activating this renderer.
    #[must_use]
    pub fn sample_metadata(name: &str) -> EffectMetadata {
        use hypercolor_types::effect::{EffectCategory, EffectId, EffectSource};
        EffectMetadata {
            id: EffectId::new(uuid::Uuid::now_v7()),
            name: name.to_owned(),
            author: "hypercolor-test".to_owned(),
            version: "0.1.0".to_owned(),
            description: format!("Mock effect: {name}"),
            category: EffectCategory::Utility,
            tags: vec!["test".to_owned(), "mock".to_owned()],
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from(format!("mock/{name}.wgsl")),
            },
            license: Some("Apache-2.0".to_owned()),
        }
    }
}

impl EffectRenderer for MockEffectRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> Result<()> {
        if let Some(ref msg) = self.init_error {
            return Err(anyhow::anyhow!("{msg}"));
        }
        self.initialized = true;
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> Result<()> {
        self.tick_count += 1;
        if canvas.width() != input.canvas_width || canvas.height() != input.canvas_height {
            *canvas = Canvas::new(input.canvas_width, input.canvas_height);
        }

        match &self.mode {
            MockRenderMode::Solid(rgba) => {
                canvas.fill(Rgba::new(rgba[0], rgba[1], rgba[2], rgba[3]));
            }
            MockRenderMode::RainbowGradient => {
                render_rainbow(canvas, input);
            }
            MockRenderMode::AudioReactive { base_color } => {
                render_audio_reactive(canvas, input, *base_color);
            }
        }

        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        self.controls.insert(name.to_owned(), value.clone());
    }

    fn destroy(&mut self) {
        self.destroyed = true;
    }
}

// ── Render Helpers ──────────────────────────────────────────────────────────

/// Render a horizontal rainbow gradient across the canvas.
///
/// Hue varies from 0 to 360 degrees along the x-axis; saturation and
/// value are fixed at 1.0. The gradient shifts over time for animation.
#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
fn render_rainbow(canvas: &mut Canvas, input: &FrameInput) {
    let w = canvas.width();
    let time_offset = input.time_secs * 30.0; // Slow drift
    let row_len = w as usize * BYTES_PER_PIXEL;

    if row_len == 0 {
        return;
    }

    let pixels = canvas.as_rgba_bytes_mut();
    let (first_row, remaining_rows) = pixels.split_at_mut(row_len);
    for (x, pixel) in first_row.chunks_exact_mut(BYTES_PER_PIXEL).enumerate() {
        let hue = ((x as f32 / w.max(1) as f32) * 360.0 + time_offset).rem_euclid(360.0);
        let (r, g, b) = hsv_to_rgb(hue, 1.0, 1.0);
        pixel[0] = r;
        pixel[1] = g;
        pixel[2] = b;
        pixel[3] = 255;
    }

    for row in remaining_rows.chunks_exact_mut(row_len) {
        row.copy_from_slice(first_row);
    }
}

/// Render an audio-reactive solid fill. Brightness scales with RMS level.
#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn render_audio_reactive(canvas: &mut Canvas, input: &FrameInput, base: [u8; 4]) {
    let level = input.audio.rms_level.clamp(0.0, 1.0);
    let r = (f32::from(base[0]) * level).round() as u8;
    let g = (f32::from(base[1]) * level).round() as u8;
    let b = (f32::from(base[2]) * level).round() as u8;
    canvas.fill(Rgba::new(r, g, b, base[3]));
}

/// Simple HSV to RGB conversion. H in [0, 360), S and V in [0, 1].
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let m = v - c;

    #[allow(clippy::cast_precision_loss)]
    let (r1, g1, b1) = if h_prime < 1.0 {
        (c, x, 0.0)
    } else if h_prime < 2.0 {
        (x, c, 0.0)
    } else if h_prime < 3.0 {
        (0.0, c, x)
    } else if h_prime < 4.0 {
        (0.0, x, c)
    } else if h_prime < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    (
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    )
}

// ── Shared Helpers ──────────────────────────────────────────────────────────

/// Convert a spatial [`LedTopology`] to a device-level [`DeviceTopologyHint`].
fn spatial_to_device_topology(topology: &LedTopology) -> DeviceTopologyHint {
    match topology {
        LedTopology::Strip { .. } => DeviceTopologyHint::Strip,
        LedTopology::Matrix { width, height, .. } => DeviceTopologyHint::Matrix {
            rows: *height,
            cols: *width,
        },
        LedTopology::Ring { count, .. } => DeviceTopologyHint::Ring { count: *count },
        LedTopology::Point => DeviceTopologyHint::Point,
        LedTopology::Custom { .. }
        | LedTopology::ConcentricRings { .. }
        | LedTopology::PerimeterLoop { .. } => DeviceTopologyHint::Custom,
    }
}

/// Build a [`DeviceInfo`] from mock parameters.
fn build_device_info(
    id: DeviceId,
    name: &str,
    led_count: u32,
    topology: &LedTopology,
) -> DeviceInfo {
    let device_topology = spatial_to_device_topology(topology);

    DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "Hypercolor Mock".to_owned(),
        family: DeviceFamily::named("Mock"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("mock", "mock", ConnectionType::Network),
        zones: vec![ZoneInfo {
            name: format!("{name} Zone"),
            led_count,
            topology: device_topology,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("mock-1.0.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}
