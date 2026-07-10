use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use hypercolor_core::device::{BackendManager, DeviceOutputStatistics};
use hypercolor_types::device::DeviceId;

pub const DEVICE_METRICS_INTERVAL: Duration = Duration::from_millis(500);
const MAX_LAST_ERROR_LEN: usize = 240;

pub type DeviceMetricsSnapshotStore = Arc<ArcSwap<DeviceMetricsSnapshot>>;

const DEVICE_FPS_EWMA_ALPHA: f32 = 0.35;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceMetrics {
    pub id: DeviceId,
    pub backend_id: String,
    pub mapped_layout_ids: Vec<String>,
    pub uses_frame_sink: bool,
    pub worker_finished: bool,
    #[serde(default)]
    pub worker_recoveries: u64,
    #[serde(default)]
    pub delivered_fps: f32,
    #[serde(default)]
    pub accepted_fps: f32,
    pub fps_sent: f32,
    pub fps_queued: f32,
    pub fps_actual: f32,
    pub fps_target: u32,
    pub target_interval_ms: Option<u64>,
    pub payload_bps_estimate: u64,
    pub avg_latency_ms: u32,
    pub avg_queue_wait_ms: u32,
    pub avg_write_ms: u32,
    #[serde(default)]
    pub avg_transport_latency_ms: u32,
    pub frames_received: u64,
    #[serde(default)]
    pub accepted: u64,
    pub frames_sent: u64,
    #[serde(default)]
    pub transport_started: u64,
    #[serde(default)]
    pub transport_completed: u64,
    #[serde(default)]
    pub transport_failed: u64,
    #[serde(default)]
    pub completed_payload_bytes: u64,
    pub frames_suppressed: u64,
    pub frames_dropped: u64,
    #[serde(default)]
    pub coalesced: u64,
    #[serde(default)]
    pub coalesced_target_cadence: u64,
    #[serde(default)]
    pub coalesced_backend_overrun: u64,
    pub errors_total: u64,
    pub write_failure_warnings_total: u64,
    pub last_error: Option<String>,
    pub last_sent_ago_ms: Option<u64>,
    pub last_sequence: u64,
    #[serde(default)]
    pub queue_generation: u64,
    #[serde(default)]
    pub last_transport_started_sequence: u64,
    #[serde(default)]
    pub last_transport_completed_sequence: u64,
    #[serde(default)]
    pub last_transport_failed_sequence: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DeviceMetricsSnapshot {
    pub taken_at_ms: i64,
    pub items: Vec<DeviceMetrics>,
}

#[derive(Debug, Clone)]
struct CollectorBaseline {
    sampled_at: Instant,
    stats_by_device: HashMap<DeviceId, DeviceOutputStatistics>,
    smoothed_rates_by_device: HashMap<DeviceId, SmoothedDeviceRates>,
}

#[derive(Debug, Clone, Copy, Default)]
struct SmoothedDeviceRates {
    delivered_fps: f32,
    accepted_fps: f32,
    fps_queued: f32,
}

#[derive(Debug)]
pub struct DeviceMetricsCollector {
    snapshot: DeviceMetricsSnapshotStore,
    previous: Option<CollectorBaseline>,
}

impl DeviceMetricsCollector {
    #[must_use]
    pub fn new(snapshot: DeviceMetricsSnapshotStore) -> Self {
        Self {
            snapshot,
            previous: None,
        }
    }

    pub async fn update_from_backend_manager(
        &mut self,
        backend_manager: &Arc<Mutex<BackendManager>>,
    ) -> DeviceMetricsSnapshot {
        let statistics = {
            let manager = backend_manager.lock().await;
            manager.device_output_statistics()
        };

        self.update_from_statistics_at(statistics, Instant::now(), unix_timestamp_ms())
    }

    pub fn update_from_statistics_at(
        &mut self,
        statistics: Vec<DeviceOutputStatistics>,
        sampled_at: Instant,
        taken_at_ms: i64,
    ) -> DeviceMetricsSnapshot {
        let elapsed_secs = self.previous.as_ref().map_or(0.0, |previous| {
            sampled_at
                .saturating_duration_since(previous.sampled_at)
                .as_secs_f64()
        });

        let previous_stats = self
            .previous
            .as_ref()
            .map(|previous| &previous.stats_by_device);
        let previous_rates = self
            .previous
            .as_ref()
            .map(|previous| &previous.smoothed_rates_by_device);
        let mut stats_by_device = HashMap::with_capacity(statistics.len());
        let mut smoothed_rates_by_device = HashMap::with_capacity(statistics.len());
        let mut items = Vec::with_capacity(statistics.len());

        for stats in statistics {
            let previous = previous_stats.and_then(|entries| entries.get(&stats.device_id));
            let previous_rate =
                previous_rates.and_then(|entries| entries.get(&stats.device_id).copied());
            let metrics = build_device_metrics(&stats, previous, previous_rate, elapsed_secs);
            smoothed_rates_by_device.insert(
                stats.device_id,
                SmoothedDeviceRates {
                    delivered_fps: metrics.delivered_fps,
                    accepted_fps: metrics.accepted_fps,
                    fps_queued: metrics.fps_queued,
                },
            );
            items.push(metrics);
            stats_by_device.insert(stats.device_id, stats);
        }

        items.sort_by_cached_key(|item| item.id.to_string());

        let snapshot = DeviceMetricsSnapshot { taken_at_ms, items };
        self.snapshot.store(Arc::new(snapshot.clone()));
        self.previous = Some(CollectorBaseline {
            sampled_at,
            stats_by_device,
            smoothed_rates_by_device,
        });

        snapshot
    }
}

pub fn spawn_device_metrics_collector(
    snapshot: DeviceMetricsSnapshotStore,
    backend_manager: Arc<Mutex<BackendManager>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut collector = DeviceMetricsCollector::new(snapshot);
        let mut ticker = tokio::time::interval(DEVICE_METRICS_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            ticker.tick().await;
            let _ = collector
                .update_from_backend_manager(&backend_manager)
                .await;
        }
    })
}

fn build_device_metrics(
    stats: &DeviceOutputStatistics,
    previous: Option<&DeviceOutputStatistics>,
    previous_rate: Option<SmoothedDeviceRates>,
    elapsed_secs: f64,
) -> DeviceMetrics {
    let delta_frames_received = previous.map_or(0, |baseline| {
        stats
            .frames_received
            .saturating_sub(baseline.frames_received)
    });
    let delta_frames_sent = previous.map_or(0, |baseline| {
        stats
            .transport_completed
            .saturating_sub(baseline.transport_completed)
    });
    let delta_accepted = previous.map_or(0, |baseline| {
        stats.accepted.saturating_sub(baseline.accepted)
    });
    let delta_bytes_sent = previous.map_or(0, |baseline| {
        stats
            .completed_payload_bytes
            .saturating_sub(baseline.completed_payload_bytes)
    });
    let has_rate_baseline = previous.is_some() && elapsed_secs > f64::EPSILON;
    let delivered_fps = smooth_rate(
        previous_rate.map(|rate| rate.delivered_fps),
        rate_as_f32(delta_frames_sent, elapsed_secs),
        has_rate_baseline,
    );
    let accepted_fps = smooth_rate(
        previous_rate.map(|rate| rate.accepted_fps),
        rate_as_f32(delta_accepted, elapsed_secs),
        has_rate_baseline,
    );
    let fps_queued = smooth_rate(
        previous_rate.map(|rate| rate.fps_queued),
        rate_as_f32(delta_frames_received, elapsed_secs),
        has_rate_baseline,
    );

    DeviceMetrics {
        id: stats.device_id,
        backend_id: stats.backend_id.clone(),
        mapped_layout_ids: stats.mapped_layout_ids.clone(),
        uses_frame_sink: stats.uses_frame_sink,
        worker_finished: stats.worker_finished,
        worker_recoveries: stats.worker_recoveries,
        delivered_fps,
        accepted_fps,
        fps_sent: delivered_fps,
        fps_queued,
        fps_actual: delivered_fps,
        fps_target: stats.target_fps,
        target_interval_ms: stats.target_interval_ms,
        payload_bps_estimate: rate_as_u64(delta_bytes_sent, elapsed_secs),
        avg_latency_ms: u32::try_from(stats.avg_latency_ms).unwrap_or(u32::MAX),
        avg_queue_wait_ms: u32::try_from(stats.avg_queue_wait_ms).unwrap_or(u32::MAX),
        avg_write_ms: u32::try_from(stats.avg_write_ms).unwrap_or(u32::MAX),
        avg_transport_latency_ms: u32::try_from(stats.avg_transport_latency_ms).unwrap_or(u32::MAX),
        frames_received: stats.frames_received,
        accepted: stats.accepted,
        frames_sent: stats.frames_sent,
        transport_started: stats.transport_started,
        transport_completed: stats.transport_completed,
        transport_failed: stats.transport_failed,
        completed_payload_bytes: stats.completed_payload_bytes,
        frames_suppressed: stats.frames_suppressed,
        frames_dropped: stats.frames_dropped,
        coalesced: stats.coalesced,
        coalesced_target_cadence: stats.coalesced_target_cadence,
        coalesced_backend_overrun: stats.coalesced_backend_overrun,
        errors_total: stats.errors_total,
        write_failure_warnings_total: stats.write_failure_warnings_total,
        last_error: sanitize_last_error(stats.last_error.as_deref()),
        last_sent_ago_ms: stats.last_sent_ago_ms,
        last_sequence: stats.last_sequence,
        queue_generation: stats.queue_generation,
        last_transport_started_sequence: stats.last_transport_started_sequence,
        last_transport_completed_sequence: stats.last_transport_completed_sequence,
        last_transport_failed_sequence: stats.last_transport_failed_sequence,
    }
}

fn smooth_rate(previous_rate: Option<f32>, instant_rate: f32, has_rate_baseline: bool) -> f32 {
    if !has_rate_baseline {
        return previous_rate.unwrap_or(0.0).max(0.0);
    }

    let instant_rate = instant_rate.max(0.0);
    match previous_rate {
        Some(previous) if previous > 0.0 => previous.mul_add(
            1.0 - DEVICE_FPS_EWMA_ALPHA,
            instant_rate * DEVICE_FPS_EWMA_ALPHA,
        ),
        _ => instant_rate,
    }
}

fn rate_as_f32(delta: u64, elapsed_secs: f64) -> f32 {
    if elapsed_secs <= f64::EPSILON {
        0.0
    } else {
        (delta as f64 / elapsed_secs) as f32
    }
}

fn rate_as_u64(delta: u64, elapsed_secs: f64) -> u64 {
    if elapsed_secs <= f64::EPSILON {
        return 0;
    }

    let rate = (delta as f64 / elapsed_secs).round();
    if !rate.is_finite() || rate <= 0.0 {
        0
    } else if rate >= u64::MAX as f64 {
        u64::MAX
    } else {
        rate as u64
    }
}

fn sanitize_last_error(error: Option<&str>) -> Option<String> {
    let flattened = error
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| !value.is_empty())?;

    if flattened.chars().count() <= MAX_LAST_ERROR_LEN {
        return Some(flattened);
    }

    let mut truncated = flattened
        .chars()
        .take(MAX_LAST_ERROR_LEN.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    Some(truncated)
}

fn unix_timestamp_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}
