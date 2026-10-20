//! Transport-independent daemon diagnostics.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::config::ConfigPersistence;
use hypercolor_core::device::{UsbActorMetricsSnapshot, usb_actor_metrics_snapshot};
use hypercolor_core::input::InputManager;
use hypercolor_types::api::diagnose::{
    DiagnoseCheck, DiagnoseDeviceOutputItem, DiagnoseDeviceOutputSnapshot,
    DiagnoseDisplayOutputSnapshot, DiagnoseLatestFrameSnapshot, DiagnoseRenderSnapshot,
    DiagnoseRenderWindowSnapshot, DiagnoseResponse, DiagnoseSnapshot, DiagnoseSummary,
    DiagnoseUsbActorSnapshot,
};
use hypercolor_types::api::system::{InputStatus, SoftwareConflictsStatus};
use hypercolor_types::device::USB_OUTPUT_BACKEND_ID;

use crate::device_metrics::{DeviceMetrics, DeviceMetricsSnapshot, DeviceMetricsSnapshotStore};
use crate::display_frames::DisplayOutputMetricsSnapshot;
use crate::domain::context::{DeviceContext, PlatformContext};
use crate::domain::display::DisplayContext;
use crate::domain::input_status::{actionable_input_diagnostics, input_status_snapshot};
use crate::domain::output::{OutputContext, RenderLoopStatus};
use crate::domain::spatial::SpatialService;
use crate::output_power::{OutputOverride, OutputPowerState};
use crate::performance::{LatestFrameMetrics, PerformanceSnapshot};
use crate::render_thread::gpu_errors::GpuErrorTotals;
use crate::render_thread::{RenderSupervisionSnapshot, RenderThreadPhase};
use hypercolor_core::engine::RenderLoopState;

const RENDER_FRAME_STALE_WARNING_MS: f64 = 2_000.0;
const RENDER_FRAME_STALE_FAIL_MS: f64 = 10_000.0;
/// How long the render thread may go without completing a frame (or, when
/// paused, without polling) before the `render_thread` check fails. A
/// healthy loop completes a throttled idle frame at least every quarter
/// second, so a few seconds of silence means it is wedged or gone.
pub const RENDER_THREAD_STALL_FAIL: Duration = Duration::from_secs(3);
const DEFAULT_SAFE_CHECKS: [&str; 8] = [
    "daemon",
    "render",
    "devices",
    "conflicts",
    "config",
    "input",
    "memory",
    "openrgb",
];

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
    config_persistence: Option<(PathBuf, ConfigPersistence)>,
    render_supervision: RenderSupervisionSnapshot,
    output_power: OutputPowerState,
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
                "devices" => {
                    checks.extend(device_checks(&inputs, &snapshot));
                    checks.extend(super::hardware_broker_diagnostics::smbus_broker_check().await);
                }
                "conflicts" => checks.push(self.conflicts_check().await),
                "config" => checks.extend(config_checks(&inputs)),
                "input" => checks.extend(input_checks(&snapshot.input)),
                "memory" => checks.push(servo_memory_check().await),
                "openrgb" => checks.push(self.openrgb_check().await),
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

    /// Report competing RGB software from a scan no older than the watch
    /// interval, scanning first (within a time budget) only when the
    /// latest one is older.
    async fn conflicts_check(&self) -> DiagnoseCheck {
        let store = self.authorities.devices.software_conflicts();
        crate::software_conflicts::scan_if_stale(
            &store,
            crate::software_conflicts::RESCAN_INTERVAL,
            crate::software_conflicts::DIAGNOSE_SCAN_BUDGET,
        )
        .await;
        conflicts_check(&store.status())
    }

    /// Probe the OpenRGB bridge: reachability and protocol handshake per
    /// configured endpoint, plus the routes the daemon holds output-disabled.
    async fn openrgb_check(&self) -> DiagnoseCheck {
        use super::openrgb_diagnostics::{OpenRgbCheckState, OpenRgbProbeConfig};

        let devices = &self.authorities.devices;
        let registry = devices.driver_registry();
        let Some(driver) = registry.get(super::openrgb_diagnostics::OPENRGB_DRIVER_ID) else {
            return super::openrgb_diagnostics::openrgb_check(&OpenRgbCheckState::Unavailable);
        };
        let config = devices
            .config_snapshot()
            .unwrap_or_else(|| Arc::new(hypercolor_types::config::HypercolorConfig::default()));
        if !crate::network::module_enabled(&config, &driver.module_descriptor()) {
            return super::openrgb_diagnostics::openrgb_check(&OpenRgbCheckState::Disabled);
        }
        let entry = crate::network::driver_config_entry(
            &config,
            super::openrgb_diagnostics::OPENRGB_DRIVER_ID,
        );
        let probe_config = OpenRgbProbeConfig::from_driver_entry(&entry);
        let probes = super::openrgb_diagnostics::probe_openrgb_endpoints(&probe_config).await;
        let disabled_routes =
            super::openrgb_diagnostics::output_disabled_routes(&devices.discovery_runtime()).await;
        super::openrgb_diagnostics::openrgb_check(&OpenRgbCheckState::Probed {
            probes,
            disabled_routes,
        })
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
            config_persistence: self.authorities.platform.config_persistence(),
            render_supervision: self.authorities.output.render_supervision().snapshot(),
            output_power: self.authorities.output.power_state(),
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
    let pause = render_loop_pause_reason(render_loop_stats.state, inputs.output_power)
        .map_or_else(String::new, |reason| format!("; {reason}"));
    let mut render_thread = render_thread_check(
        &inputs.render_supervision,
        render_loop_stats.state,
        RENDER_THREAD_STALL_FAIL,
    );
    render_thread.detail.push_str(&pause);
    let mut checks = vec![
        DiagnoseCheck {
            category: "render".to_owned(),
            name: "render_loop".to_owned(),
            status: if running { "pass" } else { "warning" }.to_owned(),
            detail: format!(
                "state={}, tier={}{pause}",
                render_loop_stats.state, render_loop_stats.tier
            ),
        },
        render_thread,
        gpu_errors_check(&inputs.render_supervision.gpu_errors),
    ];
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

    let (status, detail) = render_multi_zone_sampling_status(performance.latest_frame.as_ref());
    checks.push(DiagnoseCheck {
        category: "render".to_owned(),
        name: "multi_zone_sampling".to_owned(),
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

/// What paused the render loop, for the render checks' details.
///
/// A paused loop renders nothing by design, which looks like a hang from
/// the outside, so the details say where the pause came from.
fn render_loop_pause_reason(
    loop_state: RenderLoopState,
    power: OutputPowerState,
) -> Option<&'static str> {
    if loop_state != RenderLoopState::Paused {
        return None;
    }
    Some(match power.output_override {
        OutputOverride::Paused => {
            "paused by output power: a manual pause (PATCH /api/v1/output or a pause \
             control), which persists across restarts until power is set back to running"
        }
        OutputOverride::Stopped => "paused by output power: the last effect was stopped",
        OutputOverride::None if power.session_sleeping => "paused while the session sleeps",
        OutputOverride::None => "paused, though output power reports running",
    })
}

/// Whether the supervised render thread is alive and producing frames.
///
/// Fails while a pipeline is restarting or the daemon is terminating after
/// an unrecoverable failure, when the render thread is gone while the loop
/// still runs, and when a running loop has completed no frame (or a paused
/// loop has not polled) for `stall_after`.
#[must_use]
pub fn render_thread_check(
    supervision: &RenderSupervisionSnapshot,
    loop_state: RenderLoopState,
    stall_after: Duration,
) -> DiagnoseCheck {
    let (status, detail) = render_thread_status(supervision, loop_state, stall_after);
    DiagnoseCheck {
        category: "render".to_owned(),
        name: "render_thread".to_owned(),
        status: status.to_owned(),
        detail,
    }
}

fn render_thread_status(
    supervision: &RenderSupervisionSnapshot,
    loop_state: RenderLoopState,
    stall_after: Duration,
) -> (&'static str, String) {
    let failure = supervision.last_failure.as_ref().map_or_else(
        || "none".to_owned(),
        |record| {
            format!(
                "{}{} at {}: {}",
                record.kind.as_str(),
                if supervision.last_failure_in_this_process {
                    ""
                } else {
                    " (earlier daemon run)"
                },
                record.occurred_at,
                record.message
            )
        },
    );
    let loop_active = matches!(
        loop_state,
        RenderLoopState::Running | RenderLoopState::Paused
    );
    match supervision.phase {
        RenderThreadPhase::Terminating => (
            "fail",
            format!(
                "the render thread could not recover after {} restart(s); the daemon is \
                 exiting so its service manager restarts it. Last failure: {failure}",
                supervision.restarts_total
            ),
        ),
        RenderThreadPhase::Restarting => (
            "fail",
            format!(
                "the render pipeline is restarting after {} failure(s) in a row ({} restart(s) \
                 so far, {} allowed per window). Last failure: {failure}",
                supervision.consecutive_failures,
                supervision.restarts_total,
                supervision.max_restarts
            ),
        ),
        RenderThreadPhase::Running => {
            let (age, activity) = if loop_state == RenderLoopState::Paused {
                (supervision.last_heartbeat_age, "polled")
            } else {
                (supervision.last_frame_age, "completed a frame")
            };
            match age {
                Some(age) if loop_active && age >= stall_after => (
                    "fail",
                    format!(
                        "the render thread has not {activity} for {} ms while the render loop \
                         is {loop_state}; restarts={}, last_failure={failure}",
                        age.as_millis(),
                        supervision.restarts_total
                    ),
                ),
                _ if loop_state == RenderLoopState::Paused => (
                    "pass",
                    format!(
                        "phase=running, the render loop is paused so no frames are due, \
                         last_poll_age_ms={}, last_frame_age_ms={}, restarts={}, \
                         last_failure={failure}",
                        millis_or_none(supervision.last_heartbeat_age),
                        millis_or_none(supervision.last_frame_age),
                        supervision.restarts_total
                    ),
                ),
                _ => (
                    "pass",
                    format!(
                        "phase=running, last_frame_age_ms={}, restarts={}, last_failure={failure}",
                        millis_or_none(supervision.last_frame_age),
                        supervision.restarts_total
                    ),
                ),
            }
        }
        phase @ (RenderThreadPhase::NotStarted
        | RenderThreadPhase::Starting
        | RenderThreadPhase::Stopped) => {
            if loop_active {
                (
                    "fail",
                    format!(
                        "the render thread is {} while the render loop is {loop_state}; nothing \
                         is rendering. Last failure: {failure}",
                        phase.as_str()
                    ),
                )
            } else {
                (
                    "pass",
                    format!(
                        "phase={}, render loop {loop_state}, last_failure={failure}",
                        phase.as_str()
                    ),
                )
            }
        }
    }
}

/// How recent the latest uncaptured GPU error must be for the
/// `gpu_errors` check to warn. Older errors stay in the counts.
pub const GPU_ERROR_WARNING_WINDOW: Duration = Duration::from_mins(10);

fn millis_or_none(age: Option<Duration>) -> String {
    age.map_or_else(|| "none".to_owned(), |age| age.as_millis().to_string())
}

/// Whether a GPU error went uncaptured recently.
///
/// Uncaptured validation, out-of-memory, and internal errors no longer
/// panic the thread that raised them, so this check is where they stay
/// visible: the counts since startup are always in the detail, an error
/// within [`GPU_ERROR_WARNING_WINDOW`] warns, and a lost device fails.
#[must_use]
pub fn gpu_errors_check(totals: &GpuErrorTotals) -> DiagnoseCheck {
    let now_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    gpu_errors_check_at(totals, now_unix_ms)
}

fn gpu_errors_check_at(totals: &GpuErrorTotals, now_unix_ms: u64) -> DiagnoseCheck {
    let last_age_ms = totals
        .last
        .as_ref()
        .map(|last| now_unix_ms.saturating_sub(last.at_unix_ms));
    let last = totals.last.as_ref().map_or_else(String::new, |last| {
        format!(
            "; last ({} ms ago): {}: {}: {}",
            last_age_ms.unwrap_or_default(),
            last.device,
            last.kind,
            last.message
        )
    });
    let counts = format!(
        "validation={}, out_of_memory={}, internal={}, device_lost={}",
        totals.validation, totals.out_of_memory, totals.internal, totals.device_lost
    );
    let window_ms = u64::try_from(GPU_ERROR_WARNING_WINDOW.as_millis()).unwrap_or(u64::MAX);
    let status = if totals.device_lost > 0 {
        "fail"
    } else if totals.uncaptured() > 0 && last_age_ms.is_none_or(|age| age < window_ms) {
        "warning"
    } else {
        "pass"
    };
    DiagnoseCheck {
        category: "render".to_owned(),
        name: "gpu_errors".to_owned(),
        status: status.to_owned(),
        detail: format!("{counts}{last}"),
    }
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

fn conflicts_check(status: &SoftwareConflictsStatus) -> DiagnoseCheck {
    let found = status
        .conflicts
        .iter()
        .map(|conflict| {
            format!(
                "{} ({}): {}",
                conflict.name,
                conflict.matched.join(", "),
                conflict.remedy
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let (status_label, detail) = if !status.supported {
        (
            "pass",
            "running software is not inspected on this platform yet".to_owned(),
        )
    } else if status.scan_failed && found.is_empty() {
        (
            "warning",
            "could not list running software, so competing programs are unknown".to_owned(),
        )
    } else if status.scan_failed {
        (
            "warning",
            format!("could not list running software; the last successful scan found {found}"),
        )
    } else if found.is_empty() {
        ("pass", "no competing RGB software is running".to_owned())
    } else {
        ("warning", found)
    };
    DiagnoseCheck {
        category: "devices".to_owned(),
        name: "competing_software".to_owned(),
        status: status_label.to_owned(),
        detail,
    }
}

fn config_checks(inputs: &DiagnosticsInputs) -> Vec<DiagnoseCheck> {
    let has_manager = inputs.config_available;
    let mut checks = vec![DiagnoseCheck {
        category: "config".to_owned(),
        name: "config_manager".to_owned(),
        status: if has_manager { "pass" } else { "warning" }.to_owned(),
        detail: if has_manager {
            "available".to_owned()
        } else {
            "using defaults/test state".to_owned()
        },
    }];
    if let Some((path, persistence)) = &inputs.config_persistence {
        checks.push(config_persistence_check(path, persistence));
    }
    checks
}

/// Warn while the config file lags the live config.
///
/// A config write the file system refuses stays live, and the flush
/// registry retries the file in the background. Until a retry lands, a
/// restart loads the older file, so the check names the file and the
/// failure that left it behind, and passes again once the retry lands.
fn config_persistence_check(
    path: &std::path::Path,
    persistence: &ConfigPersistence,
) -> DiagnoseCheck {
    let (status, detail) = match persistence {
        ConfigPersistence::Durable => ("pass", format!("{} holds the live config", path.display())),
        ConfigPersistence::Lagging { path, error } => (
            "warning",
            format!(
                "live config is ahead of {}; the daemon keeps retrying the write, and a restart \
                 before it lands loads the older file. Last error: {error}",
                path.display()
            ),
        ),
    };
    DiagnoseCheck {
        category: "config".to_owned(),
        name: "config_persistence".to_owned(),
        status: status.to_owned(),
        detail,
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

/// Whether every zone of the latest frame's per-zone sampling set had a
/// frame to sample (Spec 86 §7).
///
/// A held zone has no samplable frame, so its LEDs hold their published
/// colors or go black. The frame metrics carry counts, not zone names, so
/// the warning reports how many zones held. Frames that sampled no zone
/// set (a single-zone scene, or the CPU backend) pass as not applicable.
fn render_multi_zone_sampling_status(
    latest_frame: Option<&LatestFrameMetrics>,
) -> (&'static str, String) {
    let Some(frame) = latest_frame else {
        return ("warning", "no completed frame recorded".to_owned());
    };

    let zones = frame
        .gpu_sampled_zone_count
        .saturating_add(frame.cpu_sampled_zone_count)
        .saturating_add(frame.held_zone_count);
    if zones == 0 {
        return (
            "pass",
            format!(
                "not applicable: the latest frame sampled no per-zone set, compositor_backend={}",
                frame.compositor_backend.as_str()
            ),
        );
    }

    let counts = format!(
        "gpu_sampled_zones={}, cpu_sampled_zones={}, held_zones={}, gpu_sampling_passes={}, zone_composition_skips={}",
        frame.gpu_sampled_zone_count,
        frame.cpu_sampled_zone_count,
        frame.held_zone_count,
        frame.gpu_sampling_passes,
        frame.zone_composition_skips
    );
    if frame.held_zone_count == 0 {
        return ("pass", counts);
    }
    (
        "warning",
        format!(
            "{} of {zones} zone(s) had no frame to sample and held their published colors or went black: {counts}",
            frame.held_zone_count
        ),
    )
}

fn build_diagnose_snapshot(inputs: &DiagnosticsInputs) -> DiagnoseSnapshot {
    DiagnoseSnapshot {
        input: inputs.input.clone(),
        render: build_render_snapshot(
            &inputs.performance,
            inputs.render_elapsed_ms(),
            &inputs.render_supervision,
        ),
        usb: build_usb_actor_snapshot(inputs.usb_actor),
        display_output: build_display_output_snapshot(inputs.display_output.clone()),
        device_output: build_device_output_snapshot(inputs.device_metrics.as_ref()),
        macos_screen_parity: None,
    }
}

fn build_render_snapshot(
    performance: &PerformanceSnapshot,
    render_elapsed_ms: f64,
    supervision: &RenderSupervisionSnapshot,
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
                gpu_sampled_zone_count: frame.gpu_sampled_zone_count,
                cpu_sampled_zone_count: frame.cpu_sampled_zone_count,
                held_zone_count: frame.held_zone_count,
                gpu_sampling_passes: frame.gpu_sampling_passes,
                zone_composition_skips: frame.zone_composition_skips,
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
        supervision: supervision.status(true),
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
            transport_declined: item.transport_declined,
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
    use crate::performance::{
        CompositorBackendKind, FrameTimeline, LatestFrameMetrics, OutputFrameSourceKind,
        PerformanceSnapshot,
    };

    use hypercolor_types::api::system::{SoftwareConflict, SoftwareConflictsStatus};

    use super::{
        build_device_output_snapshot, build_render_snapshot, conflicts_check,
        render_frame_liveness_status, render_led_freshness_status,
        render_multi_zone_sampling_status,
    };
    use crate::device_metrics::{DeviceMetrics, DeviceMetricsSnapshot};
    use hypercolor_types::device::DeviceId;

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
    fn device_output_snapshot_reports_declined_deliveries() {
        let item = DeviceMetrics {
            id: DeviceId::new(),
            backend_id: "blocks".to_owned(),
            mapped_layout_ids: vec!["blocks:pad".to_owned()],
            uses_frame_sink: false,
            worker_finished: false,
            worker_recoveries: 0,
            delivered_fps: 25.0,
            accepted_fps: 25.0,
            fps_sent: 25.0,
            fps_queued: 25.0,
            fps_actual: 25.0,
            fps_target: 25,
            target_interval_ms: None,
            payload_bps_estimate: 900,
            avg_latency_ms: 4,
            avg_queue_wait_ms: 1,
            avg_write_ms: 3,
            avg_transport_latency_ms: 3,
            frames_received: 5,
            accepted: 5,
            frames_sent: 3,
            transport_started: 5,
            transport_completed: 3,
            transport_failed: 0,
            transport_declined: 2,
            completed_payload_bytes: 0,
            frames_suppressed: 0,
            frames_dropped: 0,
            coalesced: 0,
            coalesced_target_cadence: 0,
            coalesced_backend_overrun: 0,
            errors_total: 0,
            write_failure_warnings_total: 0,
            last_error: None,
            last_sent_ago_ms: None,
            last_sequence: 5,
            queue_generation: 0,
            last_transport_started_sequence: 5,
            last_transport_completed_sequence: 3,
            last_transport_failed_sequence: 0,
            display_queue_generation: None,
            display_transport_started: 0,
            display_transport_completed: 0,
            display_transport_failed: 0,
        };

        let snapshot = build_device_output_snapshot(&DeviceMetricsSnapshot {
            taken_at_ms: 0,
            items: vec![item],
        });

        let item = &snapshot.items[0];
        assert_eq!(item.transport_declined, 2);
        assert_eq!(
            item.transport_started,
            item.transport_completed + item.transport_failed + item.transport_declined
        );
        assert_eq!(snapshot.errors_total, 0, "a decline is not an error");
    }

    #[test]
    fn render_snapshot_reports_the_zone_sampling_counters() {
        let snapshot = build_render_snapshot(
            &PerformanceSnapshot {
                latest_frame: Some(LatestFrameMetrics {
                    gpu_sampled_zone_count: 2,
                    cpu_sampled_zone_count: 1,
                    held_zone_count: 1,
                    gpu_sampling_passes: 3,
                    zone_composition_skips: 2,
                    ..LatestFrameMetrics::default()
                }),
                ..PerformanceSnapshot::default()
            },
            0.0,
            &crate::render_thread::RenderSupervision::detached().snapshot(),
        );

        let frame = snapshot
            .latest_frame
            .expect("a recorded frame should be reported");
        assert_eq!(frame.gpu_sampled_zone_count, 2);
        assert_eq!(frame.cpu_sampled_zone_count, 1);
        assert_eq!(frame.held_zone_count, 1);
        assert_eq!(frame.gpu_sampling_passes, 3);
        assert_eq!(frame.zone_composition_skips, 2);
    }

    fn supervision(
        phase: crate::render_thread::RenderThreadPhase,
        frame_age_ms: u64,
        heartbeat_age_ms: u64,
    ) -> crate::render_thread::RenderSupervisionSnapshot {
        crate::render_thread::RenderSupervisionSnapshot {
            phase,
            restarts_total: 0,
            consecutive_failures: 0,
            max_restarts: 5,
            last_heartbeat_age: Some(std::time::Duration::from_millis(heartbeat_age_ms)),
            last_frame_age: Some(std::time::Duration::from_millis(frame_age_ms)),
            current_run: None,
            last_failure: None,
            last_failure_in_this_process: false,
            gpu_errors: crate::render_thread::gpu_errors::GpuErrorTotals::default(),
        }
    }

    #[test]
    fn render_thread_check_fails_on_stalls_restarts_and_a_missing_thread() {
        use crate::render_thread::RenderThreadPhase;
        use hypercolor_core::engine::RenderLoopState;

        let stall = std::time::Duration::from_secs(3);
        let cases = [
            (
                RenderThreadPhase::Running,
                RenderLoopState::Running,
                20,
                20,
                "pass",
            ),
            (
                RenderThreadPhase::Running,
                RenderLoopState::Running,
                5_000,
                20,
                "fail",
            ),
            (
                RenderThreadPhase::Running,
                RenderLoopState::Paused,
                60_000,
                40,
                "pass",
            ),
            (
                RenderThreadPhase::Running,
                RenderLoopState::Paused,
                60_000,
                4_000,
                "fail",
            ),
            (
                RenderThreadPhase::Restarting,
                RenderLoopState::Running,
                20,
                20,
                "fail",
            ),
            (
                RenderThreadPhase::Terminating,
                RenderLoopState::Running,
                20,
                20,
                "fail",
            ),
            (
                RenderThreadPhase::Stopped,
                RenderLoopState::Running,
                20,
                20,
                "fail",
            ),
            (
                RenderThreadPhase::Stopped,
                RenderLoopState::Stopped,
                9_000,
                9_000,
                "pass",
            ),
            (
                RenderThreadPhase::NotStarted,
                RenderLoopState::Created,
                0,
                0,
                "pass",
            ),
        ];
        for (phase, loop_state, frame_age, heartbeat_age, expected) in cases {
            let check = super::render_thread_check(
                &supervision(phase, frame_age, heartbeat_age),
                loop_state,
                stall,
            );
            assert_eq!(
                check.status, expected,
                "{phase:?} with the loop {loop_state} and a {frame_age} ms frame age: {}",
                check.detail
            );
            assert_eq!(check.name, "render_thread");
        }
    }

    #[test]
    fn a_paused_loop_passes_the_render_thread_check_and_names_its_pause() {
        use crate::output_power::{OutputOverride, OutputPowerState};
        use crate::render_thread::RenderThreadPhase;
        use hypercolor_core::engine::RenderLoopState;

        let paused_for_a_minute = supervision(RenderThreadPhase::Running, 60_000, 40);
        let check = super::render_thread_check(
            &paused_for_a_minute,
            RenderLoopState::Paused,
            std::time::Duration::from_secs(3),
        );
        assert_eq!(check.status, "pass", "{}", check.detail);
        assert!(
            check.detail.contains("no frames are due"),
            "{}",
            check.detail
        );

        let manual = OutputPowerState {
            output_override: OutputOverride::Paused,
            ..OutputPowerState::default()
        };
        assert!(
            super::render_loop_pause_reason(RenderLoopState::Paused, manual)
                .is_some_and(|reason| reason.contains("manual pause")
                    && reason.contains("persists across restarts"))
        );
        let stopped = OutputPowerState {
            output_override: OutputOverride::Stopped,
            ..OutputPowerState::default()
        };
        assert!(
            super::render_loop_pause_reason(RenderLoopState::Paused, stopped)
                .is_some_and(|reason| reason.contains("last effect was stopped"))
        );
        assert_eq!(
            super::render_loop_pause_reason(RenderLoopState::Running, manual),
            None
        );
    }

    #[test]
    fn gpu_errors_check_warns_on_recent_errors_and_fails_on_device_loss() {
        use crate::render_thread::gpu_errors::{GpuErrorSample, GpuErrorTotals};

        let now_ms = 50_000_000;
        assert_eq!(
            super::gpu_errors_check_at(&GpuErrorTotals::default(), now_ms).status,
            "pass"
        );
        let validation = GpuErrorTotals {
            validation: 2,
            last: Some(GpuErrorSample {
                device: "SparkleFlinger GPU compositor".to_owned(),
                kind: "validation".to_owned(),
                message: "Usage flags are invalid".to_owned(),
                at_unix_ms: now_ms - 1_000,
            }),
            ..GpuErrorTotals::default()
        };
        let check = super::gpu_errors_check_at(&validation, now_ms);
        assert_eq!(check.status, "warning");
        let window_ms = u64::try_from(super::GPU_ERROR_WARNING_WINDOW.as_millis())
            .expect("the window fits in milliseconds");
        let settled = super::gpu_errors_check_at(&validation, now_ms + window_ms);
        assert_eq!(settled.status, "pass");
        assert!(
            settled.detail.contains("validation=2"),
            "{}",
            settled.detail
        );
        assert!(check.detail.contains("validation=2"), "{}", check.detail);
        assert!(
            check.detail.contains("Usage flags are invalid"),
            "{}",
            check.detail
        );
        let lost = GpuErrorTotals {
            device_lost: 1,
            ..validation
        };
        assert_eq!(super::gpu_errors_check_at(&lost, now_ms).status, "fail");
    }

    #[test]
    fn multi_zone_sampling_passes_when_every_zone_samples_on_the_gpu() {
        let (status, detail) = render_multi_zone_sampling_status(Some(&LatestFrameMetrics {
            compositor_backend: CompositorBackendKind::Gpu,
            gpu_sampled_zone_count: 2,
            gpu_sampling_passes: 3,
            zone_composition_skips: 1,
            ..LatestFrameMetrics::default()
        }));

        assert_eq!(status, "pass");
        assert_eq!(
            detail,
            "gpu_sampled_zones=2, cpu_sampled_zones=0, held_zones=0, gpu_sampling_passes=3, zone_composition_skips=1"
        );
    }

    #[test]
    fn multi_zone_sampling_warns_with_the_held_zone_count() {
        let (status, detail) = render_multi_zone_sampling_status(Some(&LatestFrameMetrics {
            compositor_backend: CompositorBackendKind::Gpu,
            gpu_sampled_zone_count: 1,
            cpu_sampled_zone_count: 1,
            held_zone_count: 1,
            gpu_sampling_passes: 1,
            ..LatestFrameMetrics::default()
        }));

        assert_eq!(status, "warning");
        assert!(
            detail.starts_with("1 of 3 zone(s) had no frame to sample"),
            "{detail}"
        );
        assert!(detail.contains("held_zones=1"), "{detail}");
    }

    #[test]
    fn multi_zone_sampling_is_not_applicable_without_a_zone_set() {
        for backend in [CompositorBackendKind::Cpu, CompositorBackendKind::Gpu] {
            let (status, detail) = render_multi_zone_sampling_status(Some(&LatestFrameMetrics {
                compositor_backend: backend,
                gpu_zone_sampling: true,
                ..LatestFrameMetrics::default()
            }));

            assert_eq!(status, "pass");
            assert_eq!(
                detail,
                format!(
                    "not applicable: the latest frame sampled no per-zone set, compositor_backend={}",
                    backend.as_str()
                )
            );
        }
    }

    #[test]
    fn multi_zone_sampling_warns_without_a_frame() {
        let (status, detail) = render_multi_zone_sampling_status(None);

        assert_eq!(status, "warning");
        assert_eq!(detail, "no completed frame recorded");
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

    #[test]
    fn competing_software_check_names_each_program_and_its_remedy() {
        let status = SoftwareConflictsStatus {
            supported: true,
            scanned: true,
            scan_failed: false,
            conflicts: vec![SoftwareConflict {
                id: "signalrgb".to_owned(),
                name: "SignalRGB".to_owned(),
                matched: vec!["SignalRgb.exe".to_owned()],
                driver_ids: Vec::new(),
                all_drivers: true,
                smbus: true,
                remedy: "Quit SignalRGB from its tray icon.".to_owned(),
            }],
        };
        let check = conflicts_check(&status);
        assert_eq!(check.category, "devices");
        assert_eq!(check.name, "competing_software");
        assert_eq!(check.status, "warning");
        assert_eq!(
            check.detail,
            "SignalRGB (SignalRgb.exe): Quit SignalRGB from its tray icon."
        );
    }

    #[test]
    fn competing_software_check_passes_when_nothing_or_nothing_known_runs() {
        let clear = conflicts_check(&SoftwareConflictsStatus {
            supported: true,
            scanned: true,
            scan_failed: false,
            conflicts: Vec::new(),
        });
        assert_eq!(clear.status, "pass");
        assert_eq!(clear.detail, "no competing RGB software is running");

        let unsupported = conflicts_check(&SoftwareConflictsStatus::default());
        assert_eq!(unsupported.status, "pass");
        assert!(unsupported.detail.contains("not inspected"));
    }

    #[test]
    fn competing_software_check_warns_when_the_scan_failed() {
        let blind = conflicts_check(&SoftwareConflictsStatus {
            supported: true,
            scanned: true,
            scan_failed: true,
            conflicts: Vec::new(),
        });
        assert_eq!(blind.status, "warning");
        assert!(blind.detail.contains("could not list running software"));

        let stale = conflicts_check(&SoftwareConflictsStatus {
            supported: true,
            scanned: true,
            scan_failed: true,
            conflicts: vec![SoftwareConflict {
                id: "signalrgb".to_owned(),
                name: "SignalRGB".to_owned(),
                matched: vec!["SignalRgb.exe".to_owned()],
                driver_ids: Vec::new(),
                all_drivers: true,
                smbus: true,
                remedy: "Quit it.".to_owned(),
            }],
        });
        assert_eq!(stale.status, "warning");
        assert!(
            stale
                .detail
                .contains("last successful scan found SignalRGB")
        );
    }
}
