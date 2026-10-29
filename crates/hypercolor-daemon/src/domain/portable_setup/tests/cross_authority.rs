use super::*;
use hypercolor_core::portable_setup::archive::{read_archive, write_archive};
use hypercolor_types::portable_setup::{
    PortableBackupV1, PortableDevice, PortableDeviceOutput, PortableDisplayTarget,
};
use hypercolor_types::scene::{SceneKind, ZoneId, ZoneRole};
use std::io::{Seek, SeekFrom};

fn offline_display(mut manifest: PortableSetupV1) -> PortableSetupV1 {
    let display = Uuid::now_v7();
    manifest.devices.insert(
        display,
        PortableDevice {
            name: "Unplugged panel".into(),
            portable_key: None,
            driver_id: "test-display".into(),
            vendor_id: None,
            product_id: None,
            model: None,
            led_count: 64,
            binding_hint: None,
        },
    );
    manifest.device_output.insert(
        display,
        PortableDeviceOutput {
            disabled: true,
            brightness: 0.37,
            rotation: hypercolor_types::scene::DisplayRotation::Deg90,
        },
    );
    let mut scene = manifest.scenes[&Uuid::nil()].clone();
    scene.kind = SceneKind::Named;
    scene.name = "Offline panel scene".into();
    let mut zone = scene.zones[0].clone();
    zone.id = ZoneId::new();
    zone.role = ZoneRole::Display;
    zone.display_target = Some(PortableDisplayTarget {
        device: display,
        blend_mode: hypercolor_types::layer::BlendMode::Alpha,
        opacity: 0.75,
    });
    scene.zones.push(zone);
    manifest.scenes.insert(Uuid::now_v7(), scene);
    manifest
}

fn assert_offline_display(manifest: &PortableSetupV1) {
    let (id, _) = manifest
        .devices
        .iter()
        .find(|(_, device)| device.name == "Unplugged panel")
        .expect("offline device is authored state");
    assert!(
        manifest
            .scenes
            .values()
            .flat_map(|scene| &scene.zones)
            .any(|zone| zone
                .display_target
                .as_ref()
                .is_some_and(|target| target.device == *id))
    );
    let output = &manifest.device_output[id];
    assert!(output.disabled);
    assert_eq!(output.brightness.to_bits(), 0.37_f32.to_bits());
    assert_eq!(
        output.rotation,
        hypercolor_types::scene::DisplayRotation::Deg90
    );
    manifest.validate().expect("retargeted graph remains valid");
}

#[tokio::test]
async fn cloud_restore_survives_manual_archive_export() {
    let fixture = Fixture::new("local");
    let empty = fixture
        .service
        .export(
            SetupIdentity {
                setup_id: Uuid::now_v7(),
                name: "Cloud studio".into(),
            },
            &SetupBindings::default(),
        )
        .await
        .expect("cloud baseline");
    let desired = offline_display(empty.manifest);
    fixture
        .drive(fixture.service.restore(
            &desired,
            &SetupBindings::default(),
            options(empty.revision),
        ))
        .await
        .expect("cloud restore");
    let archive = fixture
        .service
        .export_archive()
        .await
        .expect("manual export");
    let backup = read_archive(archive, &fixture.service.blobs).expect("manual archive");
    assert_ne!(backup.setup.setup_id, desired.setup_id);
    assert_eq!(backup.setup.name, "Cloud studio");
    assert_offline_display(&backup.setup);
}

#[tokio::test]
async fn manual_import_survives_first_cloud_association_export() {
    let fixture = Fixture::new("local");
    let mut desired = offline_display(fixture.empty(Uuid::now_v7()).await.manifest);
    desired.name = "Studio".into();
    let backup = PortableBackupV1 {
        tui_preferences: None,
        schema_version: 1,
        setup: desired,
        preferences: None,
        favorites: Vec::new(),
    };
    let mut archive = fixture.service.archive_upload_file().expect("spool");
    write_archive(&mut archive, &backup, &fixture.service.blobs).expect("archive");
    archive.seek(SeekFrom::Start(0)).expect("rewind");
    let preview = fixture
        .service
        .preview_archive(archive)
        .await
        .expect("preview");
    fixture
        .drive(fixture.service.apply_archive(preview.operation_id))
        .await
        .expect("manual restore");
    let recreated = PortableSetupService::new(Arc::clone(&fixture.state)).expect("new service");
    let exported = read_archive(
        recreated.export_archive().await.expect("manual re-export"),
        &fixture.service.blobs,
    )
    .expect("re-exported archive");
    assert_eq!(preview.name, "Studio");
    assert_eq!(exported.setup.name, "Studio");
    assert_offline_display(&exported.setup);
    let cloud_id = Uuid::now_v7();
    let bindings = fixture
        .service
        .assign_bindings(cloud_id, None)
        .await
        .expect("first cloud association");
    let cloud = fixture
        .service
        .export(
            SetupIdentity {
                setup_id: cloud_id,
                name: "Cloud rig".into(),
            },
            &bindings,
        )
        .await
        .expect("cloud export");
    assert_eq!(cloud.manifest.name, "Cloud rig");
    assert_offline_display(&cloud.manifest);
}

#[tokio::test]
async fn deleted_offline_state_does_not_return_from_an_older_rig_receipt() {
    let fixture = Fixture::new("local");
    let old_id = Uuid::now_v7();
    let empty = fixture.empty(old_id).await;
    let desired = offline_display(empty.manifest);
    fixture
        .drive(fixture.service.restore(
            &desired,
            &SetupBindings::default(),
            options(empty.revision),
        ))
        .await
        .expect("first restore");
    let new_id = Uuid::now_v7();
    let current = fixture.empty(new_id).await;
    assert_offline_display(&current.manifest);
    let mut removed = current.manifest.clone();
    removed.devices.clear();
    removed.device_output.clear();
    removed.scenes.retain(|id, _| id.is_nil());
    let mut restore_options = options(current.revision);
    restore_options.mode = RestoreMode::Synchronize {
        previous: Box::new(current.manifest),
    };
    fixture
        .drive(
            fixture
                .service
                .restore(&removed, &SetupBindings::default(), restore_options),
        )
        .await
        .expect("delete under new identity");
    let old = fixture.empty(old_id).await;
    assert!(
        old.manifest.devices.is_empty(),
        "old operation receipts must not resurrect deleted authored state"
    );
    assert!(
        !old.manifest
            .scenes
            .values()
            .any(|scene| scene.name == "Offline panel scene")
    );
}
