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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceMetrics {
    pub id: DeviceId,
    pub fps_actual: f32,
    pub fps_target: u32,
    pub payload_bps_estimate: u64,
    pub avg_latency_ms: u32,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub errors_total: u64,
    pub last_error: Option<String>,
    pub last_sent_ago_ms: Option<u64>,
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
        let mut stats_by_device = HashMap::with_capacity(statistics.len());
        let mut items = Vec::with_capacity(statistics.len());

        for stats in statistics {
            let previous = previous_stats.and_then(|entries| entries.get(&stats.device_id));
            items.push(build_device_metrics(&stats, previous, elapsed_secs));
            stats_by_device.insert(stats.device_id, stats);
        }

        items.sort_by(|left, right| left.id.to_string().cmp(&right.id.to_string()));

        let snapshot = DeviceMetricsSnapshot { taken_at_ms, items };
        self.snapshot.store(Arc::new(snapshot.clone()));
        self.previous = Some(CollectorBaseline {
            sampled_at,
            stats_by_device,
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
    elapsed_secs: f64,
) -> DeviceMetrics {
    let delta_frames_sent = previous.map_or(0, |baseline| {
        stats.frames_sent.saturating_sub(baseline.frames_sent)
    });
    let delta_bytes_sent = previous.map_or(0, |baseline| {
        stats.bytes_sent.saturating_sub(baseline.bytes_sent)
    });

    DeviceMetrics {
        id: stats.device_id,
        fps_actual: rate_as_f32(delta_frames_sent, elapsed_secs),
        fps_target: stats.target_fps,
        payload_bps_estimate: rate_as_u64(delta_bytes_sent, elapsed_secs),
        avg_latency_ms: u32::try_from(stats.avg_latency_ms).unwrap_or(u32::MAX),
        frames_sent: stats.frames_sent,
        frames_dropped: stats.frames_dropped,
        errors_total: stats.errors_total,
        last_error: sanitize_last_error(stats.last_error.as_deref()),
        last_sent_ago_ms: stats.last_sent_ago_ms,
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
