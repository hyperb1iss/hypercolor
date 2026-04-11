use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", default)]
pub struct DisplayOverlayConfig {
    pub overlays: Vec<OverlaySlot>,
}

impl DisplayOverlayConfig {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.overlays = self
            .overlays
            .into_iter()
            .map(OverlaySlot::normalized)
            .collect();
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.overlays.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OverlaySlot {
    pub id: OverlaySlotId,
    pub name: String,
    pub source: OverlaySource,
    pub position: OverlayPosition,
    #[serde(default)]
    pub blend_mode: OverlayBlendMode,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl OverlaySlot {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.name = normalized_name(self.name);
        self.opacity = self.opacity.clamp(0.0, 1.0);
        self.source = self.source.normalized();
        self.position = self.position.normalized();
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OverlaySlotId(pub Uuid);

impl OverlaySlotId {
    #[must_use]
    pub fn new(uuid: Uuid) -> Self {
        Self(uuid)
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for OverlaySlotId {
    fn default() -> Self {
        Self::generate()
    }
}

impl fmt::Display for OverlaySlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for OverlaySlotId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OverlaySource {
    Clock(ClockConfig),
    Sensor(SensorOverlayConfig),
    Image(ImageOverlayConfig),
    Text(TextOverlayConfig),
    Html(HtmlOverlayConfig),
}

impl OverlaySource {
    #[must_use]
    pub fn normalized(self) -> Self {
        match self {
            Self::Clock(config) => Self::Clock(config.normalized()),
            Self::Sensor(config) => Self::Sensor(config.normalized()),
            Self::Image(config) => Self::Image(config.normalized()),
            Self::Text(config) => Self::Text(config.normalized()),
            Self::Html(config) => Self::Html(config.normalized()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OverlayBlendMode {
    #[default]
    Normal,
    Add,
    Screen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayPosition {
    FullScreen,
    Anchored {
        anchor: Anchor,
        offset_x: i32,
        offset_y: i32,
        width: u32,
        height: u32,
    },
}

impl OverlayPosition {
    #[must_use]
    pub fn normalized(self) -> Self {
        match self {
            Self::FullScreen => Self::FullScreen,
            Self::Anchored {
                anchor,
                offset_x,
                offset_y,
                width,
                height,
            } => Self::Anchored {
                anchor,
                offset_x,
                offset_y,
                width: width.max(1),
                height: height.max(1),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ClockConfig {
    pub style: ClockStyle,
    pub hour_format: HourFormat,
    #[serde(default)]
    pub show_seconds: bool,
    #[serde(default)]
    pub show_date: bool,
    pub date_format: Option<String>,
    pub font_family: Option<String>,
    pub color: String,
    pub secondary_color: Option<String>,
    pub template: Option<String>,
}

impl ClockConfig {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.date_format = normalized_optional_string(self.date_format);
        self.font_family = normalized_optional_string(self.font_family);
        self.color = normalized_required_string(self.color, "#ffffff");
        self.secondary_color = normalized_optional_string(self.secondary_color);
        self.template = normalized_optional_string(self.template);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockStyle {
    Digital,
    Analog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HourFormat {
    Twelve,
    TwentyFour,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SensorOverlayConfig {
    pub sensor: String,
    pub style: SensorDisplayStyle,
    pub unit_label: Option<String>,
    pub range_min: f32,
    pub range_max: f32,
    pub color_min: String,
    pub color_max: String,
    pub font_family: Option<String>,
    pub template: Option<String>,
}

impl SensorOverlayConfig {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.sensor = normalized_required_string(self.sensor, "cpu_temp");
        self.unit_label = normalized_optional_string(self.unit_label);
        if self.range_max < self.range_min {
            std::mem::swap(&mut self.range_min, &mut self.range_max);
        }
        if (self.range_max - self.range_min).abs() <= f32::EPSILON {
            self.range_max = self.range_min + 1.0;
        }
        self.color_min = normalized_required_string(self.color_min, "#80ffea");
        self.color_max = normalized_required_string(self.color_max, "#ff6ac1");
        self.font_family = normalized_optional_string(self.font_family);
        self.template = normalized_optional_string(self.template);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorDisplayStyle {
    Numeric,
    Gauge,
    Bar,
    Minimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ImageOverlayConfig {
    pub path: String,
    #[serde(default = "default_speed")]
    pub speed: f32,
    pub fit: ImageFit,
}

impl ImageOverlayConfig {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.path = normalized_required_string(self.path, ".");
        self.speed = normalized_speed(self.speed);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    Cover,
    Contain,
    Stretch,
    Original,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TextOverlayConfig {
    pub text: String,
    pub font_family: Option<String>,
    pub font_size: f32,
    pub color: String,
    pub align: TextAlign,
    #[serde(default)]
    pub scroll: bool,
    #[serde(default = "default_scroll_speed")]
    pub scroll_speed: f32,
}

impl TextOverlayConfig {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.text = normalized_required_string(self.text, "Overlay");
        self.font_family = normalized_optional_string(self.font_family);
        self.font_size = self.font_size.max(1.0);
        self.color = normalized_required_string(self.color, "#ffffff");
        self.scroll_speed = normalized_speed(self.scroll_speed);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HtmlOverlayConfig {
    pub path: String,
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
    #[serde(default = "default_render_interval_ms")]
    pub render_interval_ms: u32,
}

impl HtmlOverlayConfig {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.path = normalized_required_string(self.path, ".");
        self.render_interval_ms = self.render_interval_ms.max(16);
        self
    }
}

fn default_enabled() -> bool {
    true
}

fn default_opacity() -> f32 {
    1.0
}

fn default_speed() -> f32 {
    1.0
}

fn default_scroll_speed() -> f32 {
    30.0
}

fn default_render_interval_ms() -> u32 {
    1_000
}

fn normalized_name(name: String) -> String {
    normalized_required_string(name, "Overlay")
}

fn normalized_required_string(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn normalized_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalized_speed(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        1.0
    }
}
