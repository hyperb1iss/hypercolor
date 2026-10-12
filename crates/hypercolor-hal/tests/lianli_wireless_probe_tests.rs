//! Recovery probes must get a chance to reach a returning wireless receiver.

use std::time::{Duration, Instant};

use hypercolor_hal::drivers::lianli::wireless::pacing::{
    ABSENT_AFTER, DeliveryPacer, ECHO_TIMEOUT_CAP, MAX_WINDOW, SendKind, StallVerdict, Tag,
};

const FRAME_PERIOD: Duration = Duration::from_millis(33);

fn warm_window() -> (DeliveryPacer, Instant, Tag) {
    let mut pacer = DeliveryPacer::default();
    pacer.ensure_clusters(1);
    let mut now = Instant::now();
    let mut echo = [0; 4];
    let mut next = 1_u32;
    pacer.observe(0, echo, now);

    for _ in 0..5 {
        let mut last_sent = None;
        for _ in 0..40 {
            let content = next.to_be_bytes();
            next += 1;
            pacer.submit(0, content);
            pacer.observe(0, echo, now);
            pacer.tick(now);
            pacer.mark_window_limits(now);
            if let Some(kind) = pacer.decide(0, now) {
                last_sent = Some(pacer.note_sent(0, content, kind, now));
            }
            now += FRAME_PERIOD;
        }
        echo = last_sent.expect("each warmup window sends frames");
        pacer.observe(0, echo, now);
    }

    assert_eq!(pacer.window(0), Some(MAX_WINDOW));
    (pacer, now, echo)
}

fn send(pacer: &mut DeliveryPacer, content: u32, now: Instant) -> (Tag, SendKind) {
    let content = content.to_be_bytes();
    pacer.submit(0, content);
    pacer.mark_window_limits(now);
    let kind = pacer.decide(0, now).expect("a send must have room");
    (pacer.note_sent(0, content, kind, now), kind)
}

#[test]
fn a_full_window_allows_a_recovery_probe_and_preserves_late_echoes() {
    let (mut pacer, start, echo) = warm_window();
    let before = pacer.totals();
    let mut first = None;
    for index in 0..MAX_WINDOW {
        let now = start + FRAME_PERIOD * index;
        pacer.observe(0, echo, now);
        let (wire, kind) = send(&mut pacer, 1_000 + index, now);
        assert_eq!(kind, SendKind::Frame);
        first.get_or_insert(wire);
    }
    assert!(!pacer.has_capacity(0), "the active window is full");

    let expired = start + Duration::from_millis(3_100);
    pacer.observe(0, echo, expired);
    pacer.tick(expired);
    let (probe, kind) = send(&mut pacer, 2_000, expired);
    assert_eq!(kind, SendKind::Resend);
    assert_eq!(pacer.totals().resends - before.resends, 1);
    assert_eq!(
        pacer.totals().frames_delivered,
        before.frames_delivered,
        "a timeout and a probe cannot invent delivery"
    );

    pacer.observe(
        0,
        first.expect("the full window has a first send"),
        expired + Duration::from_millis(10),
    );
    assert_eq!(pacer.totals().late_echoes - before.late_echoes, 1);
    assert_eq!(pacer.totals().frames_delivered - before.frames_delivered, 1);
    pacer.observe(0, probe, expired + Duration::from_millis(20));
    assert_eq!(
        pacer.totals().frames_delivered - before.frames_delivered,
        u64::from(MAX_WINDOW) + 1
    );
    assert!(
        pacer
            .stall_verdict(expired + Duration::from_millis(20))
            .is_none()
    );
}

#[test]
fn a_short_receiver_absence_requires_a_new_probe_and_its_observation_budget() {
    let (mut pacer, start, echo) = warm_window();
    send(&mut pacer, 1_000, start);
    let expired = start + Duration::from_millis(3_100);
    pacer.observe(0, echo, expired);
    pacer.tick(expired);
    assert_eq!(send(&mut pacer, 1_001, expired).1, SendKind::Resend);

    let missing = start + Duration::from_millis(4_400);
    pacer.observe(0, echo, missing);
    pacer.note_missing(0);
    assert!(pacer.decide(0, missing).is_none());
    assert!(
        pacer
            .stall_verdict(start + Duration::from_millis(5_100))
            .is_none(),
        "a missing receiver cannot justify resetting the transmitter"
    );

    let returned = start + Duration::from_millis(5_200);
    pacer.observe(0, [0; 4], returned);
    assert!(pacer.stall_verdict(returned).is_none());
    let (probe, kind) = send(&mut pacer, 1_002, returned);
    assert_eq!(kind, SendKind::Resend);

    let almost_due = (returned + ECHO_TIMEOUT_CAP)
        .checked_sub(Duration::from_millis(1))
        .expect("the probe budget exceeds one millisecond");
    pacer.observe(0, [0; 4], almost_due);
    assert!(
        pacer.stall_verdict(almost_due).is_none(),
        "the pre-absence probe cannot shorten the returning receiver's budget"
    );
    pacer.observe(0, probe, almost_due);
    assert!(pacer.stall_verdict(almost_due).is_none());
    assert!(!pacer.reset_requested());
}

#[test]
fn a_single_table_omission_preserves_send_and_poll_grace() {
    let (mut pacer, start, _) = warm_window();
    send(&mut pacer, 1_000, start);
    pacer.note_missing(0);
    assert_eq!(send(&mut pacer, 1_001, start).1, SendKind::Frame);
    assert!(
        pacer.poll_due(start + Duration::from_secs(2)),
        "a transient omission must not disable the poll that can find the receiver again"
    );
    assert!(!pacer.poll_due(start + Duration::from_secs(3)));
    assert!(
        pacer
            .stall_verdict(start + Duration::from_secs(5))
            .is_none()
    );
}

#[test]
fn cached_zero_tags_and_later_probes_do_not_postpone_reset_forever() {
    let (mut pacer, start, echo) = warm_window();
    send(&mut pacer, 1_000, start);
    let expired = start + Duration::from_millis(3_100);
    pacer.observe(0, echo, expired);
    pacer.tick(expired);
    assert_eq!(send(&mut pacer, 1_001, expired).1, SendKind::Resend);
    let deadline = expired + ECHO_TIMEOUT_CAP;

    for step in 1..12 {
        let now = expired + Duration::from_millis(500) * step;
        pacer.observe(0, [0; 4], now);
        assert!(pacer.stall_verdict(now).is_none());
    }

    pacer.observe(0, [0; 4], deadline);
    pacer.tick(deadline);
    assert_eq!(send(&mut pacer, 1_002, deadline).1, SendKind::Resend);
    let verdict = pacer.stall_verdict(deadline);
    assert!(
        matches!(verdict, Some(StallVerdict::Reset { resends: 2, .. })),
        "a later probe cannot renew the first probe's expired budget: {verdict:?}"
    );
    assert_eq!(pacer.totals().resends, 2);
}

#[test]
fn timeouts_without_an_emitted_probe_cannot_request_a_reset() {
    let mut pacer = DeliveryPacer::default();
    pacer.ensure_clusters(1);
    let start = Instant::now();
    pacer.observe(0, [0; 4], start);
    send(&mut pacer, 1_000, start);

    for elapsed in [2, 5, 10, 30] {
        let now = start + Duration::from_secs(elapsed);
        pacer.observe(0, [0; 4], now);
        pacer.tick(now);
        assert!(
            pacer.stall_verdict(now).is_none(),
            "a timeout is not evidence that a recovery probe was sent"
        );
    }
    assert!(pacer.totals().timeouts > 0);
    assert_eq!(pacer.totals().resends, 0);
    assert!(!pacer.reset_requested());
}

const TABLE_PERIOD: Duration = Duration::from_millis(100);

/// One complete table reply followed by one frame-pump pass, in the
/// protocol's order. The table either repeats the cluster's unchanged echo
/// or omits the cluster, which only a complete table can do.
fn stalled_step(
    pacer: &mut DeliveryPacer,
    content: u32,
    echo: Option<Tag>,
    now: Instant,
) -> Option<StallVerdict> {
    match echo {
        Some(echo) => pacer.observe(0, echo, now),
        None => pacer.note_missing(0),
    }
    let verdict = pacer.stall_verdict(now);
    pacer.note_absences(now);
    let content = content.to_be_bytes();
    pacer.submit(0, content);
    pacer.tick(now);
    pacer.mark_window_limits(now);
    if let Some(kind) = pacer.decide(0, now) {
        pacer.note_sent(0, content, kind, now);
    }
    verdict
}

#[test]
fn repeated_brief_omissions_cannot_renew_the_probe_budget_forever() {
    let (mut pacer, start, echo) = warm_window();
    let delivered = pacer.totals().frames_delivered;
    let omission_every = Duration::from_secs(5);
    let mut omissions = 0_u32;
    let mut first_return = None;
    let mut reset = None;

    for step in 0..400_u32 {
        let elapsed = TABLE_PERIOD * step;
        let now = start + elapsed;
        let omitted = elapsed.as_millis() % omission_every.as_millis() == 4_900;
        if omitted {
            omissions += 1;
        } else if omissions == 1 && first_return.is_none() {
            first_return = Some(now);
        }
        let verdict = stalled_step(&mut pacer, 10_000 + step, (!omitted).then_some(echo), now);
        if let Some(verdict) = verdict {
            reset = Some((now, verdict));
            break;
        }
    }

    let first_return = first_return.expect("the cluster returns after its first omission");
    let (reset_at, verdict) = reset.expect(
        "a stalled cluster that keeps reappearing with an unchanged echo must still reach a TX reset",
    );
    assert!(
        omissions >= 2,
        "the reset must survive a second brief omission, saw {omissions}"
    );
    assert!(
        reset_at <= first_return + ECHO_TIMEOUT_CAP + TABLE_PERIOD,
        "only the first return may restart the probe budget: reset {:?} after the first return",
        reset_at.saturating_duration_since(first_return)
    );
    assert!(
        matches!(verdict, StallVerdict::Reset { resends, .. } if resends >= 2),
        "probes sent before a brief omission remain evidence: {verdict:?}"
    );
    assert_eq!(
        pacer.totals().frames_delivered,
        delivered,
        "the echo never advanced"
    );
}

#[test]
fn a_genuine_absence_after_a_brief_omission_still_earns_a_fresh_probe_budget() {
    let (mut pacer, start, echo) = warm_window();
    let mut step = 0_u32;
    let mut now = start;
    let mut run = |pacer: &mut DeliveryPacer, until: Instant, echo: Option<Tag>| {
        while now < until {
            let verdict = stalled_step(pacer, 20_000 + step, echo, now);
            assert!(verdict.is_none(), "no reset before {until:?}: {verdict:?}");
            step += 1;
            now += TABLE_PERIOD;
        }
        now
    };

    let omitted_at = run(&mut pacer, start + Duration::from_millis(4_900), Some(echo));
    let returned = run(&mut pacer, omitted_at + TABLE_PERIOD, None);
    let gone_at = run(&mut pacer, returned + Duration::from_secs(2), Some(echo));
    // The receiver drops out of every complete table for longer than the
    // absence threshold, then comes back still echoing the old frame.
    let back_at = run(&mut pacer, gone_at + Duration::from_secs(4), None);
    let almost_due = run(
        &mut pacer,
        back_at + ECHO_TIMEOUT_CAP.saturating_sub(TABLE_PERIOD),
        Some(echo),
    );

    assert!(
        stalled_step(&mut pacer, 29_999, Some(echo), almost_due).is_none(),
        "a receiver that was really gone gets its full observation interval"
    );
    assert!(
        matches!(
            stalled_step(&mut pacer, 30_000, Some(echo), back_at + ECHO_TIMEOUT_CAP),
            Some(StallVerdict::Reset { .. })
        ),
        "the returned receiver's probe budget still expires"
    );
}

/// The actor can be awaiting a table read while a missing receiver crosses
/// the absence threshold, so no pump logs the absence before the reply
/// that brings it back. The return must still count as a genuine absence.
#[test]
fn a_return_that_crosses_the_absence_threshold_unlogged_earns_a_fresh_budget() {
    let (mut pacer, start, echo) = warm_window();
    let mut step = 0_u32;
    let mut at = |pacer: &mut DeliveryPacer, now: Instant, echo: Option<Tag>| {
        step += 1;
        stalled_step(pacer, 40_000 + step, echo, now)
    };

    // A brief omission spends the return grace; its probe deadline is due
    // six seconds after the return.
    let mut now = start;
    while now < start + Duration::from_millis(8_100) {
        let omitted = now == start + Duration::from_millis(4_900);
        assert!(at(&mut pacer, now, (!omitted).then_some(echo)).is_none());
        now += TABLE_PERIOD;
    }
    let last_seen = now
        .checked_sub(TABLE_PERIOD)
        .expect("the receiver was seen this run");

    // The receiver drops out of every complete table. The last pump before
    // the threshold runs just short of it; the next reply lands past it,
    // after that probe deadline has expired.
    while now < last_seen + ABSENT_AFTER {
        assert!(at(&mut pacer, now, None).is_none());
        now += TABLE_PERIOD;
    }
    let returned = last_seen + ABSENT_AFTER + Duration::from_millis(50);
    let resends = pacer.totals().resends;
    let verdict = at(&mut pacer, returned, Some(echo));
    assert!(
        verdict.is_none(),
        "a genuine absence earns a fresh probe before any reset: {verdict:?}"
    );
    assert_eq!(
        pacer.totals().resends,
        resends + 1,
        "the returning receiver is probed at once"
    );

    let mut now = returned + TABLE_PERIOD;
    while now < returned + ECHO_TIMEOUT_CAP {
        let verdict = at(&mut pacer, now, Some(echo));
        assert!(
            verdict.is_none(),
            "the fresh probe keeps its full observation interval: {verdict:?}"
        );
        now += TABLE_PERIOD;
    }
    assert!(
        matches!(
            at(&mut pacer, returned + ECHO_TIMEOUT_CAP, Some(echo)),
            Some(StallVerdict::Reset { .. })
        ),
        "the fresh probe's budget still expires"
    );
}
