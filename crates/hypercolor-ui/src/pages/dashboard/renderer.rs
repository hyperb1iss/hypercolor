//! Renderer and hardware diagnostics panel for the dashboard.

use hypercolor_types::sensor::SystemSnapshot;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, SystemStatus};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;
use crate::ws::PerformanceMetrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticTone {
    Good,
    Warn,
    Bad,
    Neutral,
}

impl DiagnosticTone {
    const fn chip_classes(self) -> &'static str {
        match self {
            Self::Good => "border-success-green/30 bg-success-green/10 text-success-green",
            Self::Warn => "border-electric-yellow/30 bg-electric-yellow/10 text-electric-yellow",
            Self::Bad => "border-error-red/30 bg-error-red/10 text-error-red",
            Self::Neutral => "border-edge-subtle bg-surface-sunken/55 text-fg-secondary",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiagnosticBadge {
    label: String,
    tone: DiagnosticTone,
}

#[derive(Clone, Copy)]
struct ServoImportBadgeSnapshot {
    import_attempting: bool,
    gpu_frames: u64,
    cpu_frames: u64,
    import_failures: u64,
    import_fallbacks: u64,
    has_fallback_reason: bool,
}

/// Live renderer, GPU import, host hardware, and output-lane diagnostics.
#[component]
pub(super) fn RendererHardwarePanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
) -> impl IntoView {
    let status_resource = LocalResource::new(api::fetch_status);
    let sensors_resource = LocalResource::new(api::fetch_system_sensors);
    let status = Signal::derive(move || status_resource.get().and_then(Result::ok));
    let sensors = Signal::derive(move || sensors_resource.get().and_then(Result::ok));

    let compositor_badge = Memo::new(move |_| {
        let metric_snapshot = metrics.get();
        let status_snapshot = status.get();
        compositor_badge(metric_snapshot.as_ref(), status_snapshot.as_ref())
    });
    let import_badge = Memo::new(move |_| {
        let metric_snapshot = metrics.get();
        let status_snapshot = status.get();
        servo_import_badge(metric_snapshot.as_ref(), status_snapshot.as_ref())
    });
    let readback_badge = Memo::new(move |_| {
        let metric_snapshot = metrics.get();
        readback_badge(metric_snapshot.as_ref())
    });

    view! {
        <div class="overflow-hidden rounded-lg border border-edge-subtle/70 bg-surface-overlay/40">
            <div class="flex flex-col gap-3 border-b border-edge-subtle/55 px-4 py-3 lg:flex-row lg:items-center lg:justify-between">
                <div class="flex min-w-0 items-center gap-2">
                    <Icon icon=LuCpu width="14px" height="14px" style="color: var(--color-neon-cyan)" />
                    <h2 class="min-w-0 break-words text-[13px] font-medium text-fg-secondary">
                        "Renderer & Hardware"
                    </h2>
                    <span class="min-w-0 break-words text-[9px] font-mono uppercase tracking-[0.08em] text-fg-tertiary/55 sm:tracking-[0.12em]">
                        "live diagnostics"
                    </span>
                </div>
                <div class="flex flex-wrap items-center gap-2">
                    <HealthPill
                        label="Compositor"
                        badge=Signal::derive(move || compositor_badge.get())
                    />
                    <HealthPill
                        label="Servo Import"
                        badge=Signal::derive(move || import_badge.get())
                    />
                    <HealthPill
                        label="Readback"
                        badge=Signal::derive(move || readback_badge.get())
                    />
                </div>
            </div>

            <div class="grid grid-cols-1 divide-y divide-edge-subtle/45 lg:grid-cols-4 lg:divide-x lg:divide-y-0">
                <DiagnosticSection title="Compositor" icon=LuGauge>
                    <DetailRow label="Requested" value=Signal::derive(move || {
                        status_text(status.get(), |s| mode_label(&s.compositor_acceleration.requested_mode))
                    }) />
                    <DetailRow label="Effective" value=Signal::derive(move || {
                        status_text(status.get(), |s| mode_label(&s.compositor_acceleration.effective_mode))
                    }) />
                    <DetailRow label="Frame backend" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| mode_label(&m.timeline.compositor_backend))
                    }) />
                    <DetailRow label="Adapter" value=Signal::derive(move || {
                        probe_text(status.get(), |probe| non_empty(&probe.adapter_name))
                    }) />
                    <DetailRow label="WGPU backend" value=Signal::derive(move || {
                        probe_text(status.get(), |probe| non_empty(&probe.backend))
                    }) />
                    <DetailRow label="Texture" value=Signal::derive(move || {
                        probe_text(status.get(), |probe| non_empty(&probe.texture_format))
                    }) />
                    <DetailRow label="Max 2D texture" value=Signal::derive(move || {
                        probe_text(status.get(), |probe| {
                            if probe.max_texture_dimension_2d == 0 {
                                "n/a".to_owned()
                            } else {
                                format!("{} px", probe.max_texture_dimension_2d)
                            }
                        })
                    }) />
                    <DetailRow label="Storage textures" value=Signal::derive(move || {
                        probe_text(status.get(), |probe| {
                            if probe.max_storage_textures_per_shader_stage == 0 {
                                "n/a".to_owned()
                            } else {
                                probe.max_storage_textures_per_shader_stage.to_string()
                            }
                        })
                    }) />
                    <DetailRow label="Import policy" value=Signal::derive(move || {
                        status_text(status.get(), |s| mode_label(&s.compositor_acceleration.servo_gpu_import_mode))
                    }) />
                    <DetailRow label="Linux import backend" value=Signal::derive(move || {
                        probe_text(status.get(), |probe| {
                            if probe.linux_servo_gpu_import_backend_compatible {
                                "compatible".to_owned()
                            } else {
                                probe
                                    .linux_servo_gpu_import_backend_reason
                                    .clone()
                                    .unwrap_or_else(|| "not compatible".to_owned())
                            }
                        })
                    }) />
                </DiagnosticSection>

                <DiagnosticSection title="GPU Import" icon=LuZap>
                    <DetailRow label="Servo frames" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} GPU / {} CPU / {} cached",
                                m.effect_health.servo_render_gpu_frames_total,
                                m.effect_health.servo_render_cpu_frames_total,
                                m.effect_health.servo_render_cached_frames_total,
                            )
                        })
                    }) />
                    <DetailRow label="Producer frames" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} GPU / {} CPU",
                                m.effect_health.producer_gpu_frames_total,
                                m.effect_health.producer_cpu_frames_total,
                            )
                        })
                    }) />
                    <DetailRow label="Import failures" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} fail / {} fallback",
                                m.effect_health.servo_gpu_import_failures_total,
                                m.effect_health.servo_gpu_import_fallbacks_total,
                            )
                        })
                    }) />
                    <DetailRow label="Fallback reason" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            m.effect_health
                                .servo_gpu_import_fallback_reason
                                .clone()
                                .unwrap_or_else(|| "none".to_owned())
                        })
                    }) />
                    <DetailRow label="Import max" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| fmt_ms(m.effect_health.servo_gpu_import_max_ms))
                    }) />
                    <DetailRow label="Blit / sync max" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} / {}",
                                fmt_ms(m.effect_health.servo_gpu_import_blit_max_ms),
                                fmt_ms(m.effect_health.servo_gpu_import_sync_max_ms),
                            )
                        })
                    }) />
                    <DetailRow label="Readback max" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| fmt_ms(m.effect_health.servo_render_readback_max_ms))
                    }) />
                    <DetailRow label="GPU sample window" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| format_window_count(m.pacing.gpu_zone_sampling))
                    }) />
                    <DetailRow label="CPU fallback window" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| format_window_count(m.pacing.gpu_sample_cpu_fallback))
                    }) />
                    <DetailRow label="Readback fail window" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| format_window_count(m.pacing.gpu_readback_failed_frames))
                    }) />
                </DiagnosticSection>

                <DiagnosticSection title="Composition" icon=LuLayers>
                    <DetailRow label="Frame avg / p95" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!("{} / {}", fmt_ms(m.frame_time.avg_ms), fmt_ms(m.frame_time.p95_ms))
                        })
                    }) />
                    <DetailRow label="Budget / max" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!("{} / {}", fmt_ms(m.timeline.budget_ms), fmt_ms(m.frame_time.max_ms))
                        })
                    }) />
                    <DetailRow label="Composition" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| fmt_ms(m.stages.composition_ms))
                    }) />
                    <DetailRow label="Scene compose" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| fmt_ms(m.stages.producer_scene_compose_ms))
                    }) />
                    <DetailRow label="Producer render" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| fmt_ms(m.stages.producer_effect_rendering_ms))
                    }) />
                    <DetailRow label="Sample / output" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} / {}",
                                fmt_ms(m.stages.spatial_sampling_ms),
                                fmt_ms(m.stages.device_output_ms),
                            )
                        })
                    }) />
                    <DetailRow label="Publish split" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} fd / {} canvas / {} preview / {} evt",
                                fmt_ms(m.stages.publish_frame_data_ms),
                                fmt_ms(m.stages.publish_group_canvas_ms),
                                fmt_ms(m.stages.publish_preview_ms),
                                fmt_ms(m.stages.publish_events_ms),
                            )
                        })
                    }) />
                    <DetailRow label="Bypass / forced window" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} / {}",
                                format_window_count(m.pacing.composition_bypassed),
                                format_window_count(m.pacing.scene_canvas_forced_surface),
                            )
                        })
                    }) />
                    <DetailRow label="Full-frame copies" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} · {}",
                                m.copies.full_frame_count,
                                fmt_kib(m.copies.full_frame_kb),
                            )
                        })
                    }) />
                    <DetailRow label="Surface pool" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} slots / {} free / {} shared",
                                m.render_surfaces.scene_pool_slot_count,
                                m.render_surfaces.free_slots,
                                m.render_surfaces.scene_pool_shared_published_slots,
                            )
                        })
                    }) />
                    <DetailRow label="Pool saturation" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} scene / {} direct",
                                m.render_surfaces.scene_pool_saturation_reallocs,
                                m.render_surfaces.direct_pool_saturation_reallocs,
                            )
                        })
                    }) />
                </DiagnosticSection>

                <DiagnosticSection title="Host & Output" icon=LuMonitor>
                    <DetailRow label="CPU load / temp" value=Signal::derive(move || {
                        sensors_text(sensors.get(), |snapshot| {
                            format!(
                                "{} / {}",
                                fmt_percent_f32(snapshot.cpu_load_percent),
                                fmt_optional_temp(snapshot.cpu_temp_celsius),
                            )
                        })
                    }) />
                    <DetailRow label="GPU load / temp" value=Signal::derive(move || {
                        sensors_text(sensors.get(), |snapshot| {
                            format!(
                                "{} / {}",
                                fmt_optional_percent(snapshot.gpu_load_percent),
                                fmt_optional_temp(snapshot.gpu_temp_celsius),
                            )
                        })
                    }) />
                    <DetailRow label="GPU VRAM" value=Signal::derive(move || {
                        sensors_text(sensors.get(), |snapshot| {
                            snapshot
                                .gpu_vram_used_mb
                                .map_or_else(|| "n/a".to_owned(), |value| format!("{value:.0} MB"))
                        })
                    }) />
                    <DetailRow label="RAM used" value=Signal::derive(move || {
                        sensors_text(sensors.get(), |snapshot| {
                            format!(
                                "{} · {:.0}/{:.0} MB",
                                fmt_percent_f32(snapshot.ram_used_percent),
                                snapshot.ram_used_mb,
                                snapshot.ram_total_mb,
                            )
                        })
                    }) />
                    <DetailRow label="Devices / LEDs" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!("{} / {}", m.devices.connected, m.devices.total_leds)
                        })
                    }) />
                    <DetailRow label="Output errors" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} frame / {} write",
                                m.devices.output_errors,
                                m.display_output.write_failures_total,
                            )
                        })
                    }) />
                    <DetailRow label="Retries / attempts" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} / {}",
                                m.display_output.retry_attempts_total,
                                m.display_output.write_attempts_total,
                            )
                        })
                    }) />
                    <DetailRow label="Display lane" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} delayed / {} total",
                                m.display_output.display_lane.display_frames_delayed_for_led_total,
                                m.display_output.display_lane.display_frames_total,
                            )
                        })
                    }) />
                    <DetailRow label="LED priority wait" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            fmt_ms(m.display_output.display_lane.display_led_priority_wait_max_ms)
                        })
                    }) />
                    <DetailRow label="Captured displays" value=Signal::derive(move || {
                        metrics_text(metrics.get(), |m| {
                            format!(
                                "{} devices / {} subscribers",
                                m.display_output.captured_devices,
                                m.display_output.preview_subscribers,
                            )
                        })
                    }) />
                </DiagnosticSection>
            </div>
        </div>
    }
}

#[component]
fn HealthPill(label: &'static str, #[prop(into)] badge: Signal<DiagnosticBadge>) -> impl IntoView {
    view! {
        <div class=move || {
            format!(
                "inline-flex max-w-full flex-wrap items-center gap-1 rounded-full border px-2 py-1 text-[10px] font-mono uppercase tracking-[0.08em] sm:tracking-[0.12em] {}",
                badge.get().tone.chip_classes(),
            )
        }>
            <span class="min-w-0 break-words text-fg-tertiary/75">{label}</span>
            <span class="min-w-0 break-words font-semibold">{move || badge.get().label}</span>
        </div>
    }
}

#[component]
fn DiagnosticSection(
    title: &'static str,
    icon: icondata_core::Icon,
    children: Children,
) -> impl IntoView {
    view! {
        <section class="min-w-0 px-4 py-3">
            <div class="mb-3 flex min-w-0 items-center gap-2">
                <Icon icon=icon width="13px" height="13px" style="color: var(--color-fg-tertiary)" />
                <span class=format!("min-w-0 break-words {}", label_class(LabelSize::Small, LabelTone::Default))>
                    {title}
                </span>
            </div>
            <div class="space-y-2">
                {children()}
            </div>
        </section>
    }
}

#[component]
fn DetailRow(label: &'static str, #[prop(into)] value: Signal<String>) -> impl IntoView {
    view! {
        <div class="grid min-w-0 grid-cols-1 items-start gap-0.5 sm:grid-cols-[minmax(0,0.92fr)_minmax(0,1.28fr)] sm:gap-3">
            <span class="min-w-0 break-words text-[10px] font-mono uppercase tracking-normal text-fg-tertiary/65 sm:tracking-[0.11em]">
                {label}
            </span>
            <span class="min-w-0 break-words text-left text-[11px] font-mono tabular-nums leading-snug text-fg-secondary sm:text-right">
                {move || value.get()}
            </span>
        </div>
    }
}

fn compositor_badge(
    metrics: Option<&PerformanceMetrics>,
    status: Option<&SystemStatus>,
) -> DiagnosticBadge {
    if let Some(metrics) = metrics {
        return match metrics.timeline.compositor_backend.as_str() {
            "gpu" => badge("gpu", DiagnosticTone::Good),
            "gpu_fallback" => badge("fallback", DiagnosticTone::Warn),
            "cpu" => badge("cpu", DiagnosticTone::Warn),
            other if other.is_empty() => badge("warming", DiagnosticTone::Neutral),
            other => badge(other, DiagnosticTone::Neutral),
        };
    }

    match status.map(|s| s.compositor_acceleration.effective_mode.as_str()) {
        Some("gpu") => badge("gpu", DiagnosticTone::Good),
        Some("cpu") => badge("cpu", DiagnosticTone::Warn),
        Some(mode) if !mode.is_empty() => badge(mode, DiagnosticTone::Neutral),
        _ => badge("loading", DiagnosticTone::Neutral),
    }
}

fn servo_import_badge(
    metrics: Option<&PerformanceMetrics>,
    status: Option<&SystemStatus>,
) -> DiagnosticBadge {
    if status.is_some_and(|s| s.compositor_acceleration.servo_gpu_import_mode == "off") {
        return badge("off", DiagnosticTone::Neutral);
    }

    let Some(metrics) = metrics else {
        return if status.is_some_and(|s| s.compositor_acceleration.servo_gpu_import_attempting) {
            badge("arming", DiagnosticTone::Neutral)
        } else {
            badge("loading", DiagnosticTone::Neutral)
        };
    };

    let health = &metrics.effect_health;
    classify_servo_import_badge(ServoImportBadgeSnapshot {
        import_attempting: status
            .is_some_and(|s| s.compositor_acceleration.servo_gpu_import_attempting),
        gpu_frames: health.servo_render_gpu_frames_total,
        cpu_frames: health.servo_render_cpu_frames_total,
        import_failures: health.servo_gpu_import_failures_total,
        import_fallbacks: health.servo_gpu_import_fallbacks_total,
        has_fallback_reason: health.servo_gpu_import_fallback_reason.is_some(),
    })
}

fn classify_servo_import_badge(snapshot: ServoImportBadgeSnapshot) -> DiagnosticBadge {
    if snapshot.import_attempting && snapshot.gpu_frames > 0 {
        return badge("active", DiagnosticTone::Good);
    }

    if snapshot.import_fallbacks > 0 || snapshot.has_fallback_reason {
        return badge("fallback", DiagnosticTone::Warn);
    }

    if snapshot.gpu_frames > 0 && snapshot.cpu_frames == 0 {
        return badge("active", DiagnosticTone::Good);
    }

    if snapshot.gpu_frames > 0 {
        return badge("mixed", DiagnosticTone::Warn);
    }

    if snapshot.cpu_frames > 0 {
        return badge("cpu", DiagnosticTone::Warn);
    }

    if snapshot.import_failures > 0 && snapshot.import_attempting {
        return badge("retrying", DiagnosticTone::Warn);
    }

    if snapshot.import_failures > 0 {
        return badge("failing", DiagnosticTone::Bad);
    }

    if snapshot.import_attempting {
        return badge("arming", DiagnosticTone::Neutral);
    }

    badge("idle", DiagnosticTone::Neutral)
}

fn readback_badge(metrics: Option<&PerformanceMetrics>) -> DiagnosticBadge {
    let Some(metrics) = metrics else {
        return badge("loading", DiagnosticTone::Neutral);
    };

    if metrics.timeline.gpu_readback_failed || metrics.pacing.gpu_readback_failed_frames > 0 {
        badge("failed", DiagnosticTone::Bad)
    } else if metrics.timeline.led_sampling_readback
        || metrics.timeline.cpu_sampling_late_readback
        || metrics.pacing.led_sampling_readback > 0
        || metrics.pacing.cpu_sampling_late_readback > 0
        || metrics.effect_health.servo_render_readback_max_ms > 0.0
    {
        badge("present", DiagnosticTone::Warn)
    } else {
        badge("clean", DiagnosticTone::Good)
    }
}

fn badge(label: impl Into<String>, tone: DiagnosticTone) -> DiagnosticBadge {
    DiagnosticBadge {
        label: label.into(),
        tone,
    }
}

fn status_text(status: Option<SystemStatus>, f: impl FnOnce(&SystemStatus) -> String) -> String {
    status.as_ref().map_or_else(|| "loading".to_owned(), f)
}

fn probe_text(
    status: Option<SystemStatus>,
    f: impl FnOnce(&api::GpuCompositorProbeStatus) -> String,
) -> String {
    status
        .as_ref()
        .and_then(|status| status.compositor_acceleration.gpu_probe.as_ref())
        .map_or_else(|| "n/a".to_owned(), f)
}

fn sensors_text(
    sensors: Option<SystemSnapshot>,
    f: impl FnOnce(&SystemSnapshot) -> String,
) -> String {
    sensors.as_ref().map_or_else(|| "loading".to_owned(), f)
}

fn metrics_text(
    metrics: Option<PerformanceMetrics>,
    f: impl FnOnce(&PerformanceMetrics) -> String,
) -> String {
    metrics.as_ref().map_or_else(|| "waiting".to_owned(), f)
}

fn non_empty(value: &str) -> String {
    if value.is_empty() {
        "n/a".to_owned()
    } else {
        value.to_owned()
    }
}

fn mode_label(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "n/a".to_owned();
    }

    value.replace('_', " ")
}

fn fmt_ms(value: f64) -> String {
    format!("{value:.2} ms")
}

fn fmt_kib(value: f64) -> String {
    format!("{value:.1} KiB")
}

fn fmt_percent_f32(value: f32) -> String {
    format!("{value:.0}%")
}

fn fmt_optional_percent(value: Option<f32>) -> String {
    value.map_or_else(|| "n/a".to_owned(), fmt_percent_f32)
}

fn fmt_optional_temp(value: Option<f32>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| format!("{value:.0} C"))
}

fn format_window_count(value: u32) -> String {
    format!("{value}/120 frames")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_badge(badge: DiagnosticBadge, expected_label: &str, expected_tone: DiagnosticTone) {
        assert_eq!(badge.label, expected_label);
        assert_eq!(badge.tone, expected_tone);
    }

    #[test]
    fn servo_import_badge_stays_active_after_recovered_import_failure() {
        assert_badge(
            classify_servo_import_badge(ServoImportBadgeSnapshot {
                import_attempting: true,
                gpu_frames: 42,
                cpu_frames: 0,
                import_failures: 2,
                import_fallbacks: 0,
                has_fallback_reason: false,
            }),
            "active",
            DiagnosticTone::Good,
        );
    }

    #[test]
    fn servo_import_badge_stays_active_after_recovered_fallback() {
        assert_badge(
            classify_servo_import_badge(ServoImportBadgeSnapshot {
                import_attempting: true,
                gpu_frames: 42,
                cpu_frames: 3,
                import_failures: 2,
                import_fallbacks: 1,
                has_fallback_reason: true,
            }),
            "active",
            DiagnosticTone::Good,
        );
    }

    #[test]
    fn servo_import_badge_reports_retry_before_first_gpu_frame() {
        assert_badge(
            classify_servo_import_badge(ServoImportBadgeSnapshot {
                import_attempting: true,
                gpu_frames: 0,
                cpu_frames: 0,
                import_failures: 1,
                import_fallbacks: 0,
                has_fallback_reason: false,
            }),
            "retrying",
            DiagnosticTone::Warn,
        );
    }

    #[test]
    fn servo_import_badge_reports_failing_when_import_stops_without_frames() {
        assert_badge(
            classify_servo_import_badge(ServoImportBadgeSnapshot {
                import_attempting: false,
                gpu_frames: 0,
                cpu_frames: 0,
                import_failures: 1,
                import_fallbacks: 0,
                has_fallback_reason: false,
            }),
            "failing",
            DiagnosticTone::Bad,
        );
    }
}
