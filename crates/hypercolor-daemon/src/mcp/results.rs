use hypercolor_types::api::effects::EffectSummary;
use hypercolor_types::api::scene::ZoneResource;
use hypercolor_types::api::scenes::ActivatedSceneRef;
use hypercolor_types::api::system::InputSourceStatus;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{DeviceOrigin, DeviceState, DriverPresentation};
use hypercolor_types::effect::{ControlKind, EffectCategory};
use hypercolor_types::scene::SceneMutationMode;
use hypercolor_types::sensor::{SensorReading, SystemSnapshot};
use serde::Serialize;
use utoipa::ToSchema;

use crate::api::displays::DisplayFaceScope;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct EffectCatalogResult {
    pub(crate) effects: Vec<EffectCatalogItem>,
    pub(crate) total: usize,
    pub(crate) has_more: bool,
    pub(crate) limit: u64,
    pub(crate) offset: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct EffectCatalogItem {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) category: EffectCategory,
    pub(crate) audio_reactive: bool,
    pub(crate) tags: Vec<String>,
    pub(crate) controls: Vec<EffectControlItem>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct EffectControlItem {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: ControlKind,
    pub(crate) default: ControlValue,
    pub(crate) min: Option<f32>,
    pub(crate) max: Option<f32>,
    pub(crate) step: Option<f32>,
    pub(crate) options: Vec<String>,
    pub(crate) tooltip: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct OutputPowerResult {
    pub(crate) state: hypercolor_types::api::output::OutputPowerMode,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AdjustControlsResult {
    pub(crate) zone: ZoneResource,
    pub(crate) revision: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DeviceInventoryResult {
    pub(crate) devices: Vec<DeviceInventoryItem>,
    pub(crate) summary: DeviceInventorySummary,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DeviceInventoryItem {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) vendor: String,
    pub(crate) family: String,
    pub(crate) origin: DeviceOrigin,
    pub(crate) presentation: DriverPresentation,
    pub(crate) transport: String,
    pub(crate) state: DeviceState,
    pub(crate) led_count: u32,
    pub(crate) segments: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DeviceInventorySummary {
    pub(crate) total: usize,
    pub(crate) connected: usize,
    pub(crate) total_leds: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BrightnessResult {
    pub(crate) brightness: u8,
    pub(crate) scope: BrightnessScope,
    pub(crate) previous_brightness: u8,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BrightnessScope {
    Global,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct StatusResult {
    pub(crate) running: bool,
    pub(crate) paused: bool,
    pub(crate) brightness: u8,
    pub(crate) fps: FpsResult,
    pub(crate) effect: Option<EffectRef>,
    pub(crate) effect_count: usize,
    pub(crate) scene_count: usize,
    pub(crate) devices: DeviceStatusSummary,
    pub(crate) inputs: InputStatusResult,
    pub(crate) uptime_seconds: u64,
    pub(crate) version: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct FpsResult {
    pub(crate) target: u32,
    pub(crate) capacity: f32,
    pub(crate) delivered: f64,
    pub(crate) actual: f32,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct EffectRef {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DeviceStatusSummary {
    pub(crate) connected: usize,
    pub(crate) total: usize,
    pub(crate) total_leds: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct InputStatusResult {
    pub(crate) audio: InputAvailability,
    pub(crate) screen: InputAvailability,
    pub(crate) input: InteractionAvailability,
    pub(crate) input_devices_opened: usize,
    pub(crate) input_devices_denied: usize,
    pub(crate) input_degraded: Option<String>,
    pub(crate) source_graph_generation: u64,
    pub(crate) sources: Vec<InputSourceStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputAvailability {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InteractionAvailability {
    Enabled,
    Disabled,
    BlockedPermissions,
    NoInteractiveSession,
    Unavailable,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActivateSceneResult {
    pub(crate) activated: bool,
    pub(crate) scene: ActivatedSceneRef,
    pub(crate) transition_ms: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SceneListResult {
    pub(crate) scenes: Vec<SceneListItem>,
    pub(crate) total: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SceneListItem {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) enabled: bool,
    #[schema(value_type = String, pattern = "^(live|snapshot)$")]
    pub(crate) mutation_mode: SceneMutationMode,
    pub(crate) active: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct CreateSceneResult {
    pub(crate) scene_id: String,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    #[schema(value_type = String, pattern = "^(live|snapshot)$")]
    pub(crate) mutation_mode: SceneMutationMode,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AudioStateResult {
    pub(crate) enabled: bool,
    pub(crate) levels: AudioLevelsResult,
    pub(crate) beat: BeatResult,
    pub(crate) spectrum_bins: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AudioLevelsResult {
    pub(crate) overall: f32,
    pub(crate) bass: f32,
    pub(crate) mid: f32,
    pub(crate) treble: f32,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BeatResult {
    pub(crate) detected: bool,
    pub(crate) confidence: f32,
    pub(crate) bpm_estimate: Option<f32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SensorDataResult {
    pub(crate) snapshot: SystemSnapshot,
    pub(crate) reading: Option<SensorReading>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LayoutResult {
    pub(crate) layout: LayoutSummaryResult,
    pub(crate) zones: Vec<LayoutZoneResult>,
    pub(crate) total_devices: usize,
    pub(crate) total_leds: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LayoutSummaryResult {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) canvas_width: u32,
    pub(crate) canvas_height: u32,
    pub(crate) zone_count: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LayoutZoneResult {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) device_id: String,
    pub(crate) led_count: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DisplayFaceResult {
    pub(crate) device: DisplayDeviceResult,
    pub(crate) scope: DisplayFaceScope,
    pub(crate) live_scope: Option<DisplayFaceScope>,
    pub(crate) cleared: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scene_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effect: Option<EffectSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) zone: Option<ZoneResource>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DisplayDeviceResult {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) vendor: String,
    pub(crate) family: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) circular: bool,
}
