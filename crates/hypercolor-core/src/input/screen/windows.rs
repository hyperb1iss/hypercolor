//! Windows screen capture source backed by DXGI Desktop Duplication.
//!
//! Shaped like [`super::wayland::WaylandScreenCaptureInput`]: a worker thread
//! owns the capture session and the analysis pipeline, and the render loop
//! only clones the latest processed [`ScreenData`]. It is markedly simpler
//! than the Wayland source because Windows has nothing to negotiate — no
//! portal handshake, no source picker, no restore token, and no permission
//! grant of any kind.
//!
//! The duplication interface is opened lazily when capture goes active and
//! dropped the moment it goes idle. Windows allows one duplication per output
//! per process, and other ambient-lighting tools want the same interface, so
//! holding it while no effect needs it would be antisocial.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use hypercolor_windows_capture::{CaptureError, DesktopDuplicator};
use tracing::{debug, info, warn};

use crate::input::screen::{
    CaptureColorSpace, CaptureConfig, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame,
    CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStorage, CaptureTransferFunction, CpuCaptureStorage, LegacyScreenSnapshot,
    PhysicalOrigin, PixelExtent, RawCaptureSurface, ScreenCaptureInput, SourceScale,
    analyze_legacy_screen_frame,
};
use crate::input::traits::{InputData, InputSource};
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceStatusHandle, SourceStatusReporter,
};

/// Width the capture backend subsamples to before analysis.
///
/// Matches the resolution the Wayland source negotiates from PipeWire, so
/// both platforms feed the sector grid comparable input and cost.
const CAPTURE_TARGET_WIDTH: u32 = 1280;

/// How long a worker waits on DXGI before checking its command channel.
///
/// Bounded well under a second so a stop or deactivate lands promptly even
/// while the desktop is perfectly static and producing no frames at all.
const FRAME_WAIT: Duration = Duration::from_millis(100);
const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(1);
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(1);

/// Backoff after a failed attempt to open the duplication interface.
///
/// The common cause is another application holding it, which resolves when
/// that application exits, so retrying quietly beats surfacing an error the
/// user cannot act on.
const REOPEN_BACKOFF: Duration = Duration::from_secs(2);

/// Settings shared between the input source handle and the capture worker.
struct SharedSettings {
    config: Mutex<CaptureConfig>,
    generation: AtomicU64,
    topology_generation: AtomicU64,
    session_generation: AtomicU64,
}

impl SharedSettings {
    fn snapshot(&self) -> CaptureConfig {
        self.config
            .lock()
            .map_or_else(|_| CaptureConfig::default(), |config| config.clone())
    }
}

/// Windows-only live screen capture input source.
pub struct WindowsScreenCaptureInput {
    settings: Arc<SharedSettings>,
    running: bool,
    capture_active: bool,
    latest_snapshot: Arc<Mutex<Option<LegacyScreenSnapshot>>>,
    worker: Option<CaptureWorker>,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
}

struct CaptureWorker {
    command_tx: mpsc::Sender<WorkerCommand>,
    exit_rx: mpsc::Receiver<()>,
    join_handle: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for CaptureWorker {
    fn drop(&mut self) {
        let Some(join_handle) = self.join_handle.take() else {
            return;
        };
        self.cancel.store(true, Ordering::Release);
        let _ = self.command_tx.send(WorkerCommand::Stop);
        let _ = self.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT);
        if join_handle.is_finished() {
            let _ = join_handle.join();
            return;
        }
        if let Err(error) = thread::Builder::new()
            .name("hypercolor-screen-reaper".to_owned())
            .spawn(move || {
                if let Err(panic) = join_handle.join() {
                    warn!(?panic, "screen capture worker reaper observed a panic");
                }
            })
        {
            warn!(%error, "failed to start screen capture worker join reaper");
        }
    }
}

enum WorkerCommand {
    SetActive(bool),
    Stop,
}

impl WindowsScreenCaptureInput {
    /// Create a new Windows screen capture source.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            settings: Arc::new(SharedSettings {
                config: Mutex::new(config),
                generation: AtomicU64::new(0),
                topology_generation: AtomicU64::new(1),
                session_generation: AtomicU64::new(0),
            }),
            running: false,
            capture_active: false,
            latest_snapshot: Arc::new(Mutex::new(None)),
            worker: None,
            status: SourceStatusReporter::new(
                "windows_screen_capture",
                SourceKind::Screen,
                "dxgi_desktop_duplication",
                true,
                true,
                false,
            ),
            status_session: SourceSessionSlot::new(),
        }
    }

    fn spawn_worker(&mut self) -> anyhow::Result<()> {
        self.observe_worker_exit(false);
        if self.worker.is_some() {
            anyhow::bail!("previous Windows screen capture worker is still stopping");
        }

        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        let settings = Arc::clone(&self.settings);
        let latest_snapshot = Arc::clone(&self.latest_snapshot);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let status_session = self.status_session.clone();
        let session_generation = self
            .settings
            .session_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);

        let join_handle = thread::Builder::new()
            .name("hypercolor-screen-capture".to_owned())
            .spawn(move || {
                let _ = ready_tx.send(());
                run_worker(
                    &settings,
                    &latest_snapshot,
                    &command_rx,
                    &worker_cancel,
                    status_session,
                    session_generation,
                );
                let _ = exit_tx.send(());
            })
            .map_err(|error| anyhow!("failed to spawn screen capture worker: {error}"))?;

        self.worker = Some(CaptureWorker {
            command_tx,
            exit_rx,
            join_handle: Some(join_handle),
            cancel,
        });
        if let Err(error) = ready_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            self.shutdown_worker();
            anyhow::bail!("Windows screen capture worker readiness timed out: {error}");
        }
        if self.observe_worker_exit(true) {
            anyhow::bail!("Windows screen capture worker exited during startup");
        }
        Ok(())
    }

    fn shutdown_worker(&mut self) {
        let Some(worker) = self.worker.as_mut() else {
            return;
        };

        worker.cancel.store(true, Ordering::Release);
        let _ = worker.command_tx.send(WorkerCommand::Stop);
        let _ = worker.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT);
        let Some(join_handle) = worker.join_handle.as_ref() else {
            self.worker = None;
            return;
        };
        if !join_handle.is_finished() {
            warn!(
                "screen capture worker did not stop before the deadline; retaining its join handle"
            );
            return;
        }
        let mut worker = self.worker.take().expect("finished worker remains owned");
        if worker
            .join_handle
            .take()
            .expect("finished screen worker retains its join handle")
            .join()
            .is_err()
        {
            warn!("screen capture worker panicked during shutdown");
        }
    }

    fn observe_worker_exit(&mut self, publish_failure: bool) -> bool {
        let Some(worker) = self.worker.as_ref() else {
            return false;
        };
        if !worker
            .join_handle
            .as_ref()
            .is_some_and(thread::JoinHandle::is_finished)
        {
            return false;
        }
        let mut worker = self.worker.take().expect("finished worker remains owned");
        let failure = worker
            .join_handle
            .take()
            .expect("finished screen worker retains its join handle")
            .join()
            .err();
        if publish_failure && let Some(status) = self.status.session() {
            let reason = failure.map_or_else(
                || "Windows screen capture worker exited unexpectedly".to_owned(),
                |panic| format!("Windows screen capture worker panicked: {panic:?}"),
            );
            status.failed(SourceIssue::new(
                "windows_screen_worker_exited",
                reason,
                true,
            ));
        }
        if let Ok(mut latest) = self.latest_snapshot.lock() {
            *latest = None;
        }
        true
    }

    fn set_capture_active_state(&mut self, active: bool) -> anyhow::Result<()> {
        if self.capture_active == active {
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
        }

        if let Some(worker) = self.worker.as_ref()
            && worker
                .command_tx
                .send(WorkerCommand::SetActive(active))
                .is_err()
        {
            self.shutdown_worker();
            if active {
                anyhow::bail!("Windows screen capture worker stopped before activation");
            }
        }

        self.capture_active = active;
        Ok(())
    }

    /// Publish new settings and bump the generation the worker polls.
    fn reconfigure(&mut self, config: CaptureConfig) {
        let mut topology_changed = false;
        if let Ok(mut current) = self.settings.config.lock() {
            if *current == config {
                return;
            }
            topology_changed = current.monitor != config.monitor;
            *current = config;
        }
        if topology_changed {
            self.settings
                .topology_generation
                .fetch_add(1, Ordering::AcqRel);
        }
        self.settings.generation.fetch_add(1, Ordering::Release);
    }
}

impl InputSource for WindowsScreenCaptureInput {
    fn name(&self) -> &'static str {
        "windows_screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        if self.capture_active {
            if let Some(session) = self.status.begin_session()? {
                self.status_session.store(session);
            }
            if let Err(error) = self.spawn_worker().and_then(|()| {
                self.worker.as_ref().map_or_else(
                    || {
                        Err(anyhow!(
                            "Windows screen capture worker disappeared during startup"
                        ))
                    },
                    |worker| {
                        worker
                            .command_tx
                            .send(WorkerCommand::SetActive(true))
                            .map_err(|_| anyhow!("Windows screen capture worker rejected startup"))
                    },
                )
            }) {
                self.status_session.clear();
                self.status.stop();
                self.shutdown_worker();
                return Err(error);
            }
        } else {
            debug!(
                "Windows screen capture armed but idle until a screen-reactive effect requests capture"
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
            .map_err(|_| anyhow!("windows screen capture snapshot mutex poisoned"))?;

        let Some(snapshot) = latest.as_ref() else {
            return Ok(InputData::None);
        };
        let expected = capture_epoch(
            self.settings.snapshot().monitor,
            self.settings.topology_generation.load(Ordering::Acquire),
            self.settings.session_generation.load(Ordering::Acquire),
        )?;
        if snapshot.frame().validate_epoch(&expected).is_err() {
            return Ok(InputData::None);
        }
        Ok(InputData::Screen(snapshot.data().clone()))
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
            if active
                && self.running
                && let Some(session) = self.status.begin_session()?
            {
                self.status_session.store(session);
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
        self.reconfigure(config.clone());
        Ok(())
    }
}

impl Drop for WindowsScreenCaptureInput {
    fn drop(&mut self) {
        self.shutdown_worker();
    }
}

/// Worker loop: own the duplication session, analyze frames, publish results.
fn run_worker(
    settings: &Arc<SharedSettings>,
    latest_snapshot: &Arc<Mutex<Option<LegacyScreenSnapshot>>>,
    command_rx: &mpsc::Receiver<WorkerCommand>,
    cancel: &Arc<AtomicBool>,
    status_session: SourceSessionSlot,
    session_generation: u64,
) {
    let mut config = settings.snapshot();
    let mut generation = settings.generation.load(Ordering::Acquire);
    let mut analyzer = ScreenCaptureInput::new(config.clone());
    let mut duplicator: Option<DesktopDuplicator> = None;
    let mut active = false;
    let mut open_failure_logged = false;
    let mut sequence = 0_u64;

    loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }

        match drain_commands(command_rx, &mut active) {
            ControlFlow::Stop => break,
            ControlFlow::Continue => {}
        }

        if !active {
            // Release the duplication interface so other applications can use
            // it while no screen-reactive effect is running.
            duplicator = None;
            open_failure_logged = false;
            match command_rx.recv_timeout(FRAME_WAIT) {
                Ok(WorkerCommand::SetActive(next)) => active = next,
                Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            continue;
        }

        let latest_generation = settings.generation.load(Ordering::Acquire);
        if latest_generation != generation {
            generation = latest_generation;
            let previous_monitor = config.monitor;
            config = settings.snapshot();
            analyzer = ScreenCaptureInput::new(config.clone());
            if previous_monitor != config.monitor {
                duplicator = None;
            } else if let Some(duplicator) = duplicator.as_mut() {
                duplicator.set_max_width(CAPTURE_TARGET_WIDTH);
            }
        }

        let session = match duplicator.as_mut() {
            Some(session) => session,
            None => match DesktopDuplicator::new(config.monitor, CAPTURE_TARGET_WIDTH) {
                Ok(session) => {
                    let (width, height) = session.native_extent();
                    info!(
                        monitor = config.monitor,
                        width, height, "Windows screen capture online"
                    );
                    open_failure_logged = false;
                    duplicator.insert(session)
                }
                Err(error) => {
                    if !open_failure_logged {
                        log_open_failure(&error);
                        open_failure_logged = true;
                    }
                    if let Some(status) = status_session.load() {
                        status.unavailable(
                            SourceIssue::new(
                                "windows_desktop_duplication_unavailable",
                                error.to_string(),
                                true,
                            )
                            .with_remediation(
                                "close other capture tools using this display and retry",
                            ),
                        );
                    }
                    match command_rx.recv_timeout(REOPEN_BACKOFF) {
                        Ok(WorkerCommand::SetActive(next)) => active = next,
                        Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    continue;
                }
            },
        };

        match session.next_frame(FRAME_WAIT) {
            Ok(Some(frame)) => {
                let acquired_at = Instant::now();
                let frame_period =
                    Duration::from_secs_f64(1.0 / f64::from(config.target_fps.max(1)));
                sequence = sequence.wrapping_add(1).max(1);
                let topology_generation = settings.topology_generation.load(Ordering::Acquire);
                let raw_frame = build_capture_frame(
                    frame.rgba,
                    frame.width,
                    frame.height,
                    config.monitor,
                    topology_generation,
                    session_generation,
                    sequence,
                    acquired_at,
                    frame_period,
                );
                let snapshot = raw_frame.and_then(|frame| {
                    frame.validate_epoch(&capture_epoch(
                        config.monitor,
                        topology_generation,
                        settings.session_generation.load(Ordering::Acquire),
                    )?)?;
                    analyze_legacy_screen_frame(&mut analyzer, frame)
                });
                let Ok(snapshot) = snapshot else {
                    continue;
                };
                let published = if let Ok(mut latest) = latest_snapshot.lock() {
                    *latest = Some(snapshot);
                    true
                } else {
                    false
                };
                if published && let Some(status) = status_session.load() {
                    let _ = status.record_sample(
                        acquired_at,
                        acquired_at + frame_period + frame_period,
                        1,
                    );
                }
            }
            // Static desktop or pointer-only update: nothing new to analyze.
            Ok(None) => {}
            Err(error) => {
                warn!(%error, "Windows screen capture frame failed; reopening session");
                duplicator = None;
                match command_rx.recv_timeout(REOPEN_BACKOFF) {
                    Ok(WorkerCommand::SetActive(next)) => active = next,
                    Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }
    }

    if let Ok(mut latest) = latest_snapshot.lock() {
        *latest = None;
    }
    debug!("Windows screen capture worker stopped");
}

fn capture_source_id(monitor: usize) -> anyhow::Result<CaptureSourceId> {
    CaptureSourceId::new(Arc::<str>::from(format!("windows:monitor:{monitor}")))
        .map_err(anyhow::Error::from)
}

fn capture_epoch(
    monitor: usize,
    topology_generation: u64,
    session_generation: u64,
) -> anyhow::Result<CaptureEpoch> {
    Ok(CaptureEpoch {
        source_id: capture_source_id(monitor)?,
        topology_generation,
        session_generation,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_capture_frame(
    rgba: &[u8],
    width: u32,
    height: u32,
    monitor: usize,
    topology_generation: u64,
    session_generation: u64,
    sequence: u64,
    captured_at: Instant,
    frame_period: Duration,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let extent = PixelExtent::new(width, height)?;
    let row_stride = i64::from(width)
        .checked_mul(4)
        .ok_or_else(|| anyhow!("Windows capture row stride overflow"))?;
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id: capture_source_id(monitor)?,
            topology_generation,
            session_generation,
            sequence,
            captured_at,
            fresh_until: captured_at + frame_period + frame_period,
            geometry: CaptureGeometry::new(
                PhysicalOrigin::default(),
                extent,
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )?,
            color_space: CaptureColorSpace::Unknown,
            transfer_function: CaptureTransferFunction::Unknown,
            cursor: CaptureCursor::default(),
        },
        CaptureStorage::Cpu(CpuCaptureStorage::new(
            Arc::<[u8]>::from(rgba),
            CapturePixelFormat::Rgba8,
            row_stride,
            0,
        )),
        CaptureDamage::default(),
    )
    .map_err(anyhow::Error::from)
}

enum ControlFlow {
    Continue,
    Stop,
}

/// Apply every queued command without blocking.
fn drain_commands(command_rx: &mpsc::Receiver<WorkerCommand>, active: &mut bool) -> ControlFlow {
    loop {
        match command_rx.try_recv() {
            Ok(WorkerCommand::SetActive(next)) => *active = next,
            Ok(WorkerCommand::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                return ControlFlow::Stop;
            }
            Err(mpsc::TryRecvError::Empty) => return ControlFlow::Continue,
        }
    }
}

/// Log an open failure at a level that matches how actionable it is.
fn log_open_failure(error: &CaptureError) {
    match error {
        // Expected and self-healing: another RGB or capture tool holds the
        // single per-output duplication interface until it exits.
        CaptureError::AlreadyDuplicating => {
            info!("screen capture is held by another application; retrying in the background");
        }
        other => warn!(%other, "failed to open Windows screen capture"),
    }
}
