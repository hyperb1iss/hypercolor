//! Wayland screen capture source powered by XDG Desktop Portal + PipeWire.
//!
//! This source keeps the portal session and PipeWire stream on a dedicated
//! worker thread. The render loop only clones the latest processed
//! [`ScreenData`] snapshot, while capture demand is toggled at runtime by the
//! daemon depending on the active effect.

use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, anyhow};
use ashpd::desktop::{
    PersistMode, Session,
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream},
};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use tracing::{debug, info, warn};

use crate::input::screen::{CaptureConfig, ScreenCaptureInput};
use crate::input::traits::{InputData, InputSource, ScreenData};

const DEFAULT_CAPTURE_WIDTH: u32 = 1280;
const DEFAULT_CAPTURE_HEIGHT: u32 = 720;

/// Wayland-only live screen capture input source.
pub struct WaylandScreenCaptureInput {
    config: CaptureConfig,
    running: bool,
    capture_active: bool,
    latest_snapshot: Arc<Mutex<Option<ScreenData>>>,
    worker: Option<WaylandCaptureWorker>,
}

impl WaylandScreenCaptureInput {
    /// Create a new Wayland screen capture source.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            config,
            running: false,
            capture_active: false,
            latest_snapshot: Arc::new(Mutex::new(None)),
            worker: None,
        }
    }

    fn set_capture_active_state(&mut self, active: bool) -> anyhow::Result<()> {
        if self.capture_active == active {
            if active && self.running && self.worker.is_none() {
                self.spawn_worker()?;
            }
            return Ok(());
        }

        self.capture_active = active;

        if !self.running {
            return Ok(());
        }

        if active {
            self.spawn_worker()?;
            self.send_worker_command(WorkerCommand::SetActive(true))?;
        } else {
            self.send_worker_command(WorkerCommand::SetActive(false))?;
        }

        Ok(())
    }

    fn spawn_worker(&mut self) -> anyhow::Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }

        let latest_snapshot = Arc::clone(&self.latest_snapshot);
        let config = self.config.clone();
        let (command_tx, command_rx) = pw::channel::channel();
        let join_handle = thread::Builder::new()
            .name("hypercolor-screen-capture".to_owned())
            .spawn(move || {
                run_capture_worker(config, latest_snapshot, command_rx);
            })
            .context("failed to spawn Wayland screen capture worker")?;

        self.worker = Some(WaylandCaptureWorker {
            command_tx,
            join_handle,
        });
        Ok(())
    }

    fn send_worker_command(&mut self, command: WorkerCommand) -> anyhow::Result<()> {
        let Some(worker) = &self.worker else {
            return Ok(());
        };

        if worker.command_tx.send(command.clone()).is_ok() {
            return Ok(());
        }

        warn!("Wayland screen capture worker is no longer accepting commands");
        self.join_dead_worker();

        if matches!(command, WorkerCommand::SetActive(true)) {
            self.spawn_worker()?;
            if let Some(worker) = &self.worker {
                worker
                    .command_tx
                    .send(command)
                    .map_err(|_| anyhow!("failed to restart Wayland screen capture worker"))?;
            }
        }

        Ok(())
    }

    fn join_dead_worker(&mut self) {
        if let Some(worker) = self.worker.take()
            && let Err(panic) = worker.join_handle.join()
        {
            warn!(message = ?panic, "Wayland screen capture worker panicked");
        }
    }
}

impl InputSource for WaylandScreenCaptureInput {
    fn name(&self) -> &str {
        "wayland_screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }

        self.running = true;
        if self.capture_active {
            self.spawn_worker()?;
            self.send_worker_command(WorkerCommand::SetActive(true))?;
        } else {
            debug!(
                "Wayland screen capture armed but idle until a screen-reactive effect requests capture"
            );
        }

        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.capture_active = false;

        if let Some(worker) = &self.worker {
            let _ = worker.command_tx.send(WorkerCommand::Stop);
        }
        self.join_dead_worker();

        if let Ok(mut latest) = self.latest_snapshot.lock() {
            *latest = None;
        }
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if !self.running || !self.capture_active {
            return Ok(InputData::None);
        }

        let latest = self
            .latest_snapshot
            .lock()
            .map_err(|_| anyhow!("wayland screen capture snapshot mutex poisoned"))?;

        Ok(latest.clone().map_or(InputData::None, InputData::Screen))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn set_screen_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.set_capture_active_state(active)
    }
}

struct WaylandCaptureWorker {
    command_tx: pw::channel::Sender<WorkerCommand>,
    join_handle: thread::JoinHandle<()>,
}

#[derive(Clone, Debug)]
enum WorkerCommand {
    SetActive(bool),
    Stop,
}

struct PortalCaptureSession {
    session: Session<Screencast>,
    stream: Stream,
    fd: OwnedFd,
}

struct WaylandCaptureUserData {
    analyzer: ScreenCaptureInput,
    format: spa::param::video::VideoInfoRaw,
    latest_snapshot: Arc<Mutex<Option<ScreenData>>>,
    rgba_frame: Vec<u8>,
}

impl WaylandCaptureUserData {
    fn new(config: CaptureConfig, latest_snapshot: Arc<Mutex<Option<ScreenData>>>) -> Self {
        let mut analyzer = ScreenCaptureInput::new(config);
        let _ = analyzer.start();

        Self {
            analyzer,
            format: Default::default(),
            latest_snapshot,
            rgba_frame: Vec::new(),
        }
    }
}

fn run_capture_worker(
    config: CaptureConfig,
    latest_snapshot: Arc<Mutex<Option<ScreenData>>>,
    command_rx: pw::channel::Receiver<WorkerCommand>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            warn!(%error, "Failed to create Wayland capture runtime");
            return;
        }
    };

    let portal = match runtime.block_on(open_portal_session(&config)) {
        Ok(portal) => portal,
        Err(error) => {
            warn!(%error, "Failed to establish Wayland screencast session");
            return;
        }
    };

    let session = match run_pipewire_loop(&config, Arc::clone(&latest_snapshot), portal, command_rx)
    {
        Ok(session) => session,
        Err(error) => {
            warn!(%error, "Wayland screen capture loop exited with an error");
            return;
        }
    };

    if let Err(error) = runtime.block_on(session.close()) {
        warn!(%error, "Wayland screen capture loop exited with an error");
    }
}

async fn open_portal_session(config: &CaptureConfig) -> anyhow::Result<PortalCaptureSession> {
    let proxy = Screencast::new()
        .await
        .context("failed to connect to xdg-desktop-portal screencast interface")?;
    let session = proxy
        .create_session(Default::default())
        .await
        .context("failed to create screencast portal session")?;

    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Hidden)
                .set_sources(Some(SourceType::Monitor.into()))
                .set_multiple(false)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .context("failed to open screencast source picker")?;

    let response = proxy
        .start(&session, None, Default::default())
        .await
        .context("failed to start screencast portal session")?
        .response()
        .context("screen capture request was denied or cancelled")?;
    let stream = response
        .streams()
        .first()
        .cloned()
        .context("portal did not return a monitor stream")?;
    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .context("failed to open PipeWire remote for screencast session")?;

    info!(
        pipewire_node = stream.pipe_wire_node_id(),
        stream = ?stream,
        monitor = ?config.monitor,
        "Wayland screencast session established"
    );

    Ok(PortalCaptureSession {
        session,
        stream,
        fd,
    })
}

fn run_pipewire_loop(
    config: &CaptureConfig,
    latest_snapshot: Arc<Mutex<Option<ScreenData>>>,
    portal: PortalCaptureSession,
    command_rx: pw::channel::Receiver<WorkerCommand>,
) -> anyhow::Result<Session<Screencast>> {
    pw::init();

    let mainloop =
        pw::main_loop::MainLoopRc::new(None).context("failed to create PipeWire main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .context("failed to create PipeWire context")?;
    let core = context
        .connect_fd_rc(portal.fd, None)
        .context("failed to connect to screencast PipeWire remote")?;

    let stream = pw::stream::StreamRc::new(
        core,
        "hypercolor-screen-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .context("failed to create PipeWire capture stream")?;

    let _listener = stream
        .add_local_listener_with_user_data(WaylandCaptureUserData::new(
            config.clone(),
            latest_snapshot,
        ))
        .state_changed(|_, _, old, new| {
            debug!(?old, ?new, "Wayland screen capture stream state changed");
        })
        .param_changed(|_, user_data, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }

            let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != spa::param::format::MediaType::Video
                || media_subtype != spa::param::format::MediaSubtype::Raw
            {
                return;
            }

            if user_data.format.parse(param).is_err() {
                warn!("Failed to parse negotiated PipeWire video format");
                return;
            }

            let format = user_data.format.format();
            let size = user_data.format.size();
            if supports_video_format(format) {
                info!(
                    ?format,
                    width = size.width,
                    height = size.height,
                    "Negotiated Wayland screen capture format"
                );
            } else {
                warn!(
                    ?format,
                    width = size.width,
                    height = size.height,
                    "Negotiated unsupported Wayland screen capture format"
                );
            }
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };

            let size = user_data.format.size();
            if size.width == 0 || size.height == 0 {
                return;
            }

            let format = user_data.format.format();
            if !supports_video_format(format) {
                return;
            }

            if !copy_frame_to_rgba(
                data,
                format,
                size.width,
                size.height,
                &mut user_data.rgba_frame,
            ) {
                return;
            }

            user_data
                .analyzer
                .push_frame(&user_data.rgba_frame, size.width, size.height);

            let Ok(InputData::Screen(snapshot)) = user_data.analyzer.sample() else {
                return;
            };

            if let Ok(mut latest) = user_data.latest_snapshot.lock() {
                *latest = Some(snapshot);
            }
        })
        .register()
        .context("failed to register PipeWire screen capture listener")?;

    let format_bytes = build_format_params(config.target_fps.max(1))?;
    let mut params = [spa::pod::Pod::from_bytes(&format_bytes)
        .context("failed to deserialize PipeWire format pod")?];

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(portal.stream.pipe_wire_node_id()),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .context("failed to connect PipeWire screen capture stream")?;

    let _command_rx = command_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        let stream = stream.clone();
        move |command| match command {
            WorkerCommand::SetActive(active) => {
                if let Err(error) = stream.set_active(active) {
                    warn!(active, %error, "Failed to update PipeWire stream active state");
                }
            }
            WorkerCommand::Stop => mainloop.quit(),
        }
    });

    mainloop.run();

    if let Err(error) = stream.disconnect() {
        debug!(%error, "PipeWire screen capture stream disconnect reported an error");
    }

    Ok(portal.session)
}

fn build_format_params(target_fps: u32) -> anyhow::Result<Vec<u8>> {
    let fps = target_fps;
    let object = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::RGBA,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::ARGB,
            spa::param::video::VideoFormat::ABGR,
            spa::param::video::VideoFormat::xRGB,
            spa::param::video::VideoFormat::xBGR,
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: DEFAULT_CAPTURE_WIDTH,
                height: DEFAULT_CAPTURE_HEIGHT,
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1,
            },
            spa::utils::Rectangle {
                width: 4096,
                height: 4096,
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: fps, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 1000,
                denom: 1,
            }
        ),
    );

    Ok(spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )?
    .0
    .into_inner())
}

fn supports_video_format(format: spa::param::video::VideoFormat) -> bool {
    matches!(
        format,
        spa::param::video::VideoFormat::RGBA
            | spa::param::video::VideoFormat::BGRA
            | spa::param::video::VideoFormat::RGBx
            | spa::param::video::VideoFormat::BGRx
            | spa::param::video::VideoFormat::ARGB
            | spa::param::video::VideoFormat::ABGR
            | spa::param::video::VideoFormat::xRGB
            | spa::param::video::VideoFormat::xBGR
            | spa::param::video::VideoFormat::RGB
            | spa::param::video::VideoFormat::BGR
    )
}

fn copy_frame_to_rgba(
    data: &mut spa::buffer::Data,
    format: spa::param::video::VideoFormat,
    width: u32,
    height: u32,
    rgba: &mut Vec<u8>,
) -> bool {
    let (offset, stride) = {
        let chunk = data.chunk();
        let offset = usize::try_from(chunk.offset()).ok();
        let stride = if chunk.stride() > 0 {
            usize::try_from(chunk.stride()).ok()
        } else {
            None
        };
        let Some(offset) = offset else {
            return false;
        };
        (offset, stride)
    };

    let Some(mapped) = data.data() else {
        return false;
    };

    let width_usize = usize::try_from(width).ok();
    let height_usize = usize::try_from(height).ok();
    let Some(width_usize) = width_usize else {
        return false;
    };
    let Some(height_usize) = height_usize else {
        return false;
    };

    let bytes_per_pixel = bytes_per_pixel(format);
    let row_bytes = width_usize.checked_mul(bytes_per_pixel);
    let Some(row_bytes) = row_bytes else {
        return false;
    };

    let stride = if let Some(stride) = stride {
        Some(stride).filter(|stride| *stride >= row_bytes)
    } else {
        Some(row_bytes)
    };
    let Some(stride) = stride else {
        return false;
    };

    let row_span = stride.checked_mul(height_usize.saturating_sub(1));
    let Some(row_span) = row_span else {
        return false;
    };
    let required = offset.checked_add(row_span);
    let required = required.and_then(|base| base.checked_add(row_bytes));
    let Some(required) = required else {
        return false;
    };
    if mapped.len() < required {
        return false;
    }

    let total_rgba_bytes = width_usize
        .checked_mul(height_usize)
        .and_then(|pixels| pixels.checked_mul(4));
    let Some(total_rgba_bytes) = total_rgba_bytes else {
        return false;
    };
    rgba.resize(total_rgba_bytes, 0);

    for row in 0..height_usize {
        let src_start = offset + row * stride;
        let src_end = src_start + row_bytes;
        let dst_start = row * width_usize * 4;
        let dst_end = dst_start + width_usize * 4;
        let src_row = &mapped[src_start..src_end];
        let dst_row = &mut rgba[dst_start..dst_end];
        convert_row_to_rgba(src_row, dst_row, format);
    }

    true
}

fn bytes_per_pixel(format: spa::param::video::VideoFormat) -> usize {
    match format {
        spa::param::video::VideoFormat::RGB | spa::param::video::VideoFormat::BGR => 3,
        _ => 4,
    }
}

fn convert_row_to_rgba(src: &[u8], dst: &mut [u8], format: spa::param::video::VideoFormat) {
    match format {
        spa::param::video::VideoFormat::RGBA => {
            dst.copy_from_slice(src);
        }
        spa::param::video::VideoFormat::BGRA => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[2];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[0];
                dst_px[3] = src_px[3];
            }
        }
        spa::param::video::VideoFormat::RGBx => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[0];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[2];
                dst_px[3] = 255;
            }
        }
        spa::param::video::VideoFormat::BGRx => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[2];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[0];
                dst_px[3] = 255;
            }
        }
        spa::param::video::VideoFormat::ARGB => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[1];
                dst_px[1] = src_px[2];
                dst_px[2] = src_px[3];
                dst_px[3] = src_px[0];
            }
        }
        spa::param::video::VideoFormat::ABGR => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[3];
                dst_px[1] = src_px[2];
                dst_px[2] = src_px[1];
                dst_px[3] = src_px[0];
            }
        }
        spa::param::video::VideoFormat::xRGB => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[1];
                dst_px[1] = src_px[2];
                dst_px[2] = src_px[3];
                dst_px[3] = 255;
            }
        }
        spa::param::video::VideoFormat::xBGR => {
            for (src_px, dst_px) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[3];
                dst_px[1] = src_px[2];
                dst_px[2] = src_px[1];
                dst_px[3] = 255;
            }
        }
        spa::param::video::VideoFormat::RGB => {
            for (src_px, dst_px) in src.chunks_exact(3).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[0];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[2];
                dst_px[3] = 255;
            }
        }
        spa::param::video::VideoFormat::BGR => {
            for (src_px, dst_px) in src.chunks_exact(3).zip(dst.chunks_exact_mut(4)) {
                dst_px[0] = src_px[2];
                dst_px[1] = src_px[1];
                dst_px[2] = src_px[0];
                dst_px[3] = 255;
            }
        }
        _ => {}
    }
}
