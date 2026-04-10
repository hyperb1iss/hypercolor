//! `hyper status` -- display current system state.

use anyhow::Result;
use clap::Args;

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, Painter};

/// Show current system state: running effect, devices, FPS, audio capture.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Live-updating status (re-renders on state change).
    #[arg(long)]
    pub watch: bool,

    /// Update interval for --watch mode in seconds.
    #[arg(long, default_value = "1")]
    pub interval: f64,
}

/// Execute the `status` subcommand.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable.
pub async fn execute(args: &StatusArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    if args.watch {
        let interval = args.interval.max(0.2);
        loop {
            let response = client.get("/status").await?;
            render_status(&response, ctx)?;

            let sleep = tokio::time::sleep(std::time::Duration::from_secs_f64(interval));
            tokio::pin!(sleep);
            tokio::select! {
                () = &mut sleep => {}
                _ = tokio::signal::ctrl_c() => {
                    if !ctx.quiet {
                        println!();
                        ctx.info("Stopped status watch.");
                    }
                    break;
                }
            }
        }
        return Ok(());
    }

    let response = client.get("/status").await?;
    render_status(&response, ctx)?;

    Ok(())
}

fn render_status(data: &serde_json::Value, ctx: &OutputContext) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(data)?,
        OutputFormat::Plain => {
            let effect = data
                .get("active_effect")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Off");
            println!("{effect}");
        }
        OutputFormat::Table => print_status_table(data, ctx),
    }
    Ok(())
}

// ── Rich table layout ──────────────────────────────────────────────────

fn print_status_table(data: &serde_json::Value, ctx: &OutputContext) {
    if ctx.quiet {
        // Quiet mode: one-liner
        let running = bool_field(data, "running");
        let effect = str_field(data, "active_effect", "off");
        let dot = if ctx.painter.is_enabled() {
            ctx.painter.status_dot(running)
        } else if running {
            "*".to_string()
        } else {
            "x".to_string()
        };
        println!("  {dot} {effect}");
        return;
    }

    for line in status_table_lines(data, &ctx.painter) {
        println!("{line}");
    }
}

/// Build the rendered status lines as a Vec so tests can inspect them.
fn status_table_lines(data: &serde_json::Value, p: &Painter) -> Vec<String> {
    let mut lines = Vec::with_capacity(16);

    lines.push(String::new());
    lines.push(format!("  {}", p.help_banner_title()));
    lines.push(format!("  {}", p.muted(&"\u{2500}".repeat(21))));
    lines.push(String::new());

    // ── Header line: status · version · uptime ─────────────────────
    let running = bool_field(data, "running");
    let version = str_field(data, "version", "?");
    let uptime = u64_field(data, "uptime_seconds");

    let dot = if p.is_enabled() {
        p.status_dot(running)
    } else if running {
        "(*)".to_string()
    } else {
        "(x)".to_string()
    };
    let state_word = if running {
        p.success("running")
    } else {
        p.error("stopped")
    };
    lines.push(format!(
        "  {dot} {state_word}      {}  {}      {}  {}",
        p.muted("version"),
        p.number(version),
        p.muted("up"),
        p.number(&format_uptime(uptime)),
    ));
    lines.push(String::new());

    // ── Effect ──────────────────────────────────────────────────────
    let effect_name = str_field(data, "active_effect", "off");
    lines.push(format!(
        "  {}   {}",
        p.muted(&pad("Effect", 10)),
        p.keyword(effect_name),
    ));

    // ── Render ──────────────────────────────────────────────────────
    let target_fps = data
        .get("render_loop")
        .and_then(|r| r.get("target_fps"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let actual_fps = data
        .get("render_loop")
        .and_then(|r| r.get("actual_fps"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let fps_tier = data
        .get("render_loop")
        .and_then(|r| r.get("fps_tier"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let ceiling_fps = data
        .get("render_loop")
        .and_then(|r| r.get("ceiling_fps"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(target_fps);
    let consecutive_misses = data
        .get("render_loop")
        .and_then(|r| r.get("consecutive_misses"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let total_frames = data
        .get("render_loop")
        .and_then(|r| r.get("total_frames"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    let fps_ratio = if target_fps > 0 {
        #[allow(clippy::cast_precision_loss)]
        let r = (actual_fps / target_fps as f64).clamp(0.0, 1.0);
        r
    } else {
        0.0
    };
    let fps_pct = (fps_ratio * 100.0).round() as u32;
    let bar = render_fps_bar(fps_ratio, 24, p);
    let fps_display = format!("{actual_fps:>4.1} {} {target_fps}", p.muted("/"));
    let health_word = format_fps_health(fps_pct, p);

    lines.push(format!(
        "  {}   {}  {bar}  {health_word}",
        p.muted(&pad("Render", 10)),
        fps_display,
    ));
    lines.push(format!(
        "  {}   {}  {}  {}  {}  {}",
        p.muted(&pad("", 10)),
        p.muted(&format!("{fps_tier} tier")),
        p.muted(&format!("ceiling {ceiling_fps}")),
        if consecutive_misses > 0 {
            p.error(&format!("{consecutive_misses} misses"))
        } else {
            p.muted("0 misses")
        },
        p.muted(&format!("{} frames", format_count(total_frames))),
        p.muted(""),
    ));

    // ── Frame budget ────────────────────────────────────────────────
    if let Some(latest_frame) = data.get("latest_frame") {
        let total_ms = f64_field(latest_frame, "total_ms");
        let wake_late_ms = f64_field(latest_frame, "wake_late_ms");
        let frame_age_ms = f64_field(latest_frame, "frame_age_ms");
        let copy_count = latest_frame
            .get("full_frame_copy_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let copy_kb = f64_field(latest_frame, "full_frame_copy_kb");

        lines.push(format!(
            "  {}   {} total  {} wake  {} age",
            p.muted(&pad("Frame", 10)),
            p.number(&format!("{total_ms:.2}ms")),
            p.number(&format!("{wake_late_ms:.2}ms")),
            p.number(&format!("{frame_age_ms:.2}ms")),
        ));

        // ── Pipeline ────────────────────────────────────────────────
        let surfaces = latest_frame.get("render_surfaces");
        if let Some(s) = surfaces {
            let slot_count = u64_field(s, "slot_count");
            let free_slots = u64_field(s, "free_slots");
            let published_slots = u64_field(s, "published_slots");
            let dequeued_slots = u64_field(s, "dequeued_slots");
            let canvas_receivers = u64_field(s, "canvas_receivers");

            lines.push(format!(
                "  {}   {} slots  {} free  {} published  {} dequeued",
                p.muted(&pad("Surfaces", 10)),
                p.number(&slot_count.to_string()),
                p.number(&free_slots.to_string()),
                p.number(&published_slots.to_string()),
                p.number(&dequeued_slots.to_string()),
            ));
            lines.push(format!(
                "  {}   {} copies ({})  {} canvas rx",
                p.muted(&pad("", 10)),
                p.number(&copy_count.to_string()),
                p.muted(&format_kib(copy_kb)),
                p.number(&canvas_receivers.to_string()),
            ));
        }
    }

    // ── Preview runtime ─────────────────────────────────────────────
    if let Some(preview) = data.get("preview_runtime") {
        let canvas_rx = u64_field(preview, "canvas_receivers");
        let screen_rx = u64_field(preview, "screen_canvas_receivers");
        let canvas_frames = u64_field(preview, "canvas_frames_published");
        let screen_frames = u64_field(preview, "screen_canvas_frames_published");

        lines.push(format!(
            "  {}   {} rx ({} frames)  {} screen rx ({} frames)",
            p.muted(&pad("Preview", 10)),
            p.number(&canvas_rx.to_string()),
            p.muted(&format_count(canvas_frames)),
            p.number(&screen_rx.to_string()),
            p.muted(&format_count(screen_frames)),
        ));
    }

    // ── Inventory ───────────────────────────────────────────────────
    let device_count = u64_field(data, "device_count");
    let effect_count = u64_field(data, "effect_count");
    lines.push(format!(
        "  {}   {} devices  {}  {} effects",
        p.muted(&pad("Inventory", 10)),
        p.number(&device_count.to_string()),
        p.muted("\u{00b7}"),
        p.number(&effect_count.to_string()),
    ));

    lines.push(String::new());

    lines
}

// ── Formatting helpers ─────────────────────────────────────────────────

fn pad(s: &str, width: usize) -> String {
    if s.chars().count() >= width {
        s.to_string()
    } else {
        format!("{s:<width$}")
    }
}

/// Format seconds as a human-readable uptime string.
fn format_uptime(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3_600 {
        let m = seconds / 60;
        let s = seconds % 60;
        return format!("{m}m {s}s");
    }
    if seconds < 86_400 {
        let h = seconds / 3_600;
        let m = (seconds % 3_600) / 60;
        return format!("{h}h {m}m");
    }
    let d = seconds / 86_400;
    let h = (seconds % 86_400) / 3_600;
    format!("{d}d {h}h")
}

/// Format a KiB value as KiB or MiB depending on magnitude.
fn format_kib(kib: f64) -> String {
    if kib >= 1024.0 {
        format!("{:.1} MiB", kib / 1024.0)
    } else {
        format!("{kib:.0} KiB")
    }
}

/// Format a large count with thousands separators.
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(char::from(b));
    }
    out
}

/// Render a progress bar for FPS ratio using cyan filled blocks and dim empty.
fn render_fps_bar(ratio: f64, width: usize, p: &Painter) -> String {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let filled = (ratio * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;

    let filled_str = "\u{2588}".repeat(filled);
    let empty_str = "\u{2591}".repeat(empty);

    // Color the filled portion based on health:
    //   >=95% → cyan (healthy)
    //   >=80% → yellow (degraded)
    //   <80%  → red (critical)
    let pct = ratio * 100.0;
    let filled_colored = if pct >= 95.0 {
        p.name(&filled_str)
    } else if pct >= 80.0 {
        p.warning(&filled_str)
    } else {
        p.error(&filled_str)
    };

    format!("{filled_colored}{}", p.muted(&empty_str))
}

/// Percentage label with health coloring.
fn format_fps_health(pct: u32, p: &Painter) -> String {
    let text = format!("{pct}%");
    if pct >= 95 {
        p.success(&text)
    } else if pct >= 80 {
        p.warning(&text)
    } else {
        p.error(&text)
    }
}

// ── JSON field extractors ──────────────────────────────────────────────

fn bool_field(v: &serde_json::Value, key: &str) -> bool {
    v.get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn u64_field(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0)
}

fn f64_field(v: &serde_json::Value, key: &str) -> f64 {
    v.get(key)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0)
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str, default: &'a str) -> &'a str {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::{format_count, format_kib, format_uptime, status_table_lines};
    use crate::output::Painter;
    use serde_json::json;

    #[test]
    fn format_uptime_formats_correctly() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(75), "1m 15s");
        assert_eq!(format_uptime(3_661), "1h 1m");
        assert_eq!(format_uptime(90_000), "1d 1h");
    }

    #[test]
    fn format_kib_switches_to_mib() {
        assert_eq!(format_kib(512.0), "512 KiB");
        assert_eq!(format_kib(2_048.0), "2.0 MiB");
    }

    #[test]
    fn format_count_adds_thousands_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_234), "1,234");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn status_lines_include_core_fields() {
        let data = json!({
            "running": true,
            "version": "1.0.0",
            "uptime_seconds": 3_661,
            "render_loop": {
                "fps_tier": "60fps",
                "target_fps": 60,
                "ceiling_fps": 60,
                "actual_fps": 59.8,
                "consecutive_misses": 0,
                "total_frames": 1234
            },
            "active_effect": "Breakthrough",
            "device_count": 5,
            "effect_count": 18,
            "latest_frame": {
                "frame_token": 77,
                "total_ms": 4.32,
                "wake_late_ms": 0.15,
                "frame_age_ms": 8.5,
                "full_frame_copy_count": 1,
                "full_frame_copy_kb": 250.0,
                "render_surfaces": {
                    "slot_count": 6,
                    "free_slots": 0,
                    "published_slots": 4,
                    "dequeued_slots": 2,
                    "canvas_receivers": 2
                }
            },
            "preview_runtime": {
                "canvas_receivers": 1,
                "screen_canvas_receivers": 0,
                "canvas_frames_published": 88,
                "screen_canvas_frames_published": 12
            }
        });

        let painter = Painter::plain();
        let lines = status_table_lines(&data, &painter);
        let joined = lines.join("\n");

        assert!(joined.contains("Breakthrough"), "effect name present");
        assert!(joined.contains("running"), "running state present");
        assert!(joined.contains("1.0.0"), "version present");
        assert!(joined.contains("1h 1m"), "uptime formatted");
        assert!(joined.contains("59.8"), "actual fps present");
        assert!(joined.contains("60fps tier"), "tier present");
        assert!(
            joined.contains("1,234 frames"),
            "frame count with separator"
        );
        assert!(joined.contains("6 slots"), "surface count present");
        assert!(joined.contains("250 KiB"), "copy size present");
        assert!(joined.contains("5 devices"), "device count present");
        assert!(joined.contains("18 effects"), "effect count present");
    }
}
