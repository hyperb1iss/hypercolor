//! Run Hypercolor's native rainbow through the blocksd bridge for 15 seconds.
//!
//! Requires blocksd with renderer support. This opens only its Unix socket;
//! run without another Hypercolor instance writing to the same ROLI devices.

#[cfg(unix)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    use hypercolor_core::device::{BlocksBackend, BlocksScanner};
    use hypercolor_core::effect::builtin::create_builtin_renderer;
    use hypercolor_core::effect::{FrameDataSources, FrameInput};
    use hypercolor_core::input::InteractionData;
    use hypercolor_driver_api::DeviceBackend;
    use hypercolor_types::audio::AudioData;
    use hypercolor_types::canvas::Canvas;
    use hypercolor_types::device::DeviceTopologyHint;
    use hypercolor_types::sensor::SystemSnapshot;

    let socket = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(BlocksBackend::default_socket_path);
    let mut scanner = BlocksScanner::new(socket.clone());
    let devices = scanner.scan().await?;
    anyhow::ensure!(
        !devices.is_empty(),
        "no supported lighting surfaces discovered"
    );
    let backend = BlocksBackend::new(socket);
    let mut surfaces = Vec::new();
    for device in &devices {
        let segment = &device.info.segments[0];
        let (width, height) = match segment.topology {
            DeviceTopologyHint::Matrix { rows, cols } => (cols, rows),
            DeviceTopologyHint::Strip => (segment.led_count, 1),
            _ => anyhow::bail!("unexpected ROLI lighting topology"),
        };
        backend.adopt_device(device)?;
        backend.connect(&device.info.id).await?;
        println!("{}: {} colors", device.info.name, segment.led_count);
        surfaces.push((device.info.id, Canvas::new(width, height)));
    }

    let mut renderer = create_builtin_renderer("rainbow")
        .ok_or_else(|| anyhow::anyhow!("native rainbow renderer unavailable"))?;
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::default();
    let start = Instant::now();
    let mut previous = start;
    let mut frame_number = 0;
    let mut cadence = tokio::time::interval(Duration::from_millis(40));
    cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    while start.elapsed() < Duration::from_secs(15) {
        cadence.tick().await;
        let now = Instant::now();
        for (id, canvas) in &mut surfaces {
            let input = FrameInput {
                time_secs: now.duration_since(start).as_secs_f64(),
                delta_secs: now.duration_since(previous).as_secs_f32(),
                frame_number,
                audio: &audio,
                interaction: &interaction,
                screen: None,
                sensors: &sensors,
                sources: FrameDataSources::default(),
                canvas_width: canvas.width(),
                canvas_height: canvas.height(),
            };
            renderer.render_into(&input, canvas)?;
            let colors: Vec<[u8; 3]> = canvas
                .as_rgba_bytes()
                .chunks_exact(4)
                .map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect();
            backend.write_colors(id, &colors).await?;
        }
        previous = now;
        frame_number += 1;
    }
    println!("Submitted {frame_number} native rainbow frames per device");
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("the blocksd bridge requires a Unix socket");
    std::process::exit(1);
}
