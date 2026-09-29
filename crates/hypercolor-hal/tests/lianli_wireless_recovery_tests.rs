//! Recovery from a fan-side stall on the L-Wireless controller, against the
//! simulated radio in `support/lianli_wireless_fake.rs`.
//!
//! On the owner's rig the fans stopped confirming frames while the TX kept
//! taking writes. Three TX resets, each followed by a reconnect that ran
//! the whole connect sequence within about 25 s, did not bring them back;
//! the driver then held their lighting, and a daemon restart 99 minutes
//! later found them confirming at once. The fake models that: a stalled
//! receiver drops RGB until it hears a session start (the first clock
//! broadcast of a session) after a long enough rest with no RGB. A TX reset
//! alone never clears it. The rig reconnects the way the device lifecycle
//! does, so a session that ends is connected afresh after a short delay.

#[path = "support/lianli_wireless_fake.rs"]
mod fake;

use std::time::Duration;

use fake::{FRAME_PERIOD, FakeRadio, Rig, SessionEnd};
use hypercolor_hal::drivers::lianli::wireless::pacing::{
    MAX_RECONNECTS_WITHOUT_DELIVERY, MAX_RESETS_WITHOUT_DELIVERY, RECONNECT_REST_HELD,
    RECONNECT_RESTS, reconnects_without_delivery, resets_without_delivery,
};
use hypercolor_hal::protocol::Protocol;

/// How long the device lifecycle takes to reconnect a controller whose
/// session ended: about 1.5 s on the owner's rig.
const RECONNECT_DELAY: Duration = Duration::from_millis(1_500);

/// The owner's fans: status about every 333 ms.
const STATUS_CADENCE: Duration = Duration::from_millis(333);

/// A reconnecting rig with one cluster, streaming for 10 s before any
/// stall.
fn streaming_rig() -> Rig {
    let mut radio = FakeRadio::one_cluster();
    radio.report_interval = STATUS_CADENCE;
    let mut rig = Rig::connect(radio);
    rig.reconnect_delay = Some(RECONNECT_DELAY);
    rig.run_for(Duration::from_secs(10));
    assert!(
        rig.stats().frames_delivered > 250,
        "the radio worked first: {:?}",
        rig.stats()
    );
    rig
}

/// The session ends of the rig so far, as TX resets then restarts.
fn ends(rig: &Rig) -> (usize, usize) {
    let resets = rig
        .session_ends
        .iter()
        .filter(|end| **end == SessionEnd::TxReset)
        .count();
    (resets, rig.session_ends.len() - resets)
}

/// Frames the first cluster took over the next `span`, against what the
/// render path offered.
fn streaming_over(rig: &mut Rig, span: Duration) -> (usize, u128) {
    let before = rig.radio.clusters[0].applied_log.len();
    rig.run_for(span);
    let taken = rig.radio.clusters[0].applied_log.len() - before;
    (taken, span.as_millis() / FRAME_PERIOD.as_millis())
}

/// The owner's stall: the TX resets and the reconnects that follow them do
/// not clear it, a rest does. After two TX resets the cluster's lighting
/// rests, the session ends, and the connect sequence runs again; once a
/// rest is long enough the fans take frames at the offered rate, and the
/// recovery budget starts over.
#[test]
fn fans_stalled_past_their_tx_resets_come_back_after_a_rested_reconnect() {
    let mut rig = streaming_rig();
    let mac = rig.radio.clusters[0].mac;
    let stalled_at = rig.now();
    rig.radio.clusters[0].stall_from(stalled_at, Duration::from_secs(20));
    rig.run_for(Duration::from_mins(2));

    let cleared_at = rig.radio.clusters[0]
        .stall_cleared_at
        .expect("the fans came back");
    assert!(
        cleared_at.saturating_sub(stalled_at) <= Duration::from_secs(90),
        "a 30 s rest outlasts a 20 s stall: cleared {:?} after it began; {:?}",
        cleared_at.saturating_sub(stalled_at),
        rig.session_ends
    );
    assert_eq!(
        rig.radio.resets.len(),
        usize::try_from(MAX_RESETS_WITHOUT_DELIVERY).expect("small"),
        "the TX is reset its budget's worth of times, then no more"
    );
    assert_eq!(
        rig.session_ends[..2],
        [SessionEnd::TxReset, SessionEnd::TxReset],
        "TX resets first: {:?}",
        rig.session_ends
    );
    assert_eq!(
        ends(&rig),
        (2, 2),
        "a 10 s rest was too short and a 30 s one cleared it: {:?}",
        rig.session_ends
    );
    let (taken, offered) = streaming_over(&mut rig, Duration::from_secs(20));
    assert!(
        taken * 100 >= usize::try_from(offered).expect("small") * 90,
        "streaming at the offered rate again: {taken} of {offered} frames in 20 s"
    );
    assert_eq!(resets_without_delivery(mac), 0, "the budget starts over");
    assert_eq!(reconnects_without_delivery(mac), 0);
    assert!(rig.protocol.session_restart().is_none());
}

/// A stall that needs four minutes' rest: the rests grow until one
/// outlasts it, on the last scheduled rest before the power-cycle message,
/// and the TX is reset
/// no more than its budget allows along the way.
#[test]
fn a_long_fan_stall_is_outlasted_by_growing_rests() {
    let mut rig = streaming_rig();
    let stalled_at = rig.now();
    rig.radio.clusters[0].stall_from(stalled_at, Duration::from_mins(4));
    rig.run_for(Duration::from_mins(12));

    let cleared_at = rig.radio.clusters[0]
        .stall_cleared_at
        .expect("the fans came back");
    let (resets, restarts) = ends(&rig);
    assert_eq!(
        u32::try_from(resets).expect("small"),
        MAX_RESETS_WITHOUT_DELIVERY
    );
    assert_eq!(
        u32::try_from(restarts).expect("small"),
        MAX_RECONNECTS_WITHOUT_DELIVERY,
        "the last scheduled rest, {:?}, outlasts the stall: {:?}",
        RECONNECT_RESTS.last(),
        rig.session_ends
    );
    assert!(
        cleared_at.saturating_sub(stalled_at) <= Duration::from_mins(11),
        "cleared {:?} after the stall began",
        cleared_at.saturating_sub(stalled_at)
    );
    let (taken, offered) = streaming_over(&mut rig, Duration::from_secs(20));
    assert!(
        taken * 100 >= usize::try_from(offered).expect("small") * 90,
        "{taken} of {offered} frames in 20 s after the recovery"
    );
}

/// Fans that never come back: the TX is reset twice, the rests run their
/// course, and after the power-cycle message a reconnect still follows
/// every ten minutes, since the one recovery seen came after a long rest.
/// Between reconnects the stalled cluster gets no RGB, while fan-speed and
/// clock upkeep carry on.
#[test]
fn fans_that_never_come_back_are_reconnected_every_ten_minutes() {
    let mut rig = streaming_rig();
    let mac = rig.radio.clusters[0].mac;
    let stalled_at = rig.now();
    rig.radio.clusters[0].stall_from(stalled_at, Duration::MAX);
    rig.run_for(Duration::from_mins(70));

    assert!(rig.radio.clusters[0].stall_cleared_at.is_none());
    let (resets, restarts) = ends(&rig);
    assert_eq!(
        u32::try_from(resets).expect("small"),
        MAX_RESETS_WITHOUT_DELIVERY,
        "no reset loop: {:?}",
        rig.session_ends
    );
    let scheduled: Duration = RECONNECT_RESTS.iter().sum();
    let held_for = Duration::from_mins(70).saturating_sub(scheduled);
    let expected = u64::from(MAX_RECONNECTS_WITHOUT_DELIVERY)
        + held_for.as_secs() / RECONNECT_REST_HELD.as_secs();
    let restarts = u64::try_from(restarts).expect("small");
    assert!(
        restarts + 1 >= expected && restarts <= expected + 1,
        "about one reconnect per rest, then one per ten minutes: {restarts}, expected {expected}"
    );
    assert_eq!(
        u64::from(reconnects_without_delivery(mac)),
        restarts,
        "every reconnect is counted against the cluster"
    );

    // A minute inside a ten-minute rest: wait for the next reconnect, let
    // its session reach the stall verdict, then watch.
    let sessions = rig.sessions.len();
    let waited = rig.now();
    while rig.sessions.len() == sessions {
        assert!(
            rig.now().saturating_sub(waited) <= RECONNECT_REST_HELD + Duration::from_mins(1),
            "a reconnect follows within the held rest"
        );
        rig.run_for(Duration::from_secs(1));
    }
    rig.run_for(Duration::from_secs(10));
    let rest_started = rig.now();
    let packets_before = rig.radio.tx_packets;
    rig.run_for(Duration::from_mins(1));
    assert_eq!(
        rig.sessions.len(),
        sessions + 1,
        "no reconnect inside the rest"
    );
    assert_eq!(
        rig.transfers_since(rest_started),
        0,
        "a resting cluster gets no RGB"
    );
    assert!(
        rig.radio.tx_packets - packets_before >= 60 * 8,
        "fan-speed and clock upkeep carry on through the rest: {} packets in 60 s",
        rig.radio.tx_packets - packets_before
    );
}

/// A TX that stops delivering and comes back with its reset still recovers
/// on the first reset, as before: the rests are only for fans the resets
/// did not bring back.
#[test]
fn a_tx_that_a_reset_revives_needs_no_rest() {
    let mut rig = streaming_rig();
    rig.radio.rf_dead = true;
    rig.run_for(Duration::from_secs(8));
    rig.radio.rf_dead = false;
    rig.run_for(Duration::from_secs(20));

    assert_eq!(
        rig.session_ends,
        [SessionEnd::TxReset],
        "one reset, then streaming"
    );
    let (taken, offered) = streaming_over(&mut rig, Duration::from_secs(10));
    assert!(
        taken * 100 >= usize::try_from(offered).expect("small") * 90,
        "{taken} of {offered} frames"
    );
}
