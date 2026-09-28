//! Acknowledgement-paced RGB delivery.
//!
//! Every RGB transfer carries a tag, and each fan cluster's record in the
//! RX device table echoes the tag of the transfer its receiver last took
//! (spec 80 section 6.5, record bytes 20 to 23). L-Connect sends an effect
//! until the record echoes it and never sends more; this module holds the
//! live stream to the same contract.
//!
//! Each send gets its own wire tag: the frame's pixel hash mixed with a send
//! number that runs for the whole process from a wall-clock seed, and never
//! equals the tag the cluster is echoing at that moment. An echo therefore
//! names exactly one transfer, so a restore of the frame already showing, a
//! resend of a frame that did not land, or the first frame after a
//! reconnect is acknowledged by its own arrival and never by a report the RX
//! cached before it.
//!
//! - One transfer per cluster is out at a time. A frame that arrives while
//!   one is out is held; a newer frame replaces it, so what goes out next is
//!   always the newest frame, never a queue of stale ones.
//! - While a transfer is out the RX table is polled for its echo. The first
//!   poll waits most of the echo time seen so far, later polls follow
//!   closely, and they back off as a wait drags on.
//! - A transfer unconfirmed after its bounded wait (four echo times, and a
//!   conservative [`ECHO_WAIT_UNKNOWN`] before any echo is seen) goes out
//!   again carrying the newest frame. Nothing on the host can tell a lost
//!   transfer from one still queued in the TX, so a resend is a bet: the
//!   wait doubles with every resend, and a chain gets at most
//!   [`MAX_CHAIN_RESENDS`] before it waits only for an echo or the verdict
//!   below. A cluster can therefore have one transfer out, and at most
//!   that many more only after as many bounded waits passed in silence.
//! - An echo of an older transfer (one that landed after its wait ran out)
//!   counts as delivered late, teaches the echo time, and restarts the
//!   stall clock, since it proves the radio still delivers.
//! - A cluster the RX has stopped hearing is sent nothing until it answers.
//! - Fan-speed upkeep can knock a receiver back to its onboard lighting
//!   without changing its echo, so every upkeep leaves each cluster owing a
//!   restore: the first transfer after it pays the debt, and if the window
//!   is closed the restore goes out once the transfer ahead resolves.
//! - Fans that are heard but confirm nothing for [`ECHO_STALL`], across
//!   every resend their chain gets, mean the TX stopped delivering: the
//!   protocol asks the transport for the vendor reset, which ends the
//!   session the way a refused write does. At most
//!   [`MAX_RESETS_WITHOUT_DELIVERY`] resets are asked for on behalf of one
//!   cluster until that cluster confirms a frame again, across sessions;
//!   past that the cluster's lighting is held and the log says to
//!   power-cycle the controller, while clusters that still confirm keep
//!   streaming. A TX the reset does not revive, or firmware that does not
//!   echo live frames, ends in that bounded failure, never a reset loop.
//!
//! So the frame rate is whatever the radio confirms, up to what the render
//! path offers, and nothing caps it.
//!
//! Counters feed [`DeliveryStats`] and a periodic info line comparing frames
//! sent with frames the fans echoed back.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tracing::{debug, error, info, warn};

use super::frame::{Mac, effect_index_for};

/// An RGB transfer's tag, as the header carries it and the record echoes it.
pub type Tag = [u8; 4];

/// Delay before the first echo poll after a send, before any echo time is
/// known, and the least spacing of the polls that follow.
pub const ECHO_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Polls back off as a wait drags on, to no more than this apart.
pub const ECHO_POLL_MAX_INTERVAL: Duration = Duration::from_millis(250);
/// The bounded wait before any echo time is known: long enough for a slow
/// radio to get one transfer out before it is sent again.
pub const ECHO_WAIT_UNKNOWN: Duration = Duration::from_millis(300);
/// Bounds on the wait once echo times are known.
pub const ECHO_RESEND_MIN: Duration = Duration::from_millis(150);
pub const ECHO_RESEND_MAX: Duration = Duration::from_secs(1);
/// Resends of an unconfirmed chain back off to this spacing.
pub const ECHO_RESEND_CAP: Duration = Duration::from_secs(2);
/// No confirmation for this long is logged once as a warning.
pub const ECHO_STALL_WARN: Duration = Duration::from_secs(1);
/// No confirmation for this long, with the fans heard, is a TX that stopped
/// delivering.
pub const ECHO_STALL: Duration = Duration::from_secs(5);
/// Resends one unconfirmed chain gets; after them it waits for an echo or
/// the stall verdict.
pub const MAX_CHAIN_RESENDS: u32 = 3;
/// Resends a stall must have tried before it is called: every one its chain
/// gets.
pub const STALL_MIN_RESENDS: u32 = MAX_CHAIN_RESENDS;
/// Resets asked for on behalf of one cluster before it confirms a frame
/// again.
pub const MAX_RESETS_WITHOUT_DELIVERY: u32 = 2;
/// A cluster missing from every table reply for this long is not heard.
pub const ABSENT_AFTER: Duration = Duration::from_secs(3);
/// How often the delivery report is logged.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(10);
/// Transfers remembered per cluster, to recognise late echoes.
const RECENT_SENDS: usize = 8;
/// Weight of a new echo-time sample in the running average, as a divisor.
const LATENCY_SMOOTHING: u32 = 8;
/// An echo-time sample counts for at most this many averages.
const OUTLIER_CLIP: u32 = 4;

/// Why a transfer goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKind {
    /// A frame the cluster is not showing.
    Frame,
    /// The unconfirmed transfer again, carrying the newest frame, after its
    /// bounded wait ran out.
    Resend,
    /// The frame already showing, again after fan-speed upkeep, which can
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
        /// Resets asked for since this controller's fans last confirmed,
        /// this one included.
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
    /// Frames sent: a transfer of pixels its cluster had not been sent
    /// before in its current chain. Resends of the same pixels and
    /// restores are counted apart.
    pub frames_sent: u64,
    /// Sent frames a cluster echoed back, late ones included.
    pub frames_delivered: u64,
    /// Frames replaced by a newer one while their cluster's window was
    /// closed, so never sent.
    pub frames_coalesced: u64,
    /// Transfers sent again after their bounded wait.
    pub resends: u64,
    /// Showing frames re-sent after fan-speed upkeep.
    pub restores: u64,
    /// Echoes of a transfer that had already been superseded.
    pub late_echoes: u64,
    /// Echoes of a tag this session never sent while a frame of ours was
    /// showing: the receiver left our frame for its own lighting.
    pub drifts: u64,
    /// Echo polls sent to the RX.
    pub echo_polls: u64,
    /// Table replies that reached the protocol, polls and upkeep alike.
    pub table_replies: u64,
    /// Replies in which a cluster's clock had moved since the last one:
    /// how often the RX actually heard from the fans.
    pub cluster_reports: u64,
    /// USB packets queued for the TX, upkeep included. A batch that fails
    /// part way stops early, so around failures a per-second URB count on
    /// the TX can come out lower.
    pub tx_packets: u64,
    /// Echo-time samples and their sum and maximum.
    pub echo_samples: u64,
    pub echo_total: Duration,
    pub echo_max: Duration,
}

impl DeliveryStats {
    /// Mean time from a send to the poll that saw its echo.
    #[must_use]
    pub fn echo_mean(&self) -> Option<Duration> {
        let samples = u32::try_from(self.echo_samples).ok()?;
        (samples > 0).then(|| self.echo_total / samples)
    }

    fn add_echo(&mut self, sample: Duration) {
        self.echo_samples += 1;
        self.echo_total += sample;
        self.echo_max = self.echo_max.max(sample);
    }
}

/// A transfer out and not yet confirmed.
#[derive(Debug, Clone, Copy)]
struct InFlight {
    /// The tag this transfer carries on the wire.
    wire: Tag,
    /// The pixels it carries.
    content: Tag,
    /// The latest send of this chain.
    sent_at: Instant,
    /// When the chain began: nothing has been confirmed since.
    unconfirmed_since: Instant,
    resends: u32,
    /// The time to its echo is a sample of the radio's echo time; not so
    /// for a chain that spanned a gap in which the fans went unheard.
    clean: bool,
    resend_at: Instant,
    /// When this cluster next wants the table polled.
    poll_at: Instant,
}

#[derive(Debug, Clone, Copy)]
struct SentTag {
    wire: Tag,
    content: Tag,
    sent_at: Instant,
    /// The frame this transfer carried, counting from one, or zero for a
    /// restore; resends of the same pixels share their frame's number.
    frame: u64,
    echoed: bool,
}

/// One cluster's side of the window.
#[derive(Debug, Default)]
struct ClusterLink {
    /// The cluster's radio MAC, which keys its reset budget.
    mac: Option<Mac>,
    /// Pixels of the newest frame submitted for this cluster.
    newest: Option<Tag>,
    /// Pixels of ours the cluster last echoed; `None` while it shows
    /// something this session never sent.
    confirmed: Option<Tag>,
    in_flight: Option<InFlight>,
    recent: VecDeque<SentTag>,
    /// Frames numbered for the delivery count.
    frames: u64,
    last_delivered_frame: u64,
    last_seen_at: Option<Instant>,
    last_clock: Option<[u8; 4]>,
    /// The tag the cluster echoed last, whoever sent it.
    last_echo: Option<Tag>,
    echo_average: Option<Duration>,
    /// Fan-speed upkeep ran since the last transfer went out.
    restore_owed: bool,
    /// The cluster's reset budget is spent: it gets no RGB this session.
    holding: bool,
    stall_warned: bool,
    absent_logged: bool,
}

impl ClusterLink {
    fn heard_within(&self, now: Instant, window: Duration) -> bool {
        self.last_seen_at
            .is_some_and(|seen| now.saturating_duration_since(seen) < window)
    }

    fn absent(&self, now: Instant) -> bool {
        !self.heard_within(now, ABSENT_AFTER)
    }

    /// The bounded wait for one echo: four echo times, within bounds, or
    /// the conservative default before any echo has been seen.
    fn resend_after(&self) -> Duration {
        self.echo_average.map_or(ECHO_WAIT_UNKNOWN, |average| {
            (average * 4).clamp(ECHO_RESEND_MIN, ECHO_RESEND_MAX)
        })
    }

    /// When to poll first after a send: most of an echo time, so the poll
    /// rarely comes back empty, and never sooner than the poll spacing.
    fn first_poll_delay(&self) -> Duration {
        let wait = self.resend_after();
        self.echo_average
            .map_or(ECHO_POLL_INTERVAL, |average| average * 3 / 4)
            .clamp(ECHO_POLL_INTERVAL, wait / 2)
    }

    /// Spacing of the polls after the first: an eighth of an echo time,
    /// doubling with every resend of the chain.
    fn poll_spacing(&self, resends: u32) -> Duration {
        self.echo_average
            .map_or(ECHO_POLL_INTERVAL, |average| average / 8)
            .clamp(ECHO_POLL_INTERVAL, ECHO_POLL_MAX_INTERVAL)
            .saturating_mul(1 << resends.min(5))
            .min(ECHO_POLL_MAX_INTERVAL)
    }

    fn sent(&mut self, wire: Tag) -> Option<&mut SentTag> {
        self.recent
            .iter_mut()
            .rev()
            .find(|entry| entry.wire == wire)
    }

    fn remember(&mut self, entry: SentTag) {
        if self.recent.len() == RECENT_SENDS {
            self.recent.pop_front();
        }
        self.recent.push_back(entry);
    }

    /// Whether the echo of `entry` is the first delivery of its frame.
    fn deliver(&mut self, entry: SentTag) -> bool {
        if entry.frame == 0 || entry.frame <= self.last_delivered_frame {
            return false;
        }
        self.last_delivered_frame = entry.frame;
        true
    }

    /// Fold one echo time into the running average. A sample is clipped to
    /// a few averages first, so one outlier (a poll that timed out, a
    /// report the RX sat on) cannot stretch every wait after it; a radio
    /// that really slowed down still gets there within a few samples.
    fn sample_echo(&mut self, sample: Duration) {
        self.echo_average = Some(match self.echo_average {
            None => sample,
            Some(average) => {
                let sample = sample.min(average.saturating_mul(OUTLIER_CLIP));
                if sample >= average {
                    average + sample.saturating_sub(average) / LATENCY_SMOOTHING
                } else {
                    average.saturating_sub(average.saturating_sub(sample) / LATENCY_SMOOTHING)
                }
            }
        });
    }
}

/// The controller's echo-paced delivery state for one session.
#[derive(Debug, Default)]
pub struct DeliveryPacer {
    links: Vec<ClusterLink>,
    reset_requested: bool,
    totals: DeliveryStats,
    window: DeliveryStats,
    window_started_at: Option<Instant>,
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

    /// Clusters whose lighting is held because their reset budget is spent.
    #[must_use]
    pub fn held_clusters(&self) -> usize {
        self.links.iter().filter(|link| link.holding).count()
    }

    /// Whether the protocol has asked for the TX reset: the session is
    /// ending and nothing more should be written.
    #[must_use]
    pub const fn reset_requested(&self) -> bool {
        self.reset_requested
    }

    /// Grow the window to cover `clusters`, keeping what is known.
    pub fn ensure_clusters(&mut self, clusters: usize) {
        if self.links.len() < clusters {
            self.links.resize_with(clusters, ClusterLink::default);
        }
    }

    /// Fit the window to the frozen routing, one link per cluster MAC in
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
    /// owes a restore, paid by the next transfer it gets.
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
        let unsent = replaced.is_some_and(|previous| {
            previous != content && !link.recent.iter().any(|entry| entry.content == previous)
        });
        if unsent {
            self.count(|stats| stats.frames_coalesced += 1);
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
        if link.holding || link.absent(now) {
            return None;
        }
        match link.in_flight {
            Some(flight) if now >= flight.resend_at && flight.resends < MAX_CHAIN_RESENDS => {
                Some(SendKind::Resend)
            }
            Some(_) => None,
            None if link.confirmed != Some(newest) => Some(SendKind::Frame),
            None if link.restore_owed => Some(SendKind::Restore),
            None => None,
        }
    }

    /// Whether `cluster`'s newest frame has not gone out: held behind a
    /// transfer of other pixels, or never sent at all.
    #[must_use]
    pub fn held(&self, cluster: usize) -> bool {
        self.links.get(cluster).is_some_and(|link| {
            link.newest.is_some_and(|newest| match link.in_flight {
                Some(flight) => flight.content != newest,
                None => link.confirmed != Some(newest),
            })
        })
    }

    /// A transfer of `content` goes to `cluster` for `kind`. Returns the
    /// tag it carries on the wire.
    pub fn note_sent(&mut self, cluster: usize, content: Tag, kind: SendKind, now: Instant) -> Tag {
        self.ensure_clusters(cluster + 1);
        let link = &mut self.links[cluster];
        let mut wire = wire_tag(content, next_send_number());
        while Some(wire) == link.last_echo {
            wire = wire_tag(content, next_send_number());
        }
        link.restore_owed = false;
        let previous = link.in_flight;

        // A frame is new pixels for the chain; a resend of the same pixels
        // is the same frame again, and a restore is no frame at all.
        let same_pixels_resend =
            kind == SendKind::Resend && previous.is_some_and(|flight| flight.content == content);
        let frame = if kind == SendKind::Restore {
            0
        } else if same_pixels_resend {
            link.recent
                .iter()
                .rev()
                .find(|entry| entry.content == content)
                .map_or(0, |entry| entry.frame)
        } else {
            link.frames += 1;
            link.frames
        };
        let new_frame = kind != SendKind::Restore && !same_pixels_resend;
        link.remember(SentTag {
            wire,
            content,
            sent_at: now,
            frame,
            echoed: false,
        });

        let first_poll = now + link.first_poll_delay();
        let resend_after = link.resend_after();
        let flight = match (kind, previous) {
            (SendKind::Resend, Some(previous)) => {
                let resends = previous.resends.saturating_add(1);
                if resends == 1 {
                    debug!(
                        cluster,
                        waited_ms = millis(now.saturating_duration_since(previous.sent_at)),
                        "wireless transfer unconfirmed after its bounded wait; sending the newest frame again"
                    );
                }
                InFlight {
                    wire,
                    content,
                    sent_at: now,
                    unconfirmed_since: previous.unconfirmed_since,
                    resends,
                    clean: previous.clean,
                    resend_at: now
                        + resend_after
                            .saturating_mul(1 << resends.min(4))
                            .min(ECHO_RESEND_CAP),
                    poll_at: first_poll,
                }
            }
            _ => InFlight {
                wire,
                content,
                sent_at: now,
                unconfirmed_since: now,
                resends: 0,
                clean: true,
                resend_at: now + resend_after,
                poll_at: first_poll,
            },
        };
        link.in_flight = Some(flight);
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
                    && link.in_flight.is_some_and(|flight| now >= flight.poll_at)
            })
    }

    /// An echo poll went out.
    pub fn note_poll(&mut self, now: Instant) {
        self.reschedule_polls(now);
        self.count(|stats| stats.echo_polls += 1);
    }

    /// A table poll went out that answers whatever echo polls were due:
    /// reschedule every waiting cluster's next one, further out the longer
    /// its wait has run.
    pub fn reschedule_polls(&mut self, now: Instant) {
        for link in &mut self.links {
            let Some(flight) = link.in_flight else {
                continue;
            };
            if now >= flight.poll_at {
                let spacing = link.poll_spacing(flight.resends);
                if let Some(flight) = link.in_flight.as_mut() {
                    flight.poll_at = now + spacing;
                }
            }
        }
    }

    /// A table reply reached the protocol.
    pub fn note_table_reply(&mut self) {
        self.count(|stats| stats.table_replies += 1);
    }

    /// `cluster` reported `echo` with its clock at `clock` in a reply that
    /// reached the protocol at `now`.
    pub fn observe(&mut self, cluster: usize, echo: Tag, clock: [u8; 4], now: Instant) {
        self.ensure_clusters(cluster + 1);
        let link = &mut self.links[cluster];
        if link.absent_logged {
            link.absent_logged = false;
            info!(
                cluster,
                "wireless cluster heard again; resuming its lighting"
            );
            // Nothing could be confirmed while it was not heard, so its
            // wait starts over, the newest frame goes out at once, and the
            // time to an echo that spans the gap says nothing about the
            // radio.
            if let Some(flight) = link.in_flight.as_mut() {
                flight.unconfirmed_since = now;
                flight.resends = 0;
                flight.resend_at = now;
                flight.clean = false;
            }
        }
        link.last_seen_at = Some(now);
        link.last_echo = Some(echo);
        let reported = link.last_clock.replace(clock) != Some(clock);

        let mut delivered = false;
        let mut late = false;
        let mut drifted = false;
        let mut confirmed_any = false;
        let mut echo_sample = None;

        if let Some(flight) = link.in_flight
            && flight.wire == echo
        {
            if flight.clean {
                let sample = now.saturating_duration_since(flight.sent_at);
                echo_sample = Some(sample);
                link.sample_echo(sample);
            }
            link.in_flight = None;
            link.confirmed = Some(flight.content);
            confirmed_any = true;
            if std::mem::take(&mut link.stall_warned) {
                info!(
                    cluster,
                    unconfirmed_ms =
                        millis(now.saturating_duration_since(flight.unconfirmed_since)),
                    resends = flight.resends,
                    "wireless fans confirming frames again"
                );
            }
            if let Some(entry) = link.sent(echo) {
                entry.echoed = true;
                let entry = *entry;
                delivered = link.deliver(entry);
            }
        } else if let Some(entry) = link.sent(echo) {
            if !entry.echoed {
                // An older transfer landed after its wait ran out. The
                // radio delivers, only slower than the wait allowed: learn
                // the echo time and restart the stall clock.
                entry.echoed = true;
                let entry = *entry;
                late = true;
                confirmed_any = true;
                delivered = link.deliver(entry);
                link.sample_echo(now.saturating_duration_since(entry.sent_at));
                if let Some(flight) = link.in_flight.as_mut() {
                    flight.unconfirmed_since = now;
                    flight.resends = 0;
                }
            }
            link.confirmed = link.sent(echo).map(|entry| entry.content);
        } else {
            drifted = link.confirmed.is_some();
            link.confirmed = None;
        }

        self.count(|stats| {
            if reported {
                stats.cluster_reports += 1;
            }
            if delivered {
                stats.frames_delivered += 1;
            }
            if late {
                stats.late_echoes += 1;
            }
            if drifted {
                stats.drifts += 1;
            }
            if let Some(sample) = echo_sample {
                stats.add_echo(sample);
            }
        });
        if confirmed_any && let Some(mac) = self.links[cluster].mac {
            forget_resets(mac);
        }
        if drifted {
            debug!(
                cluster,
                echo = ?echo,
                "wireless receiver left our frame for its own lighting; sending the newest frame again"
            );
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
    /// stopped long enough, often enough, while still heard.
    pub fn stall_verdict(&mut self, now: Instant) -> Option<StallVerdict> {
        if self.silenced() {
            return None;
        }
        for (cluster, link) in self.links.iter_mut().enumerate() {
            let Some(flight) = link.in_flight.filter(|_| !link.holding) else {
                continue;
            };
            let unconfirmed_for = now.saturating_duration_since(flight.unconfirmed_since);
            if unconfirmed_for < ECHO_STALL_WARN || !link.heard_within(now, ECHO_STALL_WARN) {
                continue;
            }
            if !link.stall_warned {
                link.stall_warned = true;
                warn!(
                    cluster,
                    unconfirmed_ms = millis(unconfirmed_for),
                    resends = flight.resends,
                    "wireless fans have not confirmed a frame; resending the newest frame as the radio allows"
                );
            }
            if unconfirmed_for < ECHO_STALL || flight.resends < STALL_MIN_RESENDS {
                continue;
            }
            let resets = link.mac.map_or(0, resets_without_delivery);
            if resets < MAX_RESETS_WITHOUT_DELIVERY {
                self.reset_requested = true;
                let resets = link.mac.map_or(1, note_reset);
                return Some(StallVerdict::Reset {
                    cluster,
                    unconfirmed_for,
                    resends: flight.resends,
                    resets,
                });
            }
            link.holding = true;
            link.in_flight = None;
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
        let started = *self.window_started_at.get_or_insert(now);
        let elapsed = now.saturating_duration_since(started);
        if elapsed < REPORT_INTERVAL {
            return;
        }
        let window = std::mem::take(&mut self.window);
        self.window_started_at = Some(now);
        let clusters = self.links.len().max(1);
        let per_cluster_rate = |count: u64| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "report rates are approximate by nature"
            )]
            let rate = count as f64 / elapsed.as_secs_f64() / clusters as f64;
            (rate * 10.0).round() / 10.0
        };
        let per_second = |count: u64| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "report rates are approximate by nature"
            )]
            let rate = count as f64 / elapsed.as_secs_f64();
            (rate * 10.0).round() / 10.0
        };
        info!(
            clusters,
            window_s = elapsed.as_secs(),
            offered_fps = per_second(window.frames_offered),
            sent_fps = per_cluster_rate(window.frames_sent),
            delivered_fps = per_cluster_rate(window.frames_delivered),
            coalesced = window.frames_coalesced,
            resends = window.resends,
            restores = window.restores,
            late_echoes = window.late_echoes,
            drifts = window.drifts,
            echo_ms_mean = window.echo_mean().map_or(0, millis),
            echo_ms_max = millis(window.echo_max),
            polls_per_s = per_second(window.echo_polls),
            replies_per_s = per_second(window.table_replies),
            reports_per_s = per_second(window.cluster_reports),
            tx_packets_per_s = per_second(window.tx_packets),
            held_clusters = self.held_clusters(),
            "L-Wireless RGB delivery"
        );
    }

    fn count(&mut self, update: impl Fn(&mut DeliveryStats)) {
        update(&mut self.totals);
        update(&mut self.window);
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

    fn connected(now: Instant) -> DeliveryPacer {
        let mut pacer = DeliveryPacer::default();
        pacer.ensure_clusters(1);
        pacer.links[0].last_seen_at = Some(now);
        pacer
    }

    #[test]
    fn one_transfer_per_cluster_is_ever_unconfirmed() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, T1);
        assert_eq!(pacer.decide(0, now), Some(SendKind::Frame));
        let wire = pacer.note_sent(0, T1, SendKind::Frame, now);
        pacer.submit(0, T2);
        assert_eq!(pacer.decide(0, now), None, "the window is closed");
        let later = now + Duration::from_millis(20);
        pacer.observe(0, wire, [0, 0, 0, 1], later);
        assert_eq!(
            pacer.decide(0, later),
            Some(SendKind::Frame),
            "the echo reopens it for the newest frame"
        );
    }

    #[test]
    fn a_restore_is_confirmed_by_its_own_echo_not_the_frame_already_showing() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, T1);
        let first = pacer.note_sent(0, T1, SendKind::Frame, now);
        pacer.observe(0, first, [0, 0, 0, 1], now);
        let restore = pacer.note_sent(0, T1, SendKind::Restore, now);
        assert_ne!(restore, first, "every send carries its own wire tag");
        pacer.observe(0, first, [0, 0, 0, 2], now);
        pacer.submit(0, T2);
        assert_eq!(
            pacer.decide(0, now),
            None,
            "a report from before the restore landed does not release the next frame"
        );
        pacer.observe(0, restore, [0, 0, 0, 3], now);
        assert_eq!(pacer.decide(0, now), Some(SendKind::Frame));
    }

    #[test]
    fn a_restore_owed_by_upkeep_waits_for_the_transfer_ahead_then_goes_out() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, T1);
        let first = pacer.note_sent(0, T1, SendKind::Frame, now);
        // PWM goes out while the frame is still unconfirmed.
        pacer.owe_restores();
        assert_eq!(pacer.decide(0, now), None, "the window is closed");
        pacer.observe(0, first, [0, 0, 0, 1], now);
        assert_eq!(
            pacer.decide(0, now),
            Some(SendKind::Restore),
            "the frame may have landed before the PWM; it goes out again"
        );
        let restore = pacer.note_sent(0, T1, SendKind::Restore, now);
        pacer.observe(0, restore, [0, 0, 0, 2], now);
        assert_eq!(pacer.decide(0, now), None, "the debt is paid");
    }

    #[test]
    fn a_new_tag_never_matches_what_the_cluster_echoes_now() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, T1);
        let first = pacer.note_sent(0, T1, SendKind::Frame, now);
        pacer.observe(0, first, [0, 0, 0, 1], now);
        // A new session: nothing sent yet, the cluster still echoes the tag
        // of the last session's transfer.
        let mut session = connected(now);
        session.observe(0, first, [0, 0, 0, 2], now);
        session.submit(0, T1);
        assert_eq!(session.decide(0, now), Some(SendKind::Frame));
        let again = session.note_sent(0, T1, SendKind::Frame, now);
        assert_ne!(again, first);
        session.observe(0, first, [0, 0, 0, 3], now);
        assert_eq!(
            session.totals().frames_delivered,
            0,
            "the cached echo of the last session confirms nothing"
        );
    }

    #[test]
    fn a_chain_stops_resending_after_its_resends_and_waits_for_an_echo() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, T1);
        let mut at = now;
        let _ = pacer.note_sent(0, T1, SendKind::Frame, at);
        for _ in 0..MAX_CHAIN_RESENDS {
            at += ECHO_RESEND_CAP;
            pacer.links[0].last_seen_at = Some(at);
            assert_eq!(pacer.decide(0, at), Some(SendKind::Resend));
            let _ = pacer.note_sent(0, T1, SendKind::Resend, at);
        }
        at += ECHO_RESEND_CAP;
        pacer.links[0].last_seen_at = Some(at);
        assert_eq!(
            pacer.decide(0, at),
            None,
            "no more bets: an echo or the stall verdict decides"
        );
    }

    #[test]
    fn a_frame_replaced_while_held_is_coalesced_not_queued() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, T1);
        let _ = pacer.note_sent(0, T1, SendKind::Frame, now);
        pacer.submit(0, T2);
        pacer.submit(0, [0, 0, 0, 3]);
        assert_eq!(pacer.totals().frames_coalesced, 1);
    }

    #[test]
    fn the_newest_frame_is_held_behind_a_transfer_of_other_pixels() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, T1);
        let first = pacer.note_sent(0, T1, SendKind::Frame, now);
        pacer.observe(0, first, [0, 0, 0, 1], now);
        pacer.submit(0, T2);
        let _ = pacer.note_sent(0, T2, SendKind::Frame, now);
        // The render path goes back to what the fans last confirmed while
        // the second transfer is still out.
        pacer.submit(0, T1);
        assert!(
            pacer.held(0),
            "the confirmed pixels must go out again after the transfer ahead"
        );
    }

    #[test]
    fn the_bounded_wait_grows_with_the_echo_time() {
        let mut link = ClusterLink::default();
        assert_eq!(link.resend_after(), ECHO_WAIT_UNKNOWN);
        link.sample_echo(Duration::from_millis(100));
        assert_eq!(link.resend_after(), Duration::from_millis(400));
        for _ in 0..40 {
            link.sample_echo(Duration::from_secs(2));
        }
        assert_eq!(link.resend_after(), ECHO_RESEND_MAX);
    }

    #[test]
    fn one_outlier_barely_moves_the_echo_time() {
        let mut link = ClusterLink::default();
        link.sample_echo(Duration::from_millis(30));
        link.sample_echo(Duration::from_secs(18));
        let average = link.echo_average.expect("average");
        assert!(
            average <= Duration::from_millis(50),
            "an 18 s echo across a gap is clipped: {average:?}"
        );
    }

    #[test]
    fn wire_tags_differ_per_send_and_are_never_zero() {
        assert_ne!(wire_tag(T1, 1), wire_tag(T1, 2));
        assert_ne!(wire_tag(T1, 1), wire_tag(T2, 1));
        assert_ne!(wire_tag([0; 4], 0), [0; 4]);
    }
}
