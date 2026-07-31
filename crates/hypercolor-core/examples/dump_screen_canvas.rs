//! Dump the screen-capture canvas that screen-reactive effects actually see.
//!
//! Runs the real pipeline — platform capture backend into `ScreenCaptureInput`
//! — and writes the published `canvas_downscale` surface to PNG. That surface
//! is what `screen_cast` samples and what ambilight's sector grid is computed
//! from, so it is the honest answer to "why does the screen look wrong".
//!
//! ```text
//! cargo run --example dump_screen_canvas -p hypercolor-core -- out.png
//! ```

fn main() {
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("this example currently drives the Windows capture backend only");
        std::process::exit(1);
    }

    #[cfg(target_os = "windows")]
    {
        use std::time::{Duration, Instant};

        use hypercolor_core::input::screen::{CaptureConfig, ScreenCaptureInput};
        use hypercolor_core::input::{InputData, InputSource};
        use hypercolor_windows_capture::DesktopDuplicator;

        let path = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "screen-canvas.png".to_owned());

        let requested_extent = hypercolor_windows_capture::CaptureExtent::try_new(1280, 720)
            .expect("dump extent is non-empty");
        let mut duplicator = match DesktopDuplicator::new(0, requested_extent) {
            Ok(duplicator) => duplicator,
            Err(error) => {
                eprintln!("failed to open capture: {error}");
                std::process::exit(1);
            }
        };
        let mut analyzer = ScreenCaptureInput::new(CaptureConfig::default());
        if let Err(error) = analyzer.start() {
            eprintln!("failed to start analyzer: {error}");
            std::process::exit(1);
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match duplicator.next_frame(Duration::from_millis(200)) {
                Ok(Some(frame)) => {
                    println!("capture backend produced {}x{}", frame.width, frame.height);
                    if let Err(error) = analyzer.push_frame(frame.rgba(), frame.width, frame.height)
                    {
                        eprintln!("screen analysis resources exhausted: {error}");
                        std::process::exit(1);
                    }
                }
                Ok(None) => continue,
                Err(error) => {
                    eprintln!("capture failed: {error}");
                    std::process::exit(1);
                }
            }

            let Ok(InputData::Screen(data)) = analyzer.sample() else {
                continue;
            };
            let Some(surface) = data.canvas_downscale.as_ref() else {
                println!("no canvas_downscale published yet");
                continue;
            };

            let descriptor = surface.descriptor();
            let canvas = hypercolor_core::types::canvas::Canvas::from_published_surface(surface);
            let bytes = canvas.as_rgba_bytes().to_vec();
            println!(
                "canvas_downscale: {}x{} ({} bytes), source was {}x{}",
                descriptor.width,
                descriptor.height,
                bytes.len(),
                data.source_width,
                data.source_height
            );
            println!(
                "letterbox bars (grid units, t/b/l/r): {:?} of a {}x{} grid",
                data.letterbox, data.grid_width, data.grid_height
            );
            println!("zone colors published: {}", data.zone_colors.len());

            let Some(buffer) =
                image::RgbaImage::from_raw(descriptor.width, descriptor.height, bytes)
            else {
                eprintln!("canvas bytes did not match the descriptor");
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

        eprintln!("no frame arrived within the budget");
        std::process::exit(1);
    }
}
