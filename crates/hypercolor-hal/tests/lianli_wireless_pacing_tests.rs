//! Acknowledgement-paced RGB on the L-Wireless controller, against a
//! simulated TX, radio, fan receivers, and RX (`support/lianli_wireless_fake.rs`).
//!
//! The fans echo the tag of the transfer they last took in the RX device
//! table, refreshed on the fans' own status cadence. The protocol treats
//! the echo as a cumulative acknowledgement and keeps a window of frames in
//! flight per cluster, sized to what the radio drains, so delivery tracks
//! the offered rate when the radio keeps up and the radio's own rate, with
//! a bounded backlog, when it does not.

#[path = "support/lianli_wireless_fake.rs"]
mod fake;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fake::{FRAME_PERIOD, FakeRadio, LEDS, Rig, content_of, moving_frame};
use hypercolor_hal::drivers::lianli::wireless::pacing::{
    ECHO_STALL, MAX_RESETS_WITHOUT_DELIVERY, MAX_WINDOW, resets_without_delivery,
};
use hypercolor_hal::protocol::{Protocol, TransferType};

/// Envelopes one transfer puts on the air: the header twice, one data
/// envelope for three fans' worth of compressed pixels.
const TRANSFER_ENVELOPES: usize = 3;
/// Envelopes the 1 Hz upkeep adds per cluster (PWM) and per controller
/// (clock).
const UPKEEP_ENVELOPES: usize = 2;

/// A radio that carries 50 envelopes a second: well under the 90 a second
/// that 30 fps of unpaced transfers offer it.
fn slow_radio() -> FakeRadio {
    let mut radio = FakeRadio::one_cluster();
    radio.air_time = Duration::from_millis(20);
    radio
}

/// The owner's fans: status refreshed about every 333 ms.
const STATUS_CADENCE: Duration = Duration::from_millis(333);

/// Transfers a second the slow radio carries: 50 envelopes of 3.
const SLOW_RADIO_TRANSFERS_PER_S: u64 = 50 / 3;

/// The most envelopes the TX may hold for one cluster on the slow radio at
/// the 333 ms cadence: 20 transfers, a little over a second of what it
/// carries. A backlog is believed only once sends outlast the radio's
/// delay plus a whole status interval, at three echoes in a row, since a
/// status can be a snapshot that stale; the batches the window releases at
/// each status queue under that. The window settles near 50 envelopes.
const SLOW_BACKLOG_ENVELOPES: usize = 20 * TRANSFER_ENVELOPES + UPKEEP_ENVELOPES;

/// Estimated URBs the RX took for its polls since `from`: one OUT, seven
/// IN packets per page, and the gap read that ends a two-page upkeep
/// reply. On the owner's rig the one-frame window of PR 318 ran 64 to 72
/// a second.
fn rx_urbs_since(rig: &Rig, from: usize) -> usize {
    rig.radio.rx_polls[from..]
        .iter()
        .map(|(_, pages)| 1 + 7 * usize::from(*pages) + usize::from(*pages > 1))
        .sum()
}

/// One five-second slice of a long run.
#[derive(Debug)]
struct Slice {
    /// Frames the echoes retired.
    delivered: u64,
    /// The most envelopes the TX held.
    queue: usize,
    /// Mean age of the confirmed sends at their echoes, pooled over the
    /// slice: TX wait, air, and status wait together. A mean, so it bounds
    /// the typical wait, not every frame's.
    echo_age: Duration,
    /// Transfers each cluster took.
    taken: Vec<usize>,
}

/// Run `count` five-second slices.
fn slices(rig: &mut Rig, count: usize) -> Vec<Slice> {
    (0..count)
        .map(|_| {
            let before = rig.stats();
            let taken_before: Vec<usize> = rig
                .radio
                .clusters
                .iter()
                .map(|cluster| cluster.applied_log.len())
                .collect();
            rig.radio.max_air_queue = 0;
            rig.run_for(Duration::from_secs(5));
            let after = rig.stats();
            let samples = u32::try_from(after.echo_samples - before.echo_samples).expect("samples");
            Slice {
                delivered: after.frames_delivered - before.frames_delivered,
                queue: rig.radio.max_air_queue,
                echo_age: after
                    .echo_total
                    .saturating_sub(before.echo_total)
                    .checked_div(samples)
                    .unwrap_or_default(),
                taken: rig
                    .radio
                    .clusters
                    .iter()
                    .zip(&taken_before)
                    .map(|(cluster, before)| cluster.applied_log.len() - before)
                    .collect(),
            }
        })
        .collect()
}

/// Frames the render path offered, and frames confirmed, over `span`
/// after `warmup`.
fn steady_delivery(rig: &mut Rig, warmup: Duration, span: Duration) -> (u64, u64) {
    rig.run_for(warmup);
    let warm = rig.stats();
    rig.radio.max_air_queue = 0;
    rig.run_for(span);
    let stats = rig.stats();
    (
        stats.frames_offered - warm.frames_offered,
        stats.frames_delivered - warm.frames_delivered,
    )
}

/// Pixel hash of frame `frame` of the moving scene.
fn tag_of(frame: u64) -> [u8; 4] {
    content_of(&moving_frame(frame))
}

/// Distinct frames the first fan cluster took, a frame taken again right
/// after itself (a restore, a resend that also landed) counted once.
fn distinct_applied(rig: &Rig) -> usize {
    let log = &rig.radio.clusters[0].applied_log;
    log.iter()
        .enumerate()
        .filter(|(index, tag)| *index == 0 || log[index - 1] != **tag)
        .count()
}

/// A radio slower than the stream is kept near its own rate, and the window
/// keeps the TX backlog bounded instead of letting it grow.
#[test]
fn a_radio_slower_than_the_stream_runs_near_its_own_rate_with_a_bounded_backlog() {
    let mut radio = slow_radio();
    radio.report_interval = STATUS_CADENCE;
    let mut rig = Rig::connect(radio);
    let (_, delivered) = steady_delivery(&mut rig, Duration::from_secs(6), Duration::from_secs(10));

    assert!(
        delivered * 100 >= SLOW_RADIO_TRANSFERS_PER_S * 10 * 80,
        "at least four fifths of the radio's own rate: {delivered} frames in 10 s"
    );
    assert!(
        rig.radio.max_air_queue <= SLOW_BACKLOG_ENVELOPES,
        "the TX holds a bounded backlog, not a growing queue: {} envelopes",
        rig.radio.max_air_queue
    );
    let stats = rig.stats();
    assert!(
        stats.congestion_events > 0,
        "the window reacted to the backlog: {stats:?}"
    );
    assert_eq!(
        stats.resends, 0,
        "a slow radio is not a lossy one: {stats:?}"
    );
    assert!(rig.radio.resets.is_empty());
}

#[test]
fn a_radio_that_keeps_up_gets_every_frame_the_render_path_offers() {
    let mut rig = Rig::connect(FakeRadio::one_cluster());
    rig.run_for(Duration::from_secs(10));

    let stats = rig.stats();
    let offered = stats.frames_offered;
    assert!(
        stats.frames_delivered * 10 >= offered * 9,
        "the window must not cost frames the radio could carry: {stats:?}"
    );
    assert!(
        stats.frames_delivered >= 270,
        "about 30 fps over 10 s: {stats:?}"
    );
    assert!(
        rig.radio.max_air_queue <= TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
        "a radio that keeps up drains each transfer before the next: {}",
        rig.radio.max_air_queue
    );
}

/// The owner's fans refresh their device-table status about three times a
/// second, so an echo lands on a ~333 ms cadence however fast the radio is.
/// A radio that keeps up must still get close to every offered frame: the
/// echo is a cumulative acknowledgement, not a per-frame one. The one-frame
/// window this replaces delivered 30 of 303 frames here, as it did on the
/// owner's hardware.
#[test]
fn a_status_report_every_333_ms_still_delivers_near_the_offered_rate() {
    let mut radio = FakeRadio::one_cluster();
    radio.report_interval = STATUS_CADENCE;
    let mut rig = Rig::connect(radio);
    rig.run_for(Duration::from_secs(3));
    let polls_warm = rig.radio.rx_polls.len();
    let (offered, delivered) = steady_delivery(&mut rig, Duration::ZERO, Duration::from_secs(10));

    assert!(
        delivered * 10 >= offered * 9,
        "a radio that keeps up delivers within a tenth of the offered rate: {delivered} of {offered} in 10 s"
    );
    let stats = rig.stats();
    assert!(
        stats.advance_max >= 8,
        "each status confirms many frames at once: {stats:?}"
    );
    assert!(
        rig.protocol_window(0).is_some_and(|window| window >= 10),
        "the window grew to cover a status interval of frames: {:?}",
        rig.protocol_window(0)
    );
    assert!(
        rig.radio.max_air_queue <= TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
        "the window never builds a backlog on a radio that keeps up: {}",
        rig.radio.max_air_queue
    );
    let polls = rig.radio.rx_polls.len() - polls_warm;
    assert!(
        polls <= 10 * 10,
        "echo polls follow the status cadence, a few a second: {polls} in 10 s"
    );
    let urbs = rx_urbs_since(&rig, polls_warm);
    assert!(
        urbs <= 72 * 10,
        "streaming at 30 fps costs the RX no more than the one-frame window's 3 fps did: {urbs} URBs in 10 s"
    );
    assert!(
        rig.radio.resets.is_empty(),
        "a slow status report is not a wedge"
    );
}

/// The owner's rig under the sliding window: the status comes every 550 ms
/// or so, give or take 150, and each is a snapshot up to 400 ms stale. That
/// staleness is not a backlog: the radio keeps up, so the window must hold
/// near the offered rate instead of halving on every stale status. PR 320
/// as first run on that rig halved on it, and the fans took 13 to 15 fps.
#[test]
fn a_jittery_550_ms_status_holds_delivery_near_the_offered_rate() {
    let mut radio = FakeRadio::one_cluster();
    radio.report_interval = Duration::from_millis(550);
    radio.report_jitter = Duration::from_millis(150);
    radio.snapshot_lag = Duration::from_millis(400);
    let mut rig = Rig::connect(radio);
    rig.run_for(Duration::from_secs(5));
    let before = rig.stats();
    let steady = slices(&mut rig, 6);

    // What the fans took, slice by slice: the echoes land a status at a
    // time, so what they confirm per slice swings by a status's worth.
    for slice in &steady {
        assert!(
            slice.taken[0] >= 135,
            "the fans take at least 27 fps in every 5 s slice of a radio that keeps up: {steady:?}"
        );
    }
    let stats = rig.stats();
    let offered = stats.frames_offered - before.frames_offered;
    let delivered = stats.frames_delivered - before.frames_delivered;
    assert!(
        delivered * 10 >= offered * 9,
        "the echoes confirm within a tenth of the offered rate: {delivered} of {offered} in 30 s"
    );
    assert_eq!(
        stats.congestion_events, 0,
        "status staleness never halves the window: {stats:?}"
    );
    assert!(rig.radio.resets.is_empty());
}

/// A status as slow as a second, which the owner's rig also showed: the
/// window grows to cover it.
#[test]
fn a_one_second_status_still_delivers_near_the_offered_rate() {
    let mut radio = FakeRadio::one_cluster();
    radio.report_interval = Duration::from_secs(1);
    radio.report_jitter = Duration::from_millis(100);
    radio.snapshot_lag = Duration::from_millis(200);
    let mut rig = Rig::connect(radio);
    rig.run_for(Duration::from_secs(8));
    let before = rig.stats();
    let steady = slices(&mut rig, 4);

    for slice in &steady {
        assert!(
            slice.taken[0] >= 135,
            "the fans take at least 27 fps in every 5 s slice: {steady:?}"
        );
    }
    let stats = rig.stats();
    let offered = stats.frames_offered - before.frames_offered;
    let delivered = stats.frames_delivered - before.frames_delivered;
    assert!(
        delivered * 10 >= offered * 9,
        "{delivered} of {offered} frames confirmed in 20 s"
    );
    assert_eq!(stats.congestion_events, 0, "{stats:?}");
    assert!(
        rig.protocol_window(0).is_some_and(|window| window > 32),
        "the window covers a second of frames and more: {:?}",
        rig.protocol_window(0)
    );
}

/// The status cadence on the owner's rig moved from 333 ms to 500 ms
/// mid-run. The window follows it without losing the stream.
#[test]
fn a_status_cadence_that_slows_mid_stream_keeps_delivery_near_the_offered_rate() {
    let mut radio = FakeRadio::one_cluster();
    radio.report_interval = STATUS_CADENCE;
    let mut rig = Rig::connect(radio);
    rig.run_for(Duration::from_secs(5));
    rig.radio.report_interval = Duration::from_millis(500);
    let (offered, delivered) =
        steady_delivery(&mut rig, Duration::from_secs(3), Duration::from_secs(10));

    assert!(
        delivered * 10 >= offered * 9,
        "{delivered} of {offered} frames after the cadence slowed"
    );
    assert!(rig.radio.resets.is_empty());
}

/// Capacity that changes under a long stream: the window settles to the
/// slow radio within one slice, holds a flat backlog for the rest of the
/// slow stretch, and returns to the full rate when the radio speeds up
/// again.
#[test]
fn the_window_follows_capacity_changes_over_a_long_stream() {
    let mut radio = FakeRadio::one_cluster();
    radio.report_interval = STATUS_CADENCE;
    let mut rig = Rig::connect(radio);
    let fast = slices(&mut rig, 4);
    rig.radio.air_time = Duration::from_millis(20);
    let slow = slices(&mut rig, 8);
    rig.radio.air_time = Duration::from_millis(2);
    let _ = slices(&mut rig, 1);
    let fast_again = slices(&mut rig, 3);

    for slice in fast[1..].iter().chain(&fast_again) {
        assert!(
            slice.delivered >= 140,
            "near 150 frames in 5 s at full speed: {fast:?} {fast_again:?}"
        );
        assert!(
            slice.queue <= TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
            "no backlog: {fast:?} {fast_again:?}"
        );
    }
    let capacity = SLOW_RADIO_TRANSFERS_PER_S * 5;
    for slice in &slow[1..] {
        assert!(
            slice.delivered * 100 >= capacity * 75,
            "three quarters of the slow radio's rate in every slice: {slow:?}"
        );
        assert!(
            slice.queue <= SLOW_BACKLOG_ENVELOPES,
            "a bounded backlog: {slow:?}"
        );
        assert!(
            slice.echo_age <= Duration::from_secs(1),
            "the mean echo age stays under a second: {slow:?}"
        );
    }
    let early = slow[1..4]
        .iter()
        .map(|slice| slice.queue)
        .max()
        .unwrap_or(0);
    let late = slow[5..].iter().map(|slice| slice.queue).max().unwrap_or(0);
    assert!(
        late <= early + TRANSFER_ENVELOPES,
        "the backlog does not creep: {slow:?}"
    );
}

#[test]
fn a_held_frame_goes_out_as_soon_as_an_echo_frees_room() {
    let mut rig = Rig::connect(slow_radio());
    // A scene that stops changing: frame 40 is the last the render path
    // publishes, and it arrives while the window is full.
    rig.frame_limit = Some(41);
    rig.run_for(Duration::from_secs(3));

    assert_eq!(
        rig.radio.clusters[0].showing(),
        Some(tag_of(40)),
        "the newest frame reaches the fans with no later frame to carry it"
    );
}

#[test]
fn every_transfer_carries_the_newest_frame_the_protocol_has() {
    let mut rig = Rig::connect(slow_radio());
    rig.run_for(Duration::from_secs(5));

    let tags: HashMap<[u8; 4], u64> = (0..200).map(|frame| (tag_of(frame), frame)).collect();
    let mut previous = None;
    for (at, _, content) in &rig.radio.tx_transfers {
        let frame = *tags
            .get(content)
            .expect("every transfer carries a published frame");
        let newest = rig
            .taken_log
            .iter()
            .rev()
            .find(|(taken_at, _)| taken_at <= at)
            .map(|(_, index)| *index)
            .expect("a frame was taken before any transfer");
        assert_eq!(
            frame, newest,
            "a transfer at {at:?} carried frame {frame}, not the newest ({newest})"
        );
        assert!(
            previous.is_none_or(|previous| frame >= previous),
            "frames go out in order, none queued behind a newer one"
        );
        previous = Some(frame);
    }
    let stats = rig.stats();
    assert!(
        stats.frames_coalesced > 0,
        "frames that arrived while the window was closed were replaced, not queued: {stats:?}"
    );
    let accounted = stats.frames_sent + stats.frames_coalesced;
    assert!(
        stats.frames_offered == accounted || stats.frames_offered == accounted + 1,
        "every offered frame was sent or replaced by a newer one, but the one held now: {stats:?}"
    );
}

#[test]
fn a_slow_status_cadence_costs_no_resends_and_no_backlog() {
    let mut radio = FakeRadio::one_cluster();
    radio.report_interval = Duration::from_millis(400);
    let mut rig = Rig::connect(radio);
    let (offered, delivered) =
        steady_delivery(&mut rig, Duration::from_secs(5), Duration::from_secs(10));
    let stats = rig.stats();

    assert_eq!(
        stats.resends, 0,
        "a slow status is not a lost frame: {stats:?}"
    );
    assert_eq!(stats.timeouts, 0, "{stats:?}");
    assert!(
        delivered * 10 >= offered * 9,
        "{delivered} of {offered} frames with a 400 ms status"
    );
    assert!(rig.radio.resets.is_empty(), "a slow status is not a wedge");
    assert!(
        rig.radio.max_air_queue <= TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
        "no backlog: {}",
        rig.radio.max_air_queue
    );
}

/// A radio whose echoes lag two seconds behind outlasts the first guess at
/// the timeout. The first sends are given up on and probed; when their
/// echo lands it counts as delivered, late, and the timeout learned from
/// its age covers every later wait.
#[test]
fn a_late_echo_counts_as_delivered_and_resyncs_the_timeout() {
    let mut radio = FakeRadio::one_cluster();
    radio.report_delay = Duration::from_secs(2);
    let mut rig = Rig::connect(radio);
    rig.run_for(Duration::from_secs(4));
    let early = rig.stats();
    rig.run_for(Duration::from_secs(12));
    let stats = rig.stats();

    assert!(
        early.late_echoes >= 1,
        "the given-up sends' echo is recognised: {early:?}"
    );
    assert!(
        early.frames_delivered >= early.late_echoes,
        "a late echo confirms frames: {early:?}"
    );
    assert_eq!(
        stats.timeouts, early.timeouts,
        "the echo age learned from it covers every later wait: {early:?} then {stats:?}"
    );
    assert!(
        rig.protocol_window(0).is_some_and(|window| window >= 16),
        "slow start was not cut short by the first guess: {:?}",
        rig.protocol_window(0)
    );
    assert!(rig.radio.resets.is_empty(), "a late radio is not a wedge");
}

#[test]
fn fans_that_stop_confirming_end_in_one_tx_reset_not_endless_resends() {
    let mut rig = Rig::connect(FakeRadio::one_cluster());
    rig.run_for(Duration::from_secs(2));
    assert!(rig.stats().frames_delivered > 0, "the radio worked first");

    // The TX keeps taking USB writes but nothing reaches the air, while
    // the RX still hears the fans report their last frame.
    rig.radio.rf_dead = true;
    let failed_at = rig.now();
    rig.run_for(Duration::from_secs(20));

    assert_eq!(
        rig.radio.resets.len(),
        1,
        "exactly one reset request: {:?}",
        rig.radio.resets
    );
    // The transfer already on the air when the radio died starts the
    // stall, a frame or so before it was noticed here.
    let after = rig.radio.resets[0].saturating_sub(failed_at);
    assert!(
        after + FRAME_PERIOD >= ECHO_STALL && after <= ECHO_STALL + Duration::from_secs(3),
        "the reset follows the stall bound, not a hair trigger: {after:?}"
    );
    let transfers = rig.transfers_since(failed_at);
    assert!(
        transfers <= usize::try_from(MAX_WINDOW).expect("small"),
        "a dead radio gets a bounded number of resends, not a stream: {transfers}"
    );
    assert_eq!(
        rig.commands_after_reset, 0,
        "nothing follows the reset request; the transport ends the session"
    );
}

/// Firmware whose echo never tracks live frames, or a TX a reset does not
/// revive, must not turn into a reset loop across reconnects: each cluster
/// gets a small reset budget until it confirms a frame again, and past it
/// the cluster's lighting is held.
#[test]
fn a_tx_that_never_delivers_spends_a_bounded_reset_budget_then_holds() {
    let mut radio = FakeRadio::one_cluster();
    radio.rf_dead = true;
    let cluster = radio.clusters[0].mac;
    let mut rig = Rig::connect(radio);

    let mut resets = 0;
    for session in 0..=MAX_RESETS_WITHOUT_DELIVERY {
        if session > 0 {
            rig = rig.reconnect();
        }
        rig.run_for(ECHO_STALL + Duration::from_secs(5));
        let settled = rig.now();
        rig.run_for(Duration::from_secs(20));
        resets += rig.radio.resets.len();
        if session < MAX_RESETS_WITHOUT_DELIVERY {
            assert_eq!(
                rig.radio.resets.len(),
                1,
                "session {session} resets the TX once"
            );
        } else {
            assert!(
                rig.radio.resets.is_empty(),
                "the budget is spent, so the session holds instead"
            );
            assert_eq!(
                rig.transfers_since(settled),
                0,
                "a held cluster gets no RGB at all, not a slow probe"
            );
        }
        assert_eq!(rig.commands_after_reset, 0);
    }
    assert_eq!(
        u32::try_from(resets).expect("small"),
        MAX_RESETS_WITHOUT_DELIVERY
    );
    assert_eq!(
        resets_without_delivery(cluster),
        MAX_RESETS_WITHOUT_DELIVERY
    );

    // A power-cycled controller whose fans confirm again gets its budget
    // back.
    rig.radio.rf_dead = false;
    let mut rig = rig.reconnect();
    rig.run_for(Duration::from_secs(2));
    assert!(rig.stats().frames_delivered > 0);
    assert_eq!(resets_without_delivery(cluster), 0);
}

/// A healthy cluster confirming frames must not renew the reset budget of
/// a cluster beside it that never does, or the controller would be reset
/// forever. The failing cluster is held once its budget is spent, and the
/// healthy one keeps streaming.
#[test]
fn a_healthy_cluster_does_not_renew_a_failing_clusters_reset_budget() {
    let mut radio = FakeRadio::two_clusters();
    radio.clusters[1].deaf = true;
    let failing = radio.clusters[1].mac;
    let mut rig = Rig::connect(radio);
    rig.frame = Box::new(|index| {
        let mut colors = moving_frame(index);
        colors.extend(moving_frame(index + 7_777));
        colors
    });

    let mut resets = 0;
    for session in 0..=MAX_RESETS_WITHOUT_DELIVERY {
        if session > 0 {
            rig = rig.reconnect();
        }
        rig.run_for(ECHO_STALL + Duration::from_secs(10));
        resets += rig.radio.resets.len();
    }
    assert_eq!(
        u32::try_from(resets).expect("small"),
        MAX_RESETS_WITHOUT_DELIVERY,
        "the failing cluster's budget bounds the resets"
    );
    assert_eq!(
        resets_without_delivery(failing),
        MAX_RESETS_WITHOUT_DELIVERY
    );

    let healthy_before = rig.radio.clusters[0].applied_log.len();
    rig.run_for(Duration::from_secs(5));
    assert!(rig.radio.resets.is_empty(), "no further reset");
    assert!(
        rig.radio.clusters[0].applied_log.len() > healthy_before + 50,
        "the healthy cluster keeps streaming beside the held one"
    );
}

/// After a reconnect the receivers still echo the last session's transfer.
/// A new session's first frame, even of the same pixels, must not be taken
/// as confirmed by that cached report.
#[test]
fn a_cached_echo_from_the_last_session_confirms_nothing() {
    let mut rig = Rig::connect(FakeRadio::one_cluster());
    // A still scene: every session sends the same pixels first. The first
    // session ends before any upkeep restore, so the fans still echo its
    // very first transfer, the one a new session's first send would
    // collide with if send numbers started over.
    rig.frame_limit = Some(1);
    rig.run_for(Duration::from_millis(500));
    let first = rig.stats();
    assert_eq!(
        (first.frames_delivered, first.restores),
        (1, 0),
        "the first session confirmed its one transfer: {first:?}"
    );

    rig.radio.rf_dead = true;
    let mut rig = rig.reconnect();
    rig.run_for(Duration::from_secs(3));
    assert_eq!(
        rig.stats().frames_delivered,
        0,
        "nothing reached the fans this session, so nothing is confirmed: {:?}",
        rig.stats()
    );
}

/// A radio that takes 360 ms to carry one transfer: the window keeps a few
/// transfers queued, costs no resends, and uses most of what the radio
/// carries.
#[test]
fn a_very_slow_radio_gets_no_resends_and_a_bounded_backlog() {
    let mut radio = FakeRadio::one_cluster();
    // 120 ms an envelope: a transfer takes 360 ms of air, 2.8 a second.
    radio.air_time = Duration::from_millis(120);
    radio.report_interval = STATUS_CADENCE;
    let mut rig = Rig::connect(radio);
    let (_, delivered) = steady_delivery(&mut rig, Duration::from_secs(5), Duration::from_secs(10));
    let stats = rig.stats();

    assert_eq!(stats.resends, 0, "{stats:?}");
    assert!(
        delivered >= 17,
        "most of the radio's 28 transfers in 10 s: {delivered}"
    );
    assert!(
        rig.radio.max_air_queue <= 8 * TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
        "a bounded backlog: {} envelopes",
        rig.radio.max_air_queue
    );
    assert!(rig.radio.resets.is_empty(), "a slow radio is not a wedge");
}

#[test]
fn fans_the_rx_cannot_hear_are_sent_nothing_and_resume_when_heard() {
    let mut rig = Rig::connect(FakeRadio::one_cluster());
    rig.run_for(Duration::from_secs(2));
    rig.radio.fans_silent = true;
    let silent_at = rig.now();
    rig.run_for(Duration::from_secs(4));
    let held_at = rig.now();
    rig.run_for(Duration::from_secs(16));

    assert!(
        rig.radio.resets.is_empty(),
        "unheard fans are not a TX wedge"
    );
    assert!(
        rig.transfers_since(silent_at) <= 8,
        "a few resends before the fans count as unheard"
    );
    assert_eq!(
        rig.transfers_since(held_at),
        0,
        "nothing goes to fans the RX cannot hear"
    );

    rig.radio.fans_silent = false;
    let heard_at = rig.now();
    rig.run_for(Duration::from_secs(2));
    assert!(
        rig.transfers_since(heard_at) > 20,
        "streaming resumes once the fans answer"
    );
    let newest = rig.taken_log.last().expect("frames").1;
    assert!(
        rig.radio.clusters[0]
            .applied_log
            .iter()
            .rev()
            .take(3)
            .any(|tag| *tag == tag_of(newest) || *tag == tag_of(newest - 1)),
        "the fans are back on the live frame"
    );
}

#[test]
fn delivered_and_sent_counts_match_what_the_fans_took() {
    let mut rig = Rig::connect(FakeRadio::one_cluster());
    let tx_before = rig.radio.tx_packets;
    let rx_before = rig.radio.rx_polls.len();
    rig.run_for(Duration::from_secs(10));
    let stats = rig.stats();

    assert_eq!(stats.frames_offered, rig.frames_taken());
    // The run can stop between a frame landing and the poll that sees it.
    let applied = distinct_applied(&rig);
    let delivered = usize::try_from(stats.frames_delivered).expect("count");
    assert!(
        delivered == applied || delivered + 1 == applied,
        "delivered is the frames the fans took ({applied}): {stats:?}"
    );
    assert!(
        stats.frames_sent - stats.frames_delivered <= 1,
        "a lossless radio delivers every frame sent but the one out now: {stats:?}"
    );
    assert_eq!(
        rig.radio.tx_transfers.len(),
        usize::try_from(stats.frames_sent + stats.restores + stats.resends).expect("count"),
        "every transfer on the wire is a frame, a restore, or a resend"
    );
    assert_eq!(
        stats.tx_packets,
        rig.radio.tx_packets - tx_before,
        "the TX packet count matches what the TX took, upkeep included"
    );
    let echo_polls = rig.radio.rx_polls[rx_before..]
        .iter()
        .filter(|(_, pages)| *pages == 1)
        .count();
    assert_eq!(
        usize::try_from(stats.echo_polls).expect("count"),
        echo_polls,
        "echo polls read one page for a one-cluster table"
    );
    assert_eq!(
        usize::try_from(stats.table_replies).expect("count"),
        rig.radio.rx_polls.len(),
        "every poll the RX answered reached the protocol"
    );
}

/// Frames lost on the air while the stream flows are overtaken by the next
/// one before the status refreshes: nothing needs sending again, and the
/// cumulative count cannot tell them from frames shown briefly.
#[test]
fn a_lossy_radio_streaming_needs_no_resends() {
    let mut radio = FakeRadio::one_cluster();
    // Lose every eleventh RGB envelope on the air.
    radio.drop_rgb_envelope = Some(Box::new(|number| number % 11 == 10));
    radio.report_interval = STATUS_CADENCE;
    let mut rig = Rig::connect(radio);
    let (offered, delivered) =
        steady_delivery(&mut rig, Duration::from_secs(3), Duration::from_secs(10));
    let stats = rig.stats();

    assert_eq!(stats.resends, 0, "{stats:?}");
    assert!(delivered * 10 >= offered * 9, "{delivered} of {offered}");
    let applied = distinct_applied(&rig);
    assert!(
        stats.frames_delivered >= u64::try_from(applied).expect("count") - 1,
        "every frame the fans took is counted: {applied} taken, {stats:?}"
    );
    assert!(
        stats.frames_delivered <= stats.frames_sent,
        "and never more than was sent: {stats:?}"
    );
    assert!(rig.radio.resets.is_empty(), "packet loss is not a wedge");
}

/// When the last frame of a scene is lost, no later frame overtakes it: the
/// echo stays on the frame before. Whichever comes first, the restore the
/// next upkeep owes or the probe after the timeout, puts the newest frame
/// on the fans within a couple of seconds.
#[test]
fn a_lost_final_frame_is_resent_after_the_timeout() {
    let losing = Arc::new(AtomicBool::new(false));
    let mut radio = FakeRadio::one_cluster();
    radio.report_interval = STATUS_CADENCE;
    let lose = Arc::clone(&losing);
    radio.drop_rgb_envelope = Some(Box::new(move |_| lose.load(Ordering::Relaxed)));
    let mut rig = Rig::connect(radio);
    rig.frame_limit = Some(60);
    // Frame 59, the last, is published at 59 frame periods; the radio
    // loses everything from just before it until it is out.
    let before_last = (FRAME_PERIOD * 59).saturating_sub(Duration::from_millis(10));
    rig.run_for(before_last.saturating_sub(rig.now()));
    losing.store(true, Ordering::Relaxed);
    rig.run_for(Duration::from_millis(100));
    losing.store(false, Ordering::Relaxed);
    assert_ne!(
        rig.radio.clusters[0].showing(),
        Some(tag_of(59)),
        "the last frame was lost"
    );
    rig.run_for(Duration::from_secs(2));

    assert_eq!(
        rig.radio.clusters[0].showing(),
        Some(tag_of(59)),
        "the newest frame reached the fans again"
    );
    let stats = rig.stats();
    assert!(stats.restores + stats.resends >= 1, "{stats:?}");
    assert!(rig.radio.resets.is_empty());
}

/// Two clusters sharing a slow radio for five minutes: together they keep
/// most of what it carries, each keeps streaming, and the shared backlog
/// stays flat long after the base-delay memory has turned over. Each
/// cluster tolerates a backlog of its own, so the shared one is up to
/// twice a single cluster's. With a ten-second memory the base delay rose
/// with the backlog, and after about 75 s both windows grew without bound.
#[test]
fn two_clusters_share_a_slow_radio_with_a_bounded_backlog() {
    let mut radio = FakeRadio::two_clusters();
    radio.air_time = Duration::from_millis(20);
    radio.report_interval = STATUS_CADENCE;
    let mut rig = Rig::connect(radio);
    // Both clusters' pixels change every frame.
    rig.frame = Box::new(|index| {
        let mut colors = moving_frame(index);
        colors.extend(moving_frame(index + 7_777));
        colors
    });
    let first = slices(&mut rig, 36);
    let before_last = rig.stats();
    let last = slices(&mut rig, 24);
    let stats = rig.stats();

    let capacity = SLOW_RADIO_TRANSFERS_PER_S * 5;
    for slice in first[1..].iter().chain(&last) {
        assert!(
            slice.delivered * 100 >= capacity * 80,
            "the two share four fifths of what the radio carries: {first:?} {last:?}"
        );
        assert!(
            slice.queue <= 2 * SLOW_BACKLOG_ENVELOPES,
            "a bounded backlog per cluster: {first:?} {last:?}"
        );
        assert!(
            slice.echo_age <= Duration::from_millis(1_500),
            "the mean echo age stays under one and a half seconds: {first:?} {last:?}"
        );
        assert!(
            slice
                .taken
                .iter()
                .all(|taken| u64::try_from(*taken).expect("count") * 100 >= capacity * 25),
            "each cluster gets at least a quarter of the radio in every slice: {first:?} {last:?}"
        );
    }
    for cluster in &rig.radio.clusters {
        assert!(
            cluster.applied_log.len() > 1_500,
            "each cluster takes over 1,500 transfers in five minutes: {}",
            cluster.applied_log.len()
        );
    }
    let third_minute = first[24..]
        .iter()
        .map(|slice| slice.queue)
        .max()
        .unwrap_or(0);
    let last_minutes = last.iter().map(|slice| slice.queue).max().unwrap_or(0);
    assert!(
        last_minutes <= third_minute + 2 * TRANSFER_ENVELOPES,
        "the shared backlog stays flat: {third_minute} then {last_minutes} envelopes"
    );
    assert!(
        stats.congestion_events > before_last.congestion_events,
        "the windows still answer the backlog in the last two minutes: {stats:?}"
    );
}

/// A cluster whose echoes keep arriving late is still delivering, so it
/// never reaches the stall verdict. Clusters are paced independently, so it
/// never holds its healthy neighbour back.
#[test]
fn a_cluster_with_persistently_late_echoes_does_not_starve_its_neighbour() {
    let mut radio = FakeRadio::two_clusters();
    radio.clusters[1].report_delay = Some(Duration::from_millis(2_500));
    let mut rig = Rig::connect(radio);
    rig.frame = Box::new(|index| {
        let mut colors = moving_frame(index);
        colors.extend(moving_frame(index + 7_777));
        colors
    });
    rig.run_for(Duration::from_secs(5));
    let healthy_before = rig.radio.clusters[0].applied_log.len();
    rig.run_for(Duration::from_secs(15));

    assert!(rig.radio.resets.is_empty(), "late delivery is not a wedge");
    assert!(
        rig.radio.clusters[0].applied_log.len() > healthy_before + 300,
        "the healthy cluster keeps streaming: {} frames in 15 s",
        rig.radio.clusters[0].applied_log.len() - healthy_before
    );
    assert!(
        rig.radio.clusters[1].applied_log.len() > 5,
        "the late cluster keeps catching up"
    );
}

/// Three clusters oversubscribing a slow radio with a one-second status for
/// eight minutes: the backlog stays well short of the stall verdict, so
/// the TX is never reset, and every cluster keeps streaming. Each cluster
/// halving only its own window left the others to keep the shared queue
/// full; echo ages passed five seconds and a cluster starved into a reset.
#[test]
fn three_clusters_on_a_slow_radio_with_a_slow_status_never_reach_the_stall_verdict() {
    let mut radio = FakeRadio::two_clusters();
    radio
        .clusters
        .push(FakeRadio::one_cluster().clusters.remove(0));
    radio.air_time = Duration::from_millis(20);
    radio.report_interval = Duration::from_secs(1);
    radio.report_jitter = Duration::from_millis(100);
    radio.snapshot_lag = Duration::from_millis(200);
    let mut rig = Rig::connect(radio);
    rig.frame = Box::new(|index| {
        let mut colors = moving_frame(index);
        colors.extend(moving_frame(index + 7_777));
        colors.extend(moving_frame(index + 15_554));
        colors
    });
    let run = slices(&mut rig, 96);

    assert!(
        rig.radio.resets.is_empty(),
        "a backlog is not a wedge: {} resets",
        rig.radio.resets.len()
    );
    let capacity = SLOW_RADIO_TRANSFERS_PER_S * 5;
    for slice in &run[6..] {
        assert!(
            slice.echo_age <= Duration::from_secs(4),
            "the mean echo age stays well short of the 5 s stall verdict: {run:?}"
        );
        assert!(
            slice.delivered * 100 >= capacity * 70,
            "the three keep most of what the radio carries: {run:?}"
        );
        assert!(
            slice
                .taken
                .iter()
                .all(|taken| u64::try_from(*taken).expect("count") * 100 >= capacity * 10),
            "each cluster gets at least a tenth of the radio in every slice: {run:?}"
        );
    }
}

#[test]
fn a_still_scene_costs_only_the_upkeep_the_fans_need() {
    let mut rig = Rig::connect(FakeRadio::one_cluster());
    rig.frame_limit = Some(1);
    rig.run_for(Duration::from_secs(2));
    let before = rig.stats();
    let polls_before = rig.radio.rx_polls.len();
    rig.run_for(Duration::from_secs(10));
    let stats = rig.stats();

    assert_eq!(stats.frames_sent, before.frames_sent, "nothing new to send");
    // Per second: one PWM envelope for the cluster, one clock envelope,
    // and the confirmed frame again after the PWM, four packets per
    // envelope; nothing else reaches the TX.
    let packets_per_second = (stats.tx_packets - before.tx_packets) / 10;
    assert_eq!(packets_per_second, 4 + 4 + TRANSFER_ENVELOPES as u64 * 4);
    // Per second: the table poll, and an echo poll or three until the
    // restore's own echo comes back.
    let polls = rig.radio.rx_polls.len() - polls_before;
    assert!(
        (10..=41).contains(&polls),
        "a few RX polls a second when still: {polls} in 10 s"
    );
    assert_eq!(
        stats.restores - before.restores,
        10,
        "one restore after each PWM upkeep"
    );
}

#[test]
fn shutdown_sends_the_final_frame_the_window_held() {
    let mut rig = Rig::connect(slow_radio());
    rig.run_for(Duration::from_secs(1));
    let black = vec![[0, 0, 0]; LEDS];
    let black_tag = content_of(&black);

    // Frames back to back until the window is full; the black one after
    // them is held.
    let mut commands = Vec::new();
    for index in 0..u64::from(MAX_WINDOW) {
        rig.protocol
            .encode_frame_into(&moving_frame(9_000 + index), &mut commands);
    }
    rig.protocol.encode_frame_into(&black, &mut commands);
    assert!(commands.is_empty(), "the black frame is held");

    let shutdown = rig.protocol.shutdown_sequence();
    assert!(!shutdown.is_empty(), "the held black frame is flushed");
    assert!(
        shutdown
            .iter()
            .all(|command| command.transfer_type == TransferType::Primary),
        "shutdown only writes to the TX"
    );
    rig.execute(&shutdown);
    rig.radio.air_time = Duration::ZERO;
    let drained = rig.now() + Duration::from_secs(1);
    rig.radio.advance_to(drained);
    assert_eq!(
        rig.radio.clusters[0].showing(),
        Some(black_tag),
        "black is the last frame the fans take"
    );
}
