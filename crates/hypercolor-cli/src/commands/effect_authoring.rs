//! `hypercolor effects capture|console|install` -- the effect authoring loop.
//!
//! Install a page, read what it printed, look at what it drew: the three
//! steps an author (or an agent) repeats while writing an effect.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use serde::Serialize;

use hypercolor_types::api::effects::{
    EffectConsoleLevel, EffectConsoleResponse, InstalledEffectResponse,
};
use hypercolor_types::api::scene::{ApplyEffectRequest, ApplyEffectResponse};

use crate::client::{BinaryResponse, DaemonClient};
use crate::output::{OutputContext, OutputFormat, urlencoded};

/// Most frames one `capture` run may write.
const MAX_CAPTURE_FRAMES: u32 = 8;

/// Arguments for `effects capture`.
#[derive(Debug, Args)]
pub struct EffectCaptureArgs {
    /// Image file to write. Defaults to canvas-<frame>.<ext> in the current
    /// directory; with --frames, each file gets a -<n> suffix.
    #[arg(long, short)]
    pub output: Option<PathBuf>,

    /// Image encoding. (`--format` is the global output-format flag.)
    #[arg(long, value_enum, default_value_t = CaptureFormat::Png)]
    pub image_format: CaptureFormat,

    /// Output width in pixels; the height keeps the canvas aspect ratio.
    /// Defaults to the full canvas width.
    #[arg(long)]
    pub width: Option<u32>,

    /// Capture this many frames, one file each, to judge motion.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_CAPTURE_FRAMES)))]
    pub frames: u32,

    /// Pause between frames, in milliseconds.
    #[arg(long, default_value_t = 250)]
    pub interval_ms: u64,
}

/// Image encodings `effects capture` can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CaptureFormat {
    /// Lossless, exact pixel values.
    Png,
    /// Smaller files.
    Jpeg,
}

impl CaptureFormat {
    const fn query(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }

    const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

/// Arguments for `effects console`.
#[derive(Debug, Args)]
pub struct EffectConsoleArgs {
    /// Show only messages after this cursor, as printed by a previous run
    /// or by `effects install`.
    #[arg(long)]
    pub after: Option<u64>,

    /// Effect id or name to narrow to.
    #[arg(long)]
    pub effect: Option<String>,

    /// Lowest severity to show: debug, log, warn, or error.
    #[arg(long)]
    pub level: Option<EffectConsoleLevel>,

    /// Most messages to show.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,
}

/// Arguments for `effects install`.
#[derive(Debug, Args)]
pub struct EffectInstallArgs {
    /// HTML effect file to install. Installing the same file name again
    /// updates that effect in place.
    pub file: PathBuf,

    /// Apply the effect to the primary zone after installing it.
    #[arg(long)]
    pub activate: bool,
}

/// One written capture, as `--json` reports it.
#[derive(Debug, Serialize)]
struct CapturedFrame {
    path: String,
    frame_number: Option<u64>,
    timestamp_ms: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    source_width: Option<u32>,
    source_height: Option<u32>,
    fresh: bool,
}

/// Capture the composed canvas to image files.
///
/// # Errors
///
/// Returns an error if the daemon cannot capture a frame or a file cannot be
/// written.
pub async fn execute_capture(
    args: &EffectCaptureArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = args.width.map_or_else(
        || format!("/canvas/frame?format={}", args.image_format.query()),
        |width| {
            format!(
                "/canvas/frame?format={}&width={width}",
                args.image_format.query()
            )
        },
    );

    let mut captured = Vec::new();
    for index in 0..args.frames {
        if index > 0 {
            tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
        }
        let response = client.get_binary(&path).await?;
        let frame = header_number(&response, "x-canvas-frame-number");
        let file = capture_path(args, index, frame);
        std::fs::write(&file, &response.bytes)
            .with_context(|| format!("Failed to write {}", file.display()))?;
        let fresh = response.header("x-canvas-fresh") != Some("0");
        captured.push(CapturedFrame {
            path: file.display().to_string(),
            frame_number: frame,
            timestamp_ms: header_number(&response, "x-canvas-timestamp-ms"),
            width: header_number(&response, "x-canvas-width").and_then(narrow),
            height: header_number(&response, "x-canvas-height").and_then(narrow),
            source_width: header_number(&response, "x-canvas-source-width").and_then(narrow),
            source_height: header_number(&response, "x-canvas-source-height").and_then(narrow),
            fresh,
        });
        if !fresh {
            // Nothing is rendering; more frames would repeat this one.
            break;
        }
    }

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&captured)?,
        OutputFormat::Plain => {
            for frame in &captured {
                println!("{}", frame.path);
            }
        }
        OutputFormat::Table => {
            for frame in &captured {
                let size = match (frame.width, frame.height) {
                    (Some(width), Some(height)) => format!("{width}x{height}"),
                    _ => "?".to_owned(),
                };
                let frame_number = frame
                    .frame_number
                    .map_or_else(|| "?".to_owned(), |number| number.to_string());
                ctx.success(&format!(
                    "{}  frame {}  {}",
                    ctx.painter.name(&frame.path),
                    ctx.painter.number(&frame_number),
                    ctx.painter.muted(&size)
                ));
            }
            if captured.iter().any(|frame| !frame.fresh) {
                ctx.warning(
                    "Nothing rendered during the capture; this is the last frame published",
                );
            }
        }
    }
    Ok(())
}

fn capture_path(args: &EffectCaptureArgs, index: u32, frame: Option<u64>) -> PathBuf {
    let extension = args.image_format.extension();
    match &args.output {
        Some(path) if args.frames == 1 => path.clone(),
        Some(path) => numbered_path(path, index + 1, extension),
        None => {
            let frame = frame.map_or_else(|| (index + 1).to_string(), |number| number.to_string());
            PathBuf::from(format!("canvas-{frame}.{extension}"))
        }
    }
}

/// `out.png` becomes `out-2.png` for the second frame of a burst.
fn numbered_path(path: &Path, number: u32, default_extension: &str) -> PathBuf {
    let stem = path.file_stem().map_or_else(
        || "canvas".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    let extension = path.extension().map_or_else(
        || default_extension.to_owned(),
        |ext| ext.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!("{stem}-{number}.{extension}"))
}

fn header_number(response: &BinaryResponse, name: &str) -> Option<u64> {
    response.header(name).and_then(|value| value.parse().ok())
}

fn narrow(value: u64) -> Option<u32> {
    u32::try_from(value).ok()
}

/// Print effect console output after a cursor.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or rejects the query.
pub async fn execute_console(
    args: &EffectConsoleArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let mut query = vec![format!("limit={}", args.limit)];
    if let Some(after) = args.after {
        query.push(format!("after={after}"));
    }
    if let Some(effect) = &args.effect {
        query.push(format!("effect={}", urlencoded(effect)));
    }
    if let Some(level) = args.level {
        query.push(format!("level={level}"));
    }
    let response: EffectConsoleResponse = client
        .get(&format!("/effects/console?{}", query.join("&")))
        .await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            for entry in &response.entries {
                let location = entry
                    .line
                    .map_or_else(String::new, |line| format!(":{line}"));
                println!(
                    "{} {} {}{} {}",
                    entry.seq, entry.level, entry.source_file, location, entry.message
                );
            }
        }
        OutputFormat::Table => {
            if response.truncated {
                ctx.warning("Some messages after the cursor were evicted before this read");
            }
            let rows: Vec<Vec<String>> = response
                .entries
                .iter()
                .map(|entry| {
                    let level = match entry.level.as_str() {
                        "error" => ctx.painter.error(&entry.level),
                        "warn" => ctx.painter.warning(&entry.level),
                        _ => ctx.painter.muted(&entry.level),
                    };
                    let location = entry.line.map_or_else(
                        || entry.source_file.clone(),
                        |line| format!("{}:{line}", entry.source_file),
                    );
                    vec![
                        ctx.painter.number(&entry.seq.to_string()),
                        level,
                        ctx.painter.name(&location),
                        entry.message.clone(),
                    ]
                })
                .collect();
            if rows.is_empty() {
                ctx.info("No console output");
            } else {
                ctx.print_table(&["Seq", "Level", "Source", "Message"], &rows);
            }
            ctx.info(&format!(
                "Next: hypercolor effects console --after {}",
                response.next_seq
            ));
        }
    }
    Ok(())
}

/// Install (or update) one HTML effect, optionally applying it.
///
/// # Errors
///
/// Returns an error if the file cannot be read, the daemon refuses the
/// page, or activation fails.
pub async fn execute_install(
    args: &EffectInstallArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let contents = std::fs::read(&args.file)
        .with_context(|| format!("Failed to read {}", args.file.display()))?;
    let file_name = args.file.file_name().map_or_else(
        || "effect.html".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let installed: InstalledEffectResponse = client
        .post_file("/effects/install", &file_name, "text/html", &contents)
        .await?;

    let activated: Option<ApplyEffectResponse> = if args.activate {
        Some(
            client
                .post(
                    &format!("/effects/{}/apply", urlencoded(&installed.id)),
                    &ApplyEffectRequest::default(),
                )
                .await?,
        )
    } else {
        None
    };

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&serde_json::json!({
            "installed": installed,
            "activated": activated,
        }))?,
        OutputFormat::Plain => println!("{}", installed.id),
        OutputFormat::Table => {
            ctx.success(&format!(
                "Installed {} ({} controls)",
                ctx.painter.name(&installed.name),
                ctx.painter.number(&installed.controls.to_string())
            ));
            if activated.is_some() {
                ctx.success("Applied to the primary zone");
            }
            ctx.info(&format!(
                "Errors from this version: hypercolor effects console --after {} --level warn",
                installed.console_after
            ));
        }
    }
    Ok(())
}
