//! Progress-aware startup deadline, restart budget, and retry of a
//! supervisor that gave up.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hypercolor_app::supervisor::{
    DAEMON_STARTUP_CEILING, DAEMON_STARTUP_STALL_WINDOW, DAEMON_STARTUP_TIMEOUT,
    HEALTH_PROBE_TIMEOUT, MAX_HEALTH_BODY_BYTES, StartupGaveUp, StartupProbe, StartupStall,
    StartupVerdict, StartupWatch, SupervisorFailure, SupervisorState, WATCHDOG_FAILURE_WINDOW,
    WATCHDOG_MAX_RAPID_RESTARTS, probe_startup, restart_budget_exhausted, watchdog_gives_up,
};
use hypercolor_types::api::system::{
    DaemonStartupPhase, DaemonStartupProgress, HEALTH_STATUS_STARTING, HealthChecks, HealthResponse,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

const fn secs(value: u64) -> Duration {
    Duration::from_secs(value)
}

const fn starting(phase: DaemonStartupPhase, sequence: u64) -> StartupProbe {
    StartupProbe::Starting {
        phase,
        sequence,
        detail: None,
    }
}

fn compiling(sequence: u64, pipeline: &str) -> StartupProbe {
    StartupProbe::Starting {
        phase: DaemonStartupPhase::StartingRenderThread,
        sequence,
        detail: Some(pipeline.to_owned()),
    }
}

// ── Startup deadline ────────────────────────────────────────────────────

#[test]
fn a_daemon_that_never_answers_keeps_the_fixed_deadline() {
    let mut watch = StartupWatch::default();
    let just_before = DAEMON_STARTUP_TIMEOUT - Duration::from_millis(1);

    assert_eq!(
        watch.observe(just_before, StartupProbe::Silent),
        StartupVerdict::Wait
    );
    assert_eq!(
        watch.observe(DAEMON_STARTUP_TIMEOUT, StartupProbe::Silent),
        StartupVerdict::GiveUp(StartupStall::NoAnswer)
    );
    assert_eq!(watch.last_phase(), None);
}

#[test]
fn ready_wins_at_any_point() {
    let mut watch = StartupWatch::default();
    assert_eq!(
        watch.observe(secs(1), starting(DaemonStartupPhase::Initializing, 0)),
        StartupVerdict::Wait
    );
    assert_eq!(
        watch.observe(secs(90), StartupProbe::Ready),
        StartupVerdict::Ready
    );
}

#[test]
fn progress_resets_the_stall_clock() {
    let mut watch = StartupWatch::default();
    let mut elapsed = Duration::ZERO;

    // Each report lands just inside the stall window after the previous
    // one, so a startup well past the no-answer deadline keeps going.
    for (sequence, phase) in [
        DaemonStartupPhase::Initializing,
        DaemonStartupPhase::ProbingGpu,
        DaemonStartupPhase::ScanningEffects,
        DaemonStartupPhase::StartingRenderThread,
    ]
    .into_iter()
    .enumerate()
    {
        elapsed += DAEMON_STARTUP_STALL_WINDOW - secs(1);
        assert_eq!(
            watch.observe(elapsed, starting(phase, sequence as u64)),
            StartupVerdict::Wait,
            "progress at {elapsed:?} should keep the wait open"
        );
    }
    assert!(elapsed > DAEMON_STARTUP_TIMEOUT);
    assert_eq!(
        watch.last_phase(),
        Some(DaemonStartupPhase::StartingRenderThread)
    );
}

#[test]
fn no_progress_past_the_stall_window_gives_up_naming_the_phase() {
    let mut watch = StartupWatch::default();
    let reported_at = secs(3);
    watch.observe(
        reported_at,
        starting(DaemonStartupPhase::StartingRenderThread, 6),
    );

    // The same sequence again is not progress, and neither is silence.
    let inside = reported_at + DAEMON_STARTUP_STALL_WINDOW - Duration::from_millis(1);
    assert_eq!(
        watch.observe(
            inside,
            starting(DaemonStartupPhase::StartingRenderThread, 6)
        ),
        StartupVerdict::Wait
    );
    assert_eq!(
        watch.observe(
            reported_at + DAEMON_STARTUP_STALL_WINDOW,
            StartupProbe::Silent
        ),
        StartupVerdict::GiveUp(StartupStall::Stalled {
            phase: DaemonStartupPhase::StartingRenderThread,
        })
    );
}

#[test]
fn a_daemon_restarted_mid_wait_counts_as_progress() {
    // A service manager restarted the daemon: the new process counts its
    // startup from zero again, and that fresh startup is progress.
    let mut watch = StartupWatch::default();
    watch.observe(secs(1), starting(DaemonStartupPhase::LoadingStores, 4));
    let restarted_at = secs(15);
    assert_eq!(
        watch.observe(restarted_at, starting(DaemonStartupPhase::Initializing, 0)),
        StartupVerdict::Wait
    );
    assert_eq!(watch.last_phase(), Some(DaemonStartupPhase::Initializing));

    let inside = restarted_at + DAEMON_STARTUP_STALL_WINDOW - Duration::from_millis(1);
    assert_eq!(
        watch.observe(inside, StartupProbe::Silent),
        StartupVerdict::Wait
    );
    assert_eq!(
        watch.observe(
            restarted_at + DAEMON_STARTUP_STALL_WINDOW,
            StartupProbe::Silent
        ),
        StartupVerdict::GiveUp(StartupStall::Stalled {
            phase: DaemonStartupPhase::Initializing,
        })
    );
}

#[test]
fn a_slow_compile_that_keeps_finishing_pipelines_is_not_killed() {
    // One phase far longer than the stall window: a cold shader compile
    // where every pipeline takes most of the window, but each completes.
    let mut watch = StartupWatch::default();
    let per_pipeline = DAEMON_STARTUP_STALL_WINDOW - secs(2);
    let mut elapsed = secs(2);
    let mut sequence = 6;
    assert_eq!(
        watch.observe(
            elapsed,
            starting(DaemonStartupPhase::StartingRenderThread, sequence)
        ),
        StartupVerdict::Wait
    );
    for pipeline in [
        "SparkleFlinger GPU compose pipeline",
        "SparkleFlinger GPU source copy pipeline",
        "SparkleFlinger GPU area horizontal tile scan",
    ] {
        elapsed += per_pipeline;
        sequence += 1;
        assert_eq!(
            watch.observe(elapsed, compiling(sequence, pipeline)),
            StartupVerdict::Wait,
            "{pipeline} completed at {elapsed:?}"
        );
    }
    assert!(elapsed > DAEMON_STARTUP_STALL_WINDOW * 2);
    assert_eq!(
        watch.last_phase(),
        Some(DaemonStartupPhase::StartingRenderThread)
    );
}

#[test]
fn a_hung_compile_inside_the_phase_still_trips_the_window_and_is_named() {
    let mut watch = StartupWatch::default();
    let last_completed_at = secs(30);
    watch.observe(
        secs(10),
        starting(DaemonStartupPhase::StartingRenderThread, 6),
    );
    watch.observe(
        last_completed_at,
        compiling(7, "SparkleFlinger GPU area horizontal tile scan"),
    );

    // The next compile starts and never finishes: a new step name with
    // the same sequence is work in flight, not progress.
    let hung = "SparkleFlinger GPU area vertical tile scan";
    let inside = last_completed_at + DAEMON_STARTUP_STALL_WINDOW - Duration::from_millis(1);
    assert_eq!(
        watch.observe(inside, compiling(7, hung)),
        StartupVerdict::Wait
    );
    let verdict = watch.observe(
        last_completed_at + DAEMON_STARTUP_STALL_WINDOW,
        compiling(7, hung),
    );
    let StartupVerdict::GiveUp(stall) = verdict else {
        panic!("a compile that stops finishing must trip the stall window, got {verdict:?}");
    };
    assert_eq!(
        stall,
        StartupStall::Stalled {
            phase: DaemonStartupPhase::StartingRenderThread,
        }
    );
    assert_eq!(
        watch.gave_up(stall),
        StartupGaveUp {
            stall,
            detail: Some(hung.to_owned()),
        }
    );
}

#[test]
fn the_hard_ceiling_ends_a_startup_that_keeps_progressing() {
    let mut watch = StartupWatch::default();
    let step = secs(5);
    let mut elapsed = Duration::ZERO;
    let mut sequence = 0;
    while elapsed + step < DAEMON_STARTUP_CEILING {
        elapsed += step;
        sequence += 1;
        assert_eq!(
            watch.observe(
                elapsed,
                starting(DaemonStartupPhase::StartingServices, sequence)
            ),
            StartupVerdict::Wait
        );
    }
    assert_eq!(
        watch.observe(
            DAEMON_STARTUP_CEILING,
            starting(DaemonStartupPhase::StartingServices, sequence + 1)
        ),
        StartupVerdict::GiveUp(StartupStall::Ceiling {
            phase: DaemonStartupPhase::StartingServices,
        })
    );
}

#[test]
fn startup_deadlines_keep_their_documented_shape() {
    assert_eq!(DAEMON_STARTUP_TIMEOUT, secs(20));
    assert_eq!(DAEMON_STARTUP_STALL_WINDOW, secs(20));
    assert!(DAEMON_STARTUP_CEILING > DAEMON_STARTUP_TIMEOUT);
    assert!(DAEMON_STARTUP_CEILING > DAEMON_STARTUP_STALL_WINDOW);
}

#[test]
fn stall_reports_expose_the_phase_and_reason() {
    assert_eq!(StartupStall::NoAnswer.phase(), None);
    assert_eq!(StartupStall::NoAnswer.reason(), "no_answer");
    let stalled = StartupStall::Stalled {
        phase: DaemonStartupPhase::ScanningEffects,
    };
    assert_eq!(stalled.phase(), Some(DaemonStartupPhase::ScanningEffects));
    assert_eq!(stalled.reason(), "stalled");
    assert_eq!(
        StartupStall::Ceiling {
            phase: DaemonStartupPhase::PreparingApi,
        }
        .reason(),
        "ceiling"
    );
}

// ── Probe classification ────────────────────────────────────────────────

fn health(status: &str, startup: Option<DaemonStartupProgress>) -> HealthResponse {
    HealthResponse {
        status: status.to_owned(),
        version: "0.6.2".to_owned(),
        uptime_seconds: 2,
        checks: HealthChecks::default(),
        startup,
    }
}

#[test]
fn only_a_starting_report_counts_as_startup_progress() {
    let report = DaemonStartupProgress {
        phase: DaemonStartupPhase::ProbingGpu,
        sequence: 1,
        detail: None,
    };
    assert_eq!(
        StartupProbe::from_unavailable_health(&health(HEALTH_STATUS_STARTING, Some(report))),
        starting(DaemonStartupPhase::ProbingGpu, 1)
    );
    assert_eq!(
        StartupProbe::from_unavailable_health(&health("degraded", None)),
        StartupProbe::Silent
    );
    assert_eq!(
        StartupProbe::from_unavailable_health(&health(HEALTH_STATUS_STARTING, None)),
        StartupProbe::Silent
    );
}

/// Answer one HTTP request on a loopback port with a canned response.
async fn one_shot_server(response: Vec<u8>) -> Url {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener binds");
    let addr = listener.local_addr().expect("listener address resolves");
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("probe connects");
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).await;
        let _ = stream.write_all(&response).await;
        let _ = stream.shutdown().await;
    });
    Url::parse(&format!("http://{addr}")).expect("server url parses")
}

fn http_response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

#[tokio::test]
async fn probe_reads_the_startup_report_from_a_503() {
    let body = serde_json::to_vec(&health(
        HEALTH_STATUS_STARTING,
        Some(DaemonStartupProgress {
            phase: DaemonStartupPhase::StartingRenderThread,
            sequence: 6,
            detail: Some("SparkleFlinger GPU compose pipeline".to_owned()),
        }),
    ))
    .expect("health encodes");
    let base = one_shot_server(http_response("503 Service Unavailable", &body)).await;

    assert_eq!(
        probe_startup(&reqwest::Client::new(), &base, HEALTH_PROBE_TIMEOUT).await,
        compiling(6, "SparkleFlinger GPU compose pipeline")
    );
}

#[tokio::test]
async fn probe_treats_200_as_ready_and_oversized_bodies_as_silent() {
    let ready = one_shot_server(http_response("200 OK", b"{}")).await;
    assert_eq!(
        probe_startup(&reqwest::Client::new(), &ready, HEALTH_PROBE_TIMEOUT).await,
        StartupProbe::Ready
    );

    let oversized = one_shot_server(http_response(
        "503 Service Unavailable",
        &vec![b' '; MAX_HEALTH_BODY_BYTES + 1],
    ))
    .await;
    assert_eq!(
        probe_startup(&reqwest::Client::new(), &oversized, HEALTH_PROBE_TIMEOUT).await,
        StartupProbe::Silent
    );
}

// ── Restart budget ──────────────────────────────────────────────────────

#[test]
fn the_breaker_trips_on_the_last_allowed_failure_without_waiting() {
    let fresh_window = Some(secs(10));
    assert!(!restart_budget_exhausted(
        WATCHDOG_MAX_RAPID_RESTARTS - 1,
        fresh_window
    ));
    // The failure that fills the budget trips the breaker immediately, so
    // the watchdog skips the backoff it would otherwise sleep first.
    assert!(restart_budget_exhausted(
        WATCHDOG_MAX_RAPID_RESTARTS,
        fresh_window
    ));
    assert!(!restart_budget_exhausted(
        WATCHDOG_MAX_RAPID_RESTARTS,
        Some(WATCHDOG_FAILURE_WINDOW + secs(1))
    ));
}

#[test]
fn slow_failed_startups_still_trip_the_breaker_after_the_window_expires() {
    // Five startups that each ran to the ceiling outlast the rapid-restart
    // window, so the window alone would reset and restart forever.
    let window_age = Some(DAEMON_STARTUP_CEILING * WATCHDOG_MAX_RAPID_RESTARTS);
    assert!(window_age > Some(WATCHDOG_FAILURE_WINDOW));
    assert!(!restart_budget_exhausted(1, window_age));
    assert!(watchdog_gives_up(
        1,
        window_age,
        WATCHDOG_MAX_RAPID_RESTARTS
    ));
    assert!(!watchdog_gives_up(
        1,
        window_age,
        WATCHDOG_MAX_RAPID_RESTARTS - 1
    ));
    // Crashes after a healthy run keep the rapid-window rule.
    assert!(watchdog_gives_up(
        WATCHDOG_MAX_RAPID_RESTARTS,
        Some(secs(30)),
        0
    ));
}

// ── Retry ───────────────────────────────────────────────────────────────

fn daemon_url() -> Url {
    Url::parse("http://127.0.0.1:9420").expect("daemon url parses")
}

fn render_thread_failure() -> SupervisorFailure {
    SupervisorFailure {
        restarts: WATCHDOG_MAX_RAPID_RESTARTS,
        startup_phase: Some(DaemonStartupPhase::StartingRenderThread),
    }
}

#[test]
fn retry_clears_a_permanent_failure_exactly_once() {
    let state = SupervisorState::default();
    let notifications = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&notifications);
    state.install_status_listener(move || {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    state.remember_daemon_url(daemon_url());
    assert!(state.begin_retry().is_none(), "nothing to retry yet");

    state.record_permanent_failure(render_thread_failure());
    assert!(state.permanent_failure());
    assert_eq!(state.supervisor_failure(), Some(render_thread_failure()));

    let retry = state.begin_retry().expect("a latched failure is retryable");
    assert_eq!(retry.daemon_url, daemon_url());
    assert_eq!(retry.failure, render_thread_failure());
    assert!(!state.permanent_failure());
    assert_eq!(state.supervisor_failure(), None);
    assert!(
        state.begin_retry().is_none(),
        "a second click cannot start a second watchdog"
    );
    assert_eq!(notifications.load(Ordering::SeqCst), 2);
}

#[test]
fn a_retry_that_cannot_restart_restores_the_failure() {
    let state = SupervisorState::default();
    state.remember_daemon_url(daemon_url());
    state.record_permanent_failure(render_thread_failure());

    let retry = state.begin_retry().expect("failure is retryable");
    state.record_permanent_failure(retry.failure);

    assert!(state.permanent_failure());
    assert_eq!(state.supervisor_failure(), Some(render_thread_failure()));
}

#[test]
fn retry_without_a_started_supervisor_leaves_the_failure_latched() {
    let state = SupervisorState::default();
    state.record_permanent_failure(render_thread_failure());

    assert!(state.begin_retry().is_none());
    assert!(state.permanent_failure());
}

#[test]
fn any_supervision_start_supersedes_a_latched_failure() {
    let state = SupervisorState::default();
    let notifications = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&notifications);
    state.install_status_listener(move || {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    state.remember_daemon_url(daemon_url());
    state.record_permanent_failure(render_thread_failure());

    // An owner handover starts supervision without going through Retry.
    state.clear_permanent_failure();

    assert!(!state.permanent_failure());
    assert_eq!(state.supervisor_failure(), None);
    assert!(
        state.begin_retry().is_none(),
        "a stale Retry must not start a second watchdog"
    );
    state.clear_permanent_failure();
    assert_eq!(
        notifications.load(Ordering::SeqCst),
        2,
        "clearing an unlatched state is silent"
    );
}
