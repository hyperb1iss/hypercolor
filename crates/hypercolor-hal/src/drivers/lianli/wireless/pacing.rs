//! Acknowledgement-paced RGB delivery.
//!
//! Every RGB transfer carries a tag, and each fan cluster's record in the
//! RX device table echoes the tag its receiver last took (spec 80 section
//! 6.5, record bytes 20 to 23). L-Connect sends an effect until the record
//! echoes it and never sends more; this module holds the live stream to the
//! same contract:
//!
//! - at most one transfer per cluster is unconfirmed. A frame that arrives
//!   while one is out is held, and a newer frame replaces it, so what goes
//!   out next is always the newest frame, never a queue of stale ones;
//! - while a transfer is out, the RX table is polled for its echo; the first
//!   poll waits for most of the echo time seen so far, later polls follow
//!   closely, and they back off as a wait drags on;
//! - a transfer whose echo does not come within the bounded wait is sent
//!   again with the newest frame (a lost radio packet is normal). The wait
//!   is four echo times, so a slow radio earns a longer one;
//! - an echo of an older transfer (one that landed after its wait ran out)
//!   is counted as delivered late and restarts the stall clock, since it
//!   proves the radio still delivers;
//! - a cluster the RX has stopped hearing is sent nothing until it answers;
//! - fans that are heard but confirm nothing for [`ECHO_STALL`], across at
//!   least [`STALL_MIN_RESENDS`] resends, after confirming earlier in the
//!   session, are a TX that stopped delivering: the protocol asks the
//!   transport for the vendor reset, which ends the session the way a
//!   refused write does. A session that never saw a confirmation keeps
//!   probing slowly instead, so firmware that does not echo live frames, or
//!   a TX the reset did not revive, cannot drive a reset loop.
//!
//! So the frame rate is whatever the radio confirms, up to what the render
//! path offers, and the TX never holds more unconfirmed RGB than one
//! transfer per cluster.
//!
//! Counters feed [`DeliveryStats`] and a periodic info line comparing frames
//! sent with frames the fans echoed back.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

/// An RGB transfer's tag, as the header carries it and the record echoes it.
pub type Tag = [u8; 4];

/// Delay before the first echo poll after a send, before any echo time is
/// known, and the spacing of the polls that follow while a wait is young.
pub const ECHO_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Polls back off as a wait drags on, to no more than this apart.
pub const ECHO_POLL_MAX_INTERVAL: Duration = Duration::from_millis(250);
/// Bounds on the wait for one transfer's echo before it is sent again.
pub const ECHO_RESEND_MIN: Duration = Duration::from_millis(150);
pub const ECHO_RESEND_MAX: Duration = Duration::from_secs(1);
/// Resends of an unconfirmed transfer back off to this spacing.
pub const ECHO_RESEND_CAP: Duration = Duration::from_secs(2);
/// No confirmation for this long is logged once as a warning.
pub const ECHO_STALL_WARN: Duration = Duration::from_secs(1);
/// No confirmation for this long, with the fans heard, is a TX that stopped
/// delivering.
pub const ECHO_STALL: Duration = Duration::from_secs(5);
/// Resends a stall must have tried before it is called.
pub const STALL_MIN_RESENDS: u32 = 3;
/// A cluster missing from every table reply for this long is not heard.
pub const ABSENT_AFTER: Duration = Duration::from_secs(3);
/// How often the delivery report is logged.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(10);
/// Warnings about a stall that cannot be called repeat this often.
const STALL_REPEAT_WARN: Duration = Duration::from_mins(5);
/// Tags remembered per cluster, to recognise late echoes.
const RECENT_TAGS: usize = 8;
/// Weight of a new echo-time sample in the running average, as a divisor.
const LATENCY_SMOOTHING: u32 = 8;
/// An echo-time sample counts for at most this many averages.
const OUTLIER_CLIP: u32 = 4;

/// Why a transfer goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKind {
    /// A frame the cluster has not confirmed.
    Frame,
    /// The unconfirmed transfer again, carrying the newest frame, after its
    /// bounded wait ran out.
    Resend,
    /// The confirmed frame again after fan-speed upkeep, which can knock a
    /// receiver back to its onboard lighting without changing its echo.
    Restore,
}

/// Delivery counters, summed over every cluster of a controller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeliveryStats {
    /// Frames the render path handed the protocol.
    pub frames_offered: u64,
    /// Transfers carrying a frame their cluster had not been sent before.
    pub frames_sent: u64,
    /// Sent frames a cluster echoed back, late ones included.
    pub frames_delivered: u64,
    /// Frames replaced by a newer one while their cluster's window was
    /// closed, so never sent.
    pub frames_coalesced: u64,
    /// Transfers sent again after their bounded wait.
    pub resends: u64,
    /// Confirmed frames re-sent after fan-speed upkeep.
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
    /// USB packets written to the TX, upkeep included; the figure a
    /// per-second URB count on the TX should match.
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
    tag: Tag,
    /// The latest send of this chain.
    sent_at: Instant,
    /// When the chain began: nothing has been confirmed since.
    unconfirmed_since: Instant,
    resends: u32,
    /// This tag went out once, so the time to its echo is a clean sample
    /// of the radio's echo time. A resend of the same tag, or a restore of
    /// the frame already showing, is not.
    clean: bool,
    resend_at: Instant,
    /// When this cluster next wants the table polled.
    poll_at: Instant,
}

#[derive(Debug, Clone, Copy)]
struct SentTag {
    tag: Tag,
    sent_at: Instant,
    echoed: bool,
}

/// One cluster's side of the window.
#[derive(Debug, Default)]
struct ClusterLink {
    /// Tag of the newest frame submitted for this cluster.
    newest: Option<Tag>,
    /// The tag of ours the cluster last echoed; `None` while it shows
    /// something this session never sent.
    confirmed: Option<Tag>,
    in_flight: Option<InFlight>,
    recent: VecDeque<SentTag>,
    last_seen_at: Option<Instant>,
    last_clock: Option<[u8; 4]>,
    echo_average: Option<Duration>,
    ever_confirmed: bool,
    stall_warned: bool,
    probe_warned_at: Option<Instant>,
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

    /// The bounded wait for one echo: four echo times, within bounds.
    fn resend_after(&self) -> Duration {
        self.echo_average
            .map_or(ECHO_RESEND_MIN, |average| average * 4)
            .clamp(ECHO_RESEND_MIN, ECHO_RESEND_MAX)
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

    /// The newest record of a send of `tag`.
    fn sent(&mut self, tag: Tag) -> Option<&mut SentTag> {
        self.recent.iter_mut().rev().find(|entry| entry.tag == tag)
    }

    fn remember(&mut self, tag: Tag, now: Instant) {
        if self.recent.len() == RECENT_TAGS {
            self.recent.pop_front();
        }
        self.recent.push_back(SentTag {
            tag,
            sent_at: now,
            echoed: false,
        });
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

/// What a report or a verdict says about one cluster.
#[derive(Debug, Clone, Copy)]
pub struct StallVerdict {
    /// Index of the cluster that stopped confirming.
    pub cluster: usize,
    /// How long nothing has been confirmed.
    pub unconfirmed_for: Duration,
    /// Resends tried in that time.
    pub resends: u32,
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

    /// Whether the protocol has asked for the TX reset; the session is
    /// ending and nothing more should be sent.
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

    /// Fit the window to the frozen routing: a cluster seen during connect
    /// that did not make the routing has no link left to report on.
    pub fn fit_clusters(&mut self, clusters: usize) {
        self.links.truncate(clusters);
        self.ensure_clusters(clusters);
    }

    /// A cluster was in the table at connect: it counts as heard from then.
    pub fn note_connected(&mut self, cluster: usize, now: Instant) {
        self.ensure_clusters(cluster + 1);
        let link = &mut self.links[cluster];
        link.last_seen_at.get_or_insert(now);
    }

    /// The render path handed over a frame.
    pub fn note_frame_offered(&mut self) {
        self.count(|stats| stats.frames_offered += 1);
    }

    /// USB packets written to the TX.
    pub fn note_tx_packets(&mut self, packets: usize) {
        let packets = u64::try_from(packets).unwrap_or(u64::MAX);
        self.count(|stats| stats.tx_packets += packets);
    }

    /// The render path's newest frame carries `tag` for `cluster`.
    pub fn submit(&mut self, cluster: usize, tag: Tag) {
        self.ensure_clusters(cluster + 1);
        let link = &mut self.links[cluster];
        let replaced = link.newest.replace(tag);
        let unsent = replaced.is_some_and(|previous| {
            previous != tag && !link.recent.iter().any(|entry| entry.tag == previous)
        });
        if unsent {
            self.count(|stats| stats.frames_coalesced += 1);
        }
    }

    /// Whether `cluster` should be sent a transfer now, and why.
    ///
    /// `restore` asks for the confirmed frame again when nothing newer is
    /// waiting (the fan-speed upkeep's recovery).
    #[must_use]
    pub fn decide(&self, cluster: usize, now: Instant, restore: bool) -> Option<SendKind> {
        if self.reset_requested {
            return None;
        }
        let link = self.links.get(cluster)?;
        let newest = link.newest?;
        if link.absent(now) {
            return None;
        }
        match link.in_flight {
            Some(flight) if now >= flight.resend_at => Some(SendKind::Resend),
            Some(_) => None,
            None if link.confirmed != Some(newest) => Some(SendKind::Frame),
            None if restore => Some(SendKind::Restore),
            None => None,
        }
    }

    /// A transfer of `tag` went to `cluster` for `kind`.
    pub fn note_sent(&mut self, cluster: usize, tag: Tag, kind: SendKind, now: Instant) {
        self.ensure_clusters(cluster + 1);
        let link = &mut self.links[cluster];
        let previous = link.in_flight;
        // A frame is a new send even when its pixels match an older one; a
        // resend of the same tag and a restore are not.
        let new_frame = match kind {
            SendKind::Frame => true,
            SendKind::Resend => previous.is_none_or(|flight| flight.tag != tag),
            SendKind::Restore => false,
        };
        if new_frame {
            link.remember(tag, now);
        } else if kind == SendKind::Resend
            && let Some(entry) = link.sent(tag)
        {
            entry.sent_at = now;
        }

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
                    tag,
                    sent_at: now,
                    unconfirmed_since: previous.unconfirmed_since,
                    resends,
                    clean: new_frame,
                    resend_at: now
                        + resend_after
                            .saturating_mul(1 << resends.min(4))
                            .min(ECHO_RESEND_CAP),
                    poll_at: first_poll,
                }
            }
            _ => InFlight {
                tag,
                sent_at: now,
                unconfirmed_since: now,
                resends: 0,
                clean: kind == SendKind::Frame,
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
    }

    /// Whether `cluster`'s newest frame has not gone out: held behind an
    /// unconfirmed transfer, or never sent at all.
    #[must_use]
    pub fn held(&self, cluster: usize) -> bool {
        self.links.get(cluster).is_some_and(|link| {
            link.newest.is_some_and(|newest| {
                link.confirmed != Some(newest)
                    && link.in_flight.is_none_or(|flight| flight.tag != newest)
            })
        })
    }

    /// Whether an echo poll is due now.
    #[must_use]
    pub fn poll_due(&self, now: Instant) -> bool {
        !self.reset_requested
            && self.links.iter().any(|link| {
                !link.absent(now) && link.in_flight.is_some_and(|flight| now >= flight.poll_at)
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
        let reported = link.last_clock.replace(clock) != Some(clock);

        let mut delivered = false;
        let mut late = false;
        let mut drifted = false;
        let mut echo_sample = None;

        if let Some(flight) = link.in_flight
            && flight.tag == echo
        {
            let transition = link.confirmed != Some(echo);
            if flight.clean && transition {
                let sample = now.saturating_duration_since(flight.sent_at);
                echo_sample = Some(sample);
                link.sample_echo(sample);
            }
            link.in_flight = None;
            link.confirmed = Some(echo);
            link.ever_confirmed = true;
            if std::mem::take(&mut link.stall_warned) {
                info!(
                    cluster,
                    unconfirmed_ms =
                        millis(now.saturating_duration_since(flight.unconfirmed_since)),
                    resends = flight.resends,
                    "wireless fans confirming frames again"
                );
            }
            link.probe_warned_at = None;
            if let Some(entry) = link.sent(echo)
                && !entry.echoed
            {
                entry.echoed = true;
                delivered = true;
            }
        } else if link.confirmed != Some(echo) {
            if let Some(entry) = link.sent(echo) {
                if !entry.echoed {
                    // An older transfer landed after its wait ran out. The
                    // radio delivers, only slower than the wait allowed:
                    // learn the echo time and restart the stall clock.
                    entry.echoed = true;
                    delivered = true;
                    late = true;
                    let sample = now.saturating_duration_since(entry.sent_at);
                    link.sample_echo(sample);
                    link.ever_confirmed = true;
                    if let Some(flight) = link.in_flight.as_mut() {
                        flight.unconfirmed_since = now;
                        flight.resends = 0;
                    }
                }
                link.confirmed = Some(echo);
            } else {
                drifted = link.confirmed.is_some();
                link.confirmed = None;
            }
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

    /// Warn about clusters that stopped confirming, and return the verdict
    /// when one has stopped long enough, often enough, while still heard,
    /// after confirming earlier in the session.
    pub fn stall_verdict(&mut self, now: Instant) -> Option<StallVerdict> {
        if self.reset_requested {
            return None;
        }
        for (cluster, link) in self.links.iter_mut().enumerate() {
            let Some(flight) = link.in_flight else {
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
            if link.ever_confirmed {
                self.reset_requested = true;
                return Some(StallVerdict {
                    cluster,
                    unconfirmed_for,
                    resends: flight.resends,
                });
            }
            let warn_due = link
                .probe_warned_at
                .is_none_or(|at| now.saturating_duration_since(at) >= STALL_REPEAT_WARN);
            if warn_due {
                link.probe_warned_at = Some(now);
                warn!(
                    cluster,
                    unconfirmed_ms = millis(unconfirmed_for),
                    resends = flight.resends,
                    "wireless fans have confirmed no frame this session; probing with the newest frame every few seconds. Power-cycle the controller if this persists"
                );
            }
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
            "L-Wireless RGB delivery"
        );
    }

    fn count(&mut self, update: impl Fn(&mut DeliveryStats)) {
        update(&mut self.totals);
        update(&mut self.window);
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
        pacer.note_connected(0, now);
        pacer
    }

    #[test]
    fn one_transfer_per_cluster_is_ever_unconfirmed() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, T1);
        assert_eq!(pacer.decide(0, now, false), Some(SendKind::Frame));
        pacer.note_sent(0, T1, SendKind::Frame, now);
        pacer.submit(0, T2);
        assert_eq!(pacer.decide(0, now, false), None, "the window is closed");
        pacer.observe(0, T1, [0, 0, 0, 1], now + Duration::from_millis(20));
        assert_eq!(
            pacer.decide(0, now + Duration::from_millis(20), false),
            Some(SendKind::Frame),
            "the echo reopens it for the newest frame"
        );
    }

    #[test]
    fn a_frame_replaced_while_held_is_coalesced_not_queued() {
        let now = Instant::now();
        let mut pacer = connected(now);
        pacer.submit(0, T1);
        pacer.note_sent(0, T1, SendKind::Frame, now);
        pacer.submit(0, T2);
        pacer.submit(0, [0, 0, 0, 3]);
        assert_eq!(pacer.totals().frames_coalesced, 1);
    }

    #[test]
    fn the_bounded_wait_grows_with_the_echo_time() {
        let mut link = ClusterLink::default();
        assert_eq!(link.resend_after(), ECHO_RESEND_MIN);
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
}
