//! Per-device metrics store — wraps the raw WS snapshot with a ring buffer
//! of output-rate samples so device cards can render sparklines without re-deriving
//! history on every tick.
//!
//! Only the devices page (or any other consumer) pays the WebSocket
//! subscription cost: `DevicesPage` bumps `ws_ctx.set_device_metrics_consumers`
//! on mount and drops it on cleanup. The store itself lives for the life of
//! the app so the effect can always listen for snapshots.

use std::collections::{HashMap, VecDeque};

use leptos::prelude::*;

use crate::api::{DeviceMetrics, DeviceMetricsSnapshot};
use crate::app::WsContext;

/// Maximum number of output-rate samples retained per device for sparklines.
/// At 2 Hz that's ~30 seconds of history — tight enough to feel live but
/// long enough to show regressions from a missed device write.
pub const FPS_HISTORY_LEN: usize = 60;

/// Per-device live state held in the store.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceMetricsState {
    pub current: DeviceMetrics,
    /// Oldest-first ring of sent FPS samples, capped at `FPS_HISTORY_LEN`.
    pub fps_samples: VecDeque<f32>,
    /// Unix millis of the snapshot the current values came from. Exposed so
    /// consumers can flag stale data when the WS drops.
    pub taken_at_ms: i64,
}

/// Shared store of per-device metrics with rolling FPS history. Provided
/// at the app root so every card sees the same numbers.
#[derive(Debug, Clone, Copy)]
pub struct DeviceMetricsStore {
    pub entries: RwSignal<HashMap<String, DeviceMetricsState>>,
}

impl DeviceMetricsStore {
    /// Borrow one device's metric state, tracking only the parent store.
    ///
    /// The caller gets a `Memo` so consumers re-render only when the
    /// referenced entry changes (`DeviceMetricsState: PartialEq`).
    #[must_use]
    pub fn entry_for(self, device_id: String) -> Memo<Option<DeviceMetricsState>> {
        Memo::new(move |_| {
            self.entries
                .with(|entries| entries.get(&device_id).cloned())
        })
    }
}

/// Build the store and wire the effect that folds WS snapshots into it.
/// Returns a value suitable for `provide_context`.
#[must_use]
pub fn install_device_metrics_store(ws_ctx: WsContext) -> DeviceMetricsStore {
    let entries: RwSignal<HashMap<String, DeviceMetricsState>> = RwSignal::new(HashMap::new());

    // Fold every snapshot into the ring-buffer map. Devices that disappear
    // from the snapshot are pruned so the UI doesn't linger on a card that
    // was removed while the page stayed mounted.
    Effect::new(move |_| {
        let Some(snapshot) = ws_ctx.device_metrics.get() else {
            // Subscription dropped (disconnect or consumer count went to 0).
            // Clear state so stale values don't bleed across reconnects.
            entries.update(HashMap::clear);
            return;
        };

        entries.update(|map| fold_snapshot_into(map, &snapshot));
    });

    DeviceMetricsStore { entries }
}

fn fold_snapshot_into(
    map: &mut HashMap<String, DeviceMetricsState>,
    snapshot: &DeviceMetricsSnapshot,
) {
    // Drop entries for devices no longer present in the snapshot.
    let live_ids: std::collections::HashSet<&str> =
        snapshot.items.iter().map(|m| m.id.as_str()).collect();
    map.retain(|id, _| live_ids.contains(id.as_str()));

    for item in &snapshot.items {
        let state = map.entry(item.id.clone()).or_default();
        state.current = item.clone();
        state.taken_at_ms = snapshot.taken_at_ms;
        state.fps_samples.push_back(item.sent_fps().max(0.0));
        while state.fps_samples.len() > FPS_HISTORY_LEN {
            state.fps_samples.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(id: &str, fps: f32) -> DeviceMetrics {
        DeviceMetrics {
            id: id.to_owned(),
            fps_sent: fps,
            fps_queued: fps,
            fps_actual: fps,
            fps_target: 60,
            payload_bps_estimate: 0,
            avg_latency_ms: 0,
            frames_received: 0,
            frames_sent: 0,
            frames_dropped: 0,
            errors_total: 0,
            last_error: None,
            last_sent_ago_ms: None,
        }
    }

    #[test]
    fn fold_snapshot_appends_samples_and_caps_at_history_len() {
        let mut map = HashMap::new();
        for i in 0..(FPS_HISTORY_LEN + 10) {
            let snapshot = DeviceMetricsSnapshot {
                taken_at_ms: i as i64,
                items: vec![metrics("device-a", (i as f32).min(60.0))],
            };
            fold_snapshot_into(&mut map, &snapshot);
        }
        let state = map.get("device-a").expect("device-a present");
        assert_eq!(state.fps_samples.len(), FPS_HISTORY_LEN);
        assert!((state.current.fps_actual - 60.0).abs() < f32::EPSILON);
    }

    #[test]
    fn fold_snapshot_prunes_removed_devices() {
        let mut map = HashMap::new();
        fold_snapshot_into(
            &mut map,
            &DeviceMetricsSnapshot {
                taken_at_ms: 1,
                items: vec![metrics("device-a", 60.0), metrics("device-b", 30.0)],
            },
        );
        assert!(map.contains_key("device-a"));
        assert!(map.contains_key("device-b"));

        fold_snapshot_into(
            &mut map,
            &DeviceMetricsSnapshot {
                taken_at_ms: 2,
                items: vec![metrics("device-a", 59.0)],
            },
        );
        assert!(map.contains_key("device-a"));
        assert!(!map.contains_key("device-b"), "unplugged device is pruned");
    }

    #[test]
    fn fold_snapshot_clamps_negative_fps_to_zero() {
        let mut map = HashMap::new();
        fold_snapshot_into(
            &mut map,
            &DeviceMetricsSnapshot {
                taken_at_ms: 1,
                items: vec![metrics("device-a", -5.0)],
            },
        );
        let state = map.get("device-a").expect("device-a present");
        assert_eq!(state.fps_samples.front().copied(), Some(0.0));
    }
}
