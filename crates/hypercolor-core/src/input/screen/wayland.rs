//! Wayland screen capture source powered by XDG Desktop Portal + PipeWire.
//!
//! This source keeps the portal session and PipeWire stream on a dedicated
//! worker thread. The render loop only clones the latest processed
//! [`ScreenData`] snapshot, while capture demand is toggled at runtime by the
//! daemon depending on the active effect.

use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use ashpd::desktop::{
    CreateSessionOptions, PersistMode, Session,
    screencast::{
        CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
        StartCastOptions, Stream,
    },
};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use tracing::{debug, info, warn};

use crate::input::screen::{
    CaptureColorSpace, CaptureConfig, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame,
    CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStorage, CaptureTransferFunction, CpuCaptureStorage, LegacyScreenSnapshot,
    PhysicalOrigin, PixelExtent, RawCaptureSurface, ScreenCaptureInput, SourceScale,
    analyze_legacy_screen_frame,
};
use crate::input::traits::{InputData, InputSource};
use crate::input::worker_retention::retain_input_worker;
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceStatusHandle, SourceStatusReporter,
};

const DEFAULT_CAPTURE_WIDTH: u32 = 1280;
const DEFAULT_CAPTURE_HEIGHT: u32 = 720;
const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(1);
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(1);

/// Callback invoked when the portal hands back a new restore token (or the
/// token is cleared before a re-pick). The daemon persists it to config so
/// the picked source survives restarts without re-prompting.
pub type RestoreTokenSink = Arc<dyn Fn(Option<String>) + Send + Sync>;

/// Settings shared between the input source handle and the capture worker.
///
/// The config lives behind a mutex while the generation counter is atomic:
/// the worker polls the counter once per frame and only takes the lock when
/// a reconfiguration actually happened.
struct SharedSettings {
    config: Mutex<CaptureConfig>,
    generation: AtomicU64,
    frame_generation: AtomicU64,
    session_generation: AtomicU64,
}

#[derive(Clone)]
struct CapturedScreenSnapshot {
    legacy: LegacyScreenSnapshot,
    generation: u64,
}

impl SharedSettings {
    fn snapshot(&self) -> CaptureConfig {
        self.config
            .lock()
            .map(|config| config.clone())
            .unwrap_or_default()
    }
}

/// Wayland-only live screen capture input source.
pub struct WaylandScreenCaptureInput {
    settings: Arc<SharedSettings>,
    running: bool,
    capture_active: bool,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    status_snapshot_generation: u64,
    worker: Option<WaylandCaptureWorker>,
    retiring_workers: Vec<WaylandCaptureWorker>,
    token_sink: Option<RestoreTokenSink>,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
}

impl WaylandScreenCaptureInput {
    /// Create a new Wayland screen capture source.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            settings: Arc::new(SharedSettings {
                config: Mutex::new(config),
                generation: AtomicU64::new(0),
                frame_generation: AtomicU64::new(0),
                session_generation: AtomicU64::new(0),
            }),
            running: false,
            capture_active: false,
            latest_snapshot: Arc::new(Mutex::new(None)),
            status_snapshot_generation: 0,
            worker: None,
            retiring_workers: Vec::new(),
            token_sink: None,
            status: SourceStatusReporter::new(
                "wayland_screen_capture",
                SourceKind::Screen,
                "pipewire",
                true,
                true,
                false,
            ),
            status_session: SourceSessionSlot::new(),
        }
    }

    /// Attach a sink that persists portal restore tokens.
    #[must_use]
    pub fn with_restore_token_sink(mut self, sink: RestoreTokenSink) -> Self {
        self.token_sink = Some(sink);
        self
    }

    fn current_target_fps(&self) -> u32 {
        self.settings
            .config
            .lock()
            .map(|config| config.target_fps)
            .unwrap_or(30)
    }

    /// Apply new capture settings to the running pipeline.
    ///
    /// Analysis settings (grid, smoothing, letterbox, tuning) reach the
    /// worker without interruption. A target FPS change requires stream
    /// re-negotiation, so the worker restarts; with a restore token in
    /// place that restart is silent.
    fn reconfigure(&mut self, config: CaptureConfig) -> anyhow::Result<()> {
        let fps_changed = self.current_target_fps() != config.target_fps;

        if let Ok(mut current) = self.settings.config.lock() {
            // The worker may have written a freshly granted portal token
            // since the caller snapshotted its config; never let a stale
            // None overwrite it. Intentional clears go through
            // `reselect_source`.
            let granted_token = current.restore_token.take();
            *current = config;
            if current.restore_token.is_none() {
                current.restore_token = granted_token;
            }
        }
        self.settings.generation.fetch_add(1, Ordering::Release);

        if fps_changed && self.worker.is_some() {
            if self.portal_pending() {
                warn!(
                    "Portal source picker is open; new capture FPS applies on the next session restart"
                );
                return Ok(());
            }
            info!("Restarting Wayland capture worker for new target FPS");
            self.restart_worker()?;
        }

        Ok(())
    }

    /// Drop the persisted portal token and re-open the source picker.
    fn reselect_source(&mut self) -> anyhow::Result<()> {
        if self.portal_pending() {
            debug!("Portal source picker is already open; ignoring re-pick request");
            return Ok(());
        }

        if let Ok(mut current) = self.settings.config.lock() {
            current.restore_token = None;
        }
        if let Some(sink) = &self.token_sink {
            sink(None);
        }

        if !self.running {
            return Ok(());
        }

        info!("Re-opening Wayland screencast source picker");
        self.restart_worker()
    }

    fn portal_pending(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| worker.portal_pending.load(Ordering::SeqCst))
    }

    fn restart_worker(&mut self) -> anyhow::Result<()> {
        self.shutdown_worker();
        if self.running
            && self.capture_active
            && let Some(session) = self.status.begin_session()?
        {
            self.status_session.store(session);
        }
        self.spawn_worker()?;
        let active = self.capture_active;
        self.send_worker_command(WorkerCommand::SetActive(active))
    }

    fn set_capture_active_state(&mut self, active: bool) -> anyhow::Result<()> {
        if self.capture_active == active {
            if active && self.running && self.worker.is_none() {
                self.spawn_worker()?;
            }
            return Ok(());
        }

        if let Ok(mut latest) = self.latest_snapshot.lock() {
            *latest = None;
        }

        if !self.running {
            self.capture_active = active;
            return Ok(());
        }

        if active {
            self.spawn_worker()?;
            self.send_worker_command(WorkerCommand::SetActive(true))?;
        } else {
            self.send_worker_command(WorkerCommand::SetActive(false))?;
        }

        self.capture_active = active;
        Ok(())
    }

    fn spawn_worker(&mut self) -> anyhow::Result<()> {
        self.reap_workers(false);
        if self.worker.is_some() {
            return Ok(());
        }

        let latest_snapshot = Arc::clone(&self.latest_snapshot);
        let settings = Arc::clone(&self.settings);
        let token_sink = self.token_sink.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        // Born true: the worker is portal-bound from its first instruction,
        // and a shutdown landing before the thread even stores the flag must
        // detach rather than join into the picker freeze.
        let portal_pending = Arc::new(AtomicBool::new(true));
        let worker_flags = WorkerFlags {
            cancel: Arc::clone(&cancel),
            portal_pending: Arc::clone(&portal_pending),
        };
        let (command_tx, command_rx) = pw::channel::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        let status_session = self.status_session.clone();
        let session_generation = settings
            .session_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let join_handle = thread::Builder::new()
            .name("hypercolor-screen-capture".to_owned())
            .spawn(move || {
                let _ = ready_tx.send(());
                run_capture_worker(
                    settings,
                    latest_snapshot,
                    command_rx,
                    token_sink,
                    worker_flags,
                    status_session,
                    session_generation,
                );
                let _ = exit_tx.send(());
            })
            .context("failed to spawn Wayland screen capture worker")?;

        self.worker = Some(WaylandCaptureWorker {
            command_tx,
            exit_rx,
            join_handle: Some(join_handle),
            cancel,
            portal_pending,
        });
        if let Err(error) = ready_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            self.shutdown_worker();
            anyhow::bail!("Wayland screen capture worker readiness timed out: {error}");
        }
        if self.observe_worker_exit(true) {
            anyhow::bail!("Wayland screen capture worker exited during startup");
        }
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
        self.shutdown_worker();

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

    fn shutdown_worker(&mut self) {
        let Some(mut worker) = self.worker.take() else {
            return;
        };

        worker.cancel.store(true, Ordering::SeqCst);
        let _ = worker.command_tx.send(WorkerCommand::Stop);

        if !worker.portal_pending.load(Ordering::SeqCst) {
            let _ = worker.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT);
        }
        if worker.is_finished() {
            worker.join(None);
        } else {
            debug!("Retaining Wayland capture worker until the portal request terminates");
            self.retiring_workers.push(worker);
        }
    }

    fn observe_worker_exit(&mut self, publish_failure: bool) -> bool {
        let Some(worker) = self.worker.as_ref() else {
            self.reap_workers(false);
            return false;
        };
        if !worker.is_finished() {
            self.reap_workers(false);
            return false;
        }
        let worker = self.worker.take().expect("finished worker remains owned");
        worker.join(publish_failure.then(|| self.status.session()).flatten());
        if let Ok(mut latest) = self.latest_snapshot.lock() {
            *latest = None;
        }
        self.reap_workers(false);
        true
    }

    fn reap_workers(&mut self, wait: bool) {
        let mut retained = Vec::with_capacity(self.retiring_workers.len());
        for mut worker in self.retiring_workers.drain(..) {
            if wait && !worker.portal_pending.load(Ordering::SeqCst) {
                let _ = worker.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT);
            }
            if worker.is_finished() {
                worker.join(None);
            } else {
                retained.push(worker);
            }
        }
        self.retiring_workers = retained;
    }
}

impl InputSource for WaylandScreenCaptureInput {
    fn name(&self) -> &'static str {
        "wayland_screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }

        if self.capture_active {
            if let Some(session) = self.status.begin_session()? {
                self.status_session.store(session);
            }
            if let Err(error) = self
                .spawn_worker()
                .and_then(|()| self.send_worker_command(WorkerCommand::SetActive(true)))
            {
                self.status_session.clear();
                self.status.stop();
                self.shutdown_worker();
                return Err(error);
            }
        } else {
            debug!(
                "Wayland screen capture armed but idle until a screen-reactive effect requests capture"
            );
        }

        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status_session.clear();
        self.status.stop();
        self.running = false;
        self.capture_active = false;
        self.shutdown_worker();
        self.reap_workers(true);

        if let Ok(mut latest) = self.latest_snapshot.lock() {
            *latest = None;
        }
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.observe_worker_exit(self.running && self.capture_active);
        if !self.running || !self.capture_active {
            return Ok(InputData::None);
        }

        let latest = self
            .latest_snapshot
            .lock()
            .map_err(|_| anyhow!("wayland screen capture snapshot mutex poisoned"))?;

        let snapshot = latest.clone();
        drop(latest);
        let Some(snapshot) = snapshot else {
            return Ok(InputData::None);
        };
        let metadata = snapshot.legacy.frame().metadata();
        let expected = CaptureEpoch {
            source_id: metadata.source_id.clone(),
            topology_generation: metadata.topology_generation,
            session_generation: self.settings.session_generation.load(Ordering::Acquire),
        };
        if snapshot.legacy.frame().validate_epoch(&expected).is_err() {
            return Ok(InputData::None);
        }
        if snapshot.generation != self.status_snapshot_generation {
            if let Some(status) = self.status.session() {
                let frame_period =
                    Duration::from_secs_f64(1.0 / f64::from(self.current_target_fps().max(1)));
                status.record_sample(
                    metadata.captured_at,
                    metadata.captured_at + frame_period + frame_period,
                    1,
                )?;
            }
            self.status_snapshot_generation = snapshot.generation;
        }
        Ok(InputData::Screen(snapshot.legacy.data().clone()))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn set_screen_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        let previous = self.capture_active;
        self.status.set_policy(true, true, active)?;
        if previous != active {
            if !active {
                self.status_session.clear();
            }
            if active && self.running {
                if let Some(session) = self.status.begin_session()? {
                    self.status_session.store(session);
                }
            }
        }
        if let Err(error) = self.set_capture_active_state(active) {
            self.status_session.clear();
            self.status.stop();
            self.status.set_policy(true, true, previous)?;
            if previous
                && self.running
                && let Some(session) = self.status.begin_session()?
            {
                self.status_session.store(session);
            }
            return Err(error);
        }
        Ok(())
    }

    fn reconfigure_screen_capture(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        self.reconfigure(config.clone())
    }

    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        self.reselect_source()
    }
}

struct WaylandCaptureWorker {
    command_tx: pw::channel::Sender<WorkerCommand>,
    exit_rx: mpsc::Receiver<()>,
    join_handle: Option<thread::JoinHandle<()>>,
    /// Tells the worker to exit at its next checkpoint without touching
    /// shared state (snapshot, settings, restore token).
    cancel: Arc<AtomicBool>,
    /// True while the worker is awaiting the portal source picker — the
    /// phase during which it cannot see commands and must not be joined.
    portal_pending: Arc<AtomicBool>,
}

impl WaylandCaptureWorker {
    fn is_finished(&self) -> bool {
        self.join_handle
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }

    fn join(mut self, failure_status: Option<crate::input::SourceSessionWriter>) {
        let Some(join_handle) = self.join_handle.take() else {
            return;
        };
        let failure = join_handle.join().err();
        if let Some(status) = failure_status {
            let reason = failure.map_or_else(
                || "Wayland screen capture worker exited unexpectedly".to_owned(),
                |panic| format!("Wayland screen capture worker panicked: {panic:?}"),
            );
            status.failed(SourceIssue::new(
                "wayland_screen_worker_exited",
                reason,
                true,
            ));
        } else if let Some(panic) = failure {
            warn!(message = ?panic, "Wayland screen capture worker panicked");
        }
    }
}

impl Drop for WaylandCaptureWorker {
    fn drop(&mut self) {
        let Some(join_handle) = self.join_handle.take() else {
            return;
        };
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.command_tx.send(WorkerCommand::Stop);
        if join_handle.is_finished() {
            let _ = join_handle.join();
            return;
        }
        retain_input_worker(
            join_handle,
            "hypercolor-wayland-capture-reaper",
            "Wayland capture worker",
        );
    }
}

/// Cancellation and phase flags shared with a capture worker thread.
struct WorkerFlags {
    cancel: Arc<AtomicBool>,
    portal_pending: Arc<AtomicBool>,
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

#[derive(Clone)]
struct WaylandSourceMetadata {
    source_id: CaptureSourceId,
    origin: PhysicalOrigin,
    logical_width: Option<u32>,
    session_generation: u64,
}

impl WaylandSourceMetadata {
    fn from_stream(stream: &Stream, session_generation: u64) -> anyhow::Result<Self> {
        let source_name = stream
            .id()
            .or_else(|| stream.mapping_id())
            .unwrap_or("monitor");
        let source_id =
            CaptureSourceId::new(Arc::<str>::from(format!("wayland:portal:{source_name}")))?;
        let (x, y) = stream.position().unwrap_or_default();
        let logical_width = stream
            .size()
            .and_then(|(width, _)| u32::try_from(width).ok())
            .filter(|width| *width > 0);
        Ok(Self {
            source_id,
            origin: PhysicalOrigin { x, y },
            logical_width,
            session_generation,
        })
    }

    fn source_scale(&self, physical_width: u32) -> SourceScale {
        self.logical_width
            .and_then(|logical_width| SourceScale::new(logical_width, physical_width).ok())
            .unwrap_or(SourceScale::ONE)
    }

    fn epoch(&self, active_session_generation: u64) -> CaptureEpoch {
        CaptureEpoch {
            source_id: self.source_id.clone(),
            topology_generation: 1,
            session_generation: active_session_generation,
        }
    }
}

struct WaylandCaptureUserData {
    analyzer: ScreenCaptureInput,
    format: spa::param::video::VideoInfoRaw,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    rgba_frame: Vec<u8>,
    settings: Arc<SharedSettings>,
    applied_generation: u64,
    source: WaylandSourceMetadata,
    sequence: u64,
}

impl WaylandCaptureUserData {
    fn new(
        settings: Arc<SharedSettings>,
        latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
        source: WaylandSourceMetadata,
    ) -> Self {
        let applied_generation = settings.generation.load(Ordering::Acquire);
        let mut analyzer = ScreenCaptureInput::new(settings.snapshot());
        let _ = analyzer.start();

        Self {
            analyzer,
            format: spa::param::video::VideoInfoRaw::default(),
            latest_snapshot,
            rgba_frame: Vec::new(),
            settings,
            applied_generation,
            source,
            sequence: 0,
        }
    }

    fn sync_settings(&mut self) {
        let generation = self.settings.generation.load(Ordering::Acquire);
        if generation == self.applied_generation {
            return;
        }
        self.applied_generation = generation;
        self.analyzer.apply_settings(self.settings.snapshot());
        debug!(generation, "Applied live screen capture settings");
    }

    fn capture_frame(
        &mut self,
        captured_at: Instant,
        width: u32,
        height: u32,
    ) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let extent = PixelExtent::new(width, height)?;
        let row_stride = i64::from(width)
            .checked_mul(4)
            .ok_or_else(|| anyhow!("Wayland capture row stride overflow"))?;
        let frame_period =
            Duration::from_secs_f64(1.0 / f64::from(self.analyzer.config().target_fps.max(1)));
        let frame = CaptureFrame::new(
            CaptureFrameMetadata {
                source_id: self.source.source_id.clone(),
                topology_generation: 1,
                session_generation: self.source.session_generation,
                sequence: self.sequence,
                captured_at,
                fresh_until: captured_at + frame_period + frame_period,
                geometry: CaptureGeometry::new(
                    self.source.origin,
                    extent,
                    CaptureRotation::Identity,
                    None,
                    self.source.source_scale(width),
                )?,
                color_space: CaptureColorSpace::Unknown,
                transfer_function: CaptureTransferFunction::Unknown,
                cursor: CaptureCursor::default(),
            },
            CaptureStorage::Cpu(CpuCaptureStorage::new(
                Arc::<[u8]>::from(self.rgba_frame.as_slice()),
                CapturePixelFormat::Rgba8,
                row_stride,
                0,
            )),
            CaptureDamage::default(),
        )?;
        frame.validate_epoch(
            &self
                .source
                .epoch(self.settings.session_generation.load(Ordering::Acquire)),
        )?;
        Ok(frame)
    }
}

fn run_capture_worker(
    settings: Arc<SharedSettings>,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    command_rx: pw::channel::Receiver<WorkerCommand>,
    token_sink: Option<RestoreTokenSink>,
    flags: WorkerFlags,
    status_session: SourceSessionSlot,
    session_generation: u64,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            warn!(%error, "Failed to create Wayland capture runtime");
            if let Some(status) = status_session.load() {
                status.failed(SourceIssue::new(
                    "wayland_runtime_start_failed",
                    error.to_string(),
                    true,
                ));
            }
            return;
        }
    };

    let startup_config = settings.snapshot();
    if flags.cancel.load(Ordering::SeqCst) {
        flags.portal_pending.store(false, Ordering::SeqCst);
        debug!("Wayland capture worker cancelled before portal phase");
        return;
    }
    let portal_result = runtime.block_on(open_portal_session(&startup_config));
    flags.portal_pending.store(false, Ordering::SeqCst);

    let (portal, restore_token) = match portal_result {
        Ok(portal) => portal,
        Err(error) => {
            warn!(%error, "Failed to establish Wayland screencast session");
            if !flags.cancel.load(Ordering::SeqCst)
                && let Some(status) = status_session.load()
            {
                status.unavailable(
                    SourceIssue::new("wayland_portal_unavailable", error.to_string(), true)
                        .with_remediation("grant screen-sharing permission in the desktop portal"),
                );
            }
            return;
        }
    };

    // A cancelled (detached) worker was replaced while the picker was open;
    // close the session quietly and leave all shared state to its successor.
    if flags.cancel.load(Ordering::SeqCst) {
        debug!("Wayland capture worker cancelled during portal phase; closing session");
        if let Err(error) = runtime.block_on(portal.session.close()) {
            debug!(%error, "Cancelled capture session close reported an error");
        }
        return;
    }

    if restore_token != startup_config.restore_token {
        if let Ok(mut config) = settings.config.lock() {
            config.restore_token.clone_from(&restore_token);
        }
        if let Some(sink) = &token_sink {
            sink(restore_token);
        }
    }

    let session = match run_pipewire_loop(
        &startup_config,
        settings,
        Arc::clone(&latest_snapshot),
        portal,
        command_rx,
        session_generation,
    ) {
        Ok(session) => session,
        Err(error) => {
            warn!(%error, "Wayland screen capture loop exited with an error");
            if !flags.cancel.load(Ordering::SeqCst)
                && let Some(status) = status_session.load()
            {
                status.failed(SourceIssue::new(
                    "wayland_capture_worker_failed",
                    error.to_string(),
                    true,
                ));
            }
            return;
        }
    };

    if let Err(error) = runtime.block_on(session.close()) {
        warn!(%error, "Wayland screen capture loop exited with an error");
    }
}

async fn open_portal_session(
    config: &CaptureConfig,
) -> anyhow::Result<(PortalCaptureSession, Option<String>)> {
    let proxy = Screencast::new()
        .await
        .context("failed to connect to xdg-desktop-portal screencast interface")?;
    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .context("failed to create screencast portal session")?;

    // An invalid or revoked restore token is ignored by the portal, which
    // falls back to showing the picker — no retry path needed.
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Hidden)
                .set_sources(Some(SourceType::Monitor.into()))
                .set_multiple(false)
                .set_restore_token(config.restore_token.as_deref())
                .set_persist_mode(PersistMode::ExplicitlyRevoked),
        )
        .await
        .context("failed to open screencast source picker")?;

    let response = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .context("failed to start screencast portal session")?
        .response()
        .context("screen capture request was denied or cancelled")?;
    let restore_token = response.restore_token().map(ToOwned::to_owned);
    let stream = response
        .streams()
        .first()
        .cloned()
        .context("portal did not return a monitor stream")?;
    let fd = proxy
        .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
        .await
        .context("failed to open PipeWire remote for screencast session")?;

    info!(
        pipewire_node = stream.pipe_wire_node_id(),
        stream = ?stream,
        restored = config.restore_token.is_some(),
        "Wayland screencast session established"
    );

    Ok((
        PortalCaptureSession {
            session,
            stream,
            fd,
        },
        restore_token,
    ))
}

fn run_pipewire_loop(
    config: &CaptureConfig,
    settings: Arc<SharedSettings>,
    latest_snapshot: Arc<Mutex<Option<CapturedScreenSnapshot>>>,
    portal: PortalCaptureSession,
    command_rx: pw::channel::Receiver<WorkerCommand>,
    session_generation: u64,
) -> anyhow::Result<Session<Screencast>> {
    pw::init();
    let source = WaylandSourceMetadata::from_stream(&portal.stream, session_generation)?;

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
            settings,
            latest_snapshot,
            source,
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
            user_data.sync_settings();
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
            let acquired_at = Instant::now();

            if !copy_frame_to_rgba(
                data,
                format,
                size.width,
                size.height,
                &mut user_data.rgba_frame,
            ) {
                return;
            }

            let Ok(frame) = user_data.capture_frame(acquired_at, size.width, size.height) else {
                return;
            };
            let Ok(legacy) = analyze_legacy_screen_frame(&mut user_data.analyzer, frame) else {
                return;
            };
            let generation = user_data
                .settings
                .frame_generation
                .fetch_add(1, Ordering::Release)
                .wrapping_add(1);

            if let Ok(mut latest) = user_data.latest_snapshot.lock() {
                *latest = Some(CapturedScreenSnapshot { legacy, generation });
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
