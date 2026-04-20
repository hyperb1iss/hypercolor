use hypercolor_core::engine::FpsTier;

const EWMA_ALPHA: f64 = 0.2;
const ADMISSION_HISTORY_CAPACITY: usize = 60;
const FULL_TIER_ELIGIBLE_TOTAL_RATIO: f64 = 0.8;
const FULL_TIER_ELIGIBLE_PRODUCER_RATIO: f64 = 0.6;
const FULL_TIER_ELIGIBLE_COMPOSITION_RATIO: f64 = 0.2;
const FULL_TIER_ELIGIBLE_PUSH_RATIO: f64 = 0.16;
const FULL_TIER_ELIGIBLE_PUBLISH_RATIO: f64 = 0.1;
const FULL_TIER_ELIGIBLE_WAKE_RATIO: f64 = 0.08;
const FULL_TIER_ELIGIBLE_JITTER_RATIO: f64 = 0.1;
const FULL_TIER_ELIGIBLE_P95_RATIO: f64 = 0.85;
const FULL_TIER_REVOKE_TOTAL_RATIO: f64 = 0.92;
const FULL_TIER_REVOKE_PRODUCER_RATIO: f64 = 0.7;
const FULL_TIER_REVOKE_COMPOSITION_RATIO: f64 = 0.25;
const FULL_TIER_REVOKE_PUSH_RATIO: f64 = 0.24;
const FULL_TIER_REVOKE_PUBLISH_RATIO: f64 = 0.16;
const FULL_TIER_REVOKE_WAKE_RATIO: f64 = 0.16;
const FULL_TIER_REVOKE_JITTER_RATIO: f64 = 0.2;
const FULL_TIER_REVOKE_P95_RATIO: f64 = 0.95;
const FULL_TIER_COPY_PRESSURE_THRESHOLD: u32 = 2;
const FULL_TIER_OUTPUT_ERROR_THRESHOLD: u32 = 2;
const FULL_TIER_REVOKE_MISS_THRESHOLD: u32 = 2;
const FULL_TIER_REVOKE_PERCENTILE_MIN_SAMPLES: usize = 10;
const FULL_TIER_READMIT_MIN_SAMPLES: usize = 30;
const FULL_TIER_READMIT_STREAK: u32 = 30;

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameAdmissionSample {
    pub(crate) total_us: u32,
    pub(crate) producer_us: u32,
    pub(crate) composition_us: u32,
    pub(crate) push_us: u32,
    pub(crate) publish_us: u32,
    pub(crate) wake_late_us: u32,
    pub(crate) jitter_us: u32,
    pub(crate) full_frame_copy_count: u32,
    pub(crate) output_errors: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameAdmissionDecision {
    pub(crate) ceiling_tier: FpsTier,
}

#[derive(Debug, Clone)]
pub(crate) struct FrameAdmissionController {
    configured_max_tier: FpsTier,
    current_ceiling_tier: FpsTier,
    total_ewma_us: Option<f64>,
    producer_ewma_us: Option<f64>,
    composition_ewma_us: Option<f64>,
    push_ewma_us: Option<f64>,
    publish_ewma_us: Option<f64>,
    wake_ewma_us: Option<f64>,
    jitter_ewma_us: Option<f64>,
    recent_total_us: [u32; ADMISSION_HISTORY_CAPACITY],
    recent_total_len: usize,
    recent_total_next: usize,
    consecutive_copy_frames: u32,
    consecutive_output_error_frames: u32,
    consecutive_over_budget_frames: u32,
    consecutive_full_eligible_frames: u32,
}

impl FrameAdmissionController {
    pub(crate) fn new(configured_max_tier: FpsTier) -> Self {
        Self {
            configured_max_tier,
            current_ceiling_tier: configured_max_tier,
            total_ewma_us: None,
            producer_ewma_us: None,
            composition_ewma_us: None,
            push_ewma_us: None,
            publish_ewma_us: None,
            wake_ewma_us: None,
            jitter_ewma_us: None,
            recent_total_us: [0; ADMISSION_HISTORY_CAPACITY],
            recent_total_len: 0,
            recent_total_next: 0,
            consecutive_copy_frames: 0,
            consecutive_output_error_frames: 0,
            consecutive_over_budget_frames: 0,
            consecutive_full_eligible_frames: 0,
        }
    }

    pub(crate) fn record_frame(&mut self, sample: FrameAdmissionSample) -> FrameAdmissionDecision {
        self.recent_total_us[self.recent_total_next] = sample.total_us;
        self.recent_total_next =
            (self.recent_total_next + 1).wrapping_rem(ADMISSION_HISTORY_CAPACITY);
        if self.recent_total_len < ADMISSION_HISTORY_CAPACITY {
            self.recent_total_len += 1;
        }

        self.total_ewma_us = Some(update_ewma(self.total_ewma_us, sample.total_us));
        self.producer_ewma_us = Some(update_ewma(self.producer_ewma_us, sample.producer_us));
        self.composition_ewma_us =
            Some(update_ewma(self.composition_ewma_us, sample.composition_us));
        self.push_ewma_us = Some(update_ewma(self.push_ewma_us, sample.push_us));
        self.publish_ewma_us = Some(update_ewma(self.publish_ewma_us, sample.publish_us));
        self.wake_ewma_us = Some(update_ewma(self.wake_ewma_us, sample.wake_late_us));
        self.jitter_ewma_us = Some(update_ewma(self.jitter_ewma_us, sample.jitter_us));

        if sample.full_frame_copy_count > 0 {
            self.consecutive_copy_frames = self.consecutive_copy_frames.saturating_add(1);
        } else {
            self.consecutive_copy_frames = 0;
        }

        if sample.output_errors > 0 {
            self.consecutive_output_error_frames =
                self.consecutive_output_error_frames.saturating_add(1);
        } else {
            self.consecutive_output_error_frames = 0;
        }

        if sample.total_us > full_tier_budget_us_u32() {
            self.consecutive_over_budget_frames =
                self.consecutive_over_budget_frames.saturating_add(1);
        } else {
            self.consecutive_over_budget_frames = 0;
        }

        self.current_ceiling_tier = self.resolve_ceiling_tier();
        FrameAdmissionDecision {
            ceiling_tier: self.current_ceiling_tier,
        }
    }

    fn resolve_ceiling_tier(&mut self) -> FpsTier {
        if self.configured_max_tier < FpsTier::Full {
            return self.configured_max_tier;
        }

        if self.current_ceiling_tier == FpsTier::Full {
            if self.should_revoke_full_tier() {
                self.consecutive_full_eligible_frames = 0;
                return FpsTier::High;
            }

            return FpsTier::Full;
        }

        if self.is_full_tier_eligible() {
            self.consecutive_full_eligible_frames =
                self.consecutive_full_eligible_frames.saturating_add(1);
            if self.consecutive_full_eligible_frames >= FULL_TIER_READMIT_STREAK {
                self.consecutive_full_eligible_frames = 0;
                return FpsTier::Full;
            }
        } else {
            self.consecutive_full_eligible_frames = 0;
        }

        FpsTier::High
    }

    fn should_revoke_full_tier(&self) -> bool {
        let full_budget_us = full_tier_budget_us();
        let total_ewma_us = self.total_ewma_us.unwrap_or(full_budget_us);
        let producer_ewma_us = self.producer_ewma_us.unwrap_or(0.0);
        let composition_ewma_us = self.composition_ewma_us.unwrap_or(0.0);
        let push_ewma_us = self.push_ewma_us.unwrap_or(0.0);
        let publish_ewma_us = self.publish_ewma_us.unwrap_or(0.0);
        let wake_ewma_us = self.wake_ewma_us.unwrap_or(0.0);
        let jitter_ewma_us = self.jitter_ewma_us.unwrap_or(0.0);
        let copy_pressure = self.consecutive_copy_frames >= FULL_TIER_COPY_PRESSURE_THRESHOLD;
        let percentile_window_ready =
            self.recent_total_len >= FULL_TIER_REVOKE_PERCENTILE_MIN_SAMPLES;
        let (p95_total_us, p99_total_us) = if percentile_window_ready {
            percentile_pair_us(&self.recent_total_us[..self.recent_total_len]).unwrap_or((0.0, 0.0))
        } else {
            (0.0, 0.0)
        };

        copy_pressure
            || self.consecutive_output_error_frames >= FULL_TIER_OUTPUT_ERROR_THRESHOLD
            || self.consecutive_over_budget_frames >= FULL_TIER_REVOKE_MISS_THRESHOLD
            || (percentile_window_ready
                && total_ewma_us > full_budget_us * FULL_TIER_REVOKE_TOTAL_RATIO)
            || (percentile_window_ready
                && producer_ewma_us > full_budget_us * FULL_TIER_REVOKE_PRODUCER_RATIO)
            || (percentile_window_ready
                && composition_ewma_us > full_budget_us * FULL_TIER_REVOKE_COMPOSITION_RATIO)
            || (percentile_window_ready
                && push_ewma_us > full_budget_us * FULL_TIER_REVOKE_PUSH_RATIO)
            || (percentile_window_ready
                && publish_ewma_us > full_budget_us * FULL_TIER_REVOKE_PUBLISH_RATIO)
            || (percentile_window_ready
                && wake_ewma_us > full_budget_us * FULL_TIER_REVOKE_WAKE_RATIO)
            || (percentile_window_ready
                && jitter_ewma_us > full_budget_us * FULL_TIER_REVOKE_JITTER_RATIO)
            || (percentile_window_ready
                && p95_total_us > full_budget_us * FULL_TIER_REVOKE_P95_RATIO)
            || (percentile_window_ready && p99_total_us > full_budget_us)
    }

    fn is_full_tier_eligible(&self) -> bool {
        if self.recent_total_len < FULL_TIER_READMIT_MIN_SAMPLES {
            return false;
        }

        let full_budget_us = full_tier_budget_us();
        let total_ewma_us = self.total_ewma_us.unwrap_or(full_budget_us);
        let producer_ewma_us = self.producer_ewma_us.unwrap_or(0.0);
        let composition_ewma_us = self.composition_ewma_us.unwrap_or(0.0);
        let push_ewma_us = self.push_ewma_us.unwrap_or(0.0);
        let publish_ewma_us = self.publish_ewma_us.unwrap_or(0.0);
        let wake_ewma_us = self.wake_ewma_us.unwrap_or(0.0);
        let jitter_ewma_us = self.jitter_ewma_us.unwrap_or(0.0);
        let (p95_total_us, p99_total_us) =
            percentile_pair_us(&self.recent_total_us[..self.recent_total_len])
                .unwrap_or((full_budget_us, full_budget_us));

        self.consecutive_copy_frames == 0
            && self.consecutive_output_error_frames == 0
            && self.consecutive_over_budget_frames == 0
            && total_ewma_us <= full_budget_us * FULL_TIER_ELIGIBLE_TOTAL_RATIO
            && producer_ewma_us <= full_budget_us * FULL_TIER_ELIGIBLE_PRODUCER_RATIO
            && composition_ewma_us <= full_budget_us * FULL_TIER_ELIGIBLE_COMPOSITION_RATIO
            && push_ewma_us <= full_budget_us * FULL_TIER_ELIGIBLE_PUSH_RATIO
            && publish_ewma_us <= full_budget_us * FULL_TIER_ELIGIBLE_PUBLISH_RATIO
            && wake_ewma_us <= full_budget_us * FULL_TIER_ELIGIBLE_WAKE_RATIO
            && jitter_ewma_us <= full_budget_us * FULL_TIER_ELIGIBLE_JITTER_RATIO
            && p95_total_us <= full_budget_us * FULL_TIER_ELIGIBLE_P95_RATIO
            && p99_total_us <= full_budget_us
    }
}

fn update_ewma(previous: Option<f64>, sample_us: u32) -> f64 {
    let sample = f64::from(sample_us);
    previous.map_or(sample, |current| {
        ((1.0 - EWMA_ALPHA) * current) + (EWMA_ALPHA * sample)
    })
}

fn full_tier_budget_us() -> f64 {
    FpsTier::Full.frame_interval().as_secs_f64() * 1_000_000.0
}

fn full_tier_budget_us_u32() -> u32 {
    u32::try_from(FpsTier::Full.frame_interval().as_micros()).unwrap_or(u32::MAX)
}

fn percentile_pair_us(samples: &[u32]) -> Option<(f64, f64)> {
    if samples.is_empty() {
        return None;
    }

    let len = samples.len();
    let mut sorted = [0_u32; ADMISSION_HISTORY_CAPACITY];
    for (index, sample) in samples.iter().enumerate() {
        sorted[index] = *sample;
    }
    let sorted = &mut sorted[..len];
    sorted.sort_unstable();
    Some((
        percentile_from_sorted(sorted, 95, 100),
        percentile_from_sorted(sorted, 99, 100),
    ))
}

fn percentile_from_sorted(sorted: &[u32], numerator: usize, denominator: usize) -> f64 {
    let max_index = sorted.len().saturating_sub(1);
    let rank = max_index.saturating_mul(numerator) / denominator;
    sorted.get(rank).copied().map(f64::from).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{FrameAdmissionController, FrameAdmissionSample};
    use hypercolor_core::engine::FpsTier;

    fn sample(total_us: u32, producer_us: u32, composition_us: u32) -> FrameAdmissionSample {
        FrameAdmissionSample {
            total_us,
            producer_us,
            composition_us,
            push_us: 0,
            publish_us: 0,
            wake_late_us: 0,
            jitter_us: 0,
            full_frame_copy_count: 0,
            output_errors: 0,
        }
    }

    #[test]
    fn configured_non_full_ceiling_is_preserved() {
        let mut admission = FrameAdmissionController::new(FpsTier::Medium);
        let decision = admission.record_frame(sample(5_000, 1_000, 300));
        assert_eq!(decision.ceiling_tier, FpsTier::Medium);
    }

    #[test]
    fn lightweight_frames_keep_full_tier_admitted() {
        let mut admission = FrameAdmissionController::new(FpsTier::Full);
        for _ in 0..3 {
            let decision = admission.record_frame(sample(8_000, 4_500, 1_000));
            assert_eq!(decision.ceiling_tier, FpsTier::Full);
        }
    }

    #[test]
    fn sustained_copy_pressure_blocks_full_tier() {
        let mut admission = FrameAdmissionController::new(FpsTier::Full);
        let clean = admission.record_frame(sample(8_000, 4_500, 1_000));
        assert_eq!(clean.ceiling_tier, FpsTier::Full);

        let first_copy = admission.record_frame(FrameAdmissionSample {
            full_frame_copy_count: 1,
            ..sample(8_000, 4_500, 1_000)
        });
        assert_eq!(first_copy.ceiling_tier, FpsTier::Full);

        let second_copy = admission.record_frame(FrameAdmissionSample {
            full_frame_copy_count: 1,
            ..sample(8_000, 4_500, 1_000)
        });
        assert_eq!(second_copy.ceiling_tier, FpsTier::High);
    }

    #[test]
    fn heavy_frames_revoke_full_tier_quickly() {
        let mut admission = FrameAdmissionController::new(FpsTier::Full);
        for _ in 0..2 {
            let decision = admission.record_frame(sample(18_000, 12_000, 1_000));
            if decision.ceiling_tier == FpsTier::High {
                return;
            }
        }
        panic!("heavy producer workload should eventually block full tier admission");
    }

    #[test]
    fn full_tier_readmission_requires_clean_window() {
        let mut admission = FrameAdmissionController::new(FpsTier::Full);

        for _ in 0..2 {
            let _ = admission.record_frame(sample(18_000, 11_000, 1_200));
        }
        assert_eq!(admission.current_ceiling_tier, FpsTier::High);

        let clean_frames_before_readmit = super::ADMISSION_HISTORY_CAPACITY
            + usize::try_from(super::FULL_TIER_READMIT_STREAK).unwrap_or(usize::MAX)
            - 3;

        for _ in 0..clean_frames_before_readmit {
            let decision = admission.record_frame(sample(7_000, 4_000, 800));
            assert_eq!(decision.ceiling_tier, FpsTier::High);
        }

        let admitted = admission.record_frame(sample(7_000, 4_000, 800));
        assert_eq!(admitted.ceiling_tier, FpsTier::Full);
    }

    #[test]
    fn single_spike_does_not_revoke_full_tier() {
        let mut admission = FrameAdmissionController::new(FpsTier::Full);

        let first = admission.record_frame(sample(18_000, 11_000, 1_000));
        assert_eq!(first.ceiling_tier, FpsTier::Full);

        let recovered = admission.record_frame(sample(7_500, 4_000, 900));
        assert_eq!(recovered.ceiling_tier, FpsTier::Full);
    }

    #[test]
    fn output_errors_revoke_full_tier_immediately() {
        let mut admission = FrameAdmissionController::new(FpsTier::Full);

        let first = admission.record_frame(FrameAdmissionSample {
            output_errors: 1,
            ..sample(8_000, 4_500, 1_000)
        });
        assert_eq!(first.ceiling_tier, FpsTier::Full);

        let second = admission.record_frame(FrameAdmissionSample {
            output_errors: 1,
            ..sample(8_000, 4_500, 1_000)
        });
        assert_eq!(second.ceiling_tier, FpsTier::High);
    }

    #[test]
    fn sustained_publish_pressure_revokes_full_tier() {
        let mut admission = FrameAdmissionController::new(FpsTier::Full);

        for _ in 0..super::FULL_TIER_REVOKE_PERCENTILE_MIN_SAMPLES {
            let decision = admission.record_frame(FrameAdmissionSample {
                publish_us: 3_200,
                ..sample(9_000, 4_500, 1_000)
            });
            if decision.ceiling_tier == FpsTier::High {
                return;
            }
        }

        panic!("sustained publish pressure should revoke full tier");
    }
}
