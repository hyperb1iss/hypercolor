//! Bounded discovery shared by the network drivers.
//!
//! Every reply a scan receives is attacker-controlled, so a scan bounds what
//! it retains and how much work it starts, and reports what those bounds
//! cost as coverage gaps: exhausting a bound leaves the scan partial and
//! never truncates it silently. Drivers keep parsing, identity validation,
//! classification, and protocol retries.

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::future::Future;
use std::hash::Hash;
use std::net::IpAddr;
use std::num::NonZeroUsize;

use hypercolor_driver_api::{CoverageGap, DiscoveredDevice, DiscoveryBatch, DiscoveryObservation};
use hypercolor_types::device::DeviceFingerprint;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};

/// Probes in flight for a driver that has no protocol reason to pick its own
/// number. Each one holds a socket and little else.
pub const DEFAULT_PROBE_CONCURRENCY: NonZeroUsize = NonZeroUsize::new(32).expect("32 is not zero");

/// What one step of a scan found, and what it may have missed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOutcome<T> {
    /// What the step described.
    pub found: Vec<T>,
    /// Tracked devices that answered but could not be described.
    pub present: Vec<DeviceFingerprint>,
    /// What the step may have missed.
    pub gaps: Vec<CoverageGap>,
}

impl<T> Default for ScanOutcome<T> {
    fn default() -> Self {
        Self {
            found: Vec::new(),
            present: Vec::new(),
            gaps: Vec::new(),
        }
    }
}

impl<T> ScanOutcome<T> {
    /// The batch this scan reports, with `describe` turning each find into a
    /// device record.
    #[must_use]
    pub fn into_batch(self, describe: impl FnMut(T) -> DiscoveredDevice) -> DiscoveryBatch {
        let observations = self
            .found
            .into_iter()
            .map(describe)
            .map(DiscoveryObservation::Device)
            .chain(self.present.into_iter().map(DiscoveryObservation::Present))
            .collect();
        DiscoveryBatch::new(observations, self.gaps)
    }
}

/// What [`CandidateAdmission`] did with an offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// A new candidate, now retained.
    Added,
    /// A retained candidate took the offered value.
    Updated,
    /// The key was already retained and nothing changed.
    Duplicate,
    /// Nothing was retained: the limit is reached.
    Full,
}

/// A target a scan already knows about, which always gets a slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownTarget<K> {
    /// An identity the host tracks.
    Identity(K),
    /// A configured address whose identity is not known yet. It holds one
    /// slot, so spoofing that address cannot grow state past the known set.
    Address(IpAddr),
}

#[derive(Debug)]
struct Admitted<V> {
    known: bool,
    arrival: usize,
    value: V,
}

/// Per-scan admission of candidates.
///
/// Every check runs before any state changes, so a flood of invented
/// identities grows nothing past the limit. Configured and tracked targets
/// are stated up front: each gets a slot that never counts against the limit,
/// so unsolicited candidates cannot crowd one out, and the retained set stays
/// bounded by the limit plus the known targets. Keep any auxiliary index a
/// driver needs behind [`Admission::Added`].
#[derive(Debug)]
pub struct CandidateAdmission<K, V> {
    limit: usize,
    identities: HashSet<K>,
    open_addresses: HashSet<IpAddr>,
    entries: HashMap<K, Admitted<V>>,
    unsolicited: usize,
    arrivals: usize,
    refused: usize,
}

impl<K, V> CandidateAdmission<K, V>
where
    K: Eq + Hash,
{
    /// Admission for `known` targets plus at most `limit` unsolicited
    /// candidates.
    #[must_use]
    pub fn new(limit: usize, known: impl IntoIterator<Item = KnownTarget<K>>) -> Self {
        let mut identities = HashSet::new();
        let mut open_addresses = HashSet::new();
        for target in known {
            match target {
                KnownTarget::Identity(key) => {
                    identities.insert(key);
                }
                KnownTarget::Address(ip) => {
                    open_addresses.insert(ip);
                }
            }
        }
        Self {
            limit,
            identities,
            open_addresses,
            entries: HashMap::new(),
            unsolicited: 0,
            arrivals: 0,
            refused: 0,
        }
    }

    /// Offer a candidate heard from `source`. The first value for a key
    /// stays.
    pub fn offer(&mut self, source: IpAddr, key: K, value: V) -> Admission {
        if self.entries.contains_key(&key) {
            return Admission::Duplicate;
        }
        self.admit(source, key, value)
    }

    /// Offer a candidate heard from `source`. The latest value for a key
    /// replaces the last.
    pub fn offer_latest(&mut self, source: IpAddr, key: K, value: V) -> Admission {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.value = value;
            return Admission::Updated;
        }
        self.admit(source, key, value)
    }

    fn admit(&mut self, source: IpAddr, key: K, value: V) -> Admission {
        let known = self.identities.contains(&key) || self.open_addresses.remove(&source);
        if !known && self.unsolicited >= self.limit {
            self.refused = self.refused.saturating_add(1);
            return Admission::Full;
        }
        if !known {
            self.unsolicited += 1;
        }
        self.arrivals += 1;
        self.entries.insert(
            key,
            Admitted {
                known,
                arrival: self.arrivals,
                value,
            },
        );
        Admission::Added
    }

    /// Whether `key` holds a candidate.
    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// The retained candidates, in no particular order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values().map(|entry| &entry.value)
    }

    /// Candidates retained so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been admitted yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The admitted candidates, known targets first and then in arrival
    /// order, with the gap any refusal left.
    #[must_use]
    pub fn finish(self) -> ScanOutcome<V> {
        let mut admitted: Vec<Admitted<V>> = self.entries.into_values().collect();
        admitted.sort_unstable_by_key(|entry| (!entry.known, entry.arrival));
        ScanOutcome {
            found: admitted.into_iter().map(|entry| entry.value).collect(),
            present: Vec::new(),
            gaps: (self.refused > 0)
                .then_some(CoverageGap::AdmissionFull {
                    refused: self.refused,
                })
                .into_iter()
                .collect(),
        }
    }
}

/// How much probing a scan may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeBudget {
    /// When unfinished probes are cancelled.
    pub deadline: Instant,
    /// Probes running at once.
    pub in_flight: NonZeroUsize,
}

/// What probing one candidate established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probed<T> {
    /// The candidate was described.
    Described(T),
    /// A tracked device answered but could not be described.
    Present(DeviceFingerprint),
    /// A tracked device could not be verified, which holds that device alone.
    Unverified {
        /// The device that stays held.
        fingerprint: DeviceFingerprint,
        /// Failure detail.
        reason: String,
    },
    /// Nothing to report: not a device this driver drives, or a known
    /// target that never answered.
    Nothing,
}

impl<T> Probed<T> {
    /// A known target that did not answer. It holds the device tracked
    /// there, and with none tracked there is nothing for it to hold.
    #[must_use]
    pub fn silent(tracked: Option<DeviceFingerprint>, reason: impl Display) -> Self {
        tracked.map_or(Self::Nothing, |fingerprint| Self::Unverified {
            fingerprint,
            reason: reason.to_string(),
        })
    }

    /// A candidate that answered discovery and then failed its probe. It
    /// holds the tracked device it names. One that names none stays an
    /// error, because it could be any tracked device.
    ///
    /// # Errors
    ///
    /// Returns `error` when `tracked` is `None`.
    pub fn failed<E: Display>(tracked: Option<DeviceFingerprint>, error: E) -> Result<Self, E> {
        match tracked {
            Some(fingerprint) => Ok(Self::Unverified {
                fingerprint,
                reason: format!("{error:#}"),
            }),
            None => Err(error),
        }
    }
}

/// Probe `candidates` in order, at most `budget.in_flight` at a time, until
/// `budget.deadline`.
///
/// A probe that returns `Err` may have found a device the scan cannot name,
/// so it leaves a gap across the target. Put tracked and configured targets first:
/// the deadline falls on whatever is left, and nothing starts once it has
/// passed. Every probe still running at the deadline has been cancelled and
/// joined by the time this returns.
pub async fn probe_candidates<C, T, E, F, Fut>(
    candidates: impl IntoIterator<Item = C>,
    budget: ProbeBudget,
    mut probe: F,
) -> ScanOutcome<T>
where
    C: Display,
    T: Send + 'static,
    E: Display + Send + 'static,
    F: FnMut(C) -> Fut,
    Fut: Future<Output = Result<Probed<T>, E>> + Send + 'static,
{
    let mut pending = candidates.into_iter();
    let mut tasks = JoinSet::new();
    let mut outcome = ScanOutcome::default();
    loop {
        if Instant::now() >= budget.deadline {
            let unfinished = tasks.len() + pending.count();
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            if unfinished > 0 {
                outcome
                    .gaps
                    .push(CoverageGap::ProbesCancelled { unfinished });
            }
            return outcome;
        }
        while tasks.len() < budget.in_flight.get() {
            let Some(candidate) = pending.next() else {
                break;
            };
            let label = candidate.to_string();
            let probing = probe(candidate);
            tasks.spawn(async move { (label, probing.await) });
        }
        let Ok(joined) = timeout_at(budget.deadline, tasks.join_next()).await else {
            continue;
        };
        match joined {
            None => return outcome,
            Some(Ok((_, Ok(Probed::Described(found))))) => outcome.found.push(found),
            Some(Ok((_, Ok(Probed::Present(fingerprint))))) => outcome.present.push(fingerprint),
            Some(Ok((
                _,
                Ok(Probed::Unverified {
                    fingerprint,
                    reason,
                }),
            ))) => {
                outcome.gaps.push(CoverageGap::DeviceUnverified {
                    fingerprint,
                    reason,
                });
            }
            Some(Ok((_, Ok(Probed::Nothing)))) => {}
            Some(Ok((candidate, Err(error)))) => outcome.gaps.push(CoverageGap::ProbeFailed {
                candidate,
                reason: format!("{error:#}"),
            }),
            Some(Err(error)) => outcome.gaps.push(CoverageGap::ProbeFailed {
                candidate: "probe task".to_owned(),
                reason: error.to_string(),
            }),
        }
    }
}
