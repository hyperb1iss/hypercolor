//! Progress-aware startup deadline for a daemon the supervisor waits on.
//!
//! A starting daemon answers `/health` with `503` and a startup report whose
//! sequence advances as it works. The supervisor keeps waiting while that
//! sequence moves and gives up only when it stalls, or when the whole
//! startup outlives a hard ceiling. A daemon that never answers at all
//! (one that predates the startup report, or one hung before binding its
//! port) keeps the original fixed deadline.

use std::time::Duration;

use hypercolor_types::api::system::{DaemonStartupPhase, HEALTH_STATUS_STARTING, HealthResponse};

/// How long a daemon may go without answering `/health` with a startup
/// report before the supervisor gives up on it.
pub const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a daemon that reports startup progress may go without that
/// progress advancing before the supervisor treats it as stuck.
pub const DAEMON_STARTUP_STALL_WINDOW: Duration = Duration::from_secs(20);

/// Ceiling on one whole startup, however steadily it reports progress.
///
/// A cold first launch can legitimately take far longer than a warm one:
/// GPU shader compilation without a driver cache, antivirus scanning a
/// freshly installed binary, and one-time store migrations all land on the
/// same boot. Two minutes covers that, while still restarting a daemon that
/// keeps inching through phases without ever becoming ready.
pub const DAEMON_STARTUP_CEILING: Duration = Duration::from_mins(2);

/// Largest `/health` body the supervisor reads while classifying a probe.
pub const MAX_HEALTH_BODY_BYTES: usize = 16 * 1024;

/// One `/health` probe, classified for the startup wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupProbe {
    /// `/health` answered `2xx`: the daemon is fully ready.
    Ready,
    /// `/health` answered `503` with a startup report.
    Starting {
        phase: DaemonStartupPhase,
        sequence: u64,
    },
    /// No usable answer: refused, timed out, or any other status or body.
    Silent,
}

impl StartupProbe {
    /// Classify a `503` health body.
    #[must_use]
    pub fn from_unavailable_health(health: &HealthResponse) -> Self {
        match health.startup {
            Some(progress) if health.status == HEALTH_STATUS_STARTING => Self::Starting {
                phase: progress.phase,
                sequence: progress.sequence,
            },
            _ => Self::Silent,
        }
    }
}

/// What the supervisor does after one probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupVerdict {
    /// The daemon is ready.
    Ready,
    /// Keep probing.
    Wait,
    /// Stop waiting; the daemon is not going to become ready.
    GiveUp(StartupStall),
}

/// Why a startup wait gave up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStall {
    /// The daemon never answered with a startup report within
    /// [`DAEMON_STARTUP_TIMEOUT`].
    NoAnswer,
    /// The daemon reported progress, then none for
    /// [`DAEMON_STARTUP_STALL_WINDOW`].
    Stalled { phase: DaemonStartupPhase },
    /// The daemon was still starting at [`DAEMON_STARTUP_CEILING`].
    Ceiling { phase: DaemonStartupPhase },
}

impl StartupStall {
    /// The last phase the daemon reported, if it reported one.
    #[must_use]
    pub const fn phase(self) -> Option<DaemonStartupPhase> {
        match self {
            Self::NoAnswer => None,
            Self::Stalled { phase } | Self::Ceiling { phase } => Some(phase),
        }
    }

    /// Short stable label for logs.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::NoAnswer => "no_answer",
            Self::Stalled { .. } => "stalled",
            Self::Ceiling { .. } => "ceiling",
        }
    }
}

/// Tracks one daemon startup across successive probes.
#[derive(Debug, Default, Clone)]
pub struct StartupWatch {
    last_report: Option<(DaemonStartupPhase, u64)>,
    last_progress_at: Duration,
}

impl StartupWatch {
    /// Fold one probe taken `elapsed` after the daemon was started.
    pub fn observe(&mut self, elapsed: Duration, probe: StartupProbe) -> StartupVerdict {
        match probe {
            StartupProbe::Ready => return StartupVerdict::Ready,
            StartupProbe::Starting { phase, sequence } => {
                // Any change counts, including a drop: a service manager
                // that restarts the daemon mid-wait starts the count over,
                // and that new startup is progress. The ceiling still
                // bounds a pair of daemons trading answers.
                let advanced = self
                    .last_report
                    .is_none_or(|(_, last_sequence)| sequence != last_sequence);
                if advanced {
                    self.last_report = Some((phase, sequence));
                    self.last_progress_at = elapsed;
                }
            }
            StartupProbe::Silent => {}
        }

        let Some((phase, _)) = self.last_report else {
            return if elapsed >= DAEMON_STARTUP_TIMEOUT {
                StartupVerdict::GiveUp(StartupStall::NoAnswer)
            } else {
                StartupVerdict::Wait
            };
        };
        if elapsed >= DAEMON_STARTUP_CEILING {
            StartupVerdict::GiveUp(StartupStall::Ceiling { phase })
        } else if elapsed.saturating_sub(self.last_progress_at) >= DAEMON_STARTUP_STALL_WINDOW {
            StartupVerdict::GiveUp(StartupStall::Stalled { phase })
        } else {
            StartupVerdict::Wait
        }
    }

    /// The last phase the daemon reported.
    #[must_use]
    pub fn last_phase(&self) -> Option<DaemonStartupPhase> {
        self.last_report.map(|(phase, _)| phase)
    }
}
