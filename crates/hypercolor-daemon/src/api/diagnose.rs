//! Diagnostics endpoint — `/api/v1/diagnose`.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use hypercolor_core::device::{UsbActorMetricsSnapshot, usb_actor_metrics_snapshot};
use hypercolor_types::device::USB_OUTPUT_BACKEND_ID;
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::device_metrics::{DeviceMetrics, DeviceMetricsSnapshot};
use crate::performance::{LatestFrameMetrics, PerformanceSnapshot};

const RENDER_FRAME_STALE_WARNING_MS: f64 = 2_000.0;
const RENDER_FRAME_STALE_FAIL_MS: f64 = 10_000.0;

#[derive(Debug, Deserialize)]
pub struct DiagnoseRequest {
    pub checks: Option<Vec<String>>,
    pub system: Option<bool>,
}

#[derive(Debug, Serialize)]
struct DiagnoseResponse {
    checks: Vec<DiagnoseCheck>,
    summary: DiagnoseSummary,
    snapshot: DiagnoseSnapshot,
}

#[derive(Debug, Serialize)]
struct DiagnoseCheck {
    category: String,
    name: String,
    status: String,
    detail: String,
}

#[derive(Debug, Serialize)]
struct DiagnoseSummary {
    passed: usize,
    warnings: usize,
    failed: usize,
}

#[derive(Debug, Serialize)]
struct DiagnoseSnapshot {
    render: DiagnoseRenderSnapshot,
    usb: DiagnoseUsbActorSnapshot,
    device_output: DiagnoseDeviceOutputSnapshot,
}

#[derive(Debug, Serialize)]
struct DiagnoseRenderSnapshot {
    latest_frame: Option<DiagnoseLatestFrameSnapshot>,
    recent_window: DiagnoseRenderWindowSnapshot,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "diagnostics snapshot mirrors independent frame freshness flags"
)]
struct DiagnoseLatestFrameSnapshot {
    frame_token: u64,
    frame_age_ms: f64,
    compositor_backend: String,
    output_frame_source: String,
    output_reuses_published_frame: bool,
    output_brightness_bits: u32,
    output_brightness_generation: u64,
    output_routing_signature: u64,
    output_zone_shape_signature: u64,
    output_unassigned_behavior_generation: u64,
    devices_written: u32,
    total_leds: u32,
    gpu_zone_sampling: bool,
    gpu_sample_deferred: bool,
    gpu_sample_stale: bool,
    gpu_sample_retry_hit: bool,
    gpu_sample_queue_saturated: bool,
    gpu_sample_wait_blocked: bool,
    gpu_sample_cpu_fallback: bool,
    cpu_sampling_late_readback: bool,
    cpu_readback_skipped: bool,
    gpu_readback_failed: bool,
    led_sampling_readback: bool,
    input_us: u32,
    render_us: u32,
    producer_us: u32,
    composition_us: u32,
    sample_us: u32,
    push_us: u32,
    publish_us: u32,
    overhead_us: u32,
    total_us: u32,
    output_errors: u32,
}

#[derive(Debug, Serialize)]
struct DiagnoseRenderWindowSnapshot {
    frames: u32,
    gpu_sample_deferred: u32,
    gpu_sample_stale: u32,
    gpu_sample_retry_hit: u32,
    gpu_sample_queue_saturated: u32,
    gpu_sample_wait_blocked: u32,
    gpu_sample_cpu_fallback: u32,
    led_sampling_readback: u32,
    output_current_frame: u32,
    output_published_frame: u32,
    output_routed_reuse: u32,
    output_reused_published_frame: u32,
    output_error_frames: u32,
    push_avg_ms: f64,
    push_p95_ms: f64,
    publish_avg_ms: f64,
    publish_p95_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON names mirror the USB actor metrics exported elsewhere"
)]
struct DiagnoseUsbActorSnapshot {
    display_frames_total: u64,
    display_frames_delayed_for_led_total: u64,
    display_led_priority_wait_total_ms: f64,
    display_led_priority_wait_avg_ms: f64,
    display_led_priority_wait_max_ms: f64,
}

#[derive(Debug, Serialize)]
struct DiagnoseDeviceOutputSnapshot {
    queues: usize,
    usb_queues: usize,
    lagging_queues: usize,
    worker_finished_queues: usize,
    dropped_frames_total: u64,
    errors_total: u64,
    items: Vec<DiagnoseDeviceOutputItem>,
}

#[derive(Debug, Serialize)]
struct DiagnoseDeviceOutputItem {
    id: String,
    backend_id: String,
    mapped_layout_ids: Vec<String>,
    uses_frame_sink: bool,
    worker_finished: bool,
    fps_sent: f32,
    fps_queued: f32,
    fps_target: u32,
    frames_received: u64,
    frames_sent: u64,
    frames_dropped: u64,
    errors_total: u64,
    avg_latency_ms: u32,
    avg_queue_wait_ms: u32,
    avg_write_ms: u32,
    last_sent_ago_ms: Option<u64>,
    last_error: Option<String>,
    last_sequence: u64,
}

/// `POST /api/v1/diagnose` — Run lightweight daemon diagnostics.
#[expect(
    clippy::too_many_lines,
    reason = "diagnostics response assembly keeps checks and snapshot state in one handler"
)]
pub async fn run_diagnostics(
    State(state): State<Arc<AppState>>,
    body: Option<Json<DiagnoseRequest>>,
) -> Response {
    let requested = body
        .as_ref()
        .and_then(|b| b.checks.as_ref())
        .cloned()
        .unwrap_or_else(|| {
            vec![
                "daemon".to_owned(),
                "render".to_owned(),
                "devices".to_owned(),
                "config".to_owned(),
            ]
        });

    let include_system = body.as_ref().and_then(|b| b.system).unwrap_or(false);

    let render_elapsed_ms = state.start_time.elapsed().as_secs_f64() * 1000.0;
    let performance = state.performance.read().await.snapshot();
    let usb_actor_metrics = usb_actor_metrics_snapshot();
    let device_metrics = state.device_metrics.load_full();
    let snapshot = build_diagnose_snapshot(
        &performance,
        render_elapsed_ms,
        usb_actor_metrics,
        device_metrics.as_ref(),
    );

    let mut checks = Vec::new();

    for check in requested {
        match check.as_str() {
            "daemon" => {
                checks.push(DiagnoseCheck {
                    category: "system".to_owned(),
                    name: "daemon_running".to_owned(),
                    status: "pass".to_owned(),
                    detail: env!("CARGO_PKG_VERSION").to_owned(),
                });
            }
            "render" => {
                let loop_guard = state.render_loop.read().await;
                let running = loop_guard.is_running();
                let render_loop_stats = loop_guard.stats();
                checks.push(DiagnoseCheck {
                    category: "render".to_owned(),
                    name: "render_loop".to_owned(),
                    status: if running { "pass" } else { "warning" }.to_owned(),
                    detail: format!(
                        "state={}, tier={}",
                        render_loop_stats.state, render_loop_stats.tier
                    ),
                });
                if running {
                    let (status, detail) = render_frame_liveness_status(
                        performance.latest_frame.as_ref(),
                        render_elapsed_ms,
                    );
                    checks.push(DiagnoseCheck {
                        category: "render".to_owned(),
                        name: "frame_liveness".to_owned(),
                        status: status.to_owned(),
                        detail,
                    });

                    let (status, detail) =
                        render_led_freshness_status(performance.latest_frame.as_ref());
                    checks.push(DiagnoseCheck {
                        category: "render".to_owned(),
                        name: "led_freshness".to_owned(),
                        status: status.to_owned(),
                        detail,
                    });

                    checks.push(DiagnoseCheck {
                        category: "render".to_owned(),
                        name: "recent_output_sources".to_owned(),
                        status: "pass".to_owned(),
                        detail: format!(
                            "frames={}, current_frame={}, published_frame={}, routed_reuse={}, reused_published_frame={}, gpu_sample_stale={}",
                            performance.frame_count,
                            performance.pacing.output_current_frame,
                            performance.pacing.output_published_frame,
                            performance.pacing.output_routed_reuse,
                            performance.pacing.output_reused_published_frame,
                            performance.pacing.gpu_sample_stale
                        ),
                    });
                }
            }
            "devices" => {
                let count = state.device_registry.len().await;
                checks.push(DiagnoseCheck {
                    category: "devices".to_owned(),
                    name: "registry".to_owned(),
                    status: "pass".to_owned(),
                    detail: format!("{count} tracked"),
                });

                let output_status = if snapshot.device_output.worker_finished_queues > 0
                    || snapshot.device_output.errors_total > 0
                {
                    "fail"
                } else if snapshot.device_output.lagging_queues > 0
                    || snapshot.device_output.dropped_frames_total > 0
                {
                    "warning"
                } else {
                    "pass"
                };
                checks.push(DiagnoseCheck {
                    category: "devices".to_owned(),
                    name: "output_queues".to_owned(),
                    status: output_status.to_owned(),
                    detail: format!(
                        "queues={}, usb_queues={}, lagging={}, worker_finished={}, dropped_total={}, errors_total={}",
                        snapshot.device_output.queues,
                        snapshot.device_output.usb_queues,
                        snapshot.device_output.lagging_queues,
                        snapshot.device_output.worker_finished_queues,
                        snapshot.device_output.dropped_frames_total,
                        snapshot.device_output.errors_total
                    ),
                });
                checks.push(DiagnoseCheck {
                    category: "devices".to_owned(),
                    name: "usb_actor_display_lane".to_owned(),
                    status: if snapshot.usb.display_led_priority_wait_max_ms >= 2.0 {
                        "warning"
                    } else {
                        "pass"
                    }
                    .to_owned(),
                    detail: format!(
                        "display_frames={}, delayed_for_led={}, wait_avg_ms={:.2}, wait_max_ms={:.2}",
                        snapshot.usb.display_frames_total,
                        snapshot.usb.display_frames_delayed_for_led_total,
                        snapshot.usb.display_led_priority_wait_avg_ms,
                        snapshot.usb.display_led_priority_wait_max_ms
                    ),
                });
            }
            "config" => {
                let has_manager = state.config_manager.is_some();
                checks.push(DiagnoseCheck {
                    category: "config".to_owned(),
                    name: "config_manager".to_owned(),
                    status: if has_manager { "pass" } else { "warning" }.to_owned(),
                    detail: if has_manager {
                        "available".to_owned()
                    } else {
                        "using defaults/test state".to_owned()
                    },
                });
            }
            other => {
                checks.push(DiagnoseCheck {
                    category: "custom".to_owned(),
                    name: other.to_owned(),
                    status: "warning".to_owned(),
                    detail: "unknown check".to_owned(),
                });
            }
        }
    }

    if include_system {
        checks.push(DiagnoseCheck {
            category: "system".to_owned(),
            name: "uptime_seconds".to_owned(),
            status: "pass".to_owned(),
            detail: state.start_time.elapsed().as_secs().to_string(),
        });
    }

    let mut passed = 0usize;
    let mut warnings = 0usize;
    let mut failed = 0usize;

    for check in &checks {
        match check.status.as_str() {
            "pass" => passed += 1,
            "fail" => failed += 1,
            _ => warnings += 1,
        }
    }

    ApiResponse::ok(DiagnoseResponse {
        checks,
        summary: DiagnoseSummary {
            passed,
            warnings,
            failed,
        },
        snapshot,
    })
}

fn render_frame_liveness_status(
    latest_frame: Option<&LatestFrameMetrics>,
    render_elapsed_ms: f64,
) -> (&'static str, String) {
    let Some(frame) = latest_frame else {
        return ("warning", "no completed frame recorded".to_owned());
    };

    let frame_age_ms = if frame.timestamp_ms > 0 {
        (render_elapsed_ms - f64::from(frame.timestamp_ms)).max(0.0)
    } else {
        0.0
    };
    let status = if frame_age_ms >= RENDER_FRAME_STALE_FAIL_MS {
        "fail"
    } else if frame_age_ms >= RENDER_FRAME_STALE_WARNING_MS {
        "warning"
    } else {
        "pass"
    };

    (
        status,
        format!(
            "frame_token={}, frame_age_ms={frame_age_ms:.2}",
            frame.timeline.frame_token
        ),
    )
}

fn render_led_freshness_status(
    latest_frame: Option<&LatestFrameMetrics>,
) -> (&'static str, String) {
    let Some(frame) = latest_frame else {
        return ("warning", "no completed frame recorded".to_owned());
    };

    let status = if frame.output_errors > 0 {
        "fail"
    } else if frame.gpu_sample_stale
        || frame.gpu_sample_wait_blocked
        || frame.gpu_sample_queue_saturated
        || frame.gpu_readback_failed
    {
        "warning"
    } else {
        "pass"
    };

    (
        status,
        format!(
            "output_source={}, reused_published_frame={}, gpu_sample_stale={}, gpu_sample_wait_blocked={}, gpu_sample_queue_saturated={}, devices_written={}, total_leds={}, sample_us={}, push_us={}",
            frame.output_frame_source.as_str(),
            frame.output_reuses_published_frame,
            frame.gpu_sample_stale,
            frame.gpu_sample_wait_blocked,
            frame.gpu_sample_queue_saturated,
            frame.devices_written,
            frame.total_leds,
            frame.sample_us,
            frame.push_us
        ),
    )
}

fn build_diagnose_snapshot(
    performance: &PerformanceSnapshot,
    render_elapsed_ms: f64,
    usb_actor_metrics: UsbActorMetricsSnapshot,
    device_metrics: &DeviceMetricsSnapshot,
) -> DiagnoseSnapshot {
    DiagnoseSnapshot {
        render: build_render_snapshot(performance, render_elapsed_ms),
        usb: build_usb_actor_snapshot(usb_actor_metrics),
        device_output: build_device_output_snapshot(device_metrics),
    }
}

fn build_render_snapshot(
    performance: &PerformanceSnapshot,
    render_elapsed_ms: f64,
) -> DiagnoseRenderSnapshot {
    let pacing = performance.pacing;
    DiagnoseRenderSnapshot {
        latest_frame: performance
            .latest_frame
            .as_ref()
            .map(|frame| DiagnoseLatestFrameSnapshot {
                frame_token: frame.timeline.frame_token,
                frame_age_ms: round_2(frame_age_ms(frame, render_elapsed_ms)),
                compositor_backend: frame.compositor_backend.as_str().to_owned(),
                output_frame_source: frame.output_frame_source.as_str().to_owned(),
                output_reuses_published_frame: frame.output_reuses_published_frame,
                output_brightness_bits: frame.output_brightness_bits,
                output_brightness_generation: frame.output_brightness_generation,
                output_routing_signature: frame.output_routing_signature,
                output_zone_shape_signature: frame.output_zone_shape_signature,
                output_unassigned_behavior_generation: frame.output_unassigned_behavior_generation,
                devices_written: frame.devices_written,
                total_leds: frame.total_leds,
                gpu_zone_sampling: frame.gpu_zone_sampling,
                gpu_sample_deferred: frame.gpu_sample_deferred,
                gpu_sample_stale: frame.gpu_sample_stale,
                gpu_sample_retry_hit: frame.gpu_sample_retry_hit,
                gpu_sample_queue_saturated: frame.gpu_sample_queue_saturated,
                gpu_sample_wait_blocked: frame.gpu_sample_wait_blocked,
                gpu_sample_cpu_fallback: frame.gpu_sample_cpu_fallback,
                cpu_sampling_late_readback: frame.cpu_sampling_late_readback,
                cpu_readback_skipped: frame.cpu_readback_skipped,
                gpu_readback_failed: frame.gpu_readback_failed,
                led_sampling_readback: frame.led_sampling_readback,
                input_us: frame.input_us,
                render_us: frame.render_us,
                producer_us: frame.producer_us,
                composition_us: frame.composition_us,
                sample_us: frame.sample_us,
                push_us: frame.push_us,
                publish_us: frame.publish_us,
                overhead_us: frame.overhead_us,
                total_us: frame.total_us,
                output_errors: frame.output_errors,
            }),
        recent_window: DiagnoseRenderWindowSnapshot {
            frames: performance.frame_count,
            gpu_sample_deferred: pacing.gpu_sample_deferred,
            gpu_sample_stale: pacing.gpu_sample_stale,
            gpu_sample_retry_hit: pacing.gpu_sample_retry_hit,
            gpu_sample_queue_saturated: pacing.gpu_sample_queue_saturated,
            gpu_sample_wait_blocked: pacing.gpu_sample_wait_blocked,
            gpu_sample_cpu_fallback: pacing.gpu_sample_cpu_fallback,
            led_sampling_readback: pacing.led_sampling_readback,
            output_current_frame: pacing.output_current_frame,
            output_published_frame: pacing.output_published_frame,
            output_routed_reuse: pacing.output_routed_reuse,
            output_reused_published_frame: pacing.output_reused_published_frame,
            output_error_frames: pacing.output_error_frames,
            push_avg_ms: round_2(pacing.push_avg_ms),
            push_p95_ms: round_2(pacing.push_p95_ms),
            publish_avg_ms: round_2(pacing.publish_avg_ms),
            publish_p95_ms: round_2(pacing.publish_p95_ms),
        },
    }
}

fn build_usb_actor_snapshot(metrics: UsbActorMetricsSnapshot) -> DiagnoseUsbActorSnapshot {
    let avg_wait_ms = metrics
        .display_led_priority_wait_total_us
        .checked_div(metrics.display_frames_delayed_for_led_total)
        .map_or(0.0, us_to_ms_f64);

    DiagnoseUsbActorSnapshot {
        display_frames_total: metrics.display_frames_total,
        display_frames_delayed_for_led_total: metrics.display_frames_delayed_for_led_total,
        display_led_priority_wait_total_ms: us_to_ms_f64(
            metrics.display_led_priority_wait_total_us,
        ),
        display_led_priority_wait_avg_ms: round_2(avg_wait_ms),
        display_led_priority_wait_max_ms: us_to_ms_f64(metrics.display_led_priority_wait_max_us),
    }
}

fn build_device_output_snapshot(metrics: &DeviceMetricsSnapshot) -> DiagnoseDeviceOutputSnapshot {
    let lagging_queues = metrics
        .items
        .iter()
        .filter(|item| device_output_lagging(item))
        .count();
    let worker_finished_queues = metrics
        .items
        .iter()
        .filter(|item| item.worker_finished)
        .count();
    let usb_queues = metrics
        .items
        .iter()
        .filter(|item| item.backend_id == USB_OUTPUT_BACKEND_ID)
        .count();
    let dropped_frames_total = metrics
        .items
        .iter()
        .fold(0_u64, |acc, item| acc.saturating_add(item.frames_dropped));
    let errors_total = metrics
        .items
        .iter()
        .fold(0_u64, |acc, item| acc.saturating_add(item.errors_total));
    let items = metrics
        .items
        .iter()
        .map(|item| DiagnoseDeviceOutputItem {
            id: item.id.to_string(),
            backend_id: item.backend_id.clone(),
            mapped_layout_ids: item.mapped_layout_ids.clone(),
            uses_frame_sink: item.uses_frame_sink,
            worker_finished: item.worker_finished,
            fps_sent: item.fps_sent,
            fps_queued: item.fps_queued,
            fps_target: item.fps_target,
            frames_received: item.frames_received,
            frames_sent: item.frames_sent,
            frames_dropped: item.frames_dropped,
            errors_total: item.errors_total,
            avg_latency_ms: item.avg_latency_ms,
            avg_queue_wait_ms: item.avg_queue_wait_ms,
            avg_write_ms: item.avg_write_ms,
            last_sent_ago_ms: item.last_sent_ago_ms,
            last_error: item.last_error.clone(),
            last_sequence: item.last_sequence,
        })
        .collect();

    DiagnoseDeviceOutputSnapshot {
        queues: metrics.items.len(),
        usb_queues,
        lagging_queues,
        worker_finished_queues,
        dropped_frames_total,
        errors_total,
        items,
    }
}

fn device_output_lagging(item: &DeviceMetrics) -> bool {
    item.fps_queued > 1.0 && item.fps_sent + 1.0 < item.fps_queued * 0.75
}

fn frame_age_ms(frame: &LatestFrameMetrics, render_elapsed_ms: f64) -> f64 {
    if frame.timestamp_ms > 0 {
        (render_elapsed_ms - f64::from(frame.timestamp_ms)).max(0.0)
    } else {
        0.0
    }
}

fn us_to_ms_f64(micros: u64) -> f64 {
    let clamped = u32::try_from(micros).unwrap_or(u32::MAX);
    round_2(f64::from(clamped) / 1000.0)
}

fn round_2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// `POST /api/v1/diagnose/memory` — Capture Servo memory profiler output.
pub async fn memory_diagnostics() -> Response {
    #[cfg(all(feature = "servo", target_os = "windows"))]
    {
        ApiError::not_found(
            "Servo memory diagnostics are disabled on Windows because the embedded memory reporter can abort the daemon",
        )
    }

    #[cfg(all(feature = "servo", not(target_os = "windows")))]
    {
        match tokio::task::spawn_blocking(hypercolor_core::effect::servo_memory_report_snapshot)
            .await
        {
            Ok(Ok(snapshot)) => ApiResponse::ok(snapshot),
            Ok(Err(error)) => {
                ApiError::internal(format!("Failed to collect Servo memory report: {error}"))
            }
            Err(error) => ApiError::internal(format!(
                "Servo memory diagnostics worker task failed: {error}"
            )),
        }
    }

    #[cfg(not(feature = "servo"))]
    {
        ApiError::not_found("Servo memory diagnostics are not available in this build")
    }
}

#[cfg(test)]
mod tests {
    use crate::performance::{FrameTimeline, LatestFrameMetrics, OutputFrameSourceKind};

    use super::{render_frame_liveness_status, render_led_freshness_status};

    #[test]
    fn render_frame_liveness_fails_stale_running_frame() {
        let (status, detail) = render_frame_liveness_status(
            Some(&LatestFrameMetrics {
                timestamp_ms: 1_000,
                timeline: FrameTimeline {
                    frame_token: 42,
                    ..FrameTimeline::default()
                },
                ..LatestFrameMetrics::default()
            }),
            12_500.0,
        );

        assert_eq!(status, "fail");
        assert_eq!(detail, "frame_token=42, frame_age_ms=11500.00");
    }

    #[test]
    fn render_frame_liveness_passes_fresh_running_frame() {
        let (status, detail) = render_frame_liveness_status(
            Some(&LatestFrameMetrics {
                timestamp_ms: 9_900,
                timeline: FrameTimeline {
                    frame_token: 43,
                    ..FrameTimeline::default()
                },
                ..LatestFrameMetrics::default()
            }),
            10_000.0,
        );

        assert_eq!(status, "pass");
        assert_eq!(detail, "frame_token=43, frame_age_ms=100.00");
    }

    #[test]
    fn render_led_freshness_warns_on_stale_gpu_sample() {
        let (status, detail) = render_led_freshness_status(Some(&LatestFrameMetrics {
            output_frame_source: OutputFrameSourceKind::PublishedFrame,
            output_reuses_published_frame: true,
            gpu_sample_stale: true,
            devices_written: 2,
            total_leds: 128,
            sample_us: 111,
            push_us: 222,
            ..LatestFrameMetrics::default()
        }));

        assert_eq!(status, "warning");
        assert!(detail.contains("output_source=published_frame"));
        assert!(detail.contains("gpu_sample_stale=true"));
        assert!(detail.contains("devices_written=2"));
    }
}
