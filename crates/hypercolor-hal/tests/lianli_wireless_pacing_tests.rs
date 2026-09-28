//! Acknowledgement-paced RGB on the L-Wireless controller, against a
//! simulated TX, radio, fan receivers, and RX (`support/lianli_wireless_fake.rs`).
//!
//! The fans echo the tag of the transfer they last took in the RX device
//! table. The protocol sends a cluster a new frame only once that echo
//! confirms the last one, so the TX never holds more unconfirmed RGB than one
//! transfer per cluster, and the frame rate is whatever the radio confirms.

#[path = "support/lianli_wireless_fake.rs"]
mod fake;

use std::collections::HashMap;
use std::time::Duration;

use fake::{FRAME_PERIOD, FakeRadio, LEDS, Rig, content_of, moving_frame};
use hypercolor_hal::drivers::lianli::wireless::pacing::{
    ECHO_STALL, MAX_RESETS_WITHOUT_DELIVERY, MAX_UNRESOLVED_TRANSFERS, resets_without_delivery,
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

#[test]
fn a_radio_slower_than_the_frame_rate_never_holds_more_than_one_transfer() {
    let mut rig = Rig::connect(slow_radio());
    rig.run_for(Duration::from_secs(10));

    assert!(
        rig.radio.max_air_queue <= TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
        "the TX held {} envelopes; one transfer plus upkeep is {}",
        rig.radio.max_air_queue,
        TRANSFER_ENVELOPES + UPKEEP_ENVELOPES
    );
    let stats = rig.stats();
    assert!(
        stats.frames_delivered >= 100,
        "the radio confirms about 12 fps here and every one of them should be used: {stats:?}"
    );
    assert!(
        stats.frames_sent <= stats.frames_delivered + 1,
        "every frame sent is confirmed except the one out now: {stats:?}"
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
    let mean = stats.echo_mean().expect("echo samples");
    assert!(
        mean < Duration::from_millis(40),
        "a fast radio echoes within a frame: {mean:?}"
    );
}

#[test]
fn a_held_frame_goes_out_as_soon_as_the_transfer_ahead_is_confirmed() {
    let mut rig = Rig::connect(slow_radio());
    // A scene that stops changing: frame 40 is the last the render path
    // publishes, and it arrives while an earlier transfer is on the air.
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
fn a_slow_report_cadence_stretches_the_wait_instead_of_resending_every_frame() {
    let mut radio = FakeRadio::one_cluster();
    // The fans report 400 ms apart, so the first echo can land after the
    // initial bounded wait has run out.
    radio.report_interval = Duration::from_millis(400);
    let mut rig = Rig::connect(radio);
    rig.run_for(Duration::from_secs(5));
    let early = rig.stats();
    rig.run_for(Duration::from_secs(10));
    let stats = rig.stats();

    assert!(
        early.resends <= 1,
        "at most the first transfer is bet on before the echo time is known: {early:?}"
    );
    assert!(
        stats.resends - early.resends <= 1,
        "once the echo time is learned the wait covers it: {early:?} then {stats:?}"
    );
    assert!(
        stats.frames_delivered >= 20,
        "the two or three frames a second the reports allow get through: {stats:?}"
    );
    assert!(rig.radio.resets.is_empty(), "a slow radio is not a wedge");
    assert!(
        rig.radio.max_air_queue <= TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
        "resends never stack transfers: {}",
        rig.radio.max_air_queue
    );
}

#[test]
fn a_late_echo_counts_as_delivered_and_resyncs_the_wait() {
    let mut radio = FakeRadio::one_cluster();
    // Each transfer shows in the table 350 ms after the fans take it: past
    // the first bounded wait, so the first transfer's echo arrives after it
    // was superseded by a resend carrying a newer frame.
    radio.report_delay = Duration::from_millis(350);
    let mut rig = Rig::connect(radio);
    rig.run_for(Duration::from_secs(3));
    let early = rig.stats();
    rig.run_for(Duration::from_secs(10));
    let stats = rig.stats();

    assert!(
        early.late_echoes >= 1,
        "the superseded transfer's echo is recognised: {early:?}"
    );
    assert!(
        early.frames_delivered >= early.late_echoes,
        "a late echo is a delivered frame: {early:?}"
    );
    assert_eq!(
        stats.resends, early.resends,
        "the echo time learned from it covers every later wait: {early:?} then {stats:?}"
    );
    let mean = stats.echo_mean().expect("echo samples");
    assert!(
        mean >= Duration::from_millis(350),
        "the learned echo time is the radio's: {mean:?}"
    );
    assert!(rig.radio.resets.is_empty(), "a late radio is not a wedge");
    assert!(
        rig.radio.max_air_queue <= TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
        "a resend replaces, never stacks: {}",
        rig.radio.max_air_queue
    );
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
        transfers <= usize::try_from(MAX_UNRESOLVED_TRANSFERS).expect("small"),
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

/// Nothing on the host tells a lost transfer from one still queued in the
/// TX. A radio so slow that one transfer outlasts the first bounded wait
/// gets one resend, whose late echo teaches the wait, and no more.
#[test]
fn a_transfer_slower_than_the_first_wait_costs_one_resend_not_a_pile() {
    let mut radio = FakeRadio::one_cluster();
    // 120 ms an envelope: a transfer takes 360 ms of air.
    radio.air_time = Duration::from_millis(120);
    let mut rig = Rig::connect(radio);
    rig.run_for(Duration::from_secs(5));
    let early = rig.stats();
    rig.run_for(Duration::from_secs(10));
    let stats = rig.stats();

    assert!(
        early.resends <= 2,
        "a bet or two before the echo time is known: {early:?}"
    );
    assert_eq!(
        stats.resends, early.resends,
        "the learned wait covers the radio from then on: {stats:?}"
    );
    assert!(
        rig.radio.max_air_queue <= 2 * TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
        "at most the transfer and one resend are ever queued: {}",
        rig.radio.max_air_queue
    );
    assert!(rig.radio.resets.is_empty(), "a slow radio is not a wedge");
    assert!(
        stats.frames_delivered >= 20,
        "the radio's two or so frames a second get through: {stats:?}"
    );
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

#[test]
fn a_lossy_radio_is_resent_to_and_only_confirmed_frames_count_as_delivered() {
    let mut radio = FakeRadio::one_cluster();
    // Lose every eleventh RGB envelope on the air.
    radio.drop_rgb_envelope = Some(Box::new(|number| number % 11 == 10));
    let mut rig = Rig::connect(radio);
    rig.run_for(Duration::from_secs(10));
    let stats = rig.stats();

    assert!(
        stats.resends > 0,
        "lost transfers are sent again: {stats:?}"
    );
    let applied = distinct_applied(&rig);
    let delivered = usize::try_from(stats.frames_delivered).expect("count");
    assert!(
        delivered == applied || delivered + 1 == applied,
        "delivered counts what the fans took ({applied}), not what was sent: {stats:?}"
    );
    assert!(
        stats.frames_delivered < stats.frames_sent,
        "some sent frames never arrived: {stats:?}"
    );
    assert!(rig.radio.resets.is_empty(), "packet loss is not a wedge");
    assert!(
        rig.radio.max_air_queue <= TRANSFER_ENVELOPES + UPKEEP_ENVELOPES,
        "resends replace the lost transfer, they never stack: {}",
        rig.radio.max_air_queue
    );
}

#[test]
fn two_clusters_stream_together_one_frame_at_a_time() {
    let mut radio = FakeRadio::two_clusters();
    radio.air_time = Duration::from_millis(20);
    let mut rig = Rig::connect(radio);
    // Both clusters' pixels change every frame.
    rig.frame = Box::new(|index| {
        let mut colors = moving_frame(index);
        colors.extend(moving_frame(index + 7_777));
        colors
    });
    rig.run_for(Duration::from_secs(10));

    assert!(
        rig.radio.max_air_queue <= 2 * TRANSFER_ENVELOPES + UPKEEP_ENVELOPES + 1,
        "one transfer per cluster at most: {}",
        rig.radio.max_air_queue
    );
    for cluster in &rig.radio.clusters {
        assert!(
            cluster.applied_log.len() > 40,
            "each cluster keeps streaming: {}",
            cluster.applied_log.len()
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
    // Per second: the table poll, and an echo poll or two until the
    // restore's own echo comes back.
    let polls = rig.radio.rx_polls.len() - polls_before;
    assert!(
        (10..=31).contains(&polls),
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

    // Two frames back to back: whichever window the first found, the
    // second is held behind a transfer that is still out.
    let mut commands = Vec::new();
    rig.protocol
        .encode_frame_into(&moving_frame(9_000), &mut commands);
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
