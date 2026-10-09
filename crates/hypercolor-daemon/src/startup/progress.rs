//! Coarse startup progress shared between daemon startup and the API
//! listener's startup surface.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hypercolor_types::api::system::{DaemonStartupPhase, DaemonStartupProgress};
use tracing::info;

/// Cheaply cloned record of how far daemon startup has come.
///
/// Startup advances it at each phase boundary. The API listener reads it
/// to answer `/health` while the full router is still being assembled, so a
/// supervisor can tell a slow startup from a stuck one.
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
                }),
            }),
        }
    }
}

impl StartupProgress {
    /// Enter `phase` and advance the progress sequence.
    pub fn enter(&self, phase: DaemonStartupPhase) {
        let sequence = {
            let mut current = self
                .inner
                .current
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            current.phase = phase;
            current.sequence = current.sequence.saturating_add(1);
            current.sequence
        };
        info!(
            %phase,
            sequence,
            elapsed_ms = u64::try_from(self.elapsed().as_millis()).unwrap_or(u64::MAX),
            "Startup phase"
        );
    }

    /// The phase and sequence startup has reached.
    #[must_use]
    pub fn snapshot(&self) -> DaemonStartupProgress {
        *self
            .inner
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Time since this startup began.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.inner.started.elapsed()
    }
}
