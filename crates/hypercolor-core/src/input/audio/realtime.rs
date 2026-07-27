use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Result of one non-blocking audio callback publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PushStats {
    /// Mono frames accepted by the analysis worker.
    pub accepted: usize,
    /// Mono frames dropped because the bounded ring was full.
    pub dropped: usize,
}

/// Preallocated callback-to-analysis queue.
///
/// A capture backend owns the sole producer and writes converted interleaved
/// samples. Its analysis worker owns the sole consumer and performs channel
/// mixing. This ownership invariant keeps the callback free of locks,
/// allocation, and blocking syscalls.
pub struct AudioFrameRing {
    slots: Box<[AtomicU32]>,
    channels: usize,
    channel_divisor: f32,
    slot_count: usize,
    read: AtomicUsize,
    write: AtomicUsize,
    accepting: AtomicBool,
    accepted: AtomicU64,
    dropped: AtomicU64,
}

impl AudioFrameRing {
    /// Allocate a bounded ring before the native callback starts.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self::with_channels(capacity, 1)
    }

    /// Allocate a bounded ring that mixes the given interleaved channel count.
    #[must_use]
    pub fn with_channels(capacity: usize, channels: usize) -> Self {
        let channels = channels.max(1);
        let channel_count = u16::try_from(channels).expect("audio channel count exceeds u16");
        let slot_count = capacity
            .max(1)
            .checked_add(1)
            .expect("audio ring capacity exceeds address space");
        let sample_slots = slot_count
            .checked_mul(channels)
            .expect("audio ring storage exceeds address space");
        Self {
            slots: (0..sample_slots)
                .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                .collect(),
            channels,
            channel_divisor: f32::from(channel_count),
            slot_count,
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            accepting: AtomicBool::new(true),
            accepted: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Pop the oldest retained mono frame without blocking.
    pub fn pop_frame(&self) -> Option<f32> {
        loop {
            let read = self.read.load(Ordering::Relaxed);
            if read == self.write.load(Ordering::Acquire) {
                return None;
            }
            let frame_start = read * self.channels;
            let sample = sanitize_sample(
                self.slots[frame_start..frame_start + self.channels]
                    .iter()
                    .map(|slot| f32::from_bits(slot.load(Ordering::Relaxed)))
                    .sum::<f32>()
                    / self.channel_divisor,
            );
            if self
                .read
                .compare_exchange_weak(
                    read,
                    increment(read, self.slot_count),
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return Some(sample);
            }
        }
    }

    pub(crate) fn close(&self) {
        self.accepting.store(false, Ordering::Release);
    }

    /// Total accepted mono frames since construction.
    #[must_use]
    pub fn accepted_total(&self) -> u64 {
        self.accepted.load(Ordering::Relaxed)
    }

    /// Total frames shed rather than blocking the native callback.
    #[must_use]
    pub fn dropped_total(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub(crate) fn channels(&self) -> usize {
        self.channels
    }

    pub(crate) fn record(&self, stats: PushStats) {
        self.accepted
            .fetch_add(to_u64(stats.accepted), Ordering::Relaxed);
        self.dropped
            .fetch_add(to_u64(stats.dropped), Ordering::Relaxed);
    }

    pub(crate) fn push_frame_with(&self, mut sample_at: impl FnMut(usize) -> f32) -> PushStats {
        if !self.accepting.load(Ordering::Acquire) {
            return PushStats {
                accepted: 0,
                dropped: 1,
            };
        }
        let write = self.write.load(Ordering::Relaxed);
        let next = increment(write, self.slot_count);
        let mut dropped = 0;
        loop {
            let read = self.read.load(Ordering::Acquire);
            if next != read {
                break;
            }
            if self
                .read
                .compare_exchange_weak(
                    read,
                    increment(read, self.slot_count),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                dropped = 1;
                break;
            }
        }
        let frame_start = write * self.channels;
        for (channel, slot) in self.slots[frame_start..frame_start + self.channels]
            .iter()
            .enumerate()
        {
            slot.store(
                sanitize_sample(sample_at(channel)).to_bits(),
                Ordering::Relaxed,
            );
        }
        self.write.store(next, Ordering::Release);
        PushStats {
            accepted: 1,
            dropped,
        }
    }
}

fn increment(index: usize, capacity: usize) -> usize {
    if index + 1 == capacity { 0 } else { index + 1 }
}

/// Publish converted interleaved frames without allocating, locking, or blocking.
#[must_use]
pub fn push_frames(input: &[f32], ring: &AudioFrameRing) -> PushStats {
    push_interleaved_frames(input, ring, |sample| sample)
}

pub(crate) fn push_interleaved_frames<T>(
    input: &[T],
    ring: &AudioFrameRing,
    convert: impl Fn(T) -> f32,
) -> PushStats
where
    T: Copy,
{
    let channel_count = ring.channels();
    let mut stats = PushStats::default();
    for frame in input.chunks_exact(channel_count) {
        let pushed = ring.push_frame_with(|channel| convert(frame[channel]));
        stats.accepted += pushed.accepted;
        stats.dropped += pushed.dropped;
    }
    ring.record(stats);
    stats
}

pub(crate) fn sanitize_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
