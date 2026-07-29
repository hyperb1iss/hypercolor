use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use hypercolor_core::input::screen::{
    InputPublicationDemandRevision, RegisteredScreenBranchDemand, ScreenAspectPolicy,
    ScreenExtentRequest, ScreenInputGraphGeneration, ScreenPlanBuilder, ScreenProcessingProfile,
    ScreenPublicationDemandError, ScreenPublicationDemandSnapshot,
    ScreenPublicationExecutorRequest, ScreenPublicationHub, ScreenPublicationKind,
    ScreenPublicationRequest, ScreenSourceSelector, ScreenUpscalePolicy,
};
use hypercolor_core::input::{InputData, InputManager, InputSource};

fn branch(kind: ScreenPublicationKind) -> RegisteredScreenBranchDemand {
    RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            kind,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::bounded(
                NonZeroU32::new(7_680),
                NonZeroU32::new(4_320),
                ScreenUpscalePolicy::Never,
            ),
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        NonZeroU32::new(144).expect("test cadence is non-zero"),
    )
}

#[test]
fn demand_snapshot_preserves_independent_arbitrary_resolution_branches() {
    let surface = branch(ScreenPublicationKind::Surface);
    let zones = branch(ScreenPublicationKind::Zones {
        columns: NonZeroU32::new(127).expect("test grid is non-zero"),
        rows: NonZeroU32::new(71).expect("test grid is non-zero"),
    });
    let branches: Arc<[_]> = [surface.clone(), zones.clone()].into();
    let snapshot = ScreenPublicationDemandSnapshot::try_new(
        InputPublicationDemandRevision::new(19),
        ScreenInputGraphGeneration::new(23),
        branches,
        Some(surface.clone()),
        Some(zones.clone()),
    )
    .expect("exact branches form a valid snapshot");

    assert_eq!(snapshot.revision().get(), 19);
    assert_eq!(snapshot.graph_generation().get(), 23);
    assert_eq!(snapshot.branches().as_ref(), &[surface, zones]);
    assert!(matches!(
        snapshot
            .compatibility_surface()
            .expect("surface compatibility is retained")
            .request()
            .kind(),
        ScreenPublicationKind::Surface
    ));
    assert!(matches!(
        snapshot
            .compatibility_zones()
            .expect("zones compatibility is retained")
            .request()
            .kind(),
        ScreenPublicationKind::Zones { .. }
    ));
    assert!(!snapshot.is_empty());
}

#[test]
fn demand_snapshot_rejects_unregistered_or_mistyped_compatibility() {
    let surface = branch(ScreenPublicationKind::Surface);
    let zones = branch(ScreenPublicationKind::Zones {
        columns: NonZeroU32::MIN,
        rows: NonZeroU32::MIN,
    });

    assert_eq!(
        ScreenPublicationDemandSnapshot::try_new(
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            Arc::from([surface.clone()]),
            Some(zones.clone()),
            None,
        ),
        Err(ScreenPublicationDemandError::CompatibilityKindMismatch)
    );
    assert_eq!(
        ScreenPublicationDemandSnapshot::try_new(
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            Arc::from([surface]),
            None,
            Some(zones),
        ),
        Err(ScreenPublicationDemandError::CompatibilityBranchMissing)
    );
}

struct ExactDemandProbe {
    hub: Arc<Mutex<Option<Arc<ScreenPublicationHub>>>>,
    demands: Arc<Mutex<Vec<ScreenPublicationDemandSnapshot>>>,
}

impl InputSource for ExactDemandProbe {
    fn name(&self) -> &'static str {
        "exact_demand_probe"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn stop(&mut self) {}

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        false
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn set_screen_publication_hub(&mut self, hub: Arc<ScreenPublicationHub>) {
        *self.hub.lock().expect("probe hub mutex is healthy") = Some(hub);
    }

    fn set_screen_publication_demand(
        &mut self,
        demand: ScreenPublicationDemandSnapshot,
    ) -> anyhow::Result<()> {
        self.demands
            .lock()
            .expect("probe demand mutex is healthy")
            .push(demand);
        Ok(())
    }
}

#[test]
fn manager_binds_one_stable_hub_and_deduplicates_exact_demand() {
    let attached_hub = Arc::new(Mutex::new(None));
    let observed_demands = Arc::new(Mutex::new(Vec::new()));
    let mut manager = InputManager::new();
    let stable_hub = manager.screen_publication_hub();
    manager.add_source(Box::new(ExactDemandProbe {
        hub: Arc::clone(&attached_hub),
        demands: Arc::clone(&observed_demands),
    }));
    let source_hub = attached_hub
        .lock()
        .expect("probe hub mutex is healthy")
        .clone()
        .expect("screen source receives a hub");
    assert!(Arc::ptr_eq(&stable_hub, &source_hub));
    assert!(Arc::ptr_eq(&stable_hub, &manager.screen_publication_hub()));

    let demand = ScreenPublicationDemandSnapshot::try_new(
        InputPublicationDemandRevision::new(5),
        ScreenInputGraphGeneration::new(1),
        Arc::from([branch(ScreenPublicationKind::Surface)]),
        None,
        None,
    )
    .expect("test demand is valid");
    manager
        .set_screen_publication_demand(demand.clone())
        .expect("first exact demand applies");
    manager
        .set_screen_publication_demand(demand)
        .expect("equal exact demand is a no-op");

    assert_eq!(
        observed_demands
            .lock()
            .expect("probe demand mutex is healthy")
            .len(),
        1
    );
    let coordinator = ScreenPlanBuilder::for_publication_hub(stable_hub);
    assert_eq!(coordinator.current().generation().get(), 0);
}
