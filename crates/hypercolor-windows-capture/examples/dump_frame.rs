//! Dump a captured desktop frame to PNG, for eyeballing the readback.
//!
//! Ambient lighting reduces frames to a handful of averaged colors, which
//! hides exactly the bugs that matter most here: a wrong row pitch, swapped
//! channels, or an off-by-one stride all still produce plausible-looking
//! zone colors. Writing the frame out makes those instantly obvious.
//!
//! ```text
//! cargo run --example dump_frame -p hypercolor-windows-capture -- out.png [source] [max_width] [max_height]
//! ```

fn main() {
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("desktop capture is Windows-only");
        std::process::exit(1);
    }

    #[cfg(target_os = "windows")]
    {
        use std::time::{Duration, Instant};

        use hypercolor_windows_capture::{CaptureExtent, DesktopDuplicator, MonitorSelector};

        let mut args = std::env::args().skip(1);
        let path = args.next().unwrap_or_else(|| "capture.png".to_owned());
        let source = args.next().unwrap_or_else(|| "auto".to_owned());
        let max_width: u32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(1280);
        let max_height: u32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(720);
        let requested_extent = match CaptureExtent::try_new(max_width, max_height) {
            Ok(extent) => extent,
            Err(error) => {
                eprintln!("invalid capture extent: {error}");
                std::process::exit(2);
            }
        };

        let mut duplicator =
            match DesktopDuplicator::open(MonitorSelector::parse(&source), requested_extent) {
                Ok(duplicator) => duplicator,
                Err(error) => {
                    eprintln!("failed to open capture: {error}");
                    std::process::exit(1);
                }
            };

        let (native_width, native_height) = duplicator.native_extent();
        println!(
            "source {} at topology generation {}: {native_width}x{native_height}",
            duplicator.source_id(),
            duplicator.topology_generation()
        );

        // The desktop only produces frames when something changes, so keep
        // asking until one lands rather than giving up on the first timeout.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match duplicator.next_frame(Duration::from_millis(200)) {
                Ok(Some(frame)) => {
                    println!(
                        "captured {}x{} ({} bytes)",
                        frame.width,
                        frame.height,
                        frame.rgba().len()
                    );
                    let Some(buffer) = image::RgbaImage::from_raw(
                        frame.width,
                        frame.height,
                        frame.rgba().to_vec(),
                    ) else {
                        eprintln!("frame buffer did not match its declared dimensions");
                        std::process::exit(1);
                    };
                    match buffer.save(&path) {
                        Ok(()) => println!("wrote {path}"),
                        Err(error) => {
                            eprintln!("failed to write {path}: {error}");
                            std::process::exit(1);
                        }
                    }
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("capture failed: {error}");
                    std::process::exit(1);
                }
            }
        }

        eprintln!("no frame arrived within the budget; is the desktop completely static?");
        std::process::exit(1);
    }
}
