use hypercolor_core::engine::FpsTier;

const EWMA_ALPHA: f64 = 0.2;
const FULL_TIER_TOTAL_RATIO: f64 = 0.8;
const FULL_TIER_PRODUCER_RATIO: f64 = 0.6;
const FULL_TIER_COMPOSITION_RATIO: f64 = 0.2;
const FULL_TIER_COPY_PRESSURE_THRESHOLD: u32 = 2;

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameAdmissionSample {
    pub(crate) total_us: u32,
    pub(crate) producer_us: u32,
    pub(crate) composition_us: u32,
    pub(crate) full_frame_copy_count: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameAdmissionDecision {
    pub(crate) ceiling_tier: FpsTier,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameAdmissionController {
    configured_max_tier: FpsTier,
    current_ceiling_tier: FpsTier,
    total_ewma_us: Option<f64>,
    producer_ewma_us: Option<f64>,
    composition_ewma_us: Option<f64>,
    consecutive_copy_frames: u32,
}

impl FrameAdmissionController {
    pub(crate) fn new(configured_max_tier: FpsTier) -> Self {
        Self {
            configured_max_tier,
            current_ceiling_tier: configured_max_tier,
            total_ewma_us: None,
            producer_ewma_us: None,
            composition_ewma_us: None,
            consecutive_copy_frames: 0,
        }
    }

    pub(crate) fn record_frame(&mut self, sample: FrameAdmissionSample) -> FrameAdmissionDecision {
        self.total_ewma_us = Some(update_ewma(self.total_ewma_us, sample.total_us));
        self.producer_ewma_us = Some(update_ewma(self.producer_ewma_us, sample.producer_us));
        self.composition_ewma_us =
            Some(update_ewma(self.composition_ewma_us, sample.composition_us));

        if sample.full_frame_copy_count > 0 {
            self.consecutive_copy_frames = self.consecutive_copy_frames.saturating_add(1);
        } else {
            self.consecutive_copy_frames = 0;
        }

        self.current_ceiling_tier = self.resolve_ceiling_tier();
        FrameAdmissionDecision {
            ceiling_tier: self.current_ceiling_tier,
        }
    }

    fn resolve_ceiling_tier(&self) -> FpsTier {
        if self.configured_max_tier < FpsTier::Full {
            return self.configured_max_tier;
        }

        let full_budget_us = FpsTier::Full.frame_interval().as_secs_f64() * 1_000_000.0;
        let total_ewma_us = self.total_ewma_us.unwrap_or(full_budget_us);
        let producer_ewma_us = self.producer_ewma_us.unwrap_or(0.0);
        let composition_ewma_us = self.composition_ewma_us.unwrap_or(0.0);
        let copy_pressure = self.consecutive_copy_frames >= FULL_TIER_COPY_PRESSURE_THRESHOLD;

        if copy_pressure
            || total_ewma_us > full_budget_us * FULL_TIER_TOTAL_RATIO
            || producer_ewma_us > full_budget_us * FULL_TIER_PRODUCER_RATIO
            || composition_ewma_us > full_budget_us * FULL_TIER_COMPOSITION_RATIO
        {
            FpsTier::High
        } else {
            FpsTier::Full
        }
    }
}

fn update_ewma(previous: Option<f64>, sample_us: u32) -> f64 {
    let sample = f64::from(sample_us);
    previous.map_or(sample, |current| {
        ((1.0 - EWMA_ALPHA) * current) + (EWMA_ALPHA * sample)
    })
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
            full_frame_copy_count: 0,
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
    fn heavy_ewma_blocks_full_tier() {
        let mut admission = FrameAdmissionController::new(FpsTier::Full);
        for _ in 0..4 {
            let decision = admission.record_frame(sample(15_000, 10_500, 1_000));
            if decision.ceiling_tier == FpsTier::High {
                return;
            }
        }
        panic!("heavy producer workload should eventually block full tier admission");
    }
}
