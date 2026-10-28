use super::{Fixture, SetupBindings, SetupIdentity};
use crate::domain::portable_setup::journal::RestoreJournal;
use crate::domain::portable_setup::{PortableSetupSnapshot, RestoreReceipt, fingerprint};
use crate::persistence::PersistenceShutdown;
use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceFamily, DeviceFingerprint, DeviceId, DeviceInfo,
    DeviceOrigin,
};
use std::collections::HashMap;
use uuid::Uuid;

pub(super) async fn register_device(fixture: &Fixture) -> DeviceId {
    fixture
        .state
        .device_registry
        .add_discovered(DiscoveredDevice {
            info: DeviceInfo {
                id: DeviceId::new(),
                name: "Portable light".into(),
                vendor: "test".into(),
                family: DeviceFamily::new_static("test", "Test"),
                model: None,
                connection_type: ConnectionType::Network,
                origin: DeviceOrigin::native("test", "test", ConnectionType::Network),
                segments: Vec::new(),
                firmware_version: None,
                capabilities: DeviceCapabilities::default(),
            },
            fingerprint: DeviceFingerprint::from_persisted("net:test:portable"),
            metadata: HashMap::new(),
            connect_behavior: DiscoveryConnectBehavior::Deferred,
            claim: None,
        })
        .await
}

#[tokio::test]
async fn changed_local_device_id_restores_saved_output_before_export() {
    let fixture = Fixture::new("local");
    let id = Uuid::now_v7();
    let local = register_device(&fixture).await;
    let bindings = fixture
        .service
        .assign_bindings(id, None)
        .await
        .expect("current associations");
    let identity = SetupIdentity {
        setup_id: id,
        name: "My lighting".into(),
    };
    let current = fixture
        .service
        .export(identity.clone(), &bindings)
        .await
        .expect("initial projection");
    let portable = bindings.devices[&local];
    let mut saved = current.manifest;
    saved.name = "Studio".into();
    saved
        .device_output
        .get_mut(&portable)
        .expect("physical output")
        .brightness = 0.37;
    let revision = fingerprint(&saved).expect("saved revision");
    // Simulate the durable receipt from the prior OS identity. Discovery has
    // already associated this rig's portable device with its new native ID.
    let receipt = RestoreReceipt {
        operation_id: Uuid::now_v7(),
        setup_id: id,
        requested_revision: revision.clone(),
        applied: PortableSetupSnapshot {
            manifest: saved.clone(),
            revision,
        },
        unresolved_bindings: Vec::new(),
        materialized_bindings: SetupBindings {
            devices: HashMap::from([(DeviceId::new(), portable)]),
            outputs: HashMap::new(),
        },
    };
    fixture
        .service
        .write_journal(
            id,
            &RestoreJournal::Committed {
                schema_version: 1,
                receipt: Box::new(receipt),
                finalized: true,
                reconnect_network_outputs: false,
                ancillary: None,
            },
            &PersistenceShutdown::default(),
        )
        .expect("prior OS receipt");
    assert!(
        fixture
            .service
            .export(identity.clone(), &bindings)
            .await
            .is_err(),
        "fresh discovery defaults must not overwrite saved authoring"
    );
    let report = fixture
        .drive(fixture.service.reconcile_projection(
            super::super::export::ProjectionIdentity::CurrentNative(identity.clone()),
        ))
        .await
        .expect("new identity reconciles")
        .expect("rematch needs materialization");
    assert!(report.unresolved_bindings.is_empty());
    assert_eq!(report.applied.manifest.name, "Studio");
    assert_eq!(
        fixture
            .service
            .read_active_projection()
            .expect("active record")
            .expect("accepted setup")
            .receipt
            .applied
            .manifest
            .name,
        "Studio",
        "stale manual identity must not rename the accepted setup during reconciliation"
    );
    assert_eq!(
        fixture
            .state
            .device_registry
            .get(&local)
            .await
            .expect("current device")
            .user_settings
            .brightness,
        0.37
    );
    let exported = fixture
        .service
        .export(identity, &bindings)
        .await
        .expect("materialized export");
    assert_eq!(
        exported.manifest.device_output[&portable],
        saved.device_output[&portable]
    );
}

#[tokio::test]
async fn recovery_preserves_a_third_device_output_value_on_the_original_binding() {
    use crate::domain::portable_setup::journal::{RestoreRecovery, RuntimeRestoreProof};

    let fixture = Fixture::new("local");
    let id = Uuid::now_v7();
    let local = register_device(&fixture).await;
    let bindings = fixture
        .service
        .assign_bindings(id, None)
        .await
        .expect("bindings");
    let before = fixture
        .service
        .export(
            SetupIdentity {
                setup_id: id,
                name: "My lighting".into(),
            },
            &bindings,
        )
        .await
        .expect("initial snapshot");
    let mut desired = before.manifest.clone();
    desired
        .device_output
        .get_mut(&bindings.devices[&local])
        .expect("output")
        .brightness = 0.4;
    let key = {
        let _mutation = fixture.state.device_settings.begin_device_mutation().await;
        let updated = fixture
            .state
            .device_registry
            .update_user_settings(&local, None, None, Some(0.7), None)
            .await
            .expect("device");
        let key = crate::device_settings::resolve_device_settings_key(
            &fixture.state.device_registry,
            &fixture.state.device_settings,
            local,
        )
        .await;
        fixture
            .state
            .device_settings
            .persist_device_settings(
                &key,
                crate::device_settings::StoredDeviceSettings {
                    name: updated.user_settings.name,
                    disabled: !updated.user_settings.enabled,
                    brightness: updated.user_settings.brightness,
                    rotation: updated.user_settings.display_rotation,
                },
            )
            .await
            .expect("persist third value");
        fixture
            .state
            .backend_manager
            .lock()
            .await
            .set_device_output_brightness(local, updated.user_settings.brightness);
        key
    };
    let runtime = crate::runtime_state::snapshot_from_scene_manager(
        &fixture.state.scene_manager.snapshot().await,
    );
    let result = Box::pin(fixture.service.apply_candidate(
        before.manifest,
        desired,
        "candidate".into(),
        bindings.clone(),
        super::options(before.revision),
        &PersistenceShutdown::default(),
        Some(RestoreRecovery {
            runtime: RuntimeRestoreProof {
                before: None,
                candidate: runtime,
            },
            bindings,
        }),
        None,
    ))
    .await;
    assert!(
        result.is_err(),
        "an unrelated output edit cannot become recovery's new baseline"
    );
    assert_eq!(
        fixture
            .state
            .device_registry
            .get(&local)
            .await
            .expect("device")
            .user_settings
            .brightness,
        0.7
    );
    assert_eq!(
        fixture
            .state
            .device_settings
            .device_settings_for_key(&key)
            .await
            .expect("persisted device")
            .brightness,
        0.7
    );
    assert!(fixture.service.read_journal(id).expect("journal").is_none());
}
