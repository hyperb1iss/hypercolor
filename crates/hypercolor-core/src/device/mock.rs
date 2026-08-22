//! Mock device backend and discovery source for integration testing.
//!
//! Provides configurable mock implementations of [`DeviceBackend`],
//! discovery sources, and [`EffectRenderer`] that simulate realistic
//! device behavior without real hardware. Every call is tracked for
//! test assertions.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use anyhow::{Result, bail};

use hypercolor_color::Hsv;
use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, Rgba};
use hypercolor_types::control::{ControlDeltaBatch, ControlValue};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint,
    FingerprintNamespace, SegmentInfo,
};
use hypercolor_types::effect::EffectMetadata;
use hypercolor_types::spatial::LedTopology;

use super::traits::{BackendInfo, DeviceBackend};
use crate::device::{DiscoveredDevice, DiscoveryConnectBehavior};
use crate::effect::{EffectRenderer, FrameInput};

// ── Call Tracking ───────────────────────────────────────────────────────────

/// A recorded method call on the mock backend, for test assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockCall {
    /// `info()` was called.
    Info,
    /// A discovered device was adopted by the backend.
    Adopt(DeviceId),
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
    /// LED topology for the device segment.
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
    state: Mutex<MockDeviceState>,

    /// If `true`, `connect` calls will fail.
    pub fail_connect: bool,

    /// If `true`, `write_colors` calls will fail.
    pub fail_write: bool,
}

#[derive(Default)]
struct MockDeviceState {
    connected: HashSet<DeviceId>,
    last_colors: HashMap<DeviceId, Vec<[u8; 3]>>,
    write_count: u64,
    calls: Vec<MockCall>,
}

impl MockDeviceBackend {
    /// Create a new empty mock backend with no pre-configured devices.
    #[must_use]
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            state: Mutex::new(MockDeviceState::default()),
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
    pub fn calls(&self) -> Vec<MockCall> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .clone()
    }

    /// Returns the total number of `write_colors` calls across all devices.
    #[must_use]
    pub fn write_count(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write_count
    }

    /// Returns the last colors written to a specific device.
    #[must_use]
    pub fn last_colors(&self, id: &DeviceId) -> Option<Vec<[u8; 3]>> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last_colors
            .get(id)
            .cloned()
    }

    /// Check whether a device is currently connected.
    #[must_use]
    pub fn is_connected(&self, id: &DeviceId) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .connected
            .contains(id)
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

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .push(MockCall::Adopt(discovered.info.id));
        if self
            .devices
            .iter()
            .any(|device| device.id == discovered.info.id)
        {
            Ok(())
        } else {
            Err(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })
        }
    }

    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.calls.push(MockCall::Connect(*id));

        if self.fail_connect {
            return Err(DeviceError::connection(id, "mock connect failure"));
        }
        if state.connected.contains(id) {
            return Err(DeviceError::connection(id, "device is already connected"));
        }
        // Verify the device is actually known
        if !self.devices.iter().any(|d| d.id == *id) {
            return Err(DeviceError::NotFound {
                device: id.to_string(),
            });
        }
        state.connected.insert(*id);
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.calls.push(MockCall::Disconnect(*id));

        if !state.connected.remove(id) {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        }
        state.last_colors.remove(id);
        Ok(())
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.calls.push(MockCall::WriteColors {
            device_id: *id,
            led_count: colors.len(),
        });

        if self.fail_write {
            return Err(DeviceError::write(id, "mock write failure"));
        }
        if !state.connected.contains(id) {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        }

        state.write_count += 1;
        state.last_colors.insert(*id, colors.to_vec());
        Ok(())
    }
}

// ── MockDiscoverySource ─────────────────────────────────────────────────────

/// A configurable discovery source for tests.
///
/// Returns a pre-built list of [`DiscoveredDevice`] entries on scan,
/// or fails if configured to do so.
pub struct MockDiscoverySource {
    /// Scanner name for logging.
    source_name: String,

    /// Devices this scanner will "find".
    devices: Vec<DiscoveredDevice>,

    /// If `true`, `scan()` returns an error.
    pub should_fail: bool,
}

impl MockDiscoverySource {
    /// Create a new mock discovery source with no devices.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            source_name: name.to_owned(),
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
            fingerprint: DeviceFingerprint::mint(
                FingerprintNamespace::Bridge,
                "mock",
                &fingerprint_key,
            ),
            connect_behavior: DiscoveryConnectBehavior::AutoConnect,
            info,
            metadata: HashMap::new(),
            // Deliberate refusal: simulated hardware has no portable identity.
            claim: None,
        });
        self
    }
}

impl MockDiscoverySource {
    /// Return the source name used in discovery reports.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.source_name
    }

    /// Produce this source's configured discovery results.
    pub fn scan(&self) -> Result<Vec<DiscoveredDevice>> {
        if self.should_fail {
            bail!("mock discovery source '{}' failed", self.source_name);
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
            input_reactive: false,
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

    fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>) -> anyhow::Result<()> {
        self.controls.extend(
            batch
                .changes
                .iter()
                .map(|(control_id, value)| (control_id.to_string(), value.clone())),
        );
        Ok(())
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
    let time_offset = (input.time_secs * 30.0).rem_euclid(360.0) as f32; // Slow drift
    let row_len = w as usize * BYTES_PER_PIXEL;

    if row_len == 0 {
        return;
    }

    let pixels = canvas.as_rgba_bytes_mut();
    let (first_row, remaining_rows) = pixels.split_at_mut(row_len);
    for (x, pixel) in first_row.chunks_exact_mut(BYTES_PER_PIXEL).enumerate() {
        let hue = ((x as f32 / w.max(1) as f32) * 360.0 + time_offset).rem_euclid(360.0);
        let rgb = Hsv::new(hue, 1.0, 1.0).to_rgb();
        pixel[0] = rgb.r;
        pixel[1] = rgb.g;
        pixel[2] = rgb.b;
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
        segments: vec![SegmentInfo {
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
