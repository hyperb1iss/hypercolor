//! Run Tahoe SDR or paired SDR/HDR screenshot reference diagnostics.
//!
//! Metadata-only mode is the default. Prompts, picker presentation, and pixel
//! writes each require an explicit command-line flag.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use hypercolor_macos_capture::MacosCaptureSelector;

#[derive(Debug)]
struct Options {
    selector: MacosCaptureSelector,
    authorize: bool,
    picker: bool,
    hdr: bool,
    timeout: Duration,
    sdr_output: Option<PathBuf>,
    hdr_output: Option<PathBuf>,
}

fn main() {
    let options = match parse_args(std::env::args_os().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{}", usage());
            return;
        }
        Err(error) => {
            eprintln!("{error}\n\n{}", usage());
            std::process::exit(2);
        }
    };

    #[cfg(not(target_os = "macos"))]
    {
        let _ = options;
        eprintln!("capture_macos_screenshot_reference requires macOS 26 or newer");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    if let Err(error) = run(options) {
        eprintln!("screenshot reference diagnostic failed: {error}");
        std::process::exit(1);
    }
}

fn usage() -> &'static str {
    "Usage: capture_macos_screenshot_reference [OPTIONS]\n\
\n\
  --source SELECTOR      auto, primary_display, display:<uuid>, or session_scoped\n\
  --hdr                  Request paired SDR/HDR references\n\
  --authorize            Explicitly request Screen Recording authorization\n\
  --picker               Explicitly present Apple's system content picker\n\
  --timeout-seconds N    Diagnostic budget, 1 through 300 (default: 30)\n\
  --sdr-output PATH      Explicitly encode the SDR reference as PNG\n\
  --hdr-output PATH      Explicitly encode the HDR reference as PNG\n\
  -h, --help             Print this help\n\
\n\
Metadata-only mode is the default. Output flags print a privacy warning first."
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Option<Options>, String> {
    let mut options = Options {
        selector: MacosCaptureSelector::Auto,
        authorize: false,
        picker: false,
        hdr: false,
        timeout: Duration::from_secs(30),
        sdr_output: None,
        hdr_output: None,
    };
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let argument = argument
            .to_str()
            .ok_or_else(|| "option names must be valid UTF-8".to_owned())?;
        match argument {
            "-h" | "--help" => return Ok(None),
            "--source" => {
                let source = next_utf8(&mut args, "--source")?;
                options.selector = MacosCaptureSelector::parse(&source)
                    .map_err(|_| "invalid --source selector".to_owned())?;
            }
            "--hdr" => options.hdr = true,
            "--authorize" => options.authorize = true,
            "--picker" => options.picker = true,
            "--timeout-seconds" => {
                let seconds = next_utf8(&mut args, "--timeout-seconds")?
                    .parse::<u64>()
                    .map_err(|_| "--timeout-seconds expects an integer".to_owned())?;
                if !(1..=300).contains(&seconds) {
                    return Err("--timeout-seconds must be between 1 and 300".to_owned());
                }
                options.timeout = Duration::from_secs(seconds);
            }
            "--sdr-output" => {
                options.sdr_output = Some(PathBuf::from(next_os(&mut args, "--sdr-output")?));
            }
            "--hdr-output" => {
                options.hdr_output = Some(PathBuf::from(next_os(&mut args, "--hdr-output")?));
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if options.selector == MacosCaptureSelector::SessionScoped && !options.picker {
        return Err("session_scoped capture requires the explicit --picker action".to_owned());
    }
    if options.hdr_output.is_some() && !options.hdr {
        return Err("--hdr-output requires --hdr".to_owned());
    }
    Ok(Some(options))
}

fn next_utf8(args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<String, String> {
    next_os(args, option)?
        .into_string()
        .map_err(|_| format!("{option} expects valid UTF-8"))
}

fn next_os(args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<OsString, String> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

#[cfg(target_os = "macos")]
fn run(options: Options) -> Result<(), String> {
    use std::time::Instant;

    use hypercolor_macos_capture::{
        MacosCaptureCadence, MacosFrameEvent, MacosScreenCaptureSession,
        MacosScreenshotPreferredDynamicRange, MacosScreenshotReferenceCapability,
        MacosScreenshotReferenceSet, MacosStreamRequest,
    };

    let request = if options.hdr {
        MacosStreamRequest::new_hdr(MacosCaptureCadence::NativeRefresh, true)
    } else {
        MacosStreamRequest::new(MacosCaptureCadence::NativeRefresh, true)
    }
    .map_err(|_| "native-refresh capture configuration was rejected".to_owned())?;
    let session = MacosScreenCaptureSession::new(request, options.selector.clone())
        .map_err(|_| "could not create the production ScreenCaptureKit session".to_owned())?;
    if options.authorize {
        println!("user action: requesting Screen Recording authorization");
        println!("authorization state: {:?}", session.request_authorization());
    }
    if !MacosScreenCaptureSession::screen_authorized() {
        return Err("Screen Recording is not authorized; use --authorize to request it".to_owned());
    }
    if options.picker {
        println!("user action: presenting Apple's system content picker");
        session
            .present_picker()
            .map_err(|_| "Apple's content picker could not be presented".to_owned())?;
    }

    println!(
        "reference mode: {}; pixels: {}",
        if options.hdr { "paired SDR/HDR" } else { "SDR" },
        if options.sdr_output.is_some() || options.hdr_output.is_some() {
            "explicit export"
        } else {
            "metadata only"
        }
    );
    let deadline = Instant::now() + options.timeout;
    let mailbox = session.mailbox();
    session.set_capture_active(true);
    let result = (|| {
        let mut reported_pending = false;
        loop {
            match session.screenshot_reference_capability() {
                Ok(MacosScreenshotReferenceCapability::PendingFirstFrame) => {
                    if !reported_pending {
                        println!(
                            "selection capability: pending_first_frame; screenshot capture: none"
                        );
                        reported_pending = true;
                    }
                }
                Ok(capability) => {
                    println!("selection capability: {}", capability_name(&capability));
                    break;
                }
                Err(_) => return Err("Tahoe screenshot capability probe failed".to_owned()),
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("timed out before the first complete frame".to_owned());
            }
            match mailbox.wait_latest(remaining) {
                Some(Ok(MacosFrameEvent::Frame(_)))
                | Some(Ok(MacosFrameEvent::Lifecycle(_)))
                | Some(Ok(MacosFrameEvent::RecoverableError(_))) => {}
                Some(Err(_)) => {
                    return Err("capture failed before capability resolution".to_owned());
                }
                None => return Err("timed out before capability resolution".to_owned()),
            }
        }
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        session
            .capture_screenshot_reference(move |result| {
                let _ = result_tx.send(result);
            })
            .map_err(|_| "Tahoe screenshot transaction could not start".to_owned())?;
        let references = result_rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "timed out waiting for Tahoe screenshot output".to_owned())?
            .map_err(|_| "Tahoe screenshot transaction failed".to_owned())?;
        match references {
            MacosScreenshotReferenceSet::Sdr { image } => {
                print_metadata("sdr", &image);
                print_reference_output(
                    "sdr",
                    &image,
                    MacosScreenshotPreferredDynamicRange::Standard,
                )?;
                write_if_requested("SDR", &image, options.sdr_output.as_deref())?;
            }
            MacosScreenshotReferenceSet::Paired { sdr, hdr } => {
                print_metadata("sdr", &sdr);
                print_metadata("hdr", &hdr);
                print_reference_output(
                    "sdr",
                    &sdr,
                    MacosScreenshotPreferredDynamicRange::Standard,
                )?;
                print_reference_output("hdr", &hdr, MacosScreenshotPreferredDynamicRange::High)?;
                write_if_requested("SDR", &sdr, options.sdr_output.as_deref())?;
                write_if_requested("HDR", &hdr, options.hdr_output.as_deref())?;
            }
        }
        Ok(())
    })();
    session.stop();
    result
}

#[cfg(target_os = "macos")]
fn capability_name(
    capability: &hypercolor_macos_capture::MacosScreenshotReferenceCapability,
) -> &'static str {
    use hypercolor_macos_capture::MacosScreenshotReferenceCapability;

    match capability {
        MacosScreenshotReferenceCapability::PendingFirstFrame => "pending_first_frame",
        MacosScreenshotReferenceCapability::SdrOnly { .. } => "sdr_only",
        MacosScreenshotReferenceCapability::PairedSdrHdr { .. } => "paired_sdr_hdr",
    }
}

#[cfg(target_os = "macos")]
fn print_metadata(label: &str, image: &hypercolor_macos_capture::MacosScreenshotReferenceImage) {
    let metadata = image.metadata();
    println!(
        "{label}: {}x{} color_space={} range={:?} bits={}x{} row_bytes={} headroom={:?} average_light={:?}",
        metadata.extent.width,
        metadata.extent.height,
        metadata.color_space,
        metadata.dynamic_range,
        metadata.bits_per_component,
        metadata.bits_per_pixel,
        metadata.bytes_per_row,
        metadata.content_headroom,
        metadata.content_average_light_level,
    );
}

#[cfg(target_os = "macos")]
fn print_reference_output(
    label: &str,
    image: &hypercolor_macos_capture::MacosScreenshotReferenceImage,
    preferred_dynamic_range: hypercolor_macos_capture::MacosScreenshotPreferredDynamicRange,
) -> Result<(), String> {
    let reference = image
        .copy_reference_rgba8(preferred_dynamic_range)
        .map_err(|_| format!("could not create the {label} Core Graphics reference output"))?;
    println!(
        "{label} reference_output: {}x{} row_bytes={} rgba8_bytes={}",
        reference.extent.width,
        reference.extent.height,
        reference.bytes_per_row,
        reference.rgba8.len(),
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn write_if_requested(
    label: &str,
    image: &hypercolor_macos_capture::MacosScreenshotReferenceImage,
    path: Option<&std::path::Path>,
) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    eprintln!("PRIVACY WARNING: writing {label} captured screen pixels to an explicit destination");
    image
        .encode_png(path)
        .map_err(|_| format!("could not encode the {label} reference PNG"))
}
