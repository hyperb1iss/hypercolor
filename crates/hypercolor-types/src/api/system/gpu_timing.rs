//! GPU execution measurements and transfers exposed by daemon diagnostics.

use serde::{Deserialize, Serialize};

/// GPU timestamp coverage for SparkleFlinger's compute passes.
///
/// Durations exclude Servo's separate GPU context, uploads, transfer commands,
/// and queue latency. Counters belong to the current compositor instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct GpuPassTimingsStatus {
    /// Changes when the compositor is recreated; zero means no compositor.
    #[serde(default)]
    pub instance_id: u64,
    pub supported: bool,
    pub enabled: bool,
    pub unavailable_reason: Option<String>,
    pub stages: Vec<GpuPassTimingStatus>,
}

/// Cumulative GPU execution time for one class of compute passes.
///
/// Pending passes are attempts minus completed, skipped, failed, and cancelled
/// passes. No duration is fabricated for passes without a completed timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct GpuPassTimingStatus {
    pub stage: String,
    pub attempts: u64,
    pub completed: u64,
    pub skipped: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub total_ns: u64,
    pub last_ns: u64,
    pub max_ns: u64,
}

/// Successful GPU mappings consumed by CPU publications or LED decoding.
/// Bytes count the mapped payload consumed, excluding RGBA row padding.
/// Counters are process totals and reset when the daemon restarts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct GpuTransferStatus {
    pub preview_readback_count: u64,
    pub preview_readback_bytes: u64,
    pub display_readback_count: u64,
    pub display_readback_bytes: u64,
    /// Packed LED colors decoded on the CPU, four bytes per LED.
    pub led_readback_count: u64,
    pub led_readback_bytes: u64,
    /// Full RGBA canvases consumed by CPU sampling or scene/display assembly.
    pub sampling_canvas_readback_count: u64,
    pub sampling_canvas_readback_bytes: u64,
}
