//! Progress-aware startup deadline for a daemon the supervisor waits on.
//!
//! A starting daemon answers `/health` with `503` and a startup report whose
//! sequence advances as it works. The supervisor keeps waiting while that
//! sequence moves and gives up only when it stalls, or when the whole
//! startup outlives a hard ceiling. Before the port answers, a child whose
//! output the supervisor owns shows a sign of life by writing to it, and
//! that output opens the same stall window. A daemon that neither answers
//! nor writes (one hung before its first log line, or a service manager's
//! daemon whose output the app never sees) keeps the original fixed
//! deadline.

use std::time::Duration;

use hypercolor_types::api::system::{DaemonStartupPhase, HEALTH_STATUS_STARTING, HealthResponse};

/// How long a daemon may go without answering `/health` with a startup
/// report, and without writing any output, before the supervisor gives up
/// on it.
pub const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a daemon that reports startup progress, or that has written
/// output but not answered yet, may go without a further sign of progress
/// before the supervisor treats it as stuck.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupProbe {
    /// `/health` answered `2xx`: the daemon is fully ready.
    Ready,
    /// `/health` answered `503` with a startup report.
    Starting {
        phase: DaemonStartupPhase,
        sequence: u64,
        /// The step running inside the phase, when the daemon names one.
        detail: Option<String>,
    },
    /// No usable answer: refused, timed out, or any other status or body.
    Silent,
}

impl StartupProbe {
    /// Classify a `503` health body.
    #[must_use]
    pub fn from_unavailable_health(health: &HealthResponse) -> Self {
        match &health.startup {
            Some(progress) if health.status == HEALTH_STATUS_STARTING => Self::Starting {
                phase: progress.phase,
                sequence: progress.sequence,
                detail: progress.detail.clone(),
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
    /// The daemon never answered with a startup report, and wrote no
    /// output, within [`DAEMON_STARTUP_TIMEOUT`].
    NoAnswer,
    /// The daemon wrote output but never answered with a startup report:
    /// it went [`DAEMON_STARTUP_STALL_WINDOW`] past its last output, or
    /// reached [`DAEMON_STARTUP_CEILING`].
    NoAnswerAfterOutput,
    /// The daemon reported progress, then none for
    /// [`DAEMON_STARTUP_STALL_WINDOW`].
    Stalled { phase: DaemonStartupPhase },
    /// The daemon was still starting at [`DAEMON_STARTUP_CEILING`].
    Ceiling { phase: DaemonStartupPhase },
}

/// A startup wait that ended without a ready daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupGaveUp {
    /// Why the wait gave up.
    pub stall: StartupStall,
    /// The step the daemon last reported running, which for a stall is
    /// the step it is stuck in.
    pub detail: Option<String>,
}

impl StartupStall {
    /// The last phase the daemon reported, if it reported one.
    #[must_use]
    pub const fn phase(self) -> Option<DaemonStartupPhase> {
        match self {
            Self::NoAnswer | Self::NoAnswerAfterOutput => None,
            Self::Stalled { phase } | Self::Ceiling { phase } => Some(phase),
        }
    }

    /// Short stable label for logs.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::NoAnswer => "no_answer",
            Self::NoAnswerAfterOutput => "no_answer_after_output",
            Self::Stalled { .. } => "stalled",
            Self::Ceiling { .. } => "ceiling",
        }
    }
}

/// Tracks one daemon startup across successive probes.
#[derive(Debug, Default, Clone)]
pub struct StartupWatch {
    last_report: Option<(DaemonStartupPhase, u64)>,
    last_detail: Option<String>,
    last_progress_at: Duration,
    last_output_at: Option<Duration>,
}

impl StartupWatch {
    /// Fold one probe taken `elapsed` after the daemon was started.
    pub fn observe(&mut self, elapsed: Duration, probe: StartupProbe) -> StartupVerdict {
        match probe {
            StartupProbe::Ready => return StartupVerdict::Ready,
            StartupProbe::Starting {
                phase,
                sequence,
                detail,
            } => {
                // Only the sequence measures progress. Any change counts,
                // including a drop: a service manager that restarts the
                // daemon mid-wait starts the count over, and that new
                // startup is progress. The ceiling still bounds a pair of
                // daemons trading answers. A new step name alone is not
                // progress; it is work that has started, not finished.
                let advanced = self
                    .last_report
                    .is_none_or(|(_, last_sequence)| sequence != last_sequence);
                if advanced {
                    self.last_report = Some((phase, sequence));
                    self.last_progress_at = elapsed;
                }
                self.last_detail = detail;
            }
            StartupProbe::Silent => {}
        }

        let Some((phase, _)) = self.last_report else {
            return match self.last_output_at {
                None if elapsed >= DAEMON_STARTUP_TIMEOUT => {
                    StartupVerdict::GiveUp(StartupStall::NoAnswer)
                }
                Some(last_output_at)
                    if elapsed >= DAEMON_STARTUP_CEILING
                        || elapsed.saturating_sub(last_output_at)
                            >= DAEMON_STARTUP_STALL_WINDOW =>
                {
                    StartupVerdict::GiveUp(StartupStall::NoAnswerAfterOutput)
                }
                None | Some(_) => StartupVerdict::Wait,
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

    /// Fold new output from the daemon, seen `elapsed` after it was
    /// started.
    ///
    /// Before the daemon answers with a startup report, output is the only
    /// sign that it is running at all (process launch, an antivirus scan of
    /// a fresh binary, configuration loading), so each new output opens the
    /// stall window afresh in place of the fixed no-answer deadline. Once a
    /// report arrives, its sequence is the only measure of progress and
    /// output is ignored, so log noise cannot hide a stuck startup.
    pub fn observe_output(&mut self, elapsed: Duration) {
        if self.last_report.is_none() {
            self.last_output_at = Some(elapsed);
        }
    }

    /// The last phase the daemon reported.
    #[must_use]
    pub fn last_phase(&self) -> Option<DaemonStartupPhase> {
        self.last_report.map(|(phase, _)| phase)
    }

    /// The step the daemon last reported running inside its phase.
    #[must_use]
    pub fn last_detail(&self) -> Option<&str> {
        self.last_detail.as_deref()
    }

    /// Describe a give-up `stall` with the step the daemon was last in.
    #[must_use]
    pub fn gave_up(&self, stall: StartupStall) -> StartupGaveUp {
        StartupGaveUp {
            stall,
            detail: self.last_detail.clone(),
        }
    }
}

/// The supervised daemon's log, watched for the child's own output.
///
/// The supervisor hands the child this file as its stdout and stderr, so
/// growth after the probe opens is output from the child. The size is read
/// through the open handle, which stays exact on Windows, where a path
/// query can report a stale size for a file another process is writing.
#[derive(Debug)]
pub struct ChildOutputProbe {
    file: std::fs::File,
    seen_len: u64,
}

impl ChildOutputProbe {
    /// Watch `file` for growth past its current length.
    ///
    /// # Errors
    ///
    /// Returns an error when the file's metadata cannot be read.
    pub fn new(file: std::fs::File) -> std::io::Result<Self> {
        let seen_len = file.metadata()?.len();
        Ok(Self { file, seen_len })
    }

    /// Whether the file grew since the last check. A shrink (truncation)
    /// is not output; it only moves the baseline.
    pub fn has_new_output(&mut self) -> bool {
        let Ok(metadata) = self.file.metadata() else {
            return false;
        };
        let grew = metadata.len() > self.seen_len;
        self.seen_len = metadata.len();
        grew
    }
}
