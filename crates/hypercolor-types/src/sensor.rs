//! System sensor snapshot types shared across the daemon, effects, and API.

use serde::{Deserialize, Serialize};

/// Published system snapshot shared across render, API, and overlays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemSnapshot {
    /// Aggregate CPU load across all cores (0.0–100.0).
    pub cpu_load_percent: f32,
    /// Per-core CPU load percentages.
    pub cpu_loads: Vec<f32>,
    /// CPU package temperature, if available.
    pub cpu_temp_celsius: Option<f32>,
    /// GPU temperature, if available.
    pub gpu_temp_celsius: Option<f32>,
    /// GPU load percentage, if available.
    pub gpu_load_percent: Option<f32>,
    /// GPU VRAM used in megabytes, if available.
    pub gpu_vram_used_mb: Option<f32>,
    /// RAM usage percentage (0.0–100.0).
    pub ram_used_percent: f32,
    /// RAM used in megabytes.
    pub ram_used_mb: f64,
    /// RAM total in megabytes.
    pub ram_total_mb: f64,
    /// Raw component readings collected from the host.
    pub components: Vec<SensorReading>,
    /// Unix timestamp in milliseconds when the snapshot was polled.
    pub polled_at_ms: u64,
}

impl SystemSnapshot {
    /// Create an empty-but-valid snapshot suitable for startup defaults.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            cpu_load_percent: 0.0,
            cpu_loads: Vec::new(),
            cpu_temp_celsius: None,
            gpu_temp_celsius: None,
            gpu_load_percent: None,
            gpu_vram_used_mb: None,
            ram_used_percent: 0.0,
            ram_used_mb: 0.0,
            ram_total_mb: 0.0,
            components: Vec::new(),
            polled_at_ms: 0,
        }
    }

    /// Return a flattened list of well-known and raw readings.
    #[must_use]
    pub fn readings(&self) -> Vec<SensorReading> {
        let mut readings = Vec::with_capacity(self.components.len().saturating_add(8));

        readings.push(SensorReading::new(
            "cpu_load",
            self.cpu_load_percent,
            SensorUnit::Percent,
            Some(0.0),
            Some(100.0),
            None,
        ));

        for (index, load) in self.cpu_loads.iter().copied().enumerate() {
            readings.push(SensorReading::new(
                format!("cpu_core_{index}_load"),
                load,
                SensorUnit::Percent,
                Some(0.0),
                Some(100.0),
                None,
            ));
        }

        if let Some(value) = self.cpu_temp_celsius {
            readings.push(SensorReading::new(
                "cpu_temp",
                value,
                SensorUnit::Celsius,
                None,
                None,
                None,
            ));
        }

        if let Some(value) = self.gpu_temp_celsius {
            readings.push(SensorReading::new(
                "gpu_temp",
                value,
                SensorUnit::Celsius,
                None,
                None,
                None,
            ));
        }

        if let Some(value) = self.gpu_load_percent {
            readings.push(SensorReading::new(
                "gpu_load",
                value,
                SensorUnit::Percent,
                Some(0.0),
                Some(100.0),
                None,
            ));
        }

        if let Some(value) = self.gpu_vram_used_mb {
            readings.push(SensorReading::new(
                "gpu_vram_used",
                value,
                SensorUnit::Megabytes,
                Some(0.0),
                None,
                None,
            ));
        }

        readings.push(SensorReading::new(
            "ram_used",
            self.ram_used_percent,
            SensorUnit::Percent,
            Some(0.0),
            Some(100.0),
            None,
        ));
        if let Some(ram_used_mb) = narrow_sensor_scalar(self.ram_used_mb) {
            readings.push(SensorReading::new(
                "ram_used_mb",
                ram_used_mb,
                SensorUnit::Megabytes,
                Some(0.0),
                narrow_sensor_scalar(self.ram_total_mb),
                None,
            ));
        }

        readings.extend(self.components.iter().cloned());
        readings
    }

    /// Find a reading by its well-known or normalized raw label.
    #[must_use]
    pub fn reading(&self, label: &str) -> Option<SensorReading> {
        let wanted = normalize_sensor_label(label);
        self.readings()
            .into_iter()
            .find(|reading| normalize_sensor_label(&reading.label) == wanted)
    }
}

impl Default for SystemSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

/// A single host sensor reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorReading {
    /// Stable sensor label.
    pub label: String,
    /// Current sensor value.
    pub value: f32,
    /// Sensor unit.
    pub unit: SensorUnit,
    /// Expected minimum value, if known.
    pub min: Option<f32>,
    /// Expected maximum value, if known.
    pub max: Option<f32>,
    /// Critical threshold, if known.
    pub critical: Option<f32>,
}

impl SensorReading {
    /// Build a single sensor reading.
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        value: f32,
        unit: SensorUnit,
        min: Option<f32>,
        max: Option<f32>,
        critical: Option<f32>,
    ) -> Self {
        Self {
            label: label.into(),
            value,
            unit,
            min,
            max,
            critical,
        }
    }
}

/// Units exposed by system sensors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorUnit {
    Celsius,
    Percent,
    Megabytes,
    Rpm,
    Watts,
    Mhz,
}

impl SensorUnit {
    /// Human-readable unit symbol used by the REST API and `LightScript`.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Celsius => "°C",
            Self::Percent => "%",
            Self::Megabytes => "MB",
            Self::Rpm => "RPM",
            Self::Watts => "W",
            Self::Mhz => "MHz",
        }
    }
}

fn normalize_sensor_label(label: &str) -> String {
    label
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
fn narrow_sensor_scalar(value: f64) -> Option<f32> {
    if value.is_finite() && value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX) {
        Some(value as f32)
    } else {
        None
    }
}
