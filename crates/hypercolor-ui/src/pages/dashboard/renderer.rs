//! Renderer and hardware diagnostics panel for the dashboard.

use hypercolor_types::sensor::SystemSnapshot;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{self, SystemStatus};
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::icons::*;
use crate::storage;
use crate::ws::PerformanceMetrics;

/// `localStorage` key holding whether the deep-diagnostics grid is expanded.
const DIAGNOSTICS_OPEN_KEY: &str = "hc-dash-renderer-diagnostics-open";

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

    /// Value color for a tinted detail row. Neutral stays at the regular
    /// secondary foreground so only meaningful rows carry color.
    const fn value_color(self) -> &'static str {
        match self {
            Self::Good => "var(--color-success-green)",
            Self::Warn => "var(--color-electric-yellow)",
            Self::Bad => "var(--color-error-red)",
            Self::Neutral => "var(--color-fg-secondary)",
        }
    }

    /// Accent for a live vitals tile. Neutral reads as cyan "live data"
    /// rather than muted text, so a calm tile still looks instrumented.
    const fn data_color(self) -> &'static str {
        match self {
            Self::Good => "var(--color-success-green)",
            Self::Warn => "var(--color-electric-yellow)",
            Self::Bad => "var(--color-error-red)",
            Self::Neutral => "var(--color-neon-cyan)",
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
///
/// The panel leads with an always-visible strip of hardware vitals — the
/// numbers worth a glance every second — and tucks the full counter grid
/// behind a collapsible "Diagnostics" section so the dense data no longer
/// reads as a wall of text. Disclosure state persists in `localStorage`
/// and defaults closed; while collapsed the grid's rows are unmounted, so
/// the panel also stops doing per-frame work it isn't showing.
#[component]
pub(super) fn RendererHardwarePanel(
    #[prop(into)] metrics: Signal<Option<PerformanceMetrics>>,
    #[prop(into)] sensors: Signal<Option<SystemSnapshot>>,
) -> impl IntoView {
    let status_resource = LocalResource::new(api::fetch_status);
    let status = Memo::new(move |_| status_resource.get().and_then(Result::ok));

    let compositor_badge = Memo::new(move |_| {
        metrics.with(|metric_snapshot| {
            status.with(|status_snapshot| {
                compositor_badge(metric_snapshot.as_ref(), status_snapshot.as_ref())
            })
        })
    });
    let import_badge = Memo::new(move |_| {
        metrics.with(|metric_snapshot| {
            status.with(|status_snapshot| {
                servo_import_badge(metric_snapshot.as_ref(), status_snapshot.as_ref())
            })
        })
    });
    let readback_badge = Memo::new(move |_| {
        metrics.with(|metric_snapshot| readback_badge(metric_snapshot.as_ref()))
    });
    let has_windows_import_metrics = Memo::new(move |_| {
        metrics.with(|metrics| {
            metrics.as_ref().is_some_and(|metrics| {
                metrics
                    .effect_health
                    .servo_gpu_import_windows_sync_mode
                    .is_some()
            })
        })
    });

    let diagnostics_open =
        RwSignal::new(storage::get(DIAGNOSTICS_OPEN_KEY).is_some_and(|value| value == "1"));
    let toggle_diagnostics = move |_| {
        let next = !diagnostics_open.get_untracked();
        diagnostics_open.set(next);
        storage::set(DIAGNOSTICS_OPEN_KEY, if next { "1" } else { "0" });
    };

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
                        badge=compositor_badge
                    />
                    <HealthPill
                        label="Servo Import"
                        badge=import_badge
                    />
                    <HealthPill
                        label="Readback"
                        badge=readback_badge
                    />
                </div>
            </div>

            // ── Vitals strip — host hardware vs hypercolor, always visible ──
            // Split into two labeled clusters so a system-wide reading (host
            // RAM, GPU VRAM) can never be misread as hypercolor's own usage.
            <div class="flex flex-col divide-y divide-edge-subtle/55 lg:flex-row lg:divide-x lg:divide-y-0">
                <div class="min-w-0 lg:flex-[4]">
                    <VitalGroupLabel label="Host" hint="this machine" />
                    <div class="grid grid-cols-2 gap-px bg-edge-subtle/40 sm:grid-cols-4">
                        <VitalTile
                            label="CPU"
                            value=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().map_or_else(|| "—".to_owned(), |s| fmt_percent_f32(s.cpu_load_percent)))
                            })
                            sub=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().map_or_else(|| "temp n/a".to_owned(), |s| fmt_temp_compact(s.cpu_temp_celsius)))
                            })
                            accent=Memo::new(move |_| {
                                temp_tone(sensors.with(|s| s.as_ref().and_then(|s| s.cpu_temp_celsius))).data_color()
                            })
                            fill=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().map_or(0.0, |s| f64::from(s.cpu_load_percent) / 100.0))
                            })
                        />
                        <VitalTile
                            label="GPU"
                            value=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().and_then(|s| s.gpu_load_percent).map_or_else(|| "—".to_owned(), fmt_percent_f32))
                            })
                            sub=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().map_or_else(|| "temp n/a".to_owned(), |s| fmt_temp_compact(s.gpu_temp_celsius)))
                            })
                            accent=Memo::new(move |_| {
                                temp_tone(sensors.with(|s| s.as_ref().and_then(|s| s.gpu_temp_celsius))).data_color()
                            })
                            fill=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().and_then(|s| s.gpu_load_percent).map_or(0.0, |v| f64::from(v) / 100.0))
                            })
                        />
                        <VitalTile
                            label="VRAM"
                            value=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().and_then(|s| s.gpu_vram_used_mb).map_or_else(|| "—".to_owned(), |mb| fmt_mem_gb(f64::from(mb))))
                            })
                            sub=Signal::derive(|| "graphics card".to_owned())
                            accent=Signal::derive(|| "var(--color-neon-cyan)")
                            fill=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().and_then(|s| s.gpu_vram_used_mb).map_or(0.0, |mb| f64::from(mb) / 8192.0))
                            })
                        />
                        <VitalTile
                            label="RAM"
                            value=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().map_or_else(|| "—".to_owned(), |s| fmt_percent_f32(s.ram_used_percent)))
                            })
                            sub=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().map_or_else(
                                    || "system memory".to_owned(),
                                    |s| format!("{:.1}/{:.1} GB", s.ram_used_mb / 1024.0, s.ram_total_mb / 1024.0),
                                ))
                            })
                            accent=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().map_or(DiagnosticTone::Neutral, |s| ram_tone(s.ram_used_percent))).data_color()
                            })
                            fill=Memo::new(move |_| {
                                sensors.with(|s| s.as_ref().map_or(0.0, |s| f64::from(s.ram_used_percent) / 100.0))
                            })
                        />
                    </div>
                </div>
                <div class="min-w-0 lg:flex-[3]">
                    <VitalGroupLabel label="Hypercolor" hint="this app" />
                    <div class="grid grid-cols-2 gap-px bg-edge-subtle/40 sm:grid-cols-3">
                        <VitalTile
                            label="Render path"
                            value=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or_else(
                                    || "—".to_owned(),
                                    |m| render_path_label(
                                        m.effect_health.servo_render_gpu_frames_total,
                                        m.effect_health.servo_render_cpu_frames_total,
                                    ).to_owned(),
                                ))
                            })
                            sub=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or_else(
                                    || "waiting".to_owned(),
                                    |m| format!(
                                        "{} gpu · {} cpu",
                                        m.effect_health.servo_render_gpu_frames_total,
                                        m.effect_health.servo_render_cpu_frames_total,
                                    ),
                                ))
                            })
                            accent=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or(DiagnosticTone::Neutral, |m| render_path_tone(
                                    m.effect_health.servo_render_gpu_frames_total,
                                    m.effect_health.servo_render_cpu_frames_total,
                                ))).data_color()
                            })
                            fill=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or(0.0, |m| {
                                    let gpu = m.effect_health.servo_render_gpu_frames_total as f64;
                                    let cpu = m.effect_health.servo_render_cpu_frames_total as f64;
                                    let total = gpu + cpu;
                                    if total > 0.0 { gpu / total } else { 0.0 }
                                }))
                            })
                        />
                        <VitalTile
                            label="Memory"
                            value=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or_else(|| "—".to_owned(), |m| fmt_mem_gb(m.memory.daemon_rss_mb)))
                            })
                            sub=Signal::derive(|| "in use".to_owned())
                            accent=Signal::derive(|| "var(--color-neon-cyan)")
                            fill=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or(0.0, |m| m.memory.daemon_rss_mb / 1024.0))
                            })
                        />
                        <VitalTile
                            label="Output"
                            value=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or_else(
                                    || "—".to_owned(),
                                    |m| format!("{} err", output_error_count(m)),
                                ))
                            })
                            sub=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or_else(
                                    || "devices".to_owned(),
                                    |m| format!("{} dev · {} led", m.devices.connected, m.devices.total_leds),
                                ))
                            })
                            accent=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or(DiagnosticTone::Neutral, |m| error_count_tone(output_error_count(m)))).data_color()
                            })
                            // A clean output reads as a full, calm bar; pressure shrinks it.
                            fill=Memo::new(move |_| {
                                metrics.with(|m| m.as_ref().map_or(0.0, |m| if output_error_count(m) == 0 { 1.0 } else { 0.15 }))
                            })
                        />
                    </div>
                </div>
            </div>

            // ── Diagnostics — the full counter grid, collapsed by default ──
            <div class="border-t border-edge-subtle/55">
                <button
                    type="button"
                    class="flex w-full min-w-0 items-center gap-2 px-4 py-2.5 text-left transition-colors hover:bg-surface-overlay/40"
                    on:click=toggle_diagnostics
                >
                    <span
                        class="inline-flex transition-transform duration-200"
                        class=("rotate-90", move || diagnostics_open.get())
                    >
                        <Icon icon=LuChevronRight width="13px" height="13px" style="color: var(--color-fg-tertiary)" />
                    </span>
                    <span class=label_class(LabelSize::Small, LabelTone::Default)>"Diagnostics"</span>
                    <span class="min-w-0 flex-1 truncate text-[10px] font-mono lowercase tracking-[0.04em] text-fg-tertiary/45">
                        "compositor · gpu import · composition · output"
                    </span>
                    <span class="shrink-0 text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/55">
                        {move || if diagnostics_open.get() { "hide" } else { "show" }}
                    </span>
                </button>

                <Show when=move || diagnostics_open.get() fallback=|| ()>
                    <div class="grid grid-cols-1 divide-y divide-edge-subtle/45 border-t border-edge-subtle/45 lg:grid-cols-4 lg:divide-x lg:divide-y-0">
                        <DiagnosticSection title="Compositor" icon=LuGauge>
                            <DetailRow label="Requested" value=Memo::new(move |_| {
                                status_text(status, |s| mode_label(&s.compositor_acceleration.requested_mode))
                            }) />
                            <DetailRow label="Effective" value=Memo::new(move |_| {
                                status_text(status, |s| mode_label(&s.compositor_acceleration.effective_mode))
                            }) />
                            <DetailRow label="Frame backend" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| mode_label(&m.timeline.compositor_backend))
                            }) />
                            <DetailRow label="Adapter" value=Memo::new(move |_| {
                                probe_text(status, |probe| non_empty(&probe.adapter_name))
                            }) />
                            <DetailRow label="WGPU backend" value=Memo::new(move |_| {
                                probe_text(status, |probe| non_empty(&probe.backend))
                            }) />
                            <DetailRow label="Texture" value=Memo::new(move |_| {
                                probe_text(status, |probe| non_empty(&probe.texture_format))
                            }) />
                            <DetailRow label="Max 2D texture" value=Memo::new(move |_| {
                                probe_text(status, |probe| {
                                    if probe.max_texture_dimension_2d == 0 {
                                        "n/a".to_owned()
                                    } else {
                                        format!("{} px", probe.max_texture_dimension_2d)
                                    }
                                })
                            }) />
                            <DetailRow label="Storage textures" value=Memo::new(move |_| {
                                probe_text(status, |probe| {
                                    if probe.max_storage_textures_per_shader_stage == 0 {
                                        "n/a".to_owned()
                                    } else {
                                        probe.max_storage_textures_per_shader_stage.to_string()
                                    }
                                })
                            }) />
                            <DetailRow label="Import policy" value=Memo::new(move |_| {
                                status_text(status, |s| mode_label(&s.compositor_acceleration.servo_gpu_import_mode))
                            }) />
                            <DetailRow label="Servo import backend" value=Memo::new(move |_| {
                                probe_text(status, |probe| {
                                    let compatible = probe.servo_gpu_import_backend_compatible
                                        || probe.linux_servo_gpu_import_backend_compatible;
                                    if compatible {
                                        "compatible".to_owned()
                                    } else {
                                        probe
                                            .servo_gpu_import_backend_reason
                                            .clone()
                                            .or_else(|| {
                                                probe
                                                    .linux_servo_gpu_import_backend_reason
                                                    .clone()
                                            })
                                            .unwrap_or_else(|| "not compatible".to_owned())
                                    }
                                })
                            }) />
                        </DiagnosticSection>

                        <DiagnosticSection title="GPU Import" icon=LuZap>
                            <DetailRow label="Servo frames" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!(
                                        "{} GPU / {} CPU / {} cached",
                                        m.effect_health.servo_render_gpu_frames_total,
                                        m.effect_health.servo_render_cpu_frames_total,
                                        m.effect_health.servo_render_cached_frames_total,
                                    )
                                })
                            }) />
                            <DetailRow label="Producer frames" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!(
                                        "{} GPU / {} CPU",
                                        m.effect_health.producer_gpu_frames_total,
                                        m.effect_health.producer_cpu_frames_total,
                                    )
                                })
                            }) />
                            <DetailRow
                                label="Import failures"
                                value=Memo::new(move |_| {
                                    metrics_text(metrics, |m| {
                                        format!(
                                            "{} fail / {} fallback",
                                            m.effect_health.servo_gpu_import_failures_total,
                                            m.effect_health.servo_gpu_import_fallbacks_total,
                                        )
                                    })
                                })
                                tone=Memo::new(move |_| {
                                    metrics.with(|m| m.as_ref().map_or(DiagnosticTone::Neutral, |m| error_count_tone(
                                        m.effect_health.servo_gpu_import_failures_total
                                            + m.effect_health.servo_gpu_import_fallbacks_total,
                                    )))
                                })
                            />
                            <DetailRow
                                label="Fallback reason"
                                value=Memo::new(move |_| {
                                    metrics_text(metrics, |m| {
                                        m.effect_health
                                            .servo_gpu_import_fallback_reason
                                            .clone()
                                            .unwrap_or_else(|| "none".to_owned())
                                    })
                                })
                                tone=Memo::new(move |_| {
                                    flag_tone(metrics.with(|m| m.as_ref().is_some_and(|m| {
                                        m.effect_health.servo_gpu_import_fallback_reason.is_some()
                                    })))
                                })
                            />
                            <Show when=move || has_windows_import_metrics.get() fallback=|| ()>
                                <DetailRow label="Sync mode" value=Memo::new(move |_| {
                                    metrics_text(metrics, |m| {
                                        m.effect_health
                                            .servo_gpu_import_windows_sync_mode
                                            .clone()
                                            .unwrap_or_else(|| "n/a".to_owned())
                                    })
                                }) />
                                <DetailRow label="Stale / adapter" value=Memo::new(move |_| {
                                    metrics_text(metrics, |m| {
                                        format!(
                                            "{} / {}",
                                            m.effect_health.servo_gpu_import_stale_frame_total,
                                            m.effect_health.servo_gpu_import_adapter_mismatch_total,
                                        )
                                    })
                                }) />
                            </Show>
                            <DetailRow label="Import max" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| fmt_ms(m.effect_health.servo_gpu_import_max_ms))
                            }) />
                            <DetailRow label="Blit / sync max" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!(
                                        "{} / {}",
                                        fmt_ms(m.effect_health.servo_gpu_import_blit_max_ms),
                                        fmt_ms(m.effect_health.servo_gpu_import_sync_max_ms),
                                    )
                                })
                            }) />
                            <DetailRow label="Readback max" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| fmt_ms(m.effect_health.servo_render_readback_max_ms))
                            }) />
                            <DetailRow label="GPU sample window" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| format_window_count(m.pacing.gpu_zone_sampling))
                            }) />
                            <DetailRow label="CPU fallback window" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| format_window_count(m.pacing.gpu_sample_cpu_fallback))
                            }) />
                            <DetailRow
                                label="Readback fail window"
                                value=Memo::new(move |_| {
                                    metrics_text(metrics, |m| format_window_count(m.pacing.gpu_readback_failed_frames))
                                })
                                tone=Memo::new(move |_| {
                                    flag_tone(metrics.with(|m| m.as_ref().is_some_and(|m| m.pacing.gpu_readback_failed_frames > 0)))
                                })
                            />
                        </DiagnosticSection>

                        <DiagnosticSection title="Composition" icon=LuLayers>
                            <DetailRow label="Frame avg / p95" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!("{} / {}", fmt_ms(m.frame_time.avg_ms), fmt_ms(m.frame_time.p95_ms))
                                })
                            }) />
                            <DetailRow label="Budget / max" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!("{} / {}", fmt_ms(m.timeline.budget_ms), fmt_ms(m.frame_time.max_ms))
                                })
                            }) />
                            <DetailRow label="Composition" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| fmt_ms(m.stages.composition_ms))
                            }) />
                            <DetailRow label="Scene compose" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| fmt_ms(m.stages.producer_scene_compose_ms))
                            }) />
                            <DetailRow label="Producer render" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| fmt_ms(m.stages.producer_effect_rendering_ms))
                            }) />
                            <DetailRow label="Sample / output" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!(
                                        "{} / {}",
                                        fmt_ms(m.stages.spatial_sampling_ms),
                                        fmt_ms(m.stages.device_output_ms),
                                    )
                                })
                            }) />
                            <DetailRow label="Publish split" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!(
                                        "{} fd / {} canvas / {} preview / {} evt",
                                        fmt_ms(m.stages.publish_frame_data_ms),
                                        fmt_ms(m.stages.publish_group_canvas_ms),
                                        fmt_ms(m.stages.publish_preview_ms),
                                        fmt_ms(m.stages.publish_events_ms),
                                    )
                                })
                            }) />
                            <DetailRow label="Bypass / forced window" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!(
                                        "{} / {}",
                                        format_window_count(m.pacing.composition_bypassed),
                                        format_window_count(m.pacing.scene_canvas_forced_surface),
                                    )
                                })
                            }) />
                            <DetailRow label="Full-frame copies" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!(
                                        "{} · {}",
                                        m.copies.full_frame_count,
                                        fmt_kib(m.copies.full_frame_kb),
                                    )
                                })
                            }) />
                            <DetailRow label="Surface pool" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!(
                                        "{} slots / {} free / {} shared",
                                        m.render_surfaces.scene_pool_slot_count,
                                        m.render_surfaces.free_slots,
                                        m.render_surfaces.scene_pool_shared_published_slots,
                                    )
                                })
                            }) />
                            <DetailRow
                                label="Pool saturation"
                                value=Memo::new(move |_| {
                                    metrics_text(metrics, |m| {
                                        format!(
                                            "{} scene / {} direct",
                                            m.render_surfaces.scene_pool_saturation_reallocs,
                                            m.render_surfaces.direct_pool_saturation_reallocs,
                                        )
                                    })
                                })
                                tone=Memo::new(move |_| {
                                    flag_tone(metrics.with(|m| m.as_ref().is_some_and(|m| {
                                        m.render_surfaces.scene_pool_saturation_reallocs
                                            + m.render_surfaces.direct_pool_saturation_reallocs
                                            > 0
                                    })))
                                })
                            />
                        </DiagnosticSection>

                        <DiagnosticSection title="Host & Output" icon=LuMonitor>
                            <DetailRow
                                label="CPU load / temp"
                                value=Memo::new(move |_| {
                                    sensors_text(sensors, |snapshot| {
                                        format!(
                                            "{} / {}",
                                            fmt_percent_f32(snapshot.cpu_load_percent),
                                            fmt_optional_temp(snapshot.cpu_temp_celsius),
                                        )
                                    })
                                })
                                tone=Memo::new(move |_| {
                                    temp_tone(sensors.with(|s| s.as_ref().and_then(|s| s.cpu_temp_celsius)))
                                })
                            />
                            <DetailRow
                                label="GPU load / temp"
                                value=Memo::new(move |_| {
                                    sensors_text(sensors, |snapshot| {
                                        format!(
                                            "{} / {}",
                                            fmt_optional_percent(snapshot.gpu_load_percent),
                                            fmt_optional_temp(snapshot.gpu_temp_celsius),
                                        )
                                    })
                                })
                                tone=Memo::new(move |_| {
                                    temp_tone(sensors.with(|s| s.as_ref().and_then(|s| s.gpu_temp_celsius)))
                                })
                            />
                            <DetailRow label="GPU VRAM" value=Memo::new(move |_| {
                                sensors_text(sensors, |snapshot| {
                                    snapshot
                                        .gpu_vram_used_mb
                                        .map_or_else(|| "n/a".to_owned(), |value| format!("{value:.0} MB"))
                                })
                            }) />
                            <DetailRow
                                label="RAM used"
                                value=Memo::new(move |_| {
                                    sensors_text(sensors, |snapshot| {
                                        format!(
                                            "{} · {:.0}/{:.0} MB",
                                            fmt_percent_f32(snapshot.ram_used_percent),
                                            snapshot.ram_used_mb,
                                            snapshot.ram_total_mb,
                                        )
                                    })
                                })
                                tone=Memo::new(move |_| {
                                    sensors.with(|s| s.as_ref().map_or(DiagnosticTone::Neutral, |s| ram_tone(s.ram_used_percent)))
                                })
                            />
                            <DetailRow label="Devices / LEDs" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!("{} / {}", m.devices.connected, m.devices.total_leds)
                                })
                            }) />
                            <DetailRow
                                label="Output errors"
                                value=Memo::new(move |_| {
                                    metrics_text(metrics, |m| {
                                        format!(
                                            "{} frame / {} write",
                                            m.devices.output_errors,
                                            m.display_output.write_failures_total,
                                        )
                                    })
                                })
                                tone=Memo::new(move |_| {
                                    metrics.with(|m| m.as_ref().map_or(DiagnosticTone::Neutral, |m| error_count_tone(output_error_count(m))))
                                })
                            />
                            <DetailRow
                                label="Retries / attempts"
                                value=Memo::new(move |_| {
                                    metrics_text(metrics, |m| {
                                        format!(
                                            "{} / {}",
                                            m.display_output.retry_attempts_total,
                                            m.display_output.write_attempts_total,
                                        )
                                    })
                                })
                                tone=Memo::new(move |_| {
                                    flag_tone(metrics.with(|m| m.as_ref().is_some_and(|m| m.display_output.retry_attempts_total > 0)))
                                })
                            />
                            <DetailRow
                                label="Display lane"
                                value=Memo::new(move |_| {
                                    metrics_text(metrics, |m| {
                                        format!(
                                            "{} delayed / {} total",
                                            m.display_output.display_lane.display_frames_delayed_for_led_total,
                                            m.display_output.display_lane.display_frames_total,
                                        )
                                    })
                                })
                                tone=Memo::new(move |_| {
                                    flag_tone(metrics.with(|m| m.as_ref().is_some_and(|m| {
                                        m.display_output.display_lane.display_frames_delayed_for_led_total > 0
                                    })))
                                })
                            />
                            <DetailRow label="LED priority wait" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    fmt_ms(m.display_output.display_lane.display_led_priority_wait_max_ms)
                                })
                            }) />
                            <DetailRow label="Captured displays" value=Memo::new(move |_| {
                                metrics_text(metrics, |m| {
                                    format!(
                                        "{} devices / {} subscribers",
                                        m.display_output.captured_devices,
                                        m.display_output.preview_subscribers,
                                    )
                                })
                            }) />
                        </DiagnosticSection>
                    </div>
                </Show>
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

/// Small two-part header that frames a vitals cluster — a strong label
/// plus a faint scope hint ("this machine" vs "this app") so the host
/// readings can't be mistaken for hypercolor's own footprint.
#[component]
fn VitalGroupLabel(label: &'static str, hint: &'static str) -> impl IntoView {
    view! {
        <div class="flex items-baseline gap-1.5 px-3 pb-1 pt-2.5">
            <span class=label_class(LabelSize::Micro, LabelTone::Strong)>{label}</span>
            <span class="truncate text-[9px] font-mono lowercase tracking-[0.04em] text-fg-tertiary/45">
                {hint}
            </span>
        </div>
    }
}

/// A single hardware vital — small caps label, a large accent-tinted
/// value, a secondary detail line, and a slim utilization bar. The accent
/// carries health (cool/normal cyan, hot yellow/red); the bar's length
/// carries magnitude.
#[component]
fn VitalTile(
    label: &'static str,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] sub: Signal<String>,
    #[prop(into)] accent: Signal<&'static str>,
    /// Bar fill in `0.0..=1.0`.
    #[prop(into)]
    fill: Signal<f64>,
) -> impl IntoView {
    view! {
        <div class="flex min-w-0 flex-col gap-1.5 bg-surface-overlay/55 px-3 py-2.5">
            <span class="truncate text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary/70">
                {label}
            </span>
            <span
                class="truncate text-[17px] font-semibold leading-none tabular-nums"
                style=move || format!("color: {}", accent.get())
            >
                {move || value.get()}
            </span>
            <span class="truncate text-[10px] font-mono tabular-nums text-fg-tertiary/65">
                {move || sub.get()}
            </span>
            <div class="mt-0.5 h-1 w-full overflow-hidden rounded-full bg-surface-sunken/70">
                <div
                    class="h-full w-full origin-left rounded-full transition-transform duration-500 will-change-transform"
                    style=move || {
                        let accent = accent.get();
                        format!(
                            "transform: scaleX({:.4}); background: {accent}; box-shadow: 0 0 6px {accent}",
                            fill.get().clamp(0.0, 1.0),
                        )
                    }
                />
            </div>
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
fn DetailRow(
    label: &'static str,
    #[prop(into)] value: Signal<String>,
    /// Optional health tint for the value. Absent rows render at the
    /// neutral secondary foreground, so only meaningful rows carry color.
    /// Memo-typed so each row's style only re-patches on tone changes.
    #[prop(optional)]
    tone: Option<Memo<DiagnosticTone>>,
) -> impl IntoView {
    let value_color =
        move || tone.map_or("var(--color-fg-secondary)", |tone| tone.get().value_color());
    view! {
        <div class="grid min-w-0 grid-cols-1 items-start gap-0.5 sm:grid-cols-[minmax(0,0.92fr)_minmax(0,1.28fr)] sm:gap-3">
            <span class="min-w-0 break-words text-[10px] font-mono uppercase tracking-normal text-fg-tertiary/65 sm:tracking-[0.11em]">
                {label}
            </span>
            <span
                class="min-w-0 break-words text-left text-[11px] font-mono tabular-nums leading-snug sm:text-right"
                style=move || format!("color: {}", value_color())
            >
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
            "" => badge("warming", DiagnosticTone::Neutral),
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
    } else if metrics.effect_health.servo_render_readback_max_ms > 0.0 {
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

// The `*_text` helpers read their signal with `.with()` so the row memos
// project straight off the shared snapshot — no per-read clone of the
// ~250-field `PerformanceMetrics` (or `SystemStatus`/`SystemSnapshot`).

fn status_text(
    status: Memo<Option<SystemStatus>>,
    f: impl FnOnce(&SystemStatus) -> String,
) -> String {
    status.with(|status| status.as_ref().map_or_else(|| "loading".to_owned(), f))
}

fn probe_text(
    status: Memo<Option<SystemStatus>>,
    f: impl FnOnce(&api::GpuCompositorProbeStatus) -> String,
) -> String {
    status.with(|status| {
        status
            .as_ref()
            .and_then(|status| status.compositor_acceleration.gpu_probe.as_ref())
            .map_or_else(|| "n/a".to_owned(), f)
    })
}

fn sensors_text(
    sensors: Signal<Option<SystemSnapshot>>,
    f: impl FnOnce(&SystemSnapshot) -> String,
) -> String {
    sensors.with(|sensors| sensors.as_ref().map_or_else(|| "loading".to_owned(), f))
}

fn metrics_text(
    metrics: Signal<Option<PerformanceMetrics>>,
    f: impl FnOnce(&PerformanceMetrics) -> String,
) -> String {
    metrics.with(|metrics| metrics.as_ref().map_or_else(|| "waiting".to_owned(), f))
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

/// Total output-error pressure: per-frame output errors plus cumulative
/// device write failures, the two signals worth surfacing as one count.
fn output_error_count(metrics: &PerformanceMetrics) -> u64 {
    u64::from(metrics.devices.output_errors) + metrics.display_output.write_failures_total
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

fn fmt_temp_compact(value: Option<f32>) -> String {
    value.map_or_else(|| "temp n/a".to_owned(), |value| format!("{value:.0}°C"))
}

fn fmt_mem_gb(megabytes: f64) -> String {
    if megabytes >= 1024.0 {
        format!("{:.1} GB", megabytes / 1024.0)
    } else {
        format!("{megabytes:.0} MB")
    }
}

fn format_window_count(value: u32) -> String {
    format!("{value}/120 frames")
}

/// Host temperature severity. A cool component is `Neutral` (calm data),
/// not `Good` — green would imply a success state where there is only a
/// reading. Heat ramps the row toward warn and then critical.
fn temp_tone(celsius: Option<f32>) -> DiagnosticTone {
    match celsius {
        Some(t) if t >= 90.0 => DiagnosticTone::Bad,
        Some(t) if t >= 80.0 => DiagnosticTone::Warn,
        _ => DiagnosticTone::Neutral,
    }
}

/// Memory-pressure severity. Normal usage stays neutral; only a nearly
/// full host warns or alarms.
fn ram_tone(percent: f32) -> DiagnosticTone {
    if percent >= 95.0 {
        DiagnosticTone::Bad
    } else if percent >= 85.0 {
        DiagnosticTone::Warn
    } else {
        DiagnosticTone::Neutral
    }
}

/// Error-count severity. Zero is an affirmative `Good` (the "all clear"
/// signal), a handful warns, and a flood alarms.
fn error_count_tone(count: u64) -> DiagnosticTone {
    if count == 0 {
        DiagnosticTone::Good
    } else if count < 10 {
        DiagnosticTone::Warn
    } else {
        DiagnosticTone::Bad
    }
}

/// A boolean condition where clear is `Good` and set is `Warn` — for
/// counters that should sit at zero (fallbacks, retries, delays).
fn flag_tone(active: bool) -> DiagnosticTone {
    if active {
        DiagnosticTone::Warn
    } else {
        DiagnosticTone::Good
    }
}

/// Servo render path health: pure GPU is `Good`, any CPU frames warn, and
/// no frames at all (warming or idle) is neutral.
fn render_path_tone(gpu_frames: u64, cpu_frames: u64) -> DiagnosticTone {
    match (gpu_frames, cpu_frames) {
        (0, 0) => DiagnosticTone::Neutral,
        (_, 0) => DiagnosticTone::Good,
        _ => DiagnosticTone::Warn,
    }
}

/// Short uppercase label for the dominant Servo render path.
fn render_path_label(gpu_frames: u64, cpu_frames: u64) -> &'static str {
    match (gpu_frames, cpu_frames) {
        (0, 0) => "IDLE",
        (_, 0) => "GPU",
        (0, _) => "CPU",
        _ => "MIXED",
    }
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

    #[test]
    fn temp_tone_ramps_neutral_then_warn_then_bad() {
        assert_eq!(temp_tone(None), DiagnosticTone::Neutral);
        assert_eq!(temp_tone(Some(41.0)), DiagnosticTone::Neutral);
        assert_eq!(temp_tone(Some(82.0)), DiagnosticTone::Warn);
        assert_eq!(temp_tone(Some(95.0)), DiagnosticTone::Bad);
    }

    #[test]
    fn ram_tone_warns_only_when_nearly_full() {
        assert_eq!(ram_tone(34.0), DiagnosticTone::Neutral);
        assert_eq!(ram_tone(88.0), DiagnosticTone::Warn);
        assert_eq!(ram_tone(99.0), DiagnosticTone::Bad);
    }

    #[test]
    fn error_count_tone_is_good_only_at_zero() {
        assert_eq!(error_count_tone(0), DiagnosticTone::Good);
        assert_eq!(error_count_tone(3), DiagnosticTone::Warn);
        assert_eq!(error_count_tone(42), DiagnosticTone::Bad);
    }

    #[test]
    fn flag_tone_is_good_when_clear() {
        assert_eq!(flag_tone(false), DiagnosticTone::Good);
        assert_eq!(flag_tone(true), DiagnosticTone::Warn);
    }

    #[test]
    fn render_path_is_good_only_when_pure_gpu() {
        assert_eq!(render_path_tone(0, 0), DiagnosticTone::Neutral);
        assert_eq!(render_path_tone(120, 0), DiagnosticTone::Good);
        assert_eq!(render_path_tone(120, 5), DiagnosticTone::Warn);
        assert_eq!(render_path_tone(0, 5), DiagnosticTone::Warn);
    }

    #[test]
    fn render_path_label_names_the_dominant_path() {
        assert_eq!(render_path_label(0, 0), "IDLE");
        assert_eq!(render_path_label(120, 0), "GPU");
        assert_eq!(render_path_label(0, 5), "CPU");
        assert_eq!(render_path_label(120, 5), "MIXED");
    }

    #[test]
    fn fmt_mem_gb_switches_units_at_a_gigabyte() {
        assert_eq!(fmt_mem_gb(512.0), "512 MB");
        assert_eq!(fmt_mem_gb(2177.0), "2.1 GB");
    }
}
