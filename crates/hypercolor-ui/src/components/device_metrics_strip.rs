//! Compact per-device metrics strip rendered on connected device cards.
//!
//! Shows live FPS (actual/target), payload bandwidth estimate, error count,
//! and a rolling FPS sparkline. All data comes from `DeviceMetricsStore`,
//! which is fed by the `device_metrics` WebSocket topic.

use leptos::prelude::*;

use crate::components::perf_charts::Sparkline;
use crate::device_metrics::DeviceMetricsStore;

/// Milliseconds after which a last-error timestamp is considered "cooled
/// off" — the strip stops colouring the error count in red beyond this.
const RECENT_ERROR_WINDOW_MS: u64 = 5_000;

/// Live metrics readout for a single device. Silently renders nothing when
/// no metrics have been received for the device yet (e.g. the daemon hasn't
/// published its first sample).
#[component]
pub fn DeviceMetricsStrip(
    /// Physical device id as served by `/api/v1/devices` — must match the
    /// `id` field in `DeviceMetricsSnapshot.items`.
    #[prop(into)]
    device_id: String,
) -> impl IntoView {
    let Some(store) = use_context::<DeviceMetricsStore>() else {
        return ().into_any();
    };

    let entry = store.entry_for(device_id);

    // Pre-derive fields that re-render with the entry. Keeping these as
    // separate memos lets Leptos skip work when only one field changed.
    let fps_line = Memo::new(move |_| {
        let state = entry.get()?;
        Some(format!(
            "{:.0}/{} fps",
            state.current.fps_actual.max(0.0),
            state.current.fps_target
        ))
    });

    let fps_color = Memo::new(move |_| {
        let Some(state) = entry.get() else {
            return "var(--color-fg-tertiary)";
        };
        let target = f32::from(u16::try_from(state.current.fps_target.min(u32::from(u16::MAX))).unwrap_or(0));
        if target <= 0.0 {
            return "var(--color-fg-tertiary)";
        }
        let ratio = state.current.fps_actual / target;
        if ratio >= 0.9 {
            "var(--color-success-green)"
        } else if ratio >= 0.7 {
            "var(--color-electric-yellow)"
        } else {
            "var(--color-error-red)"
        }
    });

    let bandwidth_label = Memo::new(move |_| {
        let state = entry.get()?;
        Some(format_bandwidth(state.current.payload_bps_estimate))
    });

    let error_count = Memo::new(move |_| entry.get().map_or(0, |s| s.current.errors_total));
    let last_error_tooltip = Memo::new(move |_| entry.get().and_then(|s| s.current.last_error));
    let last_sent_ago = Memo::new(move |_| entry.get().and_then(|s| s.current.last_sent_ago_ms));

    let error_color = Memo::new(move |_| {
        if error_count.get() == 0 {
            return "var(--color-fg-tertiary)";
        }
        match last_sent_ago.get() {
            Some(ms) if ms <= RECENT_ERROR_WINDOW_MS => "var(--color-error-red)",
            _ => "var(--color-electric-yellow)",
        }
    });

    let fps_samples = Memo::new(move |_| {
        entry.get().map_or_else(Vec::new, |state| {
            state
                .fps_samples
                .iter()
                .map(|v| f64::from(*v))
                .collect::<Vec<_>>()
        })
    });

    let has_entry = Memo::new(move |_| entry.get().is_some());

    view! {
        <div
            class="flex items-center gap-2 pt-1.5 border-t border-edge-subtle/35"
            class:opacity-60=move || !has_entry.get()
        >
            <span
                class="text-[10px] font-mono tabular-nums leading-none whitespace-nowrap"
                style=move || format!("color: {}", fps_color.get())
                title="Actual / target frames per second"
            >
                {move || fps_line.get().unwrap_or_else(|| "-- fps".to_owned())}
            </span>
            <span class="text-[8px] text-fg-tertiary/30">{"\u{b7}"}</span>
            <span
                class="text-[10px] font-mono tabular-nums text-fg-tertiary/80 leading-none whitespace-nowrap"
                title="Payload bandwidth estimate (excludes transport framing)"
            >
                {move || bandwidth_label.get().unwrap_or_else(|| "-- bps".to_owned())}
            </span>
            <span class="text-[8px] text-fg-tertiary/30">{"\u{b7}"}</span>
            <span
                class="text-[10px] font-mono tabular-nums leading-none whitespace-nowrap"
                style=move || format!("color: {}", error_color.get())
                title=move || {
                    last_error_tooltip
                        .get()
                        .unwrap_or_else(|| "No recent errors".to_owned())
                }
            >
                {move || {
                    let count = error_count.get();
                    if count == 0 { "0 err".to_owned() } else { format!("{count} err") }
                }}
            </span>
            <div class="ml-auto h-4 w-[80px] shrink-0 opacity-80">
                <Sparkline
                    values=Signal::derive(move || fps_samples.get())
                    stroke="var(--color-neon-cyan)"
                    fill=true
                    aria_label="Device FPS history"
                />
            </div>
        </div>
    }
    .into_any()
}

/// Format a bytes-per-second rate with a compact SI-ish unit.
/// Uses 1024-based units (KiB / MiB) because that's what matches the
/// realities of the payload packing — `zone_colors.total_bytes()` scales
/// in powers of two more often than ten, and this keeps the readout
/// honest when compared against HAL debug dumps.
fn format_bandwidth(bytes_per_sec: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;

    let bps = bytes_per_sec as f64;
    if bps >= MIB {
        format!("{:.1} MB/s", bps / MIB)
    } else if bps >= KIB {
        format!("{:.1} KB/s", bps / KIB)
    } else if bps > 0.0 {
        format!("{bytes_per_sec} B/s")
    } else {
        "idle".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bandwidth_renders_human_units() {
        assert_eq!(format_bandwidth(0), "idle");
        assert_eq!(format_bandwidth(512), "512 B/s");
        assert_eq!(format_bandwidth(1_536), "1.5 KB/s");
        assert_eq!(format_bandwidth(2 * 1024 * 1024), "2.0 MB/s");
    }
}
