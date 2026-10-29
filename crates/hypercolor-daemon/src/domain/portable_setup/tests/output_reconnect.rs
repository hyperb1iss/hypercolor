use super::{Fixture, SetupBindings, options};
use crate::domain::portable_setup::PortableSetupService;
use crate::domain::portable_setup::journal::RestoreJournal;
use crate::persistence::PersistenceShutdown;
use hypercolor_driver_api::{
    DiscoveredDevice, DiscoveryCapability, DiscoveryRequest, DriverConfigView, DriverDescriptor,
    DriverError, DriverHost, DriverModule,
};
use hypercolor_types::device::DriverTransportKind;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use uuid::Uuid;

struct CountedNetworkDriver(Arc<AtomicUsize>);

static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "portable-test-network",
    "Portable test network",
    DriverTransportKind::Network,
    true,
    true,
);

impl DriverModule for CountedNetworkDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }
    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DiscoveryCapability for CountedNetworkDriver {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        _config: DriverConfigView<'_>,
    ) -> Result<Vec<DiscoveredDevice>, DriverError> {
        self.0.fetch_add(1, Ordering::AcqRel);
        Ok(Vec::new())
    }
}

fn fixture() -> (Fixture, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = hypercolor_network::DriverModuleRegistry::new();
    registry
        .register(CountedNetworkDriver(Arc::clone(&calls)))
        .expect("register test driver");
    (Fixture::with_driver_registry("local", registry), calls)
}

async fn scan_finishes(fixture: &Fixture, calls: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while calls.load(Ordering::Acquire) != expected
            || fixture.state.discovery_in_progress.load(Ordering::Acquire)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned discovery scan finishes");
}

#[tokio::test]
async fn enabled_restore_exports_immediately_without_starting_discovery() {
    let (fixture, calls) = fixture();
    let before = fixture.empty(Uuid::now_v7()).await;
    assert!(before.manifest.output.enabled);
    fixture
        .drive(fixture.service.restore(
            &before.manifest,
            &SetupBindings::default(),
            options(before.revision),
        ))
        .await
        .expect("restore running setup");
    fixture
        .service
        .export_archive()
        .await
        .expect("immediate manual export");
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(!fixture.state.discovery_in_progress.load(Ordering::Acquire));
}

#[tokio::test]
async fn stopped_output_reconnect_intent_survives_unfinished_commit_replay() {
    let (fixture, calls) = fixture();
    fixture.state.output_power.restore_output_stopped().await;
    let id = Uuid::now_v7();
    let before = fixture.empty(id).await;
    assert!(!before.manifest.output.enabled);
    let mut desired = before.manifest;
    desired.output.enabled = true;
    fixture
        .drive(fixture.service.restore(
            &desired,
            &SetupBindings::default(),
            options(before.revision),
        ))
        .await
        .expect("resume stopped setup");
    scan_finishes(&fixture, &calls, 1).await;
    let mut journal = fixture
        .service
        .read_journal(id)
        .expect("read receipt")
        .expect("receipt");
    let RestoreJournal::Committed {
        finalized,
        reconnect_network_outputs,
        ..
    } = &mut journal
    else {
        panic!("restore committed");
    };
    assert!(*reconnect_network_outputs);
    *finalized = false;
    fixture
        .service
        .write_journal(id, &journal, &PersistenceShutdown::default())
        .expect("unfinished completion");
    let recreated =
        PortableSetupService::new(Arc::clone(&fixture.state)).expect("recreated service");
    recreated.recover().await.expect("replay completion");
    scan_finishes(&fixture, &calls, 2).await;
    assert!(matches!(
        fixture.service.read_journal(id).expect("journal"),
        Some(RestoreJournal::Committed {
            finalized: true,
            reconnect_network_outputs: true,
            ..
        })
    ));

    let mut legacy = serde_json::to_value(&journal).expect("journal JSON");
    legacy
        .as_object_mut()
        .expect("journal object")
        .remove("reconnect_network_outputs");
    let legacy: RestoreJournal =
        serde_json::from_value(legacy).expect("older receipt remains readable");
    assert!(matches!(
        legacy,
        RestoreJournal::Committed {
            reconnect_network_outputs: false,
            ..
        }
    ));
}
