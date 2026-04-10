//! `hyper status` -- display current system state.

use anyhow::Result;
use clap::Args;

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

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

/// Render the human-readable status display.
fn print_status_table(data: &serde_json::Value, ctx: &OutputContext) {
    println!();
    for line in status_table_lines(data, &ctx.painter) {
        ctx.info(&line);
    }
    println!();
}

fn status_table_lines(data: &serde_json::Value, painter: &crate::output::Painter) -> Vec<String> {
    let running = data
        .get("running")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let daemon_status = if running { "running" } else { "stopped" };
    let version = data
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let uptime = data
        .get("uptime_seconds")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let fps_tier = data
        .get("render_loop")
        .and_then(|r| r.get("fps_tier"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
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
    let effect_name = data
        .get("active_effect")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Off");
    let device_count = data
        .get("device_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let effect_count = data
        .get("effect_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let status_dot = if painter.is_enabled() {
        painter.status_dot(running)
    } else if running {
        "(*)".to_string()
    } else {
        "(x)".to_string()
    };

    let mut lines = vec![
        format!("Daemon     {status_dot} {daemon_status:<16} {version}"),
        format!("Uptime     {uptime}s"),
        format!(
            "Render     tier={fps_tier} fps={actual_fps:.1}/{target_fps} ceiling={ceiling_fps} misses={consecutive_misses} frames={total_frames}"
        ),
        format!("Effect     {effect_name}"),
        format!("Catalog    {effect_count} effects"),
        format!("Devices    {device_count} tracked"),
    ];

    if let Some(latest_frame) = data.get("latest_frame") {
        let frame_token = latest_frame
            .get("frame_token")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let total_ms = latest_frame
            .get("total_ms")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let wake_late_ms = latest_frame
            .get("wake_late_ms")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let frame_age_ms = latest_frame
            .get("frame_age_ms")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let full_frame_copy_count = latest_frame
            .get("full_frame_copy_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let full_frame_copy_kb = latest_frame
            .get("full_frame_copy_kb")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let surfaces = latest_frame.get("render_surfaces");
        let slot_count = surfaces
            .and_then(|value| value.get("slot_count"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let free_slots = surfaces
            .and_then(|value| value.get("free_slots"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let published_slots = surfaces
            .and_then(|value| value.get("published_slots"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let dequeued_slots = surfaces
            .and_then(|value| value.get("dequeued_slots"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let canvas_receivers = surfaces
            .and_then(|value| value.get("canvas_receivers"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);

        lines.push(format!(
            "Frame      token={frame_token} total={total_ms:.2}ms wake={wake_late_ms:.2}ms age={frame_age_ms:.2}ms copies={full_frame_copy_count} ({full_frame_copy_kb:.1} KiB)"
        ));
        lines.push(format!(
            "Surfaces   slots={slot_count} free={free_slots} published={published_slots} dequeued={dequeued_slots} canvas_rx={canvas_receivers}"
        ));
    }

    if let Some(preview_runtime) = data.get("preview_runtime") {
        let canvas_receivers = preview_runtime
            .get("canvas_receivers")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let screen_canvas_receivers = preview_runtime
            .get("screen_canvas_receivers")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let canvas_frames_published = preview_runtime
            .get("canvas_frames_published")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let screen_frames_published = preview_runtime
            .get("screen_canvas_frames_published")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);

        lines.push(format!(
            "Preview    canvas_rx={canvas_receivers} screen_rx={screen_canvas_receivers} canvas_frames={canvas_frames_published} screen_frames={screen_frames_published}"
        ));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::status_table_lines;
    use crate::output::Painter;
    use serde_json::json;

    #[test]
    fn status_table_lines_include_latest_frame_stats() {
        let data = json!({
            "running": true,
            "version": "1.0.0",
            "uptime_seconds": 42,
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
                "screen_canvas_frames_published": 12,
                "latest_canvas_frame_number": 77,
                "latest_screen_canvas_frame_number": 45
            }
        });

        let painter = Painter::plain();
        let lines = status_table_lines(&data, &painter);
        assert!(lines.iter().any(|line| {
            line == "Render     tier=60fps fps=59.8/60 ceiling=60 misses=0 frames=1234"
        }));
        assert!(lines.iter().any(|line| {
            line == "Surfaces   slots=6 free=0 published=4 dequeued=2 canvas_rx=2"
        }));
        assert!(lines.iter().any(|line| {
            line == "Preview    canvas_rx=1 screen_rx=0 canvas_frames=88 screen_frames=12"
        }));
    }
}
