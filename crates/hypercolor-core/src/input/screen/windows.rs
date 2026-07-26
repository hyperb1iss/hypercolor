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
use hypercolor_windows_capture::{CaptureError, DesktopDuplicator};
use tracing::{debug, info, warn};

use crate::input::screen::{CaptureConfig, ScreenCaptureInput};
use crate::input::traits::{InputData, InputSource, ScreenData};

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
    latest_snapshot: Arc<Mutex<Option<ScreenData>>>,
    worker: Option<CaptureWorker>,
}

struct CaptureWorker {
    command_tx: mpsc::Sender<WorkerCommand>,
    join_handle: thread::JoinHandle<()>,
    cancel: Arc<AtomicBool>,
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
            }),
            running: false,
            capture_active: false,
            latest_snapshot: Arc::new(Mutex::new(None)),
            worker: None,
        }
    }

    fn spawn_worker(&mut self) -> anyhow::Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }

        let (command_tx, command_rx) = mpsc::channel();
        let settings = Arc::clone(&self.settings);
        let latest_snapshot = Arc::clone(&self.latest_snapshot);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);

        let join_handle = thread::Builder::new()
            .name("hypercolor-screen-capture".to_owned())
            .spawn(move || {
                run_worker(&settings, &latest_snapshot, &command_rx, &worker_cancel);
            })
            .map_err(|error| anyhow!("failed to spawn screen capture worker: {error}"))?;

        self.worker = Some(CaptureWorker {
            command_tx,
            join_handle,
            cancel,
        });
        Ok(())
    }

    fn shutdown_worker(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };

        worker.cancel.store(true, Ordering::Release);
        let _ = worker.command_tx.send(WorkerCommand::Stop);
        if worker.join_handle.join().is_err() {
            warn!("screen capture worker panicked during shutdown");
        }
    }

    fn set_capture_active_state(&mut self, active: bool) -> anyhow::Result<()> {
        if self.capture_active == active {
            return Ok(());
        }
        self.capture_active = active;

        if !self.running {
            return Ok(());
        }

        if active {
            self.spawn_worker()?;
        }

        if let Some(worker) = self.worker.as_ref() {
            let _ = worker.command_tx.send(WorkerCommand::SetActive(active));
        }

        if !active && let Ok(mut latest) = self.latest_snapshot.lock() {
            *latest = None;
        }

        Ok(())
    }

    /// Publish new settings and bump the generation the worker polls.
    fn reconfigure(&mut self, config: CaptureConfig) {
        if let Ok(mut current) = self.settings.config.lock() {
            if *current == config {
                return;
            }
            *current = config;
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
        self.running = true;

        if self.capture_active {
            self.spawn_worker()?;
            if let Some(worker) = self.worker.as_ref() {
                let _ = worker.command_tx.send(WorkerCommand::SetActive(true));
            }
        } else {
            debug!(
                "Windows screen capture armed but idle until a screen-reactive effect requests capture"
            );
        }

        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.capture_active = false;
        self.shutdown_worker();

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
            .map_err(|_| anyhow!("windows screen capture snapshot mutex poisoned"))?;

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
    latest_snapshot: &Arc<Mutex<Option<ScreenData>>>,
    command_rx: &mpsc::Receiver<WorkerCommand>,
    cancel: &Arc<AtomicBool>,
) {
    let mut config = settings.snapshot();
    let mut generation = settings.generation.load(Ordering::Acquire);
    let mut analyzer = ScreenCaptureInput::new(config.clone());
    let mut duplicator: Option<DesktopDuplicator> = None;
    let mut active = false;
    let mut open_failure_logged = false;

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
            config = settings.snapshot();
            analyzer = ScreenCaptureInput::new(config.clone());
            if let Some(duplicator) = duplicator.as_mut() {
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
                    thread::sleep(REOPEN_BACKOFF);
                    continue;
                }
            },
        };

        match session.next_frame(FRAME_WAIT) {
            Ok(Some(frame)) => {
                analyzer.push_frame(frame.rgba, frame.width, frame.height);
                if let Ok(InputData::Screen(snapshot)) = analyzer.sample()
                    && let Ok(mut latest) = latest_snapshot.lock()
                {
                    *latest = Some(snapshot);
                }
            }
            // Static desktop or pointer-only update: nothing new to analyze.
            Ok(None) => {}
            Err(error) => {
                warn!(%error, "Windows screen capture frame failed; reopening session");
                duplicator = None;
                thread::sleep(REOPEN_BACKOFF);
            }
        }
    }

    if let Ok(mut latest) = latest_snapshot.lock() {
        *latest = None;
    }
    debug!("Windows screen capture worker stopped");
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
