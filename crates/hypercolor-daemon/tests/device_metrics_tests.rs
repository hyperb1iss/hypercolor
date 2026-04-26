use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;

use hypercolor_core::device::DeviceOutputStatistics;
use hypercolor_daemon::device_metrics::{DeviceMetricsCollector, DeviceMetricsSnapshot};
use hypercolor_types::device::DeviceId;

fn sample_stats(
    device_id: DeviceId,
    frames_sent: u64,
    bytes_sent: u64,
    errors_total: u64,
    last_error: Option<&str>,
) -> DeviceOutputStatistics {
    DeviceOutputStatistics {
        backend_id: "wled".to_owned(),
        device_id,
        mapped_layout_ids: vec!["device:test".to_owned()],
        target_fps: 60,
        frames_received: frames_sent,
        frames_sent,
        bytes_sent,
        frames_dropped: 2,
        avg_latency_ms: 14,
        avg_queue_wait_ms: 3,
        avg_write_ms: 5,
        last_error: last_error.map(str::to_owned),
        errors_total,
        write_failure_warnings_total: errors_total,
        last_sent_ago_ms: Some(25),
        last_sequence: frames_sent,
    }
}

#[test]
fn collector_reports_zero_rates_without_a_baseline() {
    let device_id = DeviceId::new();
    let snapshot = Arc::new(ArcSwap::from_pointee(DeviceMetricsSnapshot::default()));
    let mut collector = DeviceMetricsCollector::new(Arc::clone(&snapshot));
    let now = Instant::now();

    let collected = collector.update_from_statistics_at(
        vec![sample_stats(device_id, 10, 120, 0, None)],
        now,
        1_000,
    );

    assert_eq!(collected.taken_at_ms, 1_000);
    assert_eq!(collected.items.len(), 1);
    assert_eq!(collected.items[0].id, device_id);
    assert_eq!(collected.items[0].fps_actual, 0.0);
    assert_eq!(collected.items[0].payload_bps_estimate, 0);
}

#[test]
fn collector_derives_rates_from_counter_deltas_and_sanitizes_errors() {
    let device_id = DeviceId::new();
    let snapshot = Arc::new(ArcSwap::from_pointee(DeviceMetricsSnapshot::default()));
    let mut collector = DeviceMetricsCollector::new(snapshot);
    let started_at = Instant::now();

    let _ = collector.update_from_statistics_at(
        vec![sample_stats(device_id, 10, 120, 0, None)],
        started_at,
        1_000,
    );
    let collected = collector.update_from_statistics_at(
        vec![sample_stats(
            device_id,
            40,
            420,
            2,
            Some("socket timeout\n192.168.1.20"),
        )],
        started_at + Duration::from_millis(500),
        1_500,
    );

    assert_eq!(collected.items.len(), 1);
    assert_eq!(collected.items[0].fps_target, 60);
    assert!((collected.items[0].fps_actual - 60.0).abs() < f32::EPSILON);
    assert_eq!(collected.items[0].payload_bps_estimate, 600);
    assert_eq!(collected.items[0].errors_total, 2);
    assert_eq!(
        collected.items[0].last_error.as_deref(),
        Some("socket timeout 192.168.1.20")
    );
}

#[tokio::test]
async fn snapshot_store_returns_identical_values_to_concurrent_readers() {
    let device_id = DeviceId::new();
    let snapshot = Arc::new(ArcSwap::from_pointee(DeviceMetricsSnapshot::default()));
    let mut collector = DeviceMetricsCollector::new(Arc::clone(&snapshot));

    let _ = collector.update_from_statistics_at(
        vec![sample_stats(device_id, 15, 180, 1, Some("temporary error"))],
        Instant::now(),
        2_000,
    );

    let left_store = Arc::clone(&snapshot);
    let right_store = Arc::clone(&snapshot);
    let left = tokio::spawn(async move { left_store.load_full().as_ref().clone() });
    let right = tokio::spawn(async move { right_store.load_full().as_ref().clone() });

    let left_snapshot = left.await.expect("left reader should complete");
    let right_snapshot = right.await.expect("right reader should complete");
    assert_eq!(left_snapshot, right_snapshot);
}
