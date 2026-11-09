use super::{Fixture, SetupBindings, SetupIdentity, options};
use crate::domain::portable_setup::journal::RestoreJournal;
use crate::domain::portable_setup::{PortableSetupSnapshot, RestoreReceipt, fingerprint};
use crate::persistence::PersistenceShutdown;
use hypercolor_core::portable_setup::archive::read_archive;
use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
    DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint, SegmentInfo,
};
use hypercolor_types::portable_setup::{PortableLogicalDevice, PortableSetupV1};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

pub(super) async fn register_device(fixture: &Fixture) -> DeviceId {
    register_network_device(fixture, "Portable light", "net:test:portable").await
}

async fn register_network_device(fixture: &Fixture, name: &str, fingerprint: &str) -> DeviceId {
    fixture
        .state
        .device_registry
        .add_discovered(DiscoveredDevice {
            info: DeviceInfo {
                id: DeviceId::new(),
                name: name.into(),
                vendor: "test".into(),
                family: DeviceFamily::new_static("test", "Test"),
                model: None,
                connection_type: ConnectionType::Network,
                origin: DeviceOrigin::native("test", "test", ConnectionType::Network),
                segments: Vec::new(),
                firmware_version: None,
                capabilities: DeviceCapabilities::default(),
            },
            fingerprint: DeviceFingerprint::from_persisted(fingerprint),
            metadata: HashMap::new(),
            connect_behavior: DiscoveryConnectBehavior::Deferred,
            claim: None,
        })
        .await
}

/// Claimless USB hardware carries shape evidence, so an authored definition
/// from another installation can match it without a local association.
async fn register_usb_device(fixture: &Fixture) -> DeviceId {
    let fingerprint = DeviceFingerprint::from_persisted("usb:razer:1532:0099:test-hidraw8");
    fixture
        .state
        .device_registry
        .add_discovered(DiscoveredDevice {
            info: DeviceInfo {
                id: fingerprint.stable_device_id(),
                name: "Controller".into(),
                vendor: "Razer".into(),
                family: DeviceFamily::new("razer", "Razer"),
                model: Some("case".into()),
                connection_type: ConnectionType::Usb,
                origin: DeviceOrigin::native("razer", "usb", ConnectionType::Usb),
                segments: vec![SegmentInfo {
                    name: "Main".into(),
                    led_count: 16,
                    topology: DeviceTopologyHint::Strip,
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                }],
                firmware_version: None,
                capabilities: DeviceCapabilities {
                    led_count: 16,
                    ..Default::default()
                },
            },
            fingerprint,
            metadata: HashMap::new(),
            connect_behavior: DiscoveryConnectBehavior::Deferred,
            claim: None,
        })
        .await
}

/// Accept the current lighting through the restore coordinator, leaving the
/// committed receipt and active projection that later projections compare with.
async fn accept_current_setup(fixture: &Fixture, identity: &SetupIdentity) -> PortableSetupV1 {
    let bindings = fixture
        .service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("initial association");
    let before = fixture
        .service
        .export(identity.clone(), &bindings)
        .await
        .expect("initial export");
    fixture
        .drive(
            fixture
                .service
                .restore(&before.manifest, &bindings, options(before.revision)),
        )
        .await
        .expect("accepted setup")
        .applied
        .manifest
}

fn device_named(manifest: &PortableSetupV1, name: &str) -> Uuid {
    let mut ids = manifest
        .devices
        .iter()
        .filter(|(_, device)| device.name == name)
        .map(|(id, _)| *id);
    let id = ids.next().expect("named device is projected");
    assert!(ids.next().is_none(), "one device carries this name");
    id
}

#[tokio::test]
async fn device_added_after_a_restore_keeps_its_portable_identity() {
    let fixture = Fixture::new("local");
    register_device(&fixture).await;
    let identity = SetupIdentity {
        setup_id: Uuid::now_v7(),
        name: "Studio".into(),
    };
    let accepted = accept_current_setup(&fixture, &identity).await;
    let added = register_network_device(&fixture, "Canvas", "net:test:canvas").await;
    let mut projected: Option<(Uuid, PortableSetupSnapshot)> = None;
    // Each cycle is one native projection followed by its own echo: reconcile
    // against the receipt, associate, export, then associate the echoed setup.
    for cycle in 0..3 {
        assert!(
            fixture
                .drive(fixture.service.reconcile_bindings(identity.clone()))
                .await
                .expect("reconcile against the accepted receipt")
                .is_none(),
            "a device the receipt never named has nothing saved to restore"
        );
        let bindings = fixture
            .service
            .assign_bindings(identity.setup_id, None)
            .await
            .expect("projection association");
        let portable = bindings.devices[&added];
        assert!(!accepted.devices.contains_key(&portable));
        let snapshot = fixture
            .service
            .export(identity.clone(), &bindings)
            .await
            .expect("projection");
        if let Some((first, first_snapshot)) = &projected {
            assert_eq!(
                portable, *first,
                "cycle {cycle}: the added device kept its portable identity"
            );
            assert_eq!(
                snapshot.revision, first_snapshot.revision,
                "cycle {cycle}: an unchanged installation projects one revision"
            );
        }
        let echoed = fixture
            .service
            .assign_bindings(identity.setup_id, Some(&snapshot.manifest))
            .await
            .expect("echo association");
        assert_eq!(echoed.devices[&added], portable);
        projected.get_or_insert((portable, snapshot));
    }
}

#[tokio::test]
async fn backup_export_keeps_identities_of_devices_added_after_a_restore() {
    let fixture = Fixture::new("local");
    register_device(&fixture).await;
    accept_current_setup(
        &fixture,
        &SetupIdentity {
            setup_id: Uuid::now_v7(),
            name: "Studio".into(),
        },
    )
    .await;
    register_network_device(&fixture, "Canvas", "net:test:canvas").await;
    let mut exports = Vec::new();
    for _ in 0..2 {
        let archive = fixture.service.export_archive().await.expect("backup");
        exports.push(read_archive(archive, &fixture.service.blobs).expect("archive"));
    }
    assert_eq!(
        device_named(&exports[0].setup, "Canvas"),
        device_named(&exports[1].setup, "Canvas"),
        "every backup names the added device by one portable identity"
    );
    assert_eq!(exports[0].setup, exports[1].setup);
}

#[tokio::test]
async fn held_association_yields_only_to_a_matching_definition() {
    let fixture = Fixture::new("local");
    let local = register_usb_device(&fixture).await;
    let identity = SetupIdentity {
        setup_id: Uuid::now_v7(),
        name: "Dual boot".into(),
    };
    let held = fixture
        .service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("local association")
        .devices[&local];
    let bound = fixture
        .service
        .export(
            identity.clone(),
            &fixture
                .service
                .assign_bindings(identity.setup_id, None)
                .await
                .expect("association"),
        )
        .await
        .expect("bound export")
        .manifest;
    assert!(
        bound.devices[&held].binding_hint.is_some(),
        "claimless USB exports shape evidence"
    );
    // Another installation authored the same hardware under its own identity.
    let remote = Uuid::now_v7();
    let reauthored = bound
        .remap_devices_and_outputs(&BTreeMap::from([(held, remote)]))
        .expect("re-authored setup");
    let rebound = fixture
        .service
        .assign_bindings(identity.setup_id, Some(&reauthored))
        .await
        .expect("re-authored association");
    assert_eq!(
        rebound.devices[&local], remote,
        "an identity the incoming setup lacks yields to its matching definition"
    );
    // A replacement that no longer describes this hardware has nothing to
    // claim it with, so the device keeps the identity it already holds.
    let mut replaced = reauthored;
    replaced.devices.clear();
    replaced.device_output.clear();
    let unclaimed = fixture
        .service
        .assign_bindings(identity.setup_id, Some(&replaced))
        .await
        .expect("replaced association");
    assert_eq!(unclaimed.devices[&local], remote);
    let again = fixture
        .service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("later projection");
    assert_eq!(again.devices[&local], remote);
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
        missing_resources: super::super::MissingSetupResources::default(),
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

#[tokio::test]
async fn reconnecting_device_keeps_authored_definition_without_live_binding() {
    let fixture = Fixture::new("local");
    let setup_id = Uuid::now_v7();
    let local = register_device(&fixture).await;
    let bindings = fixture
        .service
        .assign_bindings(setup_id, None)
        .await
        .expect("initial association");
    let portable = bindings.devices[&local];
    let identity = SetupIdentity {
        setup_id,
        name: "Reconnect setup".into(),
    };
    let before = fixture
        .service
        .export(identity.clone(), &bindings)
        .await
        .expect("initial export");
    let report = fixture
        .drive(fixture.service.restore(
            &before.manifest,
            &bindings,
            super::options(before.revision),
        ))
        .await
        .expect("accept initial authored state");
    let tracked = fixture
        .state
        .device_registry
        .get(&local)
        .await
        .expect("device");
    let original_state = tracked.state.clone();
    let mut changed_info = tracked.info;
    changed_info.name = "Temporary reconnect metadata".into();
    fixture
        .state
        .device_registry
        .update_info(&local, changed_info)
        .await
        .expect("metadata");
    fixture
        .state
        .device_registry
        .set_state(&local, hypercolor_types::device::DeviceState::Reconnecting)
        .await;
    let offline = fixture
        .service
        .assign_bindings(setup_id, None)
        .await
        .expect("offline projection aliases");
    assert!(offline.devices.is_empty());
    let exported = fixture
        .service
        .export(identity.clone(), &offline)
        .await
        .expect("reconnecting export");
    assert_eq!(exported.manifest.devices, report.applied.manifest.devices);
    assert_eq!(
        exported.manifest.device_output,
        report.applied.manifest.device_output
    );
    fixture
        .state
        .device_registry
        .set_state(&local, original_state)
        .await;
    let returned = fixture
        .service
        .assign_bindings(setup_id, None)
        .await
        .expect("returned device");
    assert_eq!(returned.devices[&local], portable);
}

#[tokio::test]
async fn stale_association_yields_when_restored_hardware_returns() {
    let fixture = Fixture::new("local");
    let local = register_usb_device(&fixture).await;
    let identity = SetupIdentity {
        setup_id: Uuid::now_v7(),
        name: "Dual boot".into(),
    };
    let bindings = fixture
        .service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("local association");
    let held = bindings.devices[&local];
    let authored = fixture
        .service
        .export(identity.clone(), &bindings)
        .await
        .expect("authored export")
        .manifest;
    // The setup arrives from another installation while this hardware is away.
    let remote = Uuid::now_v7();
    let desired = authored
        .remap_devices_and_outputs(&BTreeMap::from([(held, remote)]))
        .expect("re-authored setup");
    fixture
        .state
        .device_registry
        .remove(&local)
        .await
        .expect("unplugged hardware");
    let absent = fixture
        .service
        .assign_bindings(identity.setup_id, Some(&desired))
        .await
        .expect("association without the hardware");
    assert!(absent.devices.is_empty());
    let current = fixture
        .service
        .export(identity.clone(), &absent)
        .await
        .expect("current export");
    let restored = fixture
        .drive(
            fixture
                .service
                .restore(&desired, &absent, options(current.revision)),
        )
        .await
        .expect("restore without the hardware");
    assert!(restored.unresolved_bindings.contains(&remote));
    assert_eq!(register_usb_device(&fixture).await, local);
    let reconciled = fixture
        .drive(fixture.service.reconcile_bindings(identity.clone()))
        .await
        .expect("reconcile returned hardware")
        .expect("returned hardware receives its saved setup");
    assert!(reconciled.unresolved_bindings.is_empty());
    let returned = fixture
        .service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("later projection");
    assert_eq!(
        returned.devices[&local], remote,
        "an association older than the restore yields to the restored definition"
    );
}

#[tokio::test]
async fn association_from_another_setup_yields_to_its_matching_definition() {
    let fixture = Fixture::new("local");
    let local = register_usb_device(&fixture).await;
    let accepted = accept_current_setup(
        &fixture,
        &SetupIdentity {
            setup_id: Uuid::now_v7(),
            name: "Source".into(),
        },
    )
    .await;
    let aliased = *accepted
        .devices
        .keys()
        .next()
        .expect("accepted hardware definition");
    // The active projection carries the source alias into every other setup.
    // A setup that authored this hardware itself must keep its own identity.
    let remote = Uuid::now_v7();
    let mut incoming = accepted
        .remap_devices_and_outputs(&BTreeMap::from([(aliased, remote)]))
        .expect("other setup");
    incoming.setup_id = Uuid::now_v7();
    let bound = fixture
        .service
        .assign_bindings(incoming.setup_id, Some(&incoming))
        .await
        .expect("other setup association");
    assert_eq!(bound.devices[&local], remote);
}

#[tokio::test]
async fn unclaimed_identity_never_takes_an_incoming_logical_identity() {
    let fixture = Fixture::new("local");
    let local = register_device(&fixture).await;
    let identity = SetupIdentity {
        setup_id: Uuid::now_v7(),
        name: "Studio".into(),
    };
    let held = fixture
        .service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("local association")
        .devices[&local];
    let authored = fixture
        .service
        .export(
            identity.clone(),
            &fixture
                .service
                .assign_bindings(identity.setup_id, None)
                .await
                .expect("association"),
        )
        .await
        .expect("authored export")
        .manifest;
    // Another installation gave this identity to a segment of hardware that
    // shares no matching evidence with the local device.
    let segmented = Uuid::now_v7();
    let mut incoming = authored
        .remap_devices_and_outputs(&BTreeMap::from([(held, segmented)]))
        .expect("other hardware");
    incoming.logical_devices.insert(
        held,
        PortableLogicalDevice {
            device: segmented,
            name: "Segment".into(),
            led_start: 0,
            led_count: 8,
            enabled: true,
        },
    );
    incoming.validate().expect("incoming setup is valid");
    let bindings = fixture
        .service
        .assign_bindings(identity.setup_id, Some(&incoming))
        .await
        .expect("incoming association");
    let current = fixture
        .service
        .export(identity.clone(), &bindings)
        .await
        .expect("current export");
    fixture
        .drive(
            fixture
                .service
                .restore(&incoming, &bindings, options(current.revision)),
        )
        .await
        .expect("a valid setup restores");
    assert_ne!(bindings.devices[&local], held);
    assert_eq!(bindings.outputs[&format!("portable:{held}")], held);
}
