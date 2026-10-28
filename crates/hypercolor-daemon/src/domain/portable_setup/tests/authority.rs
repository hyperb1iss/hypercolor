use super::{Fixture, SetupBindings, SetupIdentity, bindings::register_device, options};
use uuid::Uuid;

#[tokio::test]
async fn active_projection_translates_existing_device_aliases_between_rigs() {
    let fixture = Fixture::new("local");
    let local = register_device(&fixture).await;
    let source_id = Uuid::now_v7();
    let target_id = Uuid::now_v7();
    let source = fixture
        .service
        .assign_bindings(source_id, None)
        .await
        .expect("source aliases");
    let target = fixture
        .service
        .assign_bindings(target_id, None)
        .await
        .expect("target aliases");
    assert_ne!(source.devices[&local], target.devices[&local]);
    let identity = SetupIdentity {
        setup_id: source_id,
        name: "Source".into(),
    };
    let before = fixture
        .service
        .export(identity, &source)
        .await
        .expect("before");
    let mut desired = before.manifest;
    desired
        .device_output
        .get_mut(&source.devices[&local])
        .expect("output")
        .brightness = 0.37;
    fixture
        .drive(
            fixture
                .service
                .restore(&desired, &source, options(before.revision)),
        )
        .await
        .expect("source restore");
    let current = fixture
        .service
        .assign_bindings(target_id, None)
        .await
        .expect("current aliases");
    assert_eq!(current.devices[&local], target.devices[&local]);
    let exported = fixture
        .service
        .export(
            SetupIdentity {
                setup_id: target_id,
                name: "Target".into(),
            },
            &current,
        )
        .await
        .expect("translated export");
    assert_eq!(exported.manifest.devices.len(), 1);
    assert!(
        !exported
            .manifest
            .devices
            .contains_key(&source.devices[&local])
    );
    assert_eq!(
        exported.manifest.device_output[&target.devices[&local]].brightness,
        0.37
    );
    let active = fixture
        .service
        .read_active_projection()
        .expect("active record")
        .expect("restored state");
    assert_eq!(
        active.receipt.setup_id, source_id,
        "exporting never changes authored ownership"
    );
}

#[tokio::test]
async fn active_projection_translates_replacement_aliases_within_the_same_rig() {
    let fixture = Fixture::new("local");
    let local = register_device(&fixture).await;
    let id = Uuid::now_v7();
    let source = fixture
        .service
        .assign_bindings(id, None)
        .await
        .expect("source aliases");
    let identity = SetupIdentity {
        setup_id: id,
        name: "Source".into(),
    };
    let before = fixture
        .service
        .export(identity.clone(), &source)
        .await
        .expect("before");
    let mut desired = before.manifest;
    desired
        .device_output
        .get_mut(&source.devices[&local])
        .expect("output")
        .brightness = 0.37;
    fixture
        .drive(
            fixture
                .service
                .restore(&desired, &source, options(before.revision)),
        )
        .await
        .expect("source restore");
    let replacement = Uuid::now_v7();
    let target = SetupBindings {
        devices: source
            .devices
            .keys()
            .map(|local| (*local, replacement))
            .collect(),
        outputs: source
            .outputs
            .keys()
            .map(|local| (local.clone(), replacement))
            .collect(),
    };
    let exported = fixture
        .service
        .export(identity, &target)
        .await
        .expect("replacement export");
    assert_eq!(exported.manifest.devices.len(), 1);
    assert!(
        !exported
            .manifest
            .devices
            .contains_key(&source.devices[&local])
    );
    assert_eq!(
        exported.manifest.device_output[&replacement].brightness,
        0.37
    );
}
