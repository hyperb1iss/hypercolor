//! Transport-independent daemon diagnostics.

use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::device::{UsbActorMetricsSnapshot, usb_actor_metrics_snapshot};
use hypercolor_core::input::InputManager;
use hypercolor_types::api::diagnose::{
    DiagnoseCheck, DiagnoseDeviceOutputItem, DiagnoseDeviceOutputSnapshot,
    DiagnoseDisplayOutputSnapshot, DiagnoseLatestFrameSnapshot, DiagnoseRenderSnapshot,
    DiagnoseRenderWindowSnapshot, DiagnoseResponse, DiagnoseSnapshot, DiagnoseSummary,
    DiagnoseUsbActorSnapshot,
};
use hypercolor_types::api::system::InputStatus;
use hypercolor_types::device::USB_OUTPUT_BACKEND_ID;

use crate::device_metrics::{DeviceMetrics, DeviceMetricsSnapshot, DeviceMetricsSnapshotStore};
use crate::display_frames::DisplayOutputMetricsSnapshot;
use crate::domain::context::{DeviceContext, PlatformContext};
use crate::domain::display::DisplayContext;
use crate::domain::input_status::{actionable_input_diagnostics, input_status_snapshot};
use crate::domain::output::{OutputContext, RenderLoopStatus};
use crate::domain::spatial::SpatialService;
use crate::performance::{LatestFrameMetrics, PerformanceSnapshot};

const RENDER_FRAME_STALE_WARNING_MS: f64 = 2_000.0;
const RENDER_FRAME_STALE_FAIL_MS: f64 = 10_000.0;
const DEFAULT_SAFE_CHECKS: [&str; 6] = ["daemon", "render", "devices", "config", "input", "memory"];

/// The macOS screen-parity probe, armed only while a Metal render thread runs.
///
/// The render thread owns the mailbox the probe talks to, so the handle
/// is installed when that thread starts and dropped when it stops. Every
/// projection shares this one slot, so a restarted render thread re-arms
/// the check instead of leaving stale mailboxes behind.
#[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
#[derive(Clone)]
struct MacosScreenParityCapability {
    handle: Arc<arc_swap::ArcSwapOption<crate::render_thread::ScreenParityDiagnosticHandle>>,
    input: InputManager,
    spatial: SpatialService,
}

#[cfg(not(all(target_os = "macos", feature = "wgpu", feature = "screen-capture")))]
#[derive(Clone)]
struct MacosScreenParityCapability;

impl MacosScreenParityCapability {
    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    fn new(input: InputManager, spatial: SpatialService) -> Self {
        Self {
            handle: Arc::new(arc_swap::ArcSwapOption::empty()),
            input,
            spatial,
        }
    }

    #[cfg(not(all(target_os = "macos", feature = "wgpu", feature = "screen-capture")))]
    fn new(_input: InputManager, _spatial: SpatialService) -> Self {
        Self
    }

    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    fn install(&self, handle: Option<crate::render_thread::ScreenParityDiagnosticHandle>) {
        self.handle.store(handle.map(Arc::new));
    }

    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    async fn run(&self, snapshot: &mut DiagnoseSnapshot) -> DiagnoseCheck {
        let Some(handle) = self.handle.load_full() else {
            return DiagnoseCheck {
                category: "input".to_owned(),
                name: "macos_screen_parity".to_owned(),
                status: "warning".to_owned(),
                detail: "macOS screen parity requires the active Metal render thread".to_owned(),
            };
        };
        match super::macos_screen_parity::run_macos_screen_parity(
            handle.as_ref(),
            &self.input,
            &self.spatial,
        )
        .await
        {
            Ok(report) => {
                let detail = report.detail();
                match serde_json::to_value(report) {
                    Ok(report) => {
                        snapshot.macos_screen_parity = Some(report);
                        DiagnoseCheck {
                            category: "input".to_owned(),
                            name: "macos_screen_parity".to_owned(),
                            status: "pass".to_owned(),
                            detail,
                        }
                    }
                    Err(_) => DiagnoseCheck {
                        category: "input".to_owned(),
                        name: "macos_screen_parity".to_owned(),
                        status: "fail".to_owned(),
                        detail: "the parity report could not be serialized".to_owned(),
                    },
                }
            }
            Err(error) => DiagnoseCheck {
                category: "input".to_owned(),
                name: "macos_screen_parity".to_owned(),
                status: error.status().to_owned(),
                detail: error.detail().to_owned(),
            },
        }
    }

    #[cfg(not(all(target_os = "macos", feature = "wgpu", feature = "screen-capture")))]
    fn run(&self, _snapshot: &mut DiagnoseSnapshot) -> std::future::Ready<DiagnoseCheck> {
        std::future::ready(DiagnoseCheck {
            category: "input".to_owned(),
            name: "macos_screen_parity".to_owned(),
            status: "warning".to_owned(),
            detail: "macOS screen parity is unavailable in this build".to_owned(),
        })
    }
}

/// Everything one diagnostics transaction reads, captured once.
///
/// The checks and the reported snapshot are derived from the same
/// values, so a report can never describe a render loop from one instant
/// and a frame from another.
struct DiagnosticsInputs {
    uptime: Duration,
    performance: PerformanceSnapshot,
    render_loop: RenderLoopStatus,
    usb_actor: UsbActorMetricsSnapshot,
    display_output: DisplayOutputMetricsSnapshot,
    device_metrics: Arc<DeviceMetricsSnapshot>,
    input: InputStatus,
    tracked_devices: usize,
    config_available: bool,
}

impl DiagnosticsInputs {
    fn render_elapsed_ms(&self) -> f64 {
        self.uptime.as_secs_f64() * 1000.0
    }
}

struct DiagnosticsAuthorities {
    platform: PlatformContext,
    output: OutputContext,
    devices: DeviceContext,
    display: DisplayContext,
    device_metrics: DeviceMetricsSnapshotStore,
    parity: MacosScreenParityCapability,
}

/// Daemon health authority shared by every diagnostics transport.
///
/// Diagnostics reads across the whole daemon, so the authorities sit
/// behind one `Arc` and every projection shares them rather than
/// carrying its own copy of the graph.
#[derive(Clone)]
pub struct DiagnosticsContext {
    authorities: Arc<DiagnosticsAuthorities>,
}

impl DiagnosticsContext {
    pub(crate) fn new(
        platform: PlatformContext,
        output: OutputContext,
        devices: DeviceContext,
        display: DisplayContext,
        device_metrics: DeviceMetricsSnapshotStore,
        input: InputManager,
        spatial: SpatialService,
    ) -> Self {
        Self {
            authorities: Arc::new(DiagnosticsAuthorities {
                platform,
                output,
                devices,
                display,
                device_metrics,
                parity: MacosScreenParityCapability::new(input, spatial),
            }),
        }
    }

    /// Arm or disarm the macOS screen-parity probe around the render thread.
    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    pub(crate) fn install_macos_screen_parity(
        &self,
        handle: Option<crate::render_thread::ScreenParityDiagnosticHandle>,
    ) {
        self.authorities.parity.install(handle);
    }

    /// Run the safe default check set.
    pub async fn collect_default(&self) -> DiagnoseResponse {
        self.collect(&default_safe_checks(), false).await
    }

    /// Run the requested checks against one consistent set of readings.
    pub async fn collect(&self, requested: &[String], include_system: bool) -> DiagnoseResponse {
        let inputs = self.read_inputs().await;
        let mut snapshot = build_diagnose_snapshot(&inputs);
        let mut checks = Vec::new();

        for check in requested {
            match check.as_str() {
                "daemon" => checks.push(daemon_check()),
                "render" => checks.extend(render_checks(&inputs)),
                "devices" => checks.extend(device_checks(&inputs, &snapshot)),
                "config" => checks.push(config_check(&inputs)),
                "input" => checks.extend(input_checks(&snapshot.input)),
                "memory" => checks.push(servo_memory_check().await),
                "macos_screen_parity" => {
                    checks.push(self.authorities.parity.run(&mut snapshot).await);
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
                detail: inputs.uptime.as_secs().to_string(),
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

        DiagnoseResponse {
            checks,
            summary: DiagnoseSummary {
                passed,
                warnings,
                failed,
            },
            snapshot,
        }
    }

    async fn read_inputs(&self) -> DiagnosticsInputs {
        DiagnosticsInputs {
            uptime: self.authorities.output.uptime(),
            performance: self.authorities.output.performance_snapshot().await,
            render_loop: self.authorities.output.render_loop_status().await,
            usb_actor: usb_actor_metrics_snapshot(),
            display_output: self
                .authorities
                .display
                .frames()
                .read()
                .await
                .metrics_snapshot(),
            device_metrics: self.authorities.device_metrics.load_full(),
            input: input_status_snapshot(&self.authorities.platform),
            tracked_devices: self.authorities.devices.device_registry().len().await,
            config_available: self.authorities.platform.config_available(),
        }
    }
}

pub(crate) fn default_safe_checks() -> Vec<String> {
    DEFAULT_SAFE_CHECKS.into_iter().map(str::to_owned).collect()
}

fn daemon_check() -> DiagnoseCheck {
    DiagnoseCheck {
        category: "system".to_owned(),
        name: "daemon_running".to_owned(),
        status: "pass".to_owned(),
        detail: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

fn render_checks(inputs: &DiagnosticsInputs) -> Vec<DiagnoseCheck> {
    let performance = &inputs.performance;
    let render_elapsed_ms = inputs.render_elapsed_ms();
    let running = inputs.render_loop.running;
    let render_loop_stats = inputs.render_loop.stats;
    let mut checks = vec![DiagnoseCheck {
        category: "render".to_owned(),
        name: "render_loop".to_owned(),
        status: if running { "pass" } else { "warning" }.to_owned(),
        detail: format!(
            "state={}, tier={}",
            render_loop_stats.state, render_loop_stats.tier
        ),
    }];
    if !running {
        return checks;
    }

    let (status, detail) =
        render_frame_liveness_status(performance.latest_frame.as_ref(), render_elapsed_ms);
    checks.push(DiagnoseCheck {
        category: "render".to_owned(),
        name: "frame_liveness".to_owned(),
        status: status.to_owned(),
        detail,
    });

    let (status, detail) = render_led_freshness_status(performance.latest_frame.as_ref());
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
    checks
}

fn device_checks(inputs: &DiagnosticsInputs, snapshot: &DiagnoseSnapshot) -> Vec<DiagnoseCheck> {
    let count = inputs.tracked_devices;
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
    vec![
        DiagnoseCheck {
            category: "devices".to_owned(),
            name: "registry".to_owned(),
            status: "pass".to_owned(),
            detail: format!("{count} tracked"),
        },
        DiagnoseCheck {
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
        },
        DiagnoseCheck {
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
        },
        DiagnoseCheck {
            category: "devices".to_owned(),
            name: "display_output_encoder".to_owned(),
            status: if snapshot.display_output.encode_failures_total > 0 {
                "warning"
            } else {
                "pass"
            }
            .to_owned(),
            detail: format!(
                "attempts={}, successes={}, failures={}, avg_ms={:.2}, max_ms={:.2}, last_ms={}, last_bytes={}",
                snapshot.display_output.encode_attempts_total,
                snapshot.display_output.encode_successes_total,
                snapshot.display_output.encode_failures_total,
                snapshot.display_output.encode_avg_ms,
                snapshot.display_output.encode_max_ms,
                snapshot
                    .display_output
                    .encode_last_ms
                    .map_or_else(|| "none".to_owned(), |value| format!("{value:.2}")),
                snapshot.display_output.encoded_last_bytes
            ),
        },
    ]
}

fn config_check(inputs: &DiagnosticsInputs) -> DiagnoseCheck {
    let has_manager = inputs.config_available;
    DiagnoseCheck {
        category: "config".to_owned(),
        name: "config_manager".to_owned(),
        status: if has_manager { "pass" } else { "warning" }.to_owned(),
        detail: if has_manager {
            "available".to_owned()
        } else {
            "using defaults/test state".to_owned()
        },
    }
}

fn input_checks(input: &InputStatus) -> Vec<DiagnoseCheck> {
    let diagnostics = actionable_input_diagnostics(input);
    if diagnostics.is_empty() {
        return vec![DiagnoseCheck {
            category: "input".to_owned(),
            name: "source_health".to_owned(),
            status: "pass".to_owned(),
            detail: format!(
                "{} source(s), graph generation {}",
                input.sources.len(),
                input.source_graph_generation
            ),
        }];
    }

    diagnostics
        .into_iter()
        .map(|diagnostic| DiagnoseCheck {
            category: "input".to_owned(),
            name: diagnostic.source_id,
            status: diagnostic.status.to_owned(),
            detail: diagnostic.detail,
        })
        .collect()
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

fn build_diagnose_snapshot(inputs: &DiagnosticsInputs) -> DiagnoseSnapshot {
    DiagnoseSnapshot {
        input: inputs.input.clone(),
        render: build_render_snapshot(&inputs.performance, inputs.render_elapsed_ms()),
        usb: build_usb_actor_snapshot(inputs.usb_actor),
        display_output: build_display_output_snapshot(inputs.display_output.clone()),
        device_output: build_device_output_snapshot(inputs.device_metrics.as_ref()),
        macos_screen_parity: None,
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
                cpu_readback_skipped: frame.cpu_readback_skipped,
                gpu_readback_failed: frame.gpu_readback_failed,
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

fn build_display_output_snapshot(
    metrics: DisplayOutputMetricsSnapshot,
) -> DiagnoseDisplayOutputSnapshot {
    DiagnoseDisplayOutputSnapshot {
        captured_devices: metrics.captured_devices,
        preview_subscribers: metrics.preview_subscribers,
        encode_attempts_total: metrics.encode_attempts_total,
        encode_successes_total: metrics.encode_successes_total,
        encode_failures_total: metrics.encode_failures_total,
        encode_avg_ms: round_2(us_to_ms_f64(metrics.encode_avg_us)),
        encode_max_ms: round_2(us_to_ms_f64(metrics.encode_max_us)),
        encode_last_ms: metrics.encode_last_us.map(us_to_ms_f64).map(round_2),
        encoded_bytes_total: metrics.encoded_bytes_total,
        encoded_last_bytes: metrics.encoded_last_bytes,
        write_attempts_total: metrics.write_attempts_total,
        write_successes_total: metrics.write_successes_total,
        write_failures_total: metrics.write_failures_total,
        retry_attempts_total: metrics.retry_attempts_total,
        last_failure_age_ms: metrics.last_failure_age_ms,
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
            delivered_fps: item.delivered_fps,
            accepted_fps: item.accepted_fps,
            fps_sent: item.fps_sent,
            fps_queued: item.fps_queued,
            fps_target: item.fps_target,
            frames_received: item.frames_received,
            accepted: item.accepted,
            frames_sent: item.frames_sent,
            transport_started: item.transport_started,
            transport_completed: item.transport_completed,
            transport_failed: item.transport_failed,
            completed_payload_bytes: item.completed_payload_bytes,
            frames_dropped: item.frames_dropped,
            coalesced: item.coalesced,
            coalesced_target_cadence: item.coalesced_target_cadence,
            coalesced_backend_overrun: item.coalesced_backend_overrun,
            errors_total: item.errors_total,
            avg_latency_ms: item.avg_latency_ms,
            avg_queue_wait_ms: item.avg_queue_wait_ms,
            avg_write_ms: item.avg_write_ms,
            avg_transport_latency_ms: item.avg_transport_latency_ms,
            last_sent_ago_ms: item.last_sent_ago_ms,
            last_error: item.last_error.clone(),
            last_sequence: item.last_sequence,
            queue_generation: item.queue_generation,
            last_transport_started_sequence: item.last_transport_started_sequence,
            last_transport_completed_sequence: item.last_transport_completed_sequence,
            last_transport_failed_sequence: item.last_transport_failed_sequence,
            display_queue_generation: item.display_queue_generation,
            display_transport_started: item.display_transport_started,
            display_transport_completed: item.display_transport_completed,
            display_transport_failed: item.display_transport_failed,
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

#[cfg(all(feature = "servo", not(target_os = "windows")))]
async fn servo_memory_check() -> DiagnoseCheck {
    match tokio::task::spawn_blocking(hypercolor_core::effect::servo_memory_report_snapshot).await {
        Ok(Ok(snapshot)) => DiagnoseCheck {
            category: "memory".to_owned(),
            name: "servo_memory".to_owned(),
            status: "pass".to_owned(),
            detail: format!(
                "processes={}, reports={}, explicit_bytes={}, non_explicit_bytes={}",
                snapshot.processes.len(),
                snapshot.totals.report_count,
                snapshot.totals.explicit_bytes,
                snapshot.totals.non_explicit_bytes
            ),
        },
        Ok(Err(error)) => servo_memory_failure(error.to_string()),
        Err(error) => servo_memory_failure(format!("worker task failed: {error}")),
    }
}

#[cfg(not(all(feature = "servo", not(target_os = "windows"))))]
fn servo_memory_check() -> std::future::Ready<DiagnoseCheck> {
    let detail = if cfg!(all(feature = "servo", target_os = "windows")) {
        "Servo memory reporting is disabled on Windows"
    } else {
        "Servo memory reporting is unavailable in this build"
    };

    std::future::ready(DiagnoseCheck {
        category: "memory".to_owned(),
        name: "servo_memory".to_owned(),
        status: "warning".to_owned(),
        detail: detail.to_owned(),
    })
}

#[cfg(all(feature = "servo", not(target_os = "windows")))]
fn servo_memory_failure(detail: String) -> DiagnoseCheck {
    let unavailable = detail.contains("Servo worker is not running");
    DiagnoseCheck {
        category: "memory".to_owned(),
        name: "servo_memory".to_owned(),
        status: if unavailable { "warning" } else { "fail" }.to_owned(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use crate::performance::{FrameTimeline, LatestFrameMetrics, OutputFrameSourceKind};

    use super::{render_frame_liveness_status, render_led_freshness_status};

    #[cfg(all(feature = "servo", not(target_os = "windows")))]
    use super::servo_memory_failure;

    #[cfg(all(feature = "servo", not(target_os = "windows")))]
    #[test]
    fn servo_memory_failure_is_a_named_diagnostic_finding() {
        let finding = servo_memory_failure("memory callback failed".to_owned());

        assert_eq!(finding.category, "memory");
        assert_eq!(finding.name, "servo_memory");
        assert_eq!(finding.status, "fail");
        assert_eq!(finding.detail, "memory callback failed");
    }

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
