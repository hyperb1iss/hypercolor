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
use std::time::Duration;

use anyhow::anyhow;
use hypercolor_windows_capture::{
    CaptureError, DesktopDuplicator, DisplayRotation, Frame as NativeCaptureFrame,
    ReductionTelemetry,
};
use tracing::{debug, info, warn};

use crate::input::screen::{
    CaptureColorSpace, CaptureConfig, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame,
    CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureSourceId,
    CaptureStorage, CaptureTransferFunction, CpuCaptureStorage, LegacyScreenSnapshot,
    PhysicalOrigin, PixelExtent, RawCaptureSurface, ScreenCaptureInput, SourceScale,
    analyze_legacy_screen_frame,
};
use crate::input::status::{
    ScreenCaptureDiagnostics, ScreenCaptureReductionPath, SourceDiagnostics,
};
use crate::input::traits::{InputData, InputSource};
use crate::input::worker_retention::{retain_input_worker, spawn_input_worker};
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceSessionWriter, SourceStatusHandle,
    SourceStatusReporter,
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

/// Persists a legacy monitor selector after its stable output id is known.
pub type CaptureSourceSink = Arc<dyn Fn(ResolvedCaptureSource) + Send + Sync>;

/// A successfully opened legacy source and the stable value it resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCaptureSource {
    /// Exact configured value used to open the capture session.
    pub configured_source: String,
    /// Stable source value suitable for persistence.
    pub stable_source: String,
}

/// Settings shared between the input source handle and the capture worker.
struct SharedSettings {
    config: Mutex<VersionedCaptureConfig>,
    generation: AtomicU64,
    session_generation: AtomicU64,
    activity_generation: AtomicU64,
}

struct VersionedCaptureConfig {
    value: CaptureConfig,
    source_generation: u64,
}

struct CaptureSettingsSnapshot {
    config: CaptureConfig,
    source_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveCaptureEpoch {
    epoch: CaptureEpoch,
    source_generation: u64,
    activity_generation: u64,
    duplication_generation: u64,
}

struct CapturePublication<T> {
    source_generation: u64,
    activity_generation: u64,
    active: Option<ActiveCaptureEpoch>,
    latest: Option<T>,
}

impl<T> Default for CapturePublication<T> {
    fn default() -> Self {
        Self {
            source_generation: 0,
            activity_generation: 0,
            active: None,
            latest: None,
        }
    }
}

impl<T> CapturePublication<T> {
    fn activate(&mut self, active: ActiveCaptureEpoch) -> bool {
        if active.source_generation != self.source_generation
            || active.activity_generation != self.activity_generation
        {
            return false;
        }
        if self.active.as_ref() != Some(&active) {
            self.latest = None;
            self.active = Some(active);
        }
        true
    }

    fn fence_source(&mut self, source_generation: u64) {
        self.source_generation = source_generation;
        self.clear();
    }

    fn fence_activity(&mut self, activity_generation: u64) {
        self.activity_generation = activity_generation;
        self.clear();
    }

    fn clear(&mut self) {
        self.active = None;
        self.latest = None;
    }

    fn publish(&mut self, active: &ActiveCaptureEpoch, value: T) -> bool {
        if self.active.as_ref() != Some(active) {
            return false;
        }
        self.latest = Some(value);
        true
    }
}

impl SharedSettings {
    fn snapshot(&self) -> CaptureSettingsSnapshot {
        let config = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        CaptureSettingsSnapshot {
            config: config.value.clone(),
            source_generation: config.source_generation,
        }
    }
}

/// Windows-only live screen capture input source.
pub struct WindowsScreenCaptureInput {
    settings: Arc<SharedSettings>,
    running: bool,
    capture_active: bool,
    publication: Arc<Mutex<CapturePublication<LegacyScreenSnapshot>>>,
    worker: Option<CaptureWorker>,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
    source_sink: Option<CaptureSourceSink>,
}

struct CaptureWorker {
    command_tx: mpsc::Sender<WorkerCommand>,
    exit_rx: mpsc::Receiver<()>,
    join_handle: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    #[cfg(test)]
    processed_activity_generation: Arc<AtomicU64>,
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
        retain_input_worker(join_handle, "Windows screen capture worker");
    }
}

#[derive(Clone, Copy)]
enum WorkerCommand {
    SetActive {
        active: bool,
        activity_generation: u64,
    },
    Stop,
}

impl WindowsScreenCaptureInput {
    /// Create a new Windows screen capture source.
    #[must_use]
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            settings: Arc::new(SharedSettings {
                config: Mutex::new(VersionedCaptureConfig {
                    value: config,
                    source_generation: 0,
                }),
                generation: AtomicU64::new(0),
                session_generation: AtomicU64::new(0),
                activity_generation: AtomicU64::new(0),
            }),
            running: false,
            capture_active: false,
            publication: Arc::new(Mutex::new(CapturePublication::default())),
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
            source_sink: None,
        }
    }

    /// Attach a callback that persists resolved legacy monitor selections.
    #[must_use]
    pub fn with_capture_source_sink(mut self, sink: CaptureSourceSink) -> Self {
        self.source_sink = Some(sink);
        self
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
        let publication = Arc::clone(&self.publication);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_processed_activity_generation = Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let processed_activity_generation = Arc::clone(&worker_processed_activity_generation);
        let status_session = self.status_session.clone();
        let source_sink = self.source_sink.clone();
        let session_generation = self
            .settings
            .session_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);

        let join_handle = spawn_input_worker(
            thread::Builder::new().name("hypercolor-screen-capture".to_owned()),
            move || {
                let _ = ready_tx.send(());
                run_worker(
                    &settings,
                    &publication,
                    &command_rx,
                    &worker_cancel,
                    &worker_processed_activity_generation,
                    status_session,
                    session_generation,
                    source_sink,
                );
                let _ = exit_tx.send(());
            },
        )
        .map_err(|error| anyhow!("failed to spawn screen capture worker: {error}"))?;

        self.worker = Some(CaptureWorker {
            command_tx,
            exit_rx,
            join_handle: Some(join_handle),
            cancel,
            #[cfg(test)]
            processed_activity_generation,
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
        let exit_observed = worker.exit_rx.recv_timeout(WORKER_STOP_TIMEOUT).is_ok();
        let Some(join_handle) = worker.join_handle.as_ref() else {
            self.worker = None;
            return;
        };
        if !exit_observed && !join_handle.is_finished() {
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
        clear_capture_publication(&self.publication);
        true
    }

    fn send_activity_command(&self, active: bool, activity_generation: u64) -> bool {
        self.worker.as_ref().is_some_and(|worker| {
            worker
                .command_tx
                .send(WorkerCommand::SetActive {
                    active,
                    activity_generation,
                })
                .is_ok()
        })
    }

    fn activate_worker(&mut self, activity_generation: u64) -> anyhow::Result<()> {
        self.observe_worker_exit(false);
        if self.worker.is_none() {
            self.spawn_worker()?;
        }
        if self.send_activity_command(true, activity_generation) {
            return Ok(());
        }

        self.shutdown_worker();
        if self.worker.is_some() {
            anyhow::bail!("disconnected Windows screen capture worker could not be reaped");
        }
        self.spawn_worker()?;
        if self.send_activity_command(true, activity_generation) {
            return Ok(());
        }

        self.shutdown_worker();
        anyhow::bail!("replacement Windows screen capture worker rejected activation")
    }

    fn set_capture_active_state(&mut self, active: bool) -> anyhow::Result<()> {
        if self.capture_active == active {
            return Ok(());
        }
        let activity_generation = self
            .settings
            .activity_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fence_activity(activity_generation);

        if !self.running {
            self.capture_active = active;
            return Ok(());
        }

        if active {
            self.activate_worker(activity_generation)?;
        } else if !self.send_activity_command(false, activity_generation) {
            self.shutdown_worker();
        }

        self.capture_active = active;
        Ok(())
    }

    /// Publish new settings and bump the generation the worker polls.
    fn reconfigure(&mut self, config: CaptureConfig) {
        let mut current = self
            .settings
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.value == config {
            return;
        }
        let source_changed = current.value.source != config.source;
        let mut publication = source_changed.then(|| {
            self.publication
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        if source_changed {
            current.source_generation = current.source_generation.wrapping_add(1).max(1);
        }
        current.value = config;
        self.settings.generation.fetch_add(1, Ordering::Release);
        if source_changed {
            publication
                .as_mut()
                .expect("source changes lock the publication fence")
                .fence_source(current.source_generation);
        }
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
            let activity_generation = self.settings.activity_generation.load(Ordering::Acquire);
            if let Err(error) = self.activate_worker(activity_generation) {
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

        clear_capture_publication(&self.publication);
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.observe_worker_exit(self.running && self.capture_active);
        if !self.running || !self.capture_active {
            return Ok(InputData::None);
        }

        let publication = self
            .publication
            .lock()
            .map_err(|_| anyhow!("windows screen capture publication mutex poisoned"))?;
        let Some(active) = publication.active.as_ref() else {
            return Ok(InputData::None);
        };
        let Some(snapshot) = publication.latest.as_ref() else {
            return Ok(InputData::None);
        };
        if snapshot.frame().validate_epoch(&active.epoch).is_err() {
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

fn active_capture_epoch(
    session: &DesktopDuplicator,
    session_generation: u64,
    source_generation: u64,
    activity_generation: u64,
) -> anyhow::Result<ActiveCaptureEpoch> {
    Ok(ActiveCaptureEpoch {
        epoch: CaptureEpoch {
            source_id: capture_source_id(session.source_id())?,
            topology_generation: session.topology_generation(),
            session_generation,
        },
        source_generation,
        activity_generation,
        duplication_generation: session.duplication_generation(),
    })
}

fn activate_capture_epoch(
    publication: &Mutex<CapturePublication<LegacyScreenSnapshot>>,
    active: ActiveCaptureEpoch,
) -> bool {
    publication
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .activate(active)
}

fn clear_capture_publication(publication: &Mutex<CapturePublication<LegacyScreenSnapshot>>) {
    publication
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

fn settle_inactive_capture<T>(
    resource: &mut Option<T>,
    processed_activity_generation: &AtomicU64,
    activity_generation: u64,
) {
    *resource = None;
    processed_activity_generation.store(activity_generation, Ordering::Release);
}

/// Worker loop: own the duplication session, analyze frames, publish results.
fn run_worker(
    settings: &Arc<SharedSettings>,
    publication: &Arc<Mutex<CapturePublication<LegacyScreenSnapshot>>>,
    command_rx: &mpsc::Receiver<WorkerCommand>,
    cancel: &Arc<AtomicBool>,
    processed_activity_generation: &AtomicU64,
    status_session: SourceSessionSlot,
    session_generation: u64,
    source_sink: Option<CaptureSourceSink>,
) {
    let initial_settings = settings.snapshot();
    let mut config = initial_settings.config;
    let mut source_generation = initial_settings.source_generation;
    let mut generation = settings.generation.load(Ordering::Acquire);
    let mut analyzer = ScreenCaptureInput::new(config.clone());
    let mut duplicator: Option<DesktopDuplicator> = None;
    let mut active = false;
    let mut activity_generation = 0_u64;
    let mut open_failure_logged = false;

    loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }

        match drain_commands(command_rx, &mut active, &mut activity_generation) {
            ControlFlow::Stop => break,
            ControlFlow::Continue => {}
        }

        if !active {
            // Release the duplication interface so other applications can use
            // it while no screen-reactive effect is running.
            settle_inactive_capture(
                &mut duplicator,
                processed_activity_generation,
                activity_generation,
            );
            clear_capture_publication(publication);
            open_failure_logged = false;
            match command_rx.recv_timeout(FRAME_WAIT) {
                Ok(WorkerCommand::SetActive {
                    active: next,
                    activity_generation: next_generation,
                }) => {
                    active = next;
                    activity_generation = next_generation;
                }
                Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            continue;
        }

        processed_activity_generation.store(activity_generation, Ordering::Release);

        let latest_generation = settings.generation.load(Ordering::Acquire);
        if latest_generation != generation {
            generation = latest_generation;
            let previous_source = config.source.clone();
            let next_settings = settings.snapshot();
            config = next_settings.config;
            source_generation = next_settings.source_generation;
            analyzer = ScreenCaptureInput::new(config.clone());
            if previous_source != config.source {
                duplicator = None;
                clear_capture_publication(publication);
            } else if let Some(duplicator) = duplicator.as_mut() {
                duplicator.set_max_width(CAPTURE_TARGET_WIDTH);
            }
        }

        let session = if let Some(session) = duplicator.as_mut() {
            session
        } else {
            let configured_source = config.source.clone();
            let selector = super::monitor_selector_from_source(&configured_source);
            match DesktopDuplicator::open(selector.clone(), CAPTURE_TARGET_WIDTH) {
                Ok(session) => {
                    if let Some(source) = selector.canonical_source(session.source_id()) {
                        if let Some(sink) = source_sink.as_ref() {
                            sink(ResolvedCaptureSource {
                                configured_source,
                                stable_source: source.clone(),
                            });
                        }
                        config.source = source;
                    }
                    let (width, height) = session.native_extent();
                    info!(
                        source = session.source_id(),
                        width, height, "Windows screen capture online"
                    );
                    open_failure_logged = false;
                    duplicator.insert(session)
                }
                Err(error) => {
                    clear_capture_publication(publication);
                    if !open_failure_logged {
                        log_open_failure(&error);
                        open_failure_logged = true;
                    }
                    if let Some(status) = status_session.load() {
                        status.unavailable(capture_issue(&error));
                    }
                    match command_rx.recv_timeout(REOPEN_BACKOFF) {
                        Ok(WorkerCommand::SetActive {
                            active: next,
                            activity_generation: next_generation,
                        }) => {
                            active = next;
                            activity_generation = next_generation;
                        }
                        Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    continue;
                }
            }
        };

        let active_epoch = match active_capture_epoch(
            session,
            session_generation,
            source_generation,
            activity_generation,
        ) {
            Ok(active_epoch) => active_epoch,
            Err(error) => {
                warn!(%error, "Windows screen capture identity is invalid; reopening session");
                clear_capture_publication(publication);
                duplicator = None;
                continue;
            }
        };
        if !activate_capture_epoch(publication, active_epoch) {
            continue;
        }

        let frame_result = session.next_frame(FRAME_WAIT);
        let reduction_telemetry = session.reduction_telemetry();
        let current_epoch = if frame_result.is_ok() {
            match active_capture_epoch(
                session,
                session_generation,
                source_generation,
                activity_generation,
            ) {
                Ok(current_epoch) => {
                    if !activate_capture_epoch(publication, current_epoch.clone()) {
                        continue;
                    }
                    Some(current_epoch)
                }
                Err(error) => {
                    warn!(%error, "Windows screen capture identity became invalid");
                    clear_capture_publication(publication);
                    duplicator = None;
                    continue;
                }
            }
        } else {
            None
        };

        match frame_result {
            Ok(Some(frame)) => {
                let Some(current_epoch) = current_epoch else {
                    duplicator = None;
                    continue;
                };
                let captured_at = frame.captured_at;
                let frame_period =
                    Duration::from_secs_f64(1.0 / f64::from(config.target_fps.max(1)));
                let raw_frame = build_capture_frame(frame, session_generation, frame_period);
                let snapshot = raw_frame.and_then(|frame| {
                    frame.validate_epoch(&current_epoch.epoch)?;
                    analyze_legacy_screen_frame(&mut analyzer, frame)
                });
                let Ok(snapshot) = snapshot else {
                    clear_capture_publication(publication);
                    continue;
                };
                let published = publication
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .publish(&current_epoch, snapshot);
                if published && let Some(status) = status_session.load() {
                    record_capture_health(
                        &status,
                        captured_at,
                        captured_at + frame_period + frame_period,
                        &reduction_telemetry,
                    );
                }
            }
            // Static desktop or pointer-only update: nothing new to analyze.
            Ok(None) => {}
            Err(error) => {
                clear_capture_publication(publication);
                warn!(%error, "Windows screen capture frame failed; reopening session");
                if let Some(status) = status_session.load() {
                    status.degraded(capture_issue(&error));
                }
                duplicator = None;
                match command_rx.recv_timeout(REOPEN_BACKOFF) {
                    Ok(WorkerCommand::SetActive {
                        active: next,
                        activity_generation: next_generation,
                    }) => {
                        active = next;
                        activity_generation = next_generation;
                    }
                    Ok(WorkerCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }
    }

    clear_capture_publication(publication);
    debug!("Windows screen capture worker stopped");
}

fn capture_source_id(source_id: &str) -> anyhow::Result<CaptureSourceId> {
    CaptureSourceId::new(Arc::<str>::from(format!("windows:{source_id}")))
        .map_err(anyhow::Error::from)
}

#[cfg(test)]
fn capture_epoch(
    source_id: &str,
    topology_generation: u64,
    session_generation: u64,
) -> anyhow::Result<CaptureEpoch> {
    Ok(CaptureEpoch {
        source_id: capture_source_id(source_id)?,
        topology_generation,
        session_generation,
    })
}

fn build_capture_frame(
    frame: NativeCaptureFrame,
    session_generation: u64,
    frame_period: Duration,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let source_id = capture_source_id(&frame.source_id)?;
    let topology_generation = frame.topology_generation;
    let cursor = CaptureCursor {
        visible: frame.cursor.visible,
        position: (frame.cursor.width > 0 && frame.cursor.height > 0).then_some(PhysicalOrigin {
            x: frame.cursor.position_x,
            y: frame.cursor.position_y,
        }),
        hotspot: (frame.cursor.width > 0 && frame.cursor.height > 0).then_some(PhysicalOrigin {
            x: frame.cursor.hotspot_x,
            y: frame.cursor.hotspot_y,
        }),
        shape_extent: (frame.cursor.width > 0 && frame.cursor.height > 0)
            .then(|| PixelExtent::new(frame.cursor.width, frame.cursor.height))
            .transpose()?,
        shape_generation: (frame.cursor.shape_generation > 0)
            .then_some(frame.cursor.shape_generation),
        composed: frame.cursor.composed,
    };
    let storage_extent = PixelExtent::new(frame.width, frame.height)?;
    let native_extent = PixelExtent::new(frame.native_width, frame.native_height)?;
    let row_stride = i64::from(frame.width)
        .checked_mul(4)
        .ok_or_else(|| anyhow!("Windows capture row stride overflow"))?;
    let geometry = capture_geometry(
        native_extent,
        storage_extent,
        PhysicalOrigin {
            x: frame.origin_x,
            y: frame.origin_y,
        },
        frame.rotation,
    )?;
    CaptureFrame::new(
        CaptureFrameMetadata {
            source_id,
            topology_generation,
            session_generation,
            sequence: frame.sequence,
            captured_at: frame.captured_at,
            fresh_until: frame.captured_at + frame_period + frame_period,
            geometry,
            color_space: CaptureColorSpace::Unknown,
            transfer_function: CaptureTransferFunction::Unknown,
            cursor,
        },
        CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
            frame,
            CapturePixelFormat::Rgba8,
            row_stride,
            0,
        )),
        CaptureDamage::default(),
    )
    .map_err(anyhow::Error::from)
}

fn capture_geometry(
    native_extent: PixelExtent,
    storage_extent: PixelExtent,
    origin: PhysicalOrigin,
    rotation: DisplayRotation,
) -> Result<CaptureGeometry, crate::input::screen::CaptureFrameError> {
    CaptureGeometry::new(
        origin,
        native_extent,
        storage_extent,
        capture_rotation(rotation),
        None,
        SourceScale::ONE,
    )
}

const fn capture_rotation(rotation: DisplayRotation) -> CaptureRotation {
    match rotation {
        DisplayRotation::Identity => CaptureRotation::Identity,
        DisplayRotation::Clockwise90 => CaptureRotation::Clockwise90,
        DisplayRotation::Clockwise180 => CaptureRotation::Clockwise180,
        DisplayRotation::Clockwise270 => CaptureRotation::Clockwise270,
    }
}

enum ControlFlow {
    Continue,
    Stop,
}

/// Apply every queued command without blocking.
fn drain_commands(
    command_rx: &mpsc::Receiver<WorkerCommand>,
    active: &mut bool,
    activity_generation: &mut u64,
) -> ControlFlow {
    loop {
        match command_rx.try_recv() {
            Ok(WorkerCommand::SetActive {
                active: next,
                activity_generation: next_generation,
            }) => {
                *active = next;
                *activity_generation = next_generation;
            }
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
        CaptureError::AlreadyDuplicating => {
            info!("desktop duplication has no free client slot; retrying in the background");
        }
        CaptureError::AccessDenied
        | CaptureError::SessionUnavailable
        | CaptureError::AccessLost
        | CaptureError::Timeout => {
            debug!(%error, "Windows desktop temporarily unavailable; retrying");
        }
        other => warn!(%other, "failed to open Windows screen capture"),
    }
}

fn capture_issue(error: &CaptureError) -> SourceIssue {
    match error {
        CaptureError::AlreadyDuplicating => {
            SourceIssue::new("windows_desktop_duplication_limit", error.to_string(), true)
                .with_remediation("close an application that is capturing this desktop")
        }
        CaptureError::AccessDenied => {
            SourceIssue::new("windows_desktop_access_denied", error.to_string(), true)
                .with_remediation("dismiss the secure desktop prompt or unlock the session")
        }
        CaptureError::SessionUnavailable => {
            SourceIssue::new("windows_session_unavailable", error.to_string(), true)
                .with_remediation("return to the interactive Windows session")
        }
        CaptureError::DeviceLost => {
            SourceIssue::new("windows_capture_device_lost", error.to_string(), true)
                .with_remediation("wait for the display driver to recover")
        }
        CaptureError::AccessLost => {
            SourceIssue::new("windows_desktop_access_lost", error.to_string(), true)
                .with_remediation("wait for the desktop transition to finish")
        }
        CaptureError::Timeout => {
            SourceIssue::new("windows_capture_timeout", error.to_string(), true)
        }
        CaptureError::MonitorNotFound { .. } | CaptureError::SourceNotFound { .. } => {
            SourceIssue::new("windows_capture_source_missing", error.to_string(), true)
                .with_remediation("select an attached display")
        }
        CaptureError::UnsupportedPlatform | CaptureError::Windows { .. } => SourceIssue::new(
            "windows_desktop_duplication_unavailable",
            error.to_string(),
            true,
        ),
    }
}

fn reduction_issue(telemetry: &ReductionTelemetry) -> Option<SourceIssue> {
    telemetry.issue.as_ref().map(|issue| {
        SourceIssue::new(
            "windows_capture_gpu_reduction_degraded",
            format!(
                "path={:?}; {issue}; gpu_failures={}, gpu_completed={}, cpu_completed={}, ring_busy={}, readback_bytes={}",
                telemetry.path,
                telemetry.gpu_failures,
                telemetry.gpu_completed,
                telemetry.cpu_completed,
                telemetry.ring_busy,
                telemetry.readback_bytes,
            ),
            true,
        )
        .with_remediation("update the display driver or restart the capture session")
    })
}

fn record_capture_health(
    status: &SourceSessionWriter,
    captured_at: std::time::Instant,
    freshness_deadline: std::time::Instant,
    telemetry: &ReductionTelemetry,
) {
    let _ = status.record_sample(captured_at, freshness_deadline, 1);
    status.publish_diagnostics(reduction_diagnostics(telemetry));
    if let Some(issue) = reduction_issue(telemetry) {
        status.degraded(issue);
    }
}

fn reduction_diagnostics(telemetry: &ReductionTelemetry) -> SourceDiagnostics {
    let reduction_path = match telemetry.path {
        hypercolor_windows_capture::ReductionPath::Gpu => ScreenCaptureReductionPath::Gpu,
        hypercolor_windows_capture::ReductionPath::CpuFallback => {
            ScreenCaptureReductionPath::CpuFallback
        }
    };
    SourceDiagnostics::ScreenCapture(ScreenCaptureDiagnostics {
        reduction_path,
        gpu_submitted: telemetry.gpu_submitted,
        gpu_completed: telemetry.gpu_completed,
        cpu_completed: telemetry.cpu_completed,
        ring_busy: telemetry.ring_busy,
        readback_bytes: telemetry.readback_bytes,
        gpu_failures: telemetry.gpu_failures,
    })
}

#[cfg(test)]
mod tests;
