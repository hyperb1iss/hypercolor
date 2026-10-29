//! View-owned demand for the full diagnostics snapshots.

use leptos::prelude::*;

/// A diagnostics stream that is useful only while a view consumes it.
#[derive(Debug, Clone, Copy)]
pub enum TelemetryTopic {
    Metrics,
    Sensors,
}

#[derive(Clone, Copy)]
pub(crate) struct TelemetryDemand {
    pub metrics: RwSignal<u32>,
    pub sensors: RwSignal<u32>,
}

impl TelemetryDemand {
    pub fn install() -> Self {
        let demand = Self {
            metrics: RwSignal::new(0),
            sensors: RwSignal::new(0),
        };
        provide_context(demand);
        demand
    }
}

/// Keep `topic` subscribed until the current reactive owner is cleaned up.
/// Several mounted consumers share one subscription.
pub fn use_telemetry(topic: TelemetryTopic) {
    let demand = expect_context::<TelemetryDemand>();
    let consumers = match topic {
        TelemetryTopic::Metrics => demand.metrics,
        TelemetryTopic::Sensors => demand.sensors,
    };
    consumers.update(|count| *count = count.saturating_add(1));
    on_cleanup(move || {
        consumers.try_update(|count| *count = count.saturating_sub(1));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_release_only_their_own_telemetry_demand() {
        let root = Owner::new();
        root.with(|| {
            let demand = TelemetryDemand::install();
            let first = Owner::new();
            first.with(|| {
                use_telemetry(TelemetryTopic::Metrics);
                use_telemetry(TelemetryTopic::Sensors);
            });
            let second = Owner::new();
            second.with(|| use_telemetry(TelemetryTopic::Metrics));
            assert_eq!(demand.metrics.get_untracked(), 2);
            assert_eq!(demand.sensors.get_untracked(), 1);
            first.cleanup();
            assert_eq!(demand.metrics.get_untracked(), 1);
            assert_eq!(demand.sensors.get_untracked(), 0);
            second.cleanup();
            assert_eq!(demand.metrics.get_untracked(), 0);
        });
    }
}
