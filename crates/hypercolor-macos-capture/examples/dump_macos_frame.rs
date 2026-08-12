//! Inspect live ScreenCaptureKit frames at Hypercolor's production boundary.
//!
//! The default mode prints metadata only. Authorization prompts, Apple's
//! picker, and pixel export each require an explicit command-line flag.

use std::ffi::OsString;
#[cfg(any(target_os = "macos", all(test, feature = "capture-fixtures")))]
use std::fmt::Write as _;
#[cfg(any(target_os = "macos", all(test, feature = "capture-fixtures")))]
use std::io::{BufWriter, Write};
#[cfg(any(target_os = "macos", all(test, feature = "capture-fixtures")))]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(any(target_os = "macos", all(test, feature = "capture-fixtures")))]
use hypercolor_macos_capture::MacosCaptureFrame;
use hypercolor_macos_capture::MacosCaptureSelector;
#[cfg(target_os = "macos")]
use hypercolor_macos_capture::MacosFrameDropReason;

const DEFAULT_FRAME_COUNT: usize = 1;
const MAX_FRAME_COUNT: usize = 600;
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const MAX_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolOptions {
    frame_count: usize,
    timeout: Duration,
    selector: MacosCaptureSelector,
    authorize: bool,
    picker: bool,
    output: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolCommand {
    Run(ToolOptions),
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(target_os = "macos", all(test, feature = "capture-fixtures")))]
struct FrameTiming {
    since_start_us: u128,
    delivery_latency_us: Option<u128>,
    inter_frame_us: Option<u128>,
}

fn main() {
    let command = match parse_args(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}\n\n{}", usage());
            std::process::exit(2);
        }
    };
    if command == ToolCommand::Help {
        println!("{}", usage());
        return;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = command;
        eprintln!("dump_macos_frame requires macOS 15.2 or newer");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    if let ToolCommand::Run(options) = command
        && let Err(error) = run_macos(options)
    {
        eprintln!("capture diagnostic failed: {error}");
        std::process::exit(1);
    }
}

fn usage() -> &'static str {
    "Usage: cargo run -p hypercolor-macos-capture --example \
dump_macos_frame -- [OPTIONS]\n\
\n\
Options:\n\
  --frames COUNT       Complete frames to inspect, 1 through 600 (default: 1)\n\
  --timeout-seconds N  Total capture budget, 1 through 300 (default: 30)\n\
  --source SELECTOR    auto, primary_display, display:<uuid>, or session_scoped\n\
  --authorize          Explicitly request Screen Recording authorization\n\
  --picker             Explicitly present Apple's system content picker\n\
  --output PATH        Export one SDR BGRA frame as RGBA PAM pixels\n\
  -h, --help           Print this help\n\
\n\
Metadata-only mode is the default. --output requires --frames 1 and prints a\n\
privacy warning before touching the destination path."
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<ToolCommand, String> {
    let mut options = ToolOptions {
        frame_count: DEFAULT_FRAME_COUNT,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
        selector: MacosCaptureSelector::Auto,
        authorize: false,
        picker: false,
        output: None,
    };
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let argument = argument
            .to_str()
            .ok_or_else(|| "option names must be valid UTF-8".to_owned())?;
        match argument {
            "-h" | "--help" => return Ok(ToolCommand::Help),
            "--frames" => {
                options.frame_count = parse_bounded(args.next(), "--frames", 1, MAX_FRAME_COUNT)?;
            }
            "--timeout-seconds" => {
                let seconds =
                    parse_bounded(args.next(), "--timeout-seconds", 1, MAX_TIMEOUT_SECONDS)?;
                options.timeout = Duration::from_secs(seconds);
            }
            "--source" => {
                let source = next_utf8(&mut args, "--source")?;
                options.selector = MacosCaptureSelector::parse(&source)
                    .map_err(|_| format!("invalid --source value: {source}"))?;
            }
            "--authorize" => options.authorize = true,
            "--picker" => options.picker = true,
            "--output" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--output requires a path".to_owned())?;
                if path.is_empty() {
                    return Err("--output requires a nonempty path".to_owned());
                }
                options.output = Some(PathBuf::from(path));
            }
            unknown => return Err(format!("unknown option: {unknown}")),
        }
    }
    if options.output.is_some() && options.frame_count != 1 {
        return Err("--output requires --frames 1 to prevent implicit file naming".to_owned());
    }
    if options.selector == MacosCaptureSelector::SessionScoped && !options.picker {
        return Err("session_scoped capture requires the explicit --picker action".to_owned());
    }
    Ok(ToolCommand::Run(options))
}

fn next_utf8(args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value"))?
        .into_string()
        .map_err(|_| format!("{option} requires a UTF-8 value"))
}

fn parse_bounded<T>(
    value: Option<OsString>,
    option: &str,
    minimum: T,
    maximum: T,
) -> Result<T, String>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy,
{
    let value = value.ok_or_else(|| format!("{option} requires a value"))?;
    let value = value
        .to_str()
        .ok_or_else(|| format!("{option} requires a UTF-8 integer"))?;
    let parsed = value
        .parse::<T>()
        .map_err(|_| format!("{option} requires an integer"))?;
    if parsed < minimum || parsed > maximum {
        return Err(format!("{option} must be between {minimum} and {maximum}"));
    }
    Ok(parsed)
}

#[cfg(target_os = "macos")]
fn run_macos(options: ToolOptions) -> Result<(), String> {
    use std::time::Instant;

    use hypercolor_macos_capture::{
        MacosCaptureCadence, MacosDisplayClock, MacosFrameEvent, MacosScreenCaptureSession,
        MacosStreamRequest,
    };

    let request = MacosStreamRequest::new(MacosCaptureCadence::NativeRefresh, true)
        .map_err(|_| "native-refresh capture configuration was rejected".to_owned())?;
    let session = MacosScreenCaptureSession::new(request, options.selector.clone())
        .map_err(|_| "could not create the production ScreenCaptureKit session".to_owned())?;

    if options.authorize {
        println!("user action: requesting Screen Recording authorization");
        let state = session.request_authorization();
        println!("authorization state: {state:?}");
    }
    if !MacosScreenCaptureSession::screen_authorized() {
        return Err(
            "Screen Recording is not authorized; rerun with --authorize to request it".to_owned(),
        );
    }
    if options.picker {
        println!("user action: presenting Apple's system content picker");
        session
            .present_picker()
            .map_err(|_| "Apple's content picker could not be presented".to_owned())?;
    }

    println!(
        "capture source: {}; frame budget: {}; timeout: {}s; pixels: {}",
        redacted_selector(&options.selector),
        options.frame_count,
        options.timeout.as_secs(),
        if options.output.is_some() {
            "explicit export"
        } else {
            "metadata only"
        }
    );

    let clock = MacosDisplayClock::system().ok();
    let started = Instant::now();
    let deadline = started + options.timeout;
    let mut previous_display = None;
    let mut captured = 0_usize;
    let mailbox = session.mailbox();
    session.set_capture_active(true);
    let result = (|| {
        while captured < options.frame_count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "timed out after receiving {captured} of {} complete frames",
                    options.frame_count
                ));
            }
            let Some(delivery) = mailbox.wait_latest(remaining) else {
                continue;
            };
            match delivery {
                Ok(MacosFrameEvent::Frame(frame)) => {
                    let now = Instant::now();
                    let display = clock
                        .as_ref()
                        .and_then(|clock| clock.timestamp(frame.display_time).ok());
                    let timing = FrameTiming {
                        since_start_us: now.duration_since(started).as_micros(),
                        delivery_latency_us: display
                            .map(|display| now.saturating_duration_since(display).as_micros()),
                        inter_frame_us: display.zip(previous_display).map(|(display, previous)| {
                            display.saturating_duration_since(previous).as_micros()
                        }),
                    };
                    previous_display = display;
                    captured += 1;
                    println!("frame {captured}/{}", options.frame_count);
                    print!("{}", format_frame_metadata(&frame, timing));
                    if let Some(path) = options.output.as_deref() {
                        export_frame_with_warning(&frame, path, |warning| {
                            eprintln!("{warning}");
                        })?;
                    }
                }
                Ok(MacosFrameEvent::Lifecycle(state)) => {
                    println!("lifecycle: {state:?}");
                }
                Ok(MacosFrameEvent::RecoverableError(_)) => {
                    eprintln!("recoverable capture error; frame metadata remains redacted");
                }
                Err(_) => {
                    return Err("capture failed; native error text was redacted".to_owned());
                }
            }
        }
        Ok(())
    })();
    session.stop();

    let diagnostics = session.diagnostics();
    println!(
        "diagnostics: received={} published={} lifecycle={} superseded={} dropped={}",
        diagnostics.frames_received,
        diagnostics.frames_published,
        diagnostics.lifecycle_events,
        diagnostics.superseded_deliveries,
        diagnostics.total_dropped()
    );
    for reason in MacosFrameDropReason::ALL {
        let count = diagnostics.dropped(reason);
        if count != 0 {
            println!("  dropped.{reason:?}={count}");
        }
    }
    result
}

#[cfg(target_os = "macos")]
fn redacted_selector(selector: &MacosCaptureSelector) -> &'static str {
    match selector {
        MacosCaptureSelector::Auto => "auto",
        MacosCaptureSelector::PrimaryDisplay => "primary_display",
        MacosCaptureSelector::Display { .. } => "explicit_display",
        MacosCaptureSelector::SessionScoped => "session_scoped",
    }
}

#[cfg(any(target_os = "macos", all(test, feature = "capture-fixtures")))]
fn format_frame_metadata(frame: &MacosCaptureFrame, timing: FrameTiming) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "  descriptor: epoch={} sequence={} extent={}x{} format={:?} cursor_composed={}",
        frame.epoch,
        frame.sequence,
        frame.storage_extent.width,
        frame.storage_extent.height,
        frame.pixel_format,
        frame.cursor_composed
    );
    for plane in &*frame.planes {
        let _ = writeln!(
            output,
            "  plane[{}]: extent={}x{} stride={} length={}",
            plane.index,
            plane.extent.width,
            plane.extent.height,
            plane.bytes_per_row,
            plane.length_bytes
        );
    }
    let _ = writeln!(
        output,
        "  attachments: status=complete display_time={} display_scale={} content_scale={}",
        frame.display_time,
        frame.geometry.display_scale_factor.get(),
        frame.geometry.content_scale.get()
    );
    let _ = writeln!(
        output,
        "  content_rect_points={:?} content_rect_pixels={:?}",
        frame.geometry.content_rect_points, frame.geometry.content_rect_pixels
    );
    let _ = writeln!(
        output,
        "  screen_rect_points={:?} bounding_rect_points={:?} bounding_rect_pixels={:?}",
        frame.geometry.screen_rect_points,
        frame.geometry.bounding_rect_points,
        frame.geometry.bounding_rect_pixels
    );
    let _ = writeln!(output, "  dirty_rects={:?}", frame.damage);
    let _ = writeln!(
        output,
        "  color: primaries={:?} transfer={:?} matrix={:?} range={:?} chroma={:?}",
        frame.color.primaries,
        frame.color.transfer,
        frame.color.matrix,
        frame.color.range,
        frame.color.chroma_location
    );
    let _ = writeln!(
        output,
        "  iosurface: id={} allocation_bytes={}",
        frame.surface.iosurface_id, frame.surface.allocation_bytes
    );
    let _ = writeln!(
        output,
        "  timing: since_start_us={} delivery_latency_us={} inter_frame_us={}",
        timing.since_start_us,
        optional_micros(timing.delivery_latency_us),
        optional_micros(timing.inter_frame_us)
    );
    output
}

#[cfg(any(target_os = "macos", all(test, feature = "capture-fixtures")))]
fn optional_micros(value: Option<u128>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
}

#[cfg(any(target_os = "macos", all(test, feature = "capture-fixtures")))]
fn export_frame_with_warning(
    frame: &MacosCaptureFrame,
    path: &Path,
    warn: impl FnOnce(&str),
) -> Result<(), String> {
    let warning = format!(
        "PRIVACY WARNING: writing captured screen pixels to {}; the image may reveal private content",
        path.display()
    );
    warn(&warning);

    let row_bytes = usize::try_from(frame.storage_extent.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| "pixel export dimensions overflowed".to_owned())?;
    let length = row_bytes
        .checked_mul(frame.storage_extent.height as usize)
        .ok_or_else(|| "pixel export length overflowed".to_owned())?;
    let mut rgba = vec![0_u8; length];
    frame
        .convert_bgra8_sdr_to_rgba8(&mut rgba, row_bytes)
        .map_err(|_| "pixel export supports SDR BGRA frames only".to_owned())?;

    let file = std::fs::File::create(path)
        .map_err(|error| format!("could not create explicit output path: {error}"))?;
    let mut output = BufWriter::new(file);
    write!(
        output,
        "P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",
        frame.storage_extent.width, frame.storage_extent.height
    )
    .map_err(|error| format!("could not write PAM header: {error}"))?;
    output
        .write_all(&rgba)
        .map_err(|error| format!("could not write captured pixels: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("could not flush captured pixels: {error}"))
}

#[cfg(all(test, feature = "capture-fixtures"))]
mod tests {
    use std::sync::Arc;

    use hypercolor_macos_capture::{
        MacosCaptureColorimetry, MacosCaptureFrame, MacosCaptureGeometry, MacosCapturePixelFormat,
        MacosCapturePlane, MacosCaptureSurface, MacosColorPrimaries, MacosColorRange,
        MacosPixelExtent, MacosPixelRect, MacosPointRect, MacosScale, MacosTransferFunction,
    };

    use super::{
        FrameTiming, ToolCommand, export_frame_with_warning, format_frame_metadata, parse_args,
    };

    #[test]
    fn defaults_are_bounded_and_metadata_only() {
        let ToolCommand::Run(options) = parse_args(Vec::new()).expect("defaults should parse")
        else {
            panic!("defaults should run the diagnostic");
        };
        assert_eq!(options.frame_count, 1);
        assert_eq!(options.timeout.as_secs(), 30);
        assert!(!options.authorize);
        assert!(!options.picker);
        assert!(options.output.is_none());
    }

    #[test]
    fn parser_requires_explicit_bounded_pixel_export() {
        assert!(parse_args(["--frames".into(), "0".into()]).is_err());
        assert!(parse_args(["--frames".into(), "601".into()]).is_err());
        assert!(
            parse_args([
                "--frames".into(),
                "2".into(),
                "--output".into(),
                "capture.pam".into(),
            ])
            .is_err()
        );
        assert!(parse_args(["--source".into(), "session_scoped".into()]).is_err());
        assert!(
            parse_args([
                "--source".into(),
                "session_scoped".into(),
                "--picker".into(),
            ])
            .is_ok()
        );
    }

    #[test]
    fn metadata_contains_no_titles_pixels_or_paths() {
        let metadata = format_frame_metadata(
            &fixture_frame(),
            FrameTiming {
                since_start_us: 10,
                delivery_latency_us: Some(2),
                inter_frame_us: None,
            },
        );
        assert!(metadata.contains("allocation_bytes=4"));
        assert!(metadata.contains("status=complete"));
        assert!(!metadata.contains("window_title"));
        assert!(!metadata.contains("application_name"));
        assert!(!metadata.contains("capture.pam"));
        assert!(!metadata.contains("[10, 20, 30, 255]"));
    }

    #[test]
    fn export_warns_before_touching_the_explicit_path() {
        let path = std::env::temp_dir().join(format!(
            "hypercolor-dump-macos-frame-{}-{}.pam",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should follow Unix epoch")
                .as_nanos()
        ));
        let mut warned = false;
        export_frame_with_warning(&fixture_frame(), &path, |warning| {
            assert!(!path.exists());
            assert!(warning.starts_with("PRIVACY WARNING:"));
            assert!(warning.contains(&path.display().to_string()));
            warned = true;
        })
        .expect("explicit SDR export should succeed");
        assert!(warned);
        let bytes = std::fs::read(&path).expect("export should create the explicit path");
        assert!(bytes.starts_with(b"P7\nWIDTH 1\nHEIGHT 1\n"));
        assert!(bytes.ends_with(&[30, 20, 10, 255]));
        std::fs::remove_file(&path).expect("fixture output should be removable");
    }

    fn fixture_frame() -> MacosCaptureFrame {
        let extent = MacosPixelExtent::new(1, 1).expect("fixture extent should be valid");
        MacosCaptureFrame {
            epoch: 7,
            sequence: 3,
            display_time: 99,
            storage_extent: extent,
            planes: Arc::from([MacosCapturePlane {
                index: 0,
                extent,
                bytes_per_row: 4,
                length_bytes: 4,
            }]),
            pixel_format: MacosCapturePixelFormat::Bgra8,
            color: MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Srgb,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            geometry: MacosCaptureGeometry {
                display_scale_factor: MacosScale::display(1.0)
                    .expect("display scale should be valid"),
                content_scale: MacosScale::new(1.0).expect("content scale should be valid"),
                content_rect_points: MacosPointRect::new(0.0, 0.0, 1.0, 1.0)
                    .expect("content rect should be valid"),
                content_rect_pixels: MacosPixelRect::new(0, 0, 1, 1)
                    .expect("pixel rect should be valid"),
                screen_rect_points: None,
                bounding_rect_points: None,
                bounding_rect_pixels: None,
            },
            damage: Arc::from([
                MacosPixelRect::new(0, 0, 1, 1).expect("damage rect should be valid")
            ]),
            cursor_composed: true,
            surface: MacosCaptureSurface::new_cpu_fixture(
                1,
                4,
                11,
                vec![Arc::from([10_u8, 20, 30, 255])],
            )
            .expect("fixture surface should be valid"),
        }
    }
}
