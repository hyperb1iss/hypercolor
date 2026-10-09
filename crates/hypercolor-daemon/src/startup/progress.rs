//! Coarse startup progress shared between daemon startup and the API
//! listener's startup surface.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use hypercolor_types::api::system::{DaemonStartupPhase, DaemonStartupProgress};
use tracing::{debug, info};

/// A startup step this slow is logged at info, so a field report names it.
const SLOW_STARTUP_STEP: Duration = Duration::from_secs(1);

/// Cheaply cloned record of how far daemon startup has come.
///
/// Startup advances it at each phase boundary and after each completed
/// step inside a phase. The API listener reads it to answer `/health`
/// while the full router is still being assembled, so a supervisor can
/// tell a slow startup from a stuck one. Only completed work advances the
/// sequence; nothing ticks while work is merely in flight.
#[derive(Debug, Clone)]
pub struct StartupProgress {
    inner: Arc<StartupProgressInner>,
}

#[derive(Debug)]
struct StartupProgressInner {
    started: Instant,
    current: Mutex<DaemonStartupProgress>,
}

impl Default for StartupProgress {
    fn default() -> Self {
        Self {
            inner: Arc::new(StartupProgressInner {
                started: Instant::now(),
                current: Mutex::new(DaemonStartupProgress {
                    phase: DaemonStartupPhase::Initializing,
                    sequence: 0,
                    detail: None,
                }),
            }),
        }
    }
}

impl StartupProgress {
    /// Enter `phase` and advance the progress sequence.
    pub fn enter(&self, phase: DaemonStartupPhase) {
        let sequence = {
            let mut current = self.current();
            current.phase = phase;
            current.detail = None;
            current.sequence = current.sequence.saturating_add(1);
            current.sequence
        };
        info!(
            %phase,
            sequence,
            elapsed_ms = millis(self.elapsed()),
            "Startup phase"
        );
    }

    /// Run one unit of work inside the current phase.
    ///
    /// `detail` names the work while it runs, and the sequence advances
    /// once it returns, whatever it returns. A step that never returns
    /// leaves the sequence where it was and its name in the report.
    pub fn step<T>(&self, detail: &str, work: impl FnOnce() -> T) -> T {
        self.current().detail = Some(detail.to_owned());
        let started = Instant::now();
        let output = work();
        let step_time = started.elapsed();
        let (phase, sequence) = {
            let mut current = self.current();
            current.detail = None;
            current.sequence = current.sequence.saturating_add(1);
            (current.phase, current.sequence)
        };
        if step_time >= SLOW_STARTUP_STEP {
            info!(%phase, sequence, step = detail, step_ms = millis(step_time), "Slow startup step");
        } else {
            debug!(%phase, sequence, step = detail, step_ms = millis(step_time), "Startup step");
        }
        output
    }

    /// The phase, sequence, and running step startup has reached.
    #[must_use]
    pub fn snapshot(&self) -> DaemonStartupProgress {
        self.current().clone()
    }

    /// Time since this startup began.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.inner.started.elapsed()
    }

    fn current(&self) -> MutexGuard<'_, DaemonStartupProgress> {
        self.inner
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Run `work` as a startup step when startup is being reported, and as
/// plain work otherwise (a runtime rebuild, a test, a preview lane).
pub(crate) fn startup_step<T>(
    progress: Option<&StartupProgress>,
    detail: &str,
    work: impl FnOnce() -> T,
) -> T {
    match progress {
        Some(progress) => progress.step(detail, work),
        None => work(),
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
