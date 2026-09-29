//! Acknowledgement-paced RGB delivery over a sliding window.
//!
//! Every RGB transfer carries a tag, and each fan cluster's record in the
//! RX device table echoes the tag of the transfer its receiver last took
//! (spec 80 section 6.5, record bytes 20 to 23). The record is a status the
//! fans refresh on their own cadence (333 ms to about a second on the
//! owner's V1 rig), far slower than frames stream, so the echo is a
//! cumulative acknowledgement: the tag it names confirms that transfer, and
//! since the TX relays in order, every transfer sent before it has left the
//! TX. Pacing one frame per echo would turn the status cadence into a frame
//! rate cap; instead each cluster keeps a window of frames in flight, like
//! a TCP sender:
//!
//! - A cluster may have up to its window of sends unconfirmed. The window
//!   starts at [`INITIAL_WINDOW`], doubles per echo while it is the limit
//!   (slow start), then grows by one per window of confirmed sends, up to
//!   [`MAX_WINDOW`]. It grows only while it holds frames back, so a scene
//!   the render path offers slower than the radio carries never inflates it.
//! - Each echo measures the TX backlog directly: sends made after the
//!   confirmed one that had time to arrive and to show up in a status are
//!   overdue. "Time" is the smallest echo age over the last minute, plus a
//!   whole status interval (the longest measured lately, so a cadence that
//!   just slowed counts), since a status can be a snapshot that much older
//!   than when the RX hands it over, plus the poll spacing and a frame of
//!   jitter. In effect a send is overdue once it missed a status it should
//!   have been in. Judged against the send's own latency instead, stale
//!   statuses on real fans read as backlog and kept halving a window the
//!   radio could fill.
//! - Only sustained evidence shrinks the window: more than [`OVERDUE_LIMIT`]
//!   overdue at [`BACKLOG_EVIDENCE`] advancing echoes in a row halves it,
//!   at most once per [`DECREASE_HOLDOFF_STATUSES`] status intervals and
//!   once per window of sends. Every cluster's frames wait in the one TX
//!   queue, so the halving applies to every cluster with frames out. So
//!   the TX holds a bounded backlog when the radio really is slower than
//!   the stream, and delivery tracks the radio's own rate.
//! - Clusters are paced independently: a frame goes to every cluster it
//!   changes that has room, and waits for the rest, where a newer frame
//!   replaces it, so what goes out next is always the newest frame, never a
//!   queue of stale ones. One cluster whose echoes lag steadily, or one the
//!   RX cannot hear, never throttles the others; a backlog in the shared TX
//!   shrinks them together. A cluster whose own delay jumps by more than a
//!   status interval reads as a backlog until its base delay catches up,
//!   and its halvings then reach its neighbours too.
//! - Each send gets its own wire tag: the frame's pixel hash mixed with a
//!   send number that runs for the whole process from a wall-clock seed,
//!   and never the tag the cluster echoes at that moment. An echo therefore
//!   names exactly one send, and a report cached from before it (a restore
//!   of the frame already showing, or the first frame after a reconnect)
//!   confirms nothing.
//! - Delivered frames are counted from how far the echo advances: a
//!   cluster's frames are numbered as they are sent, and an echo confirms
//!   every frame up to the one it names.
//! - The table is polled while sends are unconfirmed, predicted from the
//!   cadence of past echoes: the first poll after an echo waits most of an
//!   echo interval, the rest follow a sixth of one apart.
//! - No echo progress for three echo intervals (a timeout) means the sends
//!   out were lost: the window collapses to [`MIN_WINDOW`] and the newest
//!   frame goes out again, one probe at a time, backing off. Until an echo
//!   interval is measured the timeout is at least [`ECHO_TIMEOUT_UNKNOWN`],
//!   and a timeout then leaves slow start's ceiling alone. Nothing is ever
//!   sent while [`MAX_WINDOW`] sends are unresolved.
//! - Fan-speed upkeep can knock a receiver back to its onboard lighting
//!   without changing its echo, so every upkeep leaves each cluster owing a
//!   restore, sent with the next room in its window.
//! - Fans that are heard but confirm nothing for [`ECHO_STALL`] mean the TX
//!   stopped delivering: the protocol asks the transport for the vendor
//!   reset, which ends the session the way a refused write does. At most
//!   [`MAX_RESETS_WITHOUT_DELIVERY`] resets are asked for on behalf of one
//!   cluster until it confirms a frame again, across sessions; past that
//!   the cluster's lighting is held and the log says to power-cycle the
//!   controller, while clusters that still confirm keep streaming.
//!
//! Counters feed [`DeliveryStats`] and a periodic info line comparing frames
//! sent with frames the echoes confirmed.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tracing::{debug, error, info, warn};

use super::frame::{Mac, effect_index_for};

/// An RGB transfer's tag, as the header carries it and the record echoes it.
pub type Tag = [u8; 4];

/// Sends a cluster may have unconfirmed when a session starts.
pub const INITIAL_WINDOW: u32 = 2;
/// The window never shrinks below this.
pub const MIN_WINDOW: u32 = 2;
/// The window never grows past this, and no send of any kind goes out while
/// this many are unresolved: the most the TX can ever hold for a cluster.
/// Fans have refreshed their status as slowly as once a second, which
/// needs about 33 frames in flight at 30 fps; this leaves room for a
/// missed status on top.
pub const MAX_WINDOW: u32 = 64;
/// Overdue sends at an echo beyond which the TX may be holding a backlog.
pub const OVERDUE_LIMIT: u64 = 1;
/// Advancing echoes in a row that must each find a backlog before the
/// window halves: one stale status is not a backlog.
pub const BACKLOG_EVIDENCE: u32 = 3;
/// Status intervals that must pass between two halvings.
pub const DECREASE_HOLDOFF_STATUSES: u32 = 3;
/// The status interval assumed before one has been measured.
pub const STATUS_UNKNOWN: Duration = Duration::from_millis(600);
/// Status intervals remembered as measured, for judging staleness: the
/// span of a run of backlog evidence, and one more.
const RECENT_GAPS: usize = BACKLOG_EVIDENCE as usize + 1;
/// Jitter allowed on top of the poll spacing before a send counts overdue:
/// about one frame interval at 30 fps.
const OVERDUE_SLACK: Duration = Duration::from_millis(35);
/// Echo ages are kept as the smallest in each bucket of this length, for
/// [`BASE_BUCKETS`] buckets: the base delay is the smallest echo age over
/// the last minute. A shorter memory rises with a backlog that grows by
/// less than a status interval per span, and the backlog is never seen:
/// two clusters sharing a slow radio grew without bound that way.
const BASE_BUCKET: Duration = Duration::from_secs(10);
const BASE_BUCKETS: u32 = 6;
/// Echo-poll spacing before the echo cadence is known.
pub const ECHO_POLL_DEFAULT: Duration = Duration::from_millis(40);
/// Bounds on echo-poll spacing.
pub const ECHO_POLL_MIN: Duration = Duration::from_millis(20);
pub const ECHO_POLL_STEADY_MAX: Duration = Duration::from_millis(100);
pub const ECHO_POLL_MAX: Duration = Duration::from_millis(250);
/// The timeout before any echo cadence is known.
pub const ECHO_TIMEOUT_UNKNOWN: Duration = Duration::from_millis(1_500);
/// Bounds on the timeout once echoes are known: three echo intervals, or
/// the usual echo age plus two, whichever is longer.
pub const ECHO_TIMEOUT_MIN: Duration = Duration::from_millis(500);
pub const ECHO_TIMEOUT_MAX: Duration = Duration::from_secs(3);
/// Timeouts back off to this.
pub const ECHO_TIMEOUT_CAP: Duration = Duration::from_secs(6);
/// No confirmation for this long is logged once as a warning.
pub const ECHO_STALL_WARN: Duration = Duration::from_secs(2);
/// No confirmation for this long, with the fans heard, is a TX that stopped
/// delivering.
pub const ECHO_STALL: Duration = Duration::from_secs(5);
/// A cluster counts as heard for the stall verdict when it was in a table
/// reply this recently.
const HEARD_WITHIN: Duration = Duration::from_secs(1);
/// Resets asked for on behalf of one cluster before it confirms a frame
/// again.
pub const MAX_RESETS_WITHOUT_DELIVERY: u32 = 2;
/// A cluster missing from every table reply for this long is not heard.
pub const ABSENT_AFTER: Duration = Duration::from_secs(3);
/// How often the delivery report is logged.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(10);
/// Weight of a new echo-interval sample in its running average, as a
/// divisor.
const GAP_SMOOTHING: u32 = 8;
/// An echo-interval sample counts for at most this many averages.
const OUTLIER_CLIP: u32 = 3;

/// Why a transfer goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKind {
    /// The newest frame, in the window's room.
    Frame,
    /// The newest frame after a timeout: a probe while the cluster
    /// recovers.
    Resend,
    /// The frame already sent, again after fan-speed upkeep, which can
    /// knock a receiver back to its onboard lighting.
    Restore,
}

/// What the pacer decided about fans that stopped confirming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallVerdict {
    /// Ask the transport for the TX reset; the session ends.
    Reset {
        /// Index of the cluster that stopped confirming.
        cluster: usize,
        /// How long nothing has been confirmed.
        unconfirmed_for: Duration,
        /// Resends tried in that time.
        resends: u32,
        /// Resets asked for since this cluster last confirmed, this one
        /// included.
        resets: u32,
    },
    /// The cluster's reset budget is spent: hold its lighting for this
    /// session.
    Hold {
        /// Index of the cluster that stopped confirming.
        cluster: usize,
        /// How long nothing has been confirmed.
        unconfirmed_for: Duration,
    },
}

/// Delivery counters, summed over every cluster of a controller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeliveryStats {
    /// Frames the render path handed the protocol.
    pub frames_offered: u64,
    /// Frames sent: sends of pixels other than the cluster's last send.
    /// Restores, and resends of the same pixels, are counted apart.
    pub frames_sent: u64,
    /// Sent frames an echo retired, cumulatively: an echo names one send
    /// the fans took, and every frame sent before it has left the TX, since
    /// the TX relays in order. A frame lost on the air but overtaken by a
    /// later one before the next status is counted too, so this is the rate
    /// the radio drains frames, an upper bound on the frames shown.
    pub frames_delivered: u64,
    /// Frames replaced by a newer one before they could be sent.
    pub frames_coalesced: u64,
    /// Probes sent after a timeout.
    pub resends: u64,
    /// Sent frames re-sent after fan-speed upkeep.
    pub restores: u64,
    /// Windows that ran out without an echo.
    pub timeouts: u64,
    /// Echoes of a send a timeout had already given up on.
    pub late_echoes: u64,
    /// Echoes of a tag this session never sent while a frame of ours was
    /// showing: the receiver left our frame for its own lighting.
    pub drifts: u64,
    /// Windows halved because the TX held a backlog.
    pub congestion_events: u64,
    /// Echoes that advanced, the frames they confirmed, and the most one
    /// confirmed.
    pub echo_advances: u64,
    pub advance_frames: u64,
    pub advance_max: u64,
    /// The most sends found overdue at one echo.
    pub overdue_max: u64,
    /// Echo polls sent to the RX.
    pub echo_polls: u64,
    /// Table replies that reached the protocol, polls and upkeep alike.
    pub table_replies: u64,
    /// USB packets queued for the TX, upkeep included. A batch that fails
    /// part way stops early, so around failures a per-second URB count on
    /// the TX can come out lower.
    pub tx_packets: u64,
    /// Age of the confirmed send at each echo: samples, sum, and maximum.
    pub echo_samples: u64,
    pub echo_total: Duration,
    pub echo_max: Duration,
    /// Intervals between advancing echoes while sends were out: samples
    /// and sum.
    pub gap_samples: u64,
    pub gap_total: Duration,
}

impl DeliveryStats {
    /// Mean age of the confirmed send at each echo.
    #[must_use]
    pub fn echo_mean(&self) -> Option<Duration> {
        mean(self.echo_total, self.echo_samples)
    }

    /// Mean interval between advancing echoes: the status cadence.
    #[must_use]
    pub fn gap_mean(&self) -> Option<Duration> {
        mean(self.gap_total, self.gap_samples)
    }

    /// Mean frames one advancing echo confirmed.
    #[must_use]
    pub fn advance_mean(&self) -> Option<f64> {
        (self.echo_advances > 0).then(|| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a mean for the report is approximate by nature"
            )]
            let mean = self.advance_frames as f64 / self.echo_advances as f64;
            mean
        })
    }
}

/// A running average with a new sample weighted one in [`GAP_SMOOTHING`],
/// clipped to [`OUTLIER_CLIP`] averages first.
fn smooth(average: Option<Duration>, sample: Duration) -> Duration {
    match average {
        None => sample,
        Some(average) => {
            let sample = sample.min(average.saturating_mul(OUTLIER_CLIP));
            if sample >= average {
                average + sample.saturating_sub(average) / GAP_SMOOTHING
            } else {
                average.saturating_sub(average.saturating_sub(sample) / GAP_SMOOTHING)
            }
        }
    }
}

fn mean(total: Duration, samples: u64) -> Option<Duration> {
    let samples = u32::try_from(samples).ok()?;
    (samples > 0).then(|| total / samples)
}

/// One send still unconfirmed.
#[derive(Debug, Clone, Copy)]
struct Outstanding {
    seq: u64,
    wire: Tag,
    content: Tag,
    sent_at: Instant,
    /// Frames numbered up to and including this send: what its echo
    /// confirms.
    frames_through: u64,
}

/// One cluster's side of the window.
#[derive(Debug)]
struct ClusterLink {
    /// The cluster's radio MAC, which keys its reset budget.
    mac: Option<Mac>,
    /// Pixels of the newest frame submitted for this cluster.
    newest: Option<Tag>,
    /// Pixels of the last send: what the fans show once everything out
    /// has landed.
    sent_content: Option<Tag>,
    /// Pixels of the last confirmed send; `None` while the cluster shows
    /// something this session never sent.
    shown: Option<Tag>,
    /// Sends after the last confirmed one, oldest first.
    log: VecDeque<Outstanding>,
    /// Wire tag of the last confirmed send.
    acked_wire: Option<Tag>,
    next_seq: u64,
    acked_seq: u64,
    /// Sends up to this one were given up on by a timeout; they stay
    /// unresolved until an echo passes them, but hold no window room.
    lost_through: u64,
    /// Frames numbered so far, and confirmed so far.
    frames: u64,
    delivered_through: u64,
    window: u32,
    ssthresh: u32,
    avoid_credit: u32,
    /// No further halving until an echo confirms this send.
    recovery_until: u64,
    /// A frame waited for room since the last advancing echo.
    window_limited: bool,
    /// Advancing echoes in a row that found a backlog.
    backlog_streak: u32,
    /// The last few status intervals measured, newest last.
    recent_gaps: VecDeque<Duration>,
    last_decrease_at: Option<Instant>,
    /// The first send made after an echo left nothing out; an advance soon
    /// after still samples the status cadence.
    resumed_at: Option<Instant>,
    timeouts: u32,
    /// When the timeout clock last started: the first send after idle, a
    /// progress echo, or a timeout.
    rto_from: Option<Instant>,
    /// When the cluster last showed progress, or started a chain of sends.
    last_progress_at: Option<Instant>,
    last_advance_at: Option<Instant>,
    /// Sends were out when the last echo advanced, so the time to the next
    /// advance samples the echo cadence.
    out_after_advance: bool,
    echo_gap: Option<Duration>,
    /// Running average of the confirmed send's age at its echo.
    echo_age: Option<Duration>,
    base_delays: VecDeque<(Instant, Duration)>,
    next_poll_at: Option<Instant>,
    last_seen_at: Option<Instant>,
    last_echo: Option<Tag>,
    restore_owed: bool,
    holding: bool,
    stall_warned: bool,
    absent_logged: bool,
}

impl Default for ClusterLink {
    fn default() -> Self {
        Self {
            mac: None,
            newest: None,
            sent_content: None,
            shown: None,
            log: VecDeque::new(),
            acked_wire: None,
            next_seq: 1,
            acked_seq: 0,
            lost_through: 0,
            frames: 0,
            delivered_through: 0,
            window: INITIAL_WINDOW,
            ssthresh: MAX_WINDOW,
            avoid_credit: 0,
            recovery_until: 0,
            window_limited: false,
            backlog_streak: 0,
            recent_gaps: VecDeque::new(),
            last_decrease_at: None,
            resumed_at: None,
            timeouts: 0,
            rto_from: None,
            last_progress_at: None,
            last_advance_at: None,
            out_after_advance: false,
            echo_gap: None,
            echo_age: None,
            base_delays: VecDeque::new(),
            next_poll_at: None,
            last_seen_at: None,
            last_echo: None,
            restore_owed: false,
            holding: false,
            stall_warned: false,
            absent_logged: false,
        }
    }
}

impl ClusterLink {
    fn heard_within(&self, now: Instant, window: Duration) -> bool {
        self.last_seen_at
            .is_some_and(|seen| now.saturating_duration_since(seen) < window)
    }

    fn absent(&self, now: Instant) -> bool {
        !self.heard_within(now, ABSENT_AFTER)
    }

    /// Whether `tag` could be confused with a send this cluster already
    /// knows: one still out, the last confirmed, or what it echoes now.
    /// Hashed tags can collide, and an echo must name exactly one send.
    fn tag_taken(&self, tag: Tag) -> bool {
        self.last_echo == Some(tag)
            || self.acked_wire == Some(tag)
            || self.log.iter().any(|sent| sent.wire == tag)
    }

    /// Sends after the last confirmed one.
    fn unresolved(&self) -> u64 {
        self.next_seq - 1 - self.acked_seq
    }

    /// Unresolved sends a timeout has not given up on: what holds room.
    fn in_flight(&self) -> u64 {
        self.next_seq - 1 - self.acked_seq.max(self.lost_through)
    }

    fn has_room(&self) -> bool {
        self.in_flight() < u64::from(self.window) && self.unresolved() < u64::from(MAX_WINDOW)
    }

    /// Whether the cluster has something to send: a frame it has not been
    /// sent, or a restore it owes.
    fn wants_send(&self) -> bool {
        self.newest
            .is_some_and(|newest| self.sent_content != Some(newest) || self.restore_owed)
    }

    /// Time without progress before the sends out count as lost: three
    /// echo intervals, or the usual echo age plus two intervals when the
    /// radio's echoes lag further behind, backing off with every timeout in
    /// a row.
    fn timeout(&self) -> Duration {
        let base = match (self.echo_gap, self.echo_age) {
            (None, None) => ECHO_TIMEOUT_UNKNOWN,
            (measured, age) => {
                let gap = measured.or(age).unwrap_or(ECHO_TIMEOUT_UNKNOWN);
                let age = age.unwrap_or(Duration::ZERO);
                let timeout = (gap * 3)
                    .max(age + gap * 2)
                    .clamp(ECHO_TIMEOUT_MIN, ECHO_TIMEOUT_MAX);
                // Until a status interval is measured, one echo's age says
                // nothing about when the next status comes: a quick first
                // echo from fans on a 550 ms cadence timed out otherwise.
                if measured.is_some() {
                    timeout
                } else {
                    timeout.max(ECHO_TIMEOUT_UNKNOWN)
                }
            }
        };
        base.saturating_mul(1 << self.timeouts.min(3))
            .min(ECHO_TIMEOUT_CAP)
    }

    /// Spacing of echo polls: a sixth of an echo interval, backing off
    /// while the cluster recovers from timeouts.
    fn poll_spacing(&self) -> Duration {
        self.echo_gap
            .map_or(ECHO_POLL_DEFAULT, |gap| {
                (gap / 6).clamp(ECHO_POLL_MIN, ECHO_POLL_STEADY_MAX)
            })
            .saturating_mul(1 << self.timeouts.min(3))
            .min(ECHO_POLL_MAX)
    }

    /// When to poll first for the echo of sends just made: just before the
    /// next status is due, when the cadence is known.
    fn first_poll(&self, now: Instant) -> Instant {
        let (Some(last), Some(gap)) = (self.last_advance_at, self.echo_gap) else {
            return now + self.poll_spacing();
        };
        if gap.is_zero() {
            return now + self.poll_spacing();
        }
        let since = now.saturating_duration_since(last);
        let periods = u32::try_from(since.as_nanos() / gap.as_nanos()).unwrap_or(u32::MAX);
        let expected = last + gap.saturating_mul(periods.saturating_add(1));
        expected
            .checked_sub(gap / 4)
            .unwrap_or(expected)
            .max(now + ECHO_POLL_MIN)
    }

    /// The smallest echo age seen lately: the radio's delay with no
    /// backlog, polling included.
    fn base_delay(&self) -> Option<Duration> {
        self.base_delays.iter().map(|(_, delay)| *delay).min()
    }

    fn sample_base_delay(&mut self, now: Instant, age: Duration) {
        let span = BASE_BUCKET.saturating_mul(BASE_BUCKETS);
        while self
            .base_delays
            .front()
            .is_some_and(|(start, _)| now.saturating_duration_since(*start) >= span)
        {
            self.base_delays.pop_front();
        }
        match self.base_delays.back_mut() {
            Some((start, smallest)) if now.saturating_duration_since(*start) < BASE_BUCKET => {
                *smallest = (*smallest).min(age);
            }
            _ => self.base_delays.push_back((now, age)),
        }
    }

    /// Fold one echo interval into the running average, clipped so one
    /// missed status cannot stretch every timeout after it.
    fn sample_gap(&mut self, sample: Duration) {
        self.echo_gap = Some(smooth(self.echo_gap, sample));
    }

    /// Fold one confirmed send's echo age into the running average.
    fn sample_age(&mut self, sample: Duration) {
        self.echo_age = Some(smooth(self.echo_age, sample));
    }

    /// The status interval: measured, or assumed until it is.
    fn status_interval(&self) -> Duration {
        self.echo_gap.unwrap_or(STATUS_UNKNOWN)
    }

    /// How stale a status may be: the longest status interval measured
    /// lately, or the running average if that is longer. The average
    /// trails a slower cadence by several statuses, and stale statuses in
    /// between would otherwise read as a backlog.
    fn staleness_allowance(&self) -> Duration {
        self.recent_gaps
            .iter()
            .copied()
            .fold(self.status_interval(), Duration::max)
    }

    fn remember_gap(&mut self, gap: Duration) {
        if self.recent_gaps.len() == RECENT_GAPS {
            self.recent_gaps.pop_front();
        }
        self.recent_gaps.push_back(gap);
    }

    /// Whether the window may halve at `now`: once per window of sends,
    /// and at most once per [`DECREASE_HOLDOFF_STATUSES`] status intervals.
    fn may_decrease(&self, now: Instant) -> bool {
        let holdoff = self
            .status_interval()
            .saturating_mul(DECREASE_HOLDOFF_STATUSES);
        let held_off = self
            .last_decrease_at
            .is_some_and(|at| now.saturating_duration_since(at) < holdoff);
        self.acked_seq >= self.recovery_until && !held_off
    }

    fn halve(&mut self, now: Instant) {
        self.ssthresh = (self.window / 2).max(MIN_WINDOW);
        self.window = self.ssthresh;
        self.avoid_credit = 0;
        self.recovery_until = self.next_seq - 1;
        self.window_limited = false;
        self.backlog_streak = 0;
        self.last_decrease_at = Some(now);
    }

    /// Grow or shrink the window on an advancing echo at `now` that
    /// confirmed `sends` sends and found `overdue` of the later ones
    /// overdue. Returns whether the window halved.
    fn adjust_window(&mut self, sends: u64, overdue: u64, now: Instant) -> bool {
        let sends = u32::try_from(sends).unwrap_or(u32::MAX);
        if overdue > OVERDUE_LIMIT {
            self.backlog_streak = self.backlog_streak.saturating_add(1);
        } else {
            self.backlog_streak = 0;
        }
        if self.backlog_streak >= BACKLOG_EVIDENCE && self.may_decrease(now) {
            self.halve(now);
            return true;
        }
        let limited = std::mem::take(&mut self.window_limited);
        if limited && self.backlog_streak == 0 && self.acked_seq >= self.recovery_until {
            if self.window < self.ssthresh {
                self.window = self
                    .window
                    .saturating_add(sends)
                    .min(self.ssthresh)
                    .min(MAX_WINDOW);
            } else {
                self.avoid_credit = self.avoid_credit.saturating_add(sends);
                while self.avoid_credit >= self.window && self.window < MAX_WINDOW {
                    self.avoid_credit -= self.window;
                    self.window += 1;
                }
            }
        }
        false
    }
}

/// The controller's echo-paced delivery state for one session.
#[derive(Debug, Default)]
pub struct DeliveryPacer {
    links: Vec<ClusterLink>,
    reset_requested: bool,
    totals: DeliveryStats,
    interval: DeliveryStats,
    interval_started_at: Option<Instant>,
}

impl DeliveryPacer {
    /// Counters since the session began.
    #[must_use]
    pub const fn totals(&self) -> DeliveryStats {
        self.totals
    }

    /// Whether the session sends RGB no more: the TX reset was asked for.
    #[must_use]
    pub const fn silenced(&self) -> bool {
        self.reset_requested
    }

    /// Whether the protocol has asked for the TX reset: the session is
    /// ending and nothing more should be written.
    #[must_use]
    pub const fn reset_requested(&self) -> bool {
        self.reset_requested
    }

    /// Clusters whose lighting is held because their reset budget is spent.
    #[must_use]
    pub fn held_clusters(&self) -> usize {
        self.links.iter().filter(|link| link.holding).count()
    }

    /// `cluster`'s current window, in sends.
    #[must_use]
    pub fn window(&self, cluster: usize) -> Option<u32> {
        self.links.get(cluster).map(|link| link.window)
    }

    /// Grow the window to cover `clusters`, keeping what is known.
    pub fn ensure_clusters(&mut self, clusters: usize) {
        if self.links.len() < clusters {
            self.links.resize_with(clusters, ClusterLink::default);
        }
    }

    /// Fit the pacer to the frozen routing, one link per cluster MAC in
    /// routing order: a cluster seen during connect that did not make the
    /// routing has no link left to report on.
    pub fn fit_clusters(&mut self, macs: &[Mac]) {
        self.links.truncate(macs.len());
        self.ensure_clusters(macs.len());
        for (link, mac) in self.links.iter_mut().zip(macs) {
            link.mac = Some(*mac);
        }
    }

    /// Fan-speed upkeep just went out: every cluster showing our lighting
    /// owes a restore, paid by the next send it gets.
    pub fn owe_restores(&mut self) {
        for link in &mut self.links {
            if link.newest.is_some() {
                link.restore_owed = true;
            }
        }
    }

    /// The render path handed over a frame.
    pub fn note_frame_offered(&mut self) {
        self.count(|stats| stats.frames_offered += 1);
    }

    /// USB packets queued for the TX.
    pub fn note_tx_packets(&mut self, packets: usize) {
        let packets = u64::try_from(packets).unwrap_or(u64::MAX);
        self.count(|stats| stats.tx_packets += packets);
    }

    /// The render path's newest frame shows `content` on `cluster`.
    pub fn submit(&mut self, cluster: usize, content: Tag) {
        self.ensure_clusters(cluster + 1);
        let link = &mut self.links[cluster];
        let replaced = link.newest.replace(content);
        let unsent = replaced
            .is_some_and(|previous| previous != content && link.sent_content != Some(previous));
        if unsent {
            self.count(|stats| stats.frames_coalesced += 1);
        }
    }

    /// Give up on sends whose window ran out without an echo. A cluster
    /// that times out collapses its window, drops out of step, and probes
    /// with the newest frame until an echo moves again.
    pub fn tick(&mut self, now: Instant) {
        if self.silenced() {
            return;
        }
        let mut timed_out = 0_u64;
        for (cluster, link) in self.links.iter_mut().enumerate() {
            if link.holding || link.in_flight() == 0 {
                continue;
            }
            let Some(from) = link.rto_from else {
                continue;
            };
            if now.saturating_duration_since(from) < link.timeout() {
                continue;
            }
            if link.timeouts == 0 {
                debug!(
                    cluster,
                    in_flight = link.in_flight(),
                    window = link.window,
                    "wireless sends unconfirmed past their timeout; collapsing the window and probing with the newest frame"
                );
            }
            link.lost_through = link.next_seq - 1;
            // A timeout before the status interval is known only means the
            // guess at the timeout was short; it says nothing about how
            // much the radio carries, so slow start keeps its ceiling.
            if link.echo_gap.is_some() {
                link.ssthresh = (link.window / 2).max(MIN_WINDOW);
            }
            link.window = MIN_WINDOW;
            link.avoid_credit = 0;
            link.timeouts = link.timeouts.saturating_add(1);
            link.rto_from = Some(now);
            timed_out += 1;
        }
        if timed_out > 0 {
            self.count(|stats| stats.timeouts += timed_out);
        }
    }

    /// Note, before a pass of sends, which clusters have something to send
    /// but no room: for them the window is the limit, and it may grow.
    pub fn mark_window_limits(&mut self, now: Instant) {
        if self.silenced() {
            return;
        }
        for link in &mut self.links {
            let pacing = !link.holding && !link.absent(now) && link.timeouts == 0;
            if pacing && link.wants_send() && !link.has_room() {
                link.window_limited = true;
            }
        }
    }

    /// Whether `cluster` should be sent a transfer now, and why.
    #[must_use]
    pub fn decide(&self, cluster: usize, now: Instant) -> Option<SendKind> {
        if self.silenced() {
            return None;
        }
        let link = self.links.get(cluster)?;
        let newest = link.newest?;
        if link.holding || link.absent(now) || link.unresolved() >= u64::from(MAX_WINDOW) {
            return None;
        }
        if link.timeouts > 0 {
            return (link.in_flight() == 0).then_some(SendKind::Resend);
        }
        if !link.has_room() {
            return None;
        }
        if link.sent_content != Some(newest) {
            Some(SendKind::Frame)
        } else if link.restore_owed {
            Some(SendKind::Restore)
        } else {
            None
        }
    }

    /// Whether `cluster` may take one more send without passing the bound
    /// on unresolved sends.
    #[must_use]
    pub fn has_capacity(&self, cluster: usize) -> bool {
        self.links.get(cluster).is_some_and(|link| {
            !self.silenced() && !link.holding && link.unresolved() < u64::from(MAX_WINDOW)
        })
    }

    /// Whether `cluster`'s newest frame has not gone out.
    #[must_use]
    pub fn held(&self, cluster: usize) -> bool {
        self.links.get(cluster).is_some_and(|link| {
            link.newest
                .is_some_and(|newest| link.sent_content != Some(newest))
        })
    }

    /// A transfer of `content` goes to `cluster` for `kind`. Returns the
    /// tag it carries on the wire.
    pub fn note_sent(&mut self, cluster: usize, content: Tag, kind: SendKind, now: Instant) -> Tag {
        self.ensure_clusters(cluster + 1);
        let link = &mut self.links[cluster];
        let wire = allocate_wire(
            content,
            |candidate| link.tag_taken(candidate),
            next_send_number,
        );
        let new_frame = kind != SendKind::Restore && link.sent_content != Some(content);
        if new_frame {
            link.frames += 1;
        }
        if link.unresolved() == 0 {
            link.last_progress_at = Some(now);
            if link.last_advance_at.is_some() && link.resumed_at.is_none() {
                link.resumed_at = Some(now);
            }
        }
        if link.in_flight() == 0 {
            link.rto_from = Some(now);
        }
        let seq = link.next_seq;
        link.next_seq += 1;
        link.log.push_back(Outstanding {
            seq,
            wire,
            content,
            sent_at: now,
            frames_through: link.frames,
        });
        link.sent_content = Some(content);
        link.restore_owed = false;
        if link.next_poll_at.is_none() {
            link.next_poll_at = Some(link.first_poll(now));
        }
        self.count(|stats| {
            match kind {
                SendKind::Frame => {}
                SendKind::Resend => stats.resends += 1,
                SendKind::Restore => stats.restores += 1,
            }
            if new_frame {
                stats.frames_sent += 1;
            }
        });
        wire
    }

    /// Whether an echo poll is due now.
    #[must_use]
    pub fn poll_due(&self, now: Instant) -> bool {
        !self.silenced()
            && self.links.iter().any(|link| {
                !link.holding
                    && !link.absent(now)
                    && link.unresolved() > 0
                    && link.next_poll_at.is_none_or(|at| now >= at)
            })
    }

    /// An echo poll went out.
    pub fn note_poll(&mut self, now: Instant) {
        self.reschedule_polls(now);
        self.count(|stats| stats.echo_polls += 1);
    }

    /// A table poll went out that answers whatever echo polls were due.
    pub fn reschedule_polls(&mut self, now: Instant) {
        for link in &mut self.links {
            if link.unresolved() > 0 && link.next_poll_at.is_none_or(|at| now >= at) {
                link.next_poll_at = Some(now + link.poll_spacing());
            }
        }
    }

    /// A table reply reached the protocol.
    pub fn note_table_reply(&mut self) {
        self.count(|stats| stats.table_replies += 1);
    }

    /// `cluster` reported `echo` in a table reply that reached the protocol
    /// at `now`.
    #[expect(
        clippy::too_many_lines,
        reason = "one echo updates delivery, window, timing, and drift together"
    )]
    pub fn observe(&mut self, cluster: usize, echo: Tag, now: Instant) {
        self.ensure_clusters(cluster + 1);
        let link = &mut self.links[cluster];
        if link.absent_logged {
            link.absent_logged = false;
            info!(
                cluster,
                "wireless cluster heard again; resuming its lighting"
            );
            // Nothing could be confirmed while it was not heard: give up on
            // what was out, and send the newest frame at once.
            link.lost_through = link.next_seq - 1;
            link.sent_content = None;
            link.timeouts = 0;
            link.rto_from = Some(now);
            if link.unresolved() > 0 {
                link.last_progress_at = Some(now);
            }
        }
        link.last_seen_at = Some(now);
        link.last_echo = Some(echo);

        let Some(position) = link.log.iter().position(|sent| sent.wire == echo) else {
            if link.acked_wire != Some(echo) && link.shown.is_some() {
                // The receiver shows something this session never sent:
                // it left our frame for its own lighting. Send the newest
                // frame again.
                link.shown = None;
                link.sent_content = None;
                self.count(|stats| stats.drifts += 1);
                debug!(
                    cluster,
                    echo = ?echo,
                    "wireless receiver left our frame for its own lighting; sending the newest frame again"
                );
            }
            return;
        };

        let confirmed = link.log[position];
        let sends = confirmed.seq - link.acked_seq;
        let late = confirmed.seq <= link.lost_through;
        let frames = confirmed
            .frames_through
            .saturating_sub(link.delivered_through);
        link.delivered_through = link.delivered_through.max(confirmed.frames_through);
        let age = now.saturating_duration_since(confirmed.sent_at);
        link.sample_base_delay(now, age);
        link.sample_age(age);
        // The time since the last advance samples the status cadence when
        // sends were out all along, or resumed promptly after it.
        let prompt = link.status_interval() / 4;
        let streaming = link.out_after_advance
            || link
                .last_advance_at
                .zip(link.resumed_at)
                .is_some_and(|(advance, resumed)| {
                    resumed.saturating_duration_since(advance) <= prompt
                });
        let gap = link
            .last_advance_at
            .filter(|_| streaming)
            .map(|previous| now.saturating_duration_since(previous));
        link.resumed_at = None;
        if let Some(gap) = gap {
            link.remember_gap(gap);
        }
        // A send is overdue once it has had time to land and to be caught
        // by a status: the radio's own delay, a whole status interval (a
        // status can be a snapshot that much older than its arrival), the
        // poll spacing, and a frame of jitter.
        let due_after = link.base_delay().unwrap_or(age)
            + link.staleness_allowance()
            + link.poll_spacing()
            + OVERDUE_SLACK;
        let overdue = link
            .log
            .iter()
            .skip(position + 1)
            .filter(|sent| sent.sent_at + due_after <= now)
            .count();
        let overdue = u64::try_from(overdue).unwrap_or(u64::MAX);
        if let Some(gap) = gap {
            link.sample_gap(gap);
        }
        link.last_advance_at = Some(now);

        link.acked_seq = confirmed.seq;
        link.acked_wire = Some(confirmed.wire);
        link.shown = Some(confirmed.content);
        link.log.drain(..=position);
        link.out_after_advance = link.unresolved() > 0;
        link.timeouts = 0;
        link.rto_from = Some(now);
        link.last_progress_at = Some(now);
        link.next_poll_at = link.out_after_advance.then(|| link.first_poll(now));
        if std::mem::take(&mut link.stall_warned) {
            info!(cluster, "wireless fans confirming frames again");
        }
        let halved = link.adjust_window(sends, overdue, now);
        let mac = link.mac;
        let mut shared = 0_u64;
        if halved {
            // Every cluster's frames wait in the one TX queue. Halving only
            // the cluster that saw the backlog leaves the others to keep it
            // full, and each cluster's base delay then absorbs it: with
            // three clusters on a slow radio the backlog grew past the stall
            // verdict. So every cluster with frames in that queue answers
            // it; one with nothing out holds none of it.
            for (other, peer) in self.links.iter_mut().enumerate() {
                if other != cluster
                    && !peer.holding
                    && peer.unresolved() > 0
                    && !peer.absent(now)
                    && peer.may_decrease(now)
                {
                    peer.halve(now);
                    shared += 1;
                }
            }
            debug!(
                cluster,
                overdue,
                window = self.links[cluster].window,
                shared,
                "wireless TX holding a backlog; halving the windows"
            );
        }

        self.count(|stats| {
            stats.echo_advances += 1;
            stats.advance_frames += frames;
            stats.advance_max = stats.advance_max.max(frames);
            stats.frames_delivered += frames;
            stats.overdue_max = stats.overdue_max.max(overdue);
            stats.echo_samples += 1;
            stats.echo_total += age;
            stats.echo_max = stats.echo_max.max(age);
            if let Some(gap) = gap {
                stats.gap_samples += 1;
                stats.gap_total += gap;
            }
            if late {
                stats.late_echoes += 1;
            }
            if halved {
                stats.congestion_events += 1 + shared;
            }
        });
        if let Some(mac) = mac {
            forget_resets(mac);
        }
    }

    /// Log clusters that dropped out of the table, once each.
    pub fn note_absences(&mut self, now: Instant) {
        for (cluster, link) in self.links.iter_mut().enumerate() {
            if link.absent(now) && !link.absent_logged && link.last_seen_at.is_some() {
                link.absent_logged = true;
                info!(
                    cluster,
                    "wireless cluster not heard by the RX; holding its lighting until it answers"
                );
            }
        }
    }

    /// Warn about clusters that stopped confirming, and decide when one has
    /// stopped long enough while still heard.
    pub fn stall_verdict(&mut self, now: Instant) -> Option<StallVerdict> {
        if self.silenced() {
            return None;
        }
        for (cluster, link) in self.links.iter_mut().enumerate() {
            if link.holding || link.unresolved() == 0 {
                continue;
            }
            let Some(since) = link.last_progress_at else {
                continue;
            };
            let unconfirmed_for = now.saturating_duration_since(since);
            if unconfirmed_for < ECHO_STALL_WARN || !link.heard_within(now, HEARD_WITHIN) {
                continue;
            }
            if !link.stall_warned {
                link.stall_warned = true;
                warn!(
                    cluster,
                    unconfirmed_ms = millis(unconfirmed_for),
                    unresolved = link.unresolved(),
                    timeouts = link.timeouts,
                    "wireless fans have not confirmed a frame; probing with the newest frame as the radio allows"
                );
            }
            if unconfirmed_for < ECHO_STALL {
                continue;
            }
            let resets = link.mac.map_or(0, resets_without_delivery);
            if resets < MAX_RESETS_WITHOUT_DELIVERY {
                self.reset_requested = true;
                let resets = link.mac.map_or(1, note_reset);
                return Some(StallVerdict::Reset {
                    cluster,
                    unconfirmed_for,
                    resends: link.timeouts,
                    resets,
                });
            }
            link.holding = true;
            error!(
                cluster,
                unconfirmed_ms = millis(unconfirmed_for),
                resets,
                "L-Wireless fans still confirm no frame after {resets} TX resets; holding their lighting. Power-cycle the controller"
            );
            return Some(StallVerdict::Hold {
                cluster,
                unconfirmed_for,
            });
        }
        None
    }

    /// Log the delivery report when one is due.
    pub fn report_if_due(&mut self, now: Instant) {
        let started = *self.interval_started_at.get_or_insert(now);
        let elapsed = now.saturating_duration_since(started);
        if elapsed < REPORT_INTERVAL {
            return;
        }
        let interval = std::mem::take(&mut self.interval);
        self.interval_started_at = Some(now);
        let clusters = self.links.len().max(1);
        let per_second = |count: u64| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "report rates are approximate by nature"
            )]
            let rate = count as f64 / elapsed.as_secs_f64();
            (rate * 10.0).round() / 10.0
        };
        let per_cluster = |count: u64| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "report rates are approximate by nature"
            )]
            let rate = per_second(count) / clusters as f64;
            (rate * 10.0).round() / 10.0
        };
        let windows = self
            .links
            .iter()
            .map(|link| link.window.to_string())
            .collect::<Vec<_>>()
            .join("/");
        let statuses = self
            .links
            .iter()
            .map(|link| link.echo_gap.map_or(0, millis).to_string())
            .collect::<Vec<_>>()
            .join("/");
        info!(
            clusters,
            window_s = elapsed.as_secs(),
            offered_fps = per_second(interval.frames_offered),
            sent_fps = per_cluster(interval.frames_sent),
            delivered_fps = per_cluster(interval.frames_delivered),
            coalesced = interval.frames_coalesced,
            window = %windows,
            status_ms = %statuses,
            echoes_per_s = per_second(interval.echo_advances),
            advance_mean = interval
                .advance_mean()
                .map_or(0.0, |mean| (mean * 10.0).round() / 10.0),
            advance_max = interval.advance_max,
            echo_gap_ms = interval.gap_mean().map_or(0, millis),
            echo_ms_mean = interval.echo_mean().map_or(0, millis),
            echo_ms_max = millis(interval.echo_max),
            overdue_max = interval.overdue_max,
            congestion = interval.congestion_events,
            timeouts = interval.timeouts,
            resends = interval.resends,
            restores = interval.restores,
            late_echoes = interval.late_echoes,
            drifts = interval.drifts,
            polls_per_s = per_second(interval.echo_polls),
            replies_per_s = per_second(interval.table_replies),
            tx_packets_per_s = per_second(interval.tx_packets),
            held_clusters = self.held_clusters(),
            "L-Wireless RGB delivery"
        );
    }

    fn count(&mut self, update: impl Fn(&mut DeliveryStats)) {
        update(&mut self.totals);
        update(&mut self.interval);
    }
}

/// The tag send number `sequence` of `content` carries on the wire: unique
/// per send, so its echo names this transfer and no other, never zero.
#[must_use]
pub fn wire_tag(content: Tag, sequence: u32) -> Tag {
    let mut seed = [0_u8; 8];
    seed[..4].copy_from_slice(&content);
    seed[4..].copy_from_slice(&sequence.to_be_bytes());
    effect_index_for(&seed)
}

/// A wire tag for `content` that `taken` does not reject, drawing send
/// numbers from `next` until one fits.
fn allocate_wire(content: Tag, taken: impl Fn(Tag) -> bool, mut next: impl FnMut() -> u32) -> Tag {
    loop {
        let wire = wire_tag(content, next());
        if !taken(wire) {
            return wire;
        }
    }
}

/// The next send number. It runs for the whole process from a wall-clock
/// seed, so a tag from an earlier session, or an earlier daemon, that a
/// receiver still echoes is not handed out again by chance.
fn next_send_number() -> u32 {
    static NEXT: OnceLock<AtomicU32> = OnceLock::new();
    NEXT.get_or_init(|| {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                since.subsec_nanos()
                    ^ u32::try_from(since.as_secs() & u64::from(u32::MAX)).unwrap_or(0)
            });
        AtomicU32::new(seed)
    })
    .fetch_add(1, Ordering::Relaxed)
}

/// Resets asked for on behalf of each cluster (by radio MAC) since it last
/// confirmed a frame. Kept for the process, so a reconnect cannot turn a TX
/// that stays dead into a reset loop, and per cluster, so fans that still
/// confirm cannot renew a failing cluster's budget.
fn reset_budget() -> &'static Mutex<HashMap<Mac, u32>> {
    static RESETS: OnceLock<Mutex<HashMap<Mac, u32>>> = OnceLock::new();
    RESETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resets asked for on behalf of the cluster `mac` since it last confirmed
/// a frame.
#[must_use]
pub fn resets_without_delivery(mac: Mac) -> u32 {
    reset_budget()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&mac)
        .copied()
        .unwrap_or(0)
}

fn note_reset(mac: Mac) -> u32 {
    let mut resets = reset_budget()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let count = resets.entry(mac).or_insert(0);
    *count = count.saturating_add(1);
    *count
}

fn forget_resets(mac: Mac) {
    let mut resets = reset_budget()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if !resets.is_empty() {
        resets.remove(&mac);
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: Tag = [0, 0, 0, 1];
    const T2: Tag = [0, 0, 0, 2];
    const T3: Tag = [0, 0, 0, 3];

    fn connected(now: Instant) -> DeliveryPacer {
        let mut pacer = DeliveryPacer::default();
        pacer.ensure_clusters(1);
        pacer.links[0].last_seen_at = Some(now);
        pacer
    }

    /// Send the newest frame whenever the pacer allows; returns the wires.
    fn send_all(pacer: &mut DeliveryPacer, contents: &[Tag], now: Instant) -> Vec<Tag> {
        let mut wires = Vec::new();
        for content in contents {
            pacer.submit(0, *content);
            pacer.mark_window_limits(now);
            if let Some(kind) = pacer.decide(0, now) {
                wires.push(pacer.note_sent(0, *content, kind, now));
            }
        }
        wires
    }

    fn frame(index: u32) -> Tag {
        index.to_be_bytes()
    }

    #[test]
    fn the_window_lets_several_frames_out_before_any_echo() {
        let now = Instant::now();
        let mut pacer = connected(now);
        let contents: Vec<Tag> = (1..=6).map(frame).collect();
        let wires = send_all(&mut pacer, &contents, now);
        assert_eq!(
            wires.len(),
            INITIAL_WINDOW as usize,
            "the initial window, not one frame, goes out unconfirmed"
        );
        assert!(
            pacer.held(0),
            "then the window is full and the newest waits"
        );
    }

    #[test]
    fn one_echo_confirms_every_send_up_to_the_one_it_names() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.links[0].window = 4;
        let contents: Vec<Tag> = (1..=4).map(frame).collect();
        let wires = send_all(&mut pacer, &contents, now);
        pacer.observe(0, wires[2], now + Duration::from_millis(300));
        let stats = pacer.totals();
        assert_eq!(stats.frames_delivered, 3, "the echo passed three frames");
        assert_eq!(stats.advance_max, 3);
        assert_eq!(
            pacer.links[0].unresolved(),
            1,
            "only the fourth is still out"
        );
    }

    #[test]
    fn a_full_window_grows_on_each_echo_that_frees_it() {
        let now = Instant::now();
        let mut pacer = connected(now);
        let mut at = now;
        let mut next = 1;
        for _ in 0..4 {
            let mut wires = Vec::new();
            for _ in 0..40 {
                pacer.submit(0, frame(next));
                next += 1;
                pacer.mark_window_limits(at);
                if let Some(kind) = pacer.decide(0, at) {
                    wires.push(pacer.note_sent(0, frame(next - 1), kind, at));
                }
                at += Duration::from_millis(8);
            }
            let last = *wires.last().expect("sends");
            pacer.links[0].last_seen_at = Some(at);
            pacer.observe(0, last, at);
        }
        assert!(
            pacer.links[0].window >= 16,
            "slow start doubles a window that keeps limiting: {}",
            pacer.links[0].window
        );
    }

    #[test]
    fn only_a_sustained_backlog_halves_the_window_and_not_too_often() {
        let now = Instant::now();
        let mut link = ClusterLink {
            window: 16,
            ssthresh: 16,
            echo_gap: Some(Duration::from_millis(550)),
            next_seq: 100,
            acked_seq: 50,
            ..ClusterLink::default()
        };
        let mut at = now;
        // One stale status, then a clean one, then another stale one: no
        // run of evidence, no halving.
        for overdue in [4, 0, 4, 1, 4] {
            at += Duration::from_millis(550);
            assert!(!link.adjust_window(1, overdue, at));
        }
        assert_eq!(link.window, 16);
        // The last of those opened a run; two more make three in a row.
        at += Duration::from_millis(550);
        assert!(!link.adjust_window(1, 4, at));
        at += Duration::from_millis(550);
        assert!(link.adjust_window(1, 4, at), "the third in a row halves");
        assert_eq!(link.window, 8);
        // Past the recovery point, a new run of three inside the holdoff
        // does not halve again.
        link.acked_seq = link.recovery_until;
        for _ in 0..3 {
            at += Duration::from_millis(300);
            assert!(!link.adjust_window(1, 4, at));
        }
        assert_eq!(link.window, 8, "at most one halving per three statuses");
        at += Duration::from_millis(1_700);
        assert!(link.adjust_window(1, 4, at), "after the holdoff it may");
        assert_eq!(link.window, 4);
    }

    #[test]
    fn a_backlog_one_cluster_sees_halves_every_window_with_frames_in_the_tx() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.ensure_clusters(5);
        for link in &mut pacer.links {
            link.window = 16;
            link.ssthresh = 16;
            link.echo_gap = Some(Duration::from_millis(550));
        }
        let mut wires = Vec::new();
        for index in 0..16 {
            let at = now + Duration::from_millis(33 * index);
            let content = frame(u32::try_from(index).expect("small") + 1);
            pacer.submit(0, content);
            let kind = pacer.decide(0, at).expect("room");
            wires.push(pacer.note_sent(0, content, kind, at));
        }
        // Cluster 1 has frames out; 2 has none; 3 has frames out but its
        // lighting is held; 4 has frames out but is still recovering from
        // its own halving.
        for link in &mut pacer.links[1..] {
            link.last_seen_at = Some(now);
        }
        for cluster in [1, 3, 4] {
            for index in 0..4 {
                let content = frame(100 * u32::try_from(cluster).expect("small") + index);
                pacer.submit(cluster, content);
                let kind = pacer.decide(cluster, now).expect("room");
                let _ = pacer.note_sent(cluster, content, kind, now);
            }
        }
        pacer.links[3].holding = true;
        pacer.links[4].recovery_until = pacer.links[4].next_seq - 1;
        pacer.links[0].sample_base_delay(now, Duration::from_millis(40));
        // Three statuses 550 ms apart, each confirming one more send while
        // the rest wait far past a status interval: a sustained backlog.
        let first = now + Duration::from_millis(33 * 15 + 1_500);
        for link in &mut pacer.links[1..] {
            link.last_seen_at = Some(first);
        }
        for (step, wire) in wires[1..4].iter().enumerate() {
            let at = first + Duration::from_millis(550 * u64::try_from(step).expect("small"));
            pacer.observe(0, *wire, at);
        }
        let windows: Vec<u32> = pacer.links.iter().map(|link| link.window).collect();
        assert_eq!(
            windows,
            [8, 8, 16, 16, 16],
            "the cluster that saw it and its neighbour with frames out halve"
        );
        assert_eq!(
            pacer.links[2].ssthresh, 16,
            "a cluster with nothing out keeps its slow-start ceiling"
        );
        assert_eq!(pacer.totals().congestion_events, 2);
    }

    #[test]
    fn a_send_is_overdue_only_after_a_whole_status_interval() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.links[0].window = 16;
        pacer.links[0].echo_gap = Some(Duration::from_millis(550));
        let mut wires = Vec::new();
        for index in 0..16 {
            let at = now + Duration::from_millis(33 * index);
            pacer.submit(0, frame(u32::try_from(index).expect("small") + 1));
            let kind = pacer.decide(0, at).expect("room");
            wires.push(pacer.note_sent(
                0,
                frame(u32::try_from(index).expect("small") + 1),
                kind,
                at,
            ));
        }
        pacer.links[0].sample_base_delay(now, Duration::from_millis(40));
        // A status 450 ms stale arrives just after the last send: it names
        // the second send. Everything after it is younger than a status
        // interval plus the radio's delay, so none is overdue.
        pacer.observe(0, wires[1], now + Duration::from_millis(33 * 15 + 5));
        assert_eq!(pacer.totals().overdue_max, 0);
        // 800 ms on, the next status names only the third send. That
        // status came 800 ms after the last, so a send is overdue once it
        // has outlasted the radio's delay plus those 800 ms: the eight sent
        // in the first 333 ms, which missed a whole status.
        pacer.observe(0, wires[2], now + Duration::from_millis(33 * 15 + 805));
        assert_eq!(pacer.totals().overdue_max, 8);
    }

    #[test]
    fn a_timeout_collapses_the_window_and_probes_with_the_newest_frame() {
        let now = Instant::now();
        let mut pacer = connected(now);
        let _ = send_all(&mut pacer, &[T1, T2], now);
        let later = now + ECHO_TIMEOUT_UNKNOWN;
        pacer.links[0].last_seen_at = Some(later);
        pacer.submit(0, T3);
        pacer.tick(later);
        assert_eq!(pacer.links[0].window, MIN_WINDOW);
        assert_eq!(pacer.decide(0, later), Some(SendKind::Resend));
        let _ = pacer.note_sent(0, T3, SendKind::Resend, later);
        assert_eq!(
            pacer.decide(0, later),
            None,
            "one probe at a time while recovering"
        );
    }

    #[test]
    fn unresolved_sends_never_pass_the_hard_bound() {
        let now = Instant::now();
        let mut pacer = connected(now);
        let mut at = now;
        let mut index = 0;
        for _ in 0..80 {
            // The radio is dead: nothing ever echoes. Timeouts keep freeing
            // window room, but the bound on unresolved sends holds.
            for _ in 0..8 {
                index += 1;
                pacer.submit(0, frame(index));
                pacer.links[0].last_seen_at = Some(at);
                pacer.tick(at);
                if let Some(kind) = pacer.decide(0, at) {
                    let _ = pacer.note_sent(0, frame(index), kind, at);
                }
            }
            at += ECHO_TIMEOUT_CAP;
        }
        assert!(pacer.links[0].unresolved() <= u64::from(MAX_WINDOW));
        assert_eq!(pacer.links[0].unresolved(), u64::from(MAX_WINDOW));
        assert!(
            !pacer.has_capacity(0),
            "not even the shutdown flush sends past the bound"
        );
    }

    #[test]
    fn a_restore_is_confirmed_by_its_own_echo_not_the_frame_already_showing() {
        let now = Instant::now();
        let mut pacer = connected(now);
        let first = send_all(&mut pacer, &[T1], now)[0];
        pacer.observe(0, first, now);
        pacer.owe_restores();
        assert_eq!(pacer.decide(0, now), Some(SendKind::Restore));
        let restore = pacer.note_sent(0, T1, SendKind::Restore, now);
        assert_ne!(restore, first, "every send carries its own wire tag");
        pacer.observe(0, first, now);
        assert_eq!(
            pacer.links[0].unresolved(),
            1,
            "a report from before the restore landed confirms nothing"
        );
        pacer.observe(0, restore, now);
        assert_eq!(pacer.links[0].unresolved(), 0);
        assert_eq!(
            pacer.totals().frames_delivered,
            1,
            "a restore is not another frame"
        );
    }

    #[test]
    fn a_new_tag_never_matches_what_the_cluster_echoes_now() {
        let now = Instant::now();
        let mut pacer = connected(now);
        let first = send_all(&mut pacer, &[T1], now)[0];
        pacer.observe(0, first, now);
        // A new session: nothing sent yet, the cluster still echoes the tag
        // of the last session's transfer.
        let mut session = connected(now);
        session.observe(0, first, now);
        let again = send_all(&mut session, &[T1], now)[0];
        assert_ne!(again, first);
        session.observe(0, first, now);
        assert_eq!(
            session.totals().frames_delivered,
            0,
            "the cached echo of the last session confirms nothing"
        );
    }

    #[test]
    fn clusters_are_paced_independently() {
        let now = Instant::now();
        let mut pacer = DeliveryPacer::default();
        pacer.ensure_clusters(2);
        for link in &mut pacer.links {
            link.last_seen_at = Some(now);
        }
        // The second cluster fills its window; the first has room left.
        for content in [T1, T2] {
            pacer.submit(1, content);
            let _ = pacer.note_sent(1, content, SendKind::Frame, now);
        }
        pacer.submit(0, T1);
        let _ = pacer.note_sent(0, T1, SendKind::Frame, now);
        pacer.submit(0, T3);
        pacer.submit(1, T3);
        pacer.mark_window_limits(now);
        assert_eq!(
            pacer.decide(0, now),
            Some(SendKind::Frame),
            "a full neighbour never holds a cluster with room"
        );
        assert_eq!(pacer.decide(1, now), None);
        assert!(pacer.links[1].window_limited, "the full one may grow");
    }

    #[test]
    fn a_cluster_recovering_from_a_timeout_probes_alone() {
        let now = Instant::now();
        let mut pacer = DeliveryPacer::default();
        pacer.ensure_clusters(2);
        for link in &mut pacer.links {
            link.last_seen_at = Some(now);
        }
        for cluster in 0..2 {
            for content in [T1, T2] {
                pacer.submit(cluster, content);
                let _ = pacer.note_sent(cluster, content, SendKind::Frame, now);
            }
        }
        let late = now + ECHO_TIMEOUT_UNKNOWN;
        for link in &mut pacer.links {
            link.last_seen_at = Some(late);
        }
        // Only the first cluster's echo arrives.
        let wire = pacer.links[0].log[1].wire;
        pacer.observe(0, wire, now + Duration::from_millis(1));
        pacer.tick(late);
        pacer.submit(0, T3);
        pacer.submit(1, T3);
        assert_eq!(pacer.decide(0, late), Some(SendKind::Frame));
        assert_eq!(pacer.decide(1, late), Some(SendKind::Resend));
    }

    #[test]
    fn a_frame_replaced_before_it_could_go_is_coalesced_not_queued() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.links[0].window = MIN_WINDOW;
        let contents: Vec<Tag> = (1..=5).map(frame).collect();
        let _ = send_all(&mut pacer, &contents, now);
        assert_eq!(
            pacer.totals().frames_coalesced,
            2,
            "frames three and four were replaced while the window was full"
        );
        assert!(pacer.held(0), "frame five waits");
    }

    #[test]
    fn the_timeout_follows_the_echo_cadence() {
        let mut link = ClusterLink::default();
        assert_eq!(link.timeout(), ECHO_TIMEOUT_UNKNOWN);
        link.sample_gap(Duration::from_millis(333));
        assert_eq!(link.timeout(), Duration::from_millis(999));
        link.timeouts = 1;
        assert_eq!(link.timeout(), Duration::from_millis(1_998));
    }

    #[test]
    fn one_missed_status_barely_moves_the_echo_cadence() {
        let mut link = ClusterLink::default();
        link.sample_gap(Duration::from_millis(333));
        link.sample_gap(Duration::from_secs(18));
        let gap = link.echo_gap.expect("gap");
        assert!(gap <= Duration::from_millis(500), "clipped: {gap:?}");
    }

    #[test]
    fn a_tag_that_collides_with_a_send_still_out_is_never_handed_out() {
        // Two different sends hash to the same tag under FNV-1a.
        let colliding = wire_tag([0, 0, 0, 2], 1);
        assert_eq!(colliding, wire_tag([0xAF, 0x66, 0x5D, 0x09], 2));

        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, [0, 0, 0, 2]);
        let first = allocate_wire([0, 0, 0, 2], |_| false, || 1);
        assert_eq!(first, colliding);
        pacer.links[0].log.push_back(Outstanding {
            seq: 1,
            wire: first,
            content: [0, 0, 0, 2],
            sent_at: now,
            frames_through: 1,
        });
        pacer.links[0].next_seq = 2;
        let mut numbers = [2, 3].into_iter();
        let second = allocate_wire(
            [0xAF, 0x66, 0x5D, 0x09],
            |candidate| pacer.links[0].tag_taken(candidate),
            || numbers.next().expect("a spare number"),
        );
        assert_ne!(
            second, first,
            "the colliding tag is skipped, so one echo names one send"
        );
    }

    #[test]
    fn wire_tags_differ_per_send_and_are_never_zero() {
        assert_ne!(wire_tag(T1, 1), wire_tag(T1, 2));
        assert_ne!(wire_tag(T1, 1), wire_tag(T2, 1));
        assert_ne!(wire_tag([0; 4], 0), [0; 4]);
    }
}
