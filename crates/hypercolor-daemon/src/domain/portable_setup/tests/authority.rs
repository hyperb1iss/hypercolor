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

async fn historical_alias_collision() -> (
    Fixture,
    crate::portable_setup_state::ActiveProjection,
    Uuid,
    Uuid,
    Uuid,
) {
    use hypercolor_types::portable_setup::{PortableDevice, PortableDeviceOutput, RestoreReceipt};
    let fixture = Fixture::new("local");
    let source = Uuid::now_v7();
    let target = Uuid::now_v7();
    let orphan = Uuid::now_v7();
    let authored = Uuid::now_v7();
    let mut applied = fixture.empty(source).await;
    let device = PortableDevice {
        name: "Offline strip".into(),
        portable_key: None,
        driver_id: "wled".into(),
        vendor_id: None,
        product_id: None,
        model: None,
        led_count: 64,
        binding_hint: None,
    };
    applied.manifest.devices.insert(orphan, device.clone());
    applied.manifest.devices.insert(authored, device);
    applied.manifest.device_output.insert(
        authored,
        PortableDeviceOutput {
            disabled: true,
            brightness: 0.37,
            rotation: hypercolor_types::scene::DisplayRotation::Deg90,
        },
    );
    applied.revision = crate::domain::portable_setup::setup_revision(&applied.manifest)
        .expect("valid legacy graph");
    let mut aliases = SetupBindings::default();
    aliases.outputs.insert("wled:192.0.2.14".into(), orphan);
    aliases.outputs.insert(format!("portable:{orphan}"), orphan);
    aliases
        .outputs
        .insert(format!("portable:{authored}"), authored);
    let active = crate::portable_setup_state::ActiveProjection {
        schema_version: 2,
        receipt: RestoreReceipt {
            operation_id: Uuid::now_v7(),
            setup_id: source,
            requested_revision: "original operation authority".into(),
            applied,
            unresolved_bindings: vec![orphan, authored],
            missing_resources: hypercolor_types::portable_setup::MissingSetupResources::default(),
            materialized_bindings: SetupBindings::default(),
        },
        aliases,
    };
    let mut target_aliases = SetupBindings::default();
    for local in active.aliases.outputs.keys() {
        target_aliases.outputs.insert(local.clone(), authored);
    }
    std::fs::write(
        fixture.service.root.join(format!("{target}.bindings.json")),
        serde_json::to_vec_pretty(&target_aliases).expect("target aliases"),
    )
    .expect("save target aliases");
    (fixture, active, target, orphan, authored)
}

fn save_active(
    fixture: &Fixture,
    active: &crate::portable_setup_state::ActiveProjection,
) -> Vec<u8> {
    let mut active = active.clone();
    active.receipt.applied.revision =
        crate::domain::portable_setup::setup_revision(&active.receipt.applied.manifest)
            .expect("valid active graph");
    let bytes = serde_json::to_vec_pretty(&active).expect("active record");
    std::fs::write(fixture.service.root.join("active-projection.json"), &bytes)
        .expect("save active record");
    bytes
}

#[tokio::test]
async fn historical_unreferenced_alias_projects_without_overwriting_authored_device() {
    let (fixture, active, target, orphan, authored) = historical_alias_collision().await;
    let original = save_active(&fixture, &active);
    let journal_path = fixture
        .service
        .root
        .join(format!("{}.transaction.json", active.receipt.setup_id));
    let journal = serde_json::to_vec(&crate::portable_setup_state::RestoreJournal::Committed {
        schema_version: 1,
        receipt: Box::new(active.receipt.clone()),
        finalized: true,
        reconnect_network_outputs: false,
        ancillary: None,
    })
    .expect("committed receipt");
    std::fs::write(&journal_path, &journal).expect("original journal");
    std::fs::write(
        fixture.service.root.join("manual-backup-identity.json"),
        serde_json::to_vec(&SetupIdentity {
            setup_id: target,
            name: "Manual transport".into(),
        })
        .expect("manual identity"),
    )
    .expect("save manual identity");
    let translated = fixture
        .service
        .active_projection_for(target, &SetupBindings::default())
        .expect("historical alias should not block a manual projection")
        .expect("active record");
    assert!(
        !translated
            .receipt
            .applied
            .manifest
            .devices
            .contains_key(&orphan)
    );
    assert_eq!(translated.receipt.applied.manifest.devices.len(), 1);
    assert_eq!(
        translated.receipt.applied.manifest.device_output,
        active.receipt.applied.manifest.device_output
    );
    assert_eq!(translated.receipt.unresolved_bindings, vec![authored]);
    assert!(
        translated
            .aliases
            .outputs
            .values()
            .all(|id| *id == authored)
    );
    let recreated = crate::domain::portable_setup::PortableSetupService::new(
        std::sync::Arc::clone(&fixture.state),
    )
    .expect("recreated service");
    let repeated = recreated
        .active_projection_for(target, &SetupBindings::default())
        .expect("repeat projection")
        .expect("active record");
    assert_eq!(
        translated.receipt.applied.manifest,
        repeated.receipt.applied.manifest
    );
    let first_archive = hypercolor_core::portable_setup::archive::read_archive(
        fixture
            .service
            .export_archive()
            .await
            .expect("manual archive"),
        &fixture.service.blobs,
    )
    .expect("archive contents");
    let repeated_archive = hypercolor_core::portable_setup::archive::read_archive(
        recreated
            .export_archive()
            .await
            .expect("manual archive after recreation"),
        &fixture.service.blobs,
    )
    .expect("repeated contents");
    assert_eq!(first_archive.setup, repeated_archive.setup);
    assert_eq!(first_archive.setup.devices.len(), 1);
    assert_eq!(
        first_archive.setup.device_output,
        active.receipt.applied.manifest.device_output
    );
    assert_eq!(std::fs::read(journal_path).expect("raw journal"), journal);
    assert_eq!(
        std::fs::read(fixture.service.root.join("active-projection.json")).expect("raw authority"),
        original,
        "projection must not rewrite the original restore receipt"
    );
}

#[tokio::test]
async fn historical_alias_cleanup_retains_every_ambiguous_or_authored_collision() {
    use hypercolor_types::device::DeviceId;
    use hypercolor_types::portable_setup::{PortableAttachments, PortableLogicalDevice};
    for scenario in [
        "independent output alias",
        "independent device alias",
        "configured source",
        "attached source",
        "logical source",
        "live source",
        "live destination",
        "materialized source",
        "materialized destination",
        "different definition",
        "multiple sources",
        "destination chain",
        "synthetic proof only",
        "resolved source",
        "foreign synthetic alias",
        "layout source",
        "scene layout source",
        "scene display source",
        "display face source",
    ] {
        let (fixture, mut active, target, orphan, authored) = historical_alias_collision().await;
        let mut live = SetupBindings::default();
        let mut target_aliases = fixture
            .service
            .saved_bindings(target)
            .expect("target aliases");
        match scenario {
            "independent output alias" => {
                active
                    .aliases
                    .outputs
                    .insert("wled:192.0.2.99".into(), authored);
            }
            "independent device alias" => {
                active.aliases.devices.insert(DeviceId::new(), authored);
            }
            "configured source" => {
                let output = active.receipt.applied.manifest.device_output[&authored].clone();
                active
                    .receipt
                    .applied
                    .manifest
                    .device_output
                    .insert(orphan, output);
            }
            "attached source" => {
                active.receipt.applied.manifest.attachments.insert(
                    orphan,
                    PortableAttachments {
                        slots: Vec::new(),
                        bindings: Vec::new(),
                    },
                );
            }
            "logical source" => {
                active.receipt.applied.manifest.logical_devices.insert(
                    Uuid::now_v7(),
                    PortableLogicalDevice {
                        device: orphan,
                        name: "Authored segment".into(),
                        led_start: 0,
                        led_count: 8,
                        enabled: true,
                    },
                );
            }
            "layout source" | "scene layout source" => {
                let sample: hypercolor_types::portable_setup::PortableSetupV1 =
                    serde_json::from_str(include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../hypercolor-types/tests/fixtures/portable_setup_v1.json"
                    )))
                    .expect("typed layout fixture");
                let mut layout = sample.layouts.values().next().expect("layout").clone();
                assert!(!layout.zones.is_empty());
                for output in &mut layout.zones {
                    output.device = orphan;
                    output.attachment = None;
                }
                if scenario == "layout source" {
                    active
                        .receipt
                        .applied
                        .manifest
                        .layouts
                        .insert(layout.id.clone(), layout);
                } else {
                    active
                        .receipt
                        .applied
                        .manifest
                        .scenes
                        .values_mut()
                        .next()
                        .expect("scene")
                        .zones[0]
                        .layout = layout;
                }
            }
            "scene display source" => {
                let zone = &mut active
                    .receipt
                    .applied
                    .manifest
                    .scenes
                    .values_mut()
                    .next()
                    .expect("scene")
                    .zones[0];
                zone.role = hypercolor_types::scene::ZoneRole::Display;
                zone.display_target =
                    Some(hypercolor_types::portable_setup::PortableDisplayTarget {
                        device: orphan,
                        blend_mode: hypercolor_types::layer::BlendMode::Alpha,
                        opacity: 1.0,
                    });
            }
            "display face source" => {
                active.receipt.applied.manifest.display_faces.insert(
                    orphan,
                    hypercolor_types::portable_setup::PortableDisplayFace {
                        effect_id: hypercolor_types::effect::EffectId::new(Uuid::now_v7()),
                        controls: std::collections::HashMap::default(),
                        blend_mode: hypercolor_types::layer::BlendMode::Alpha,
                        opacity: 1.0,
                    },
                );
            }
            "live source" => {
                live.outputs.insert("live".into(), orphan);
            }
            "live destination" => {
                live.outputs.insert("live".into(), authored);
            }
            "materialized source" => {
                active
                    .receipt
                    .materialized_bindings
                    .outputs
                    .insert("live".into(), orphan);
            }
            "materialized destination" => {
                active
                    .receipt
                    .materialized_bindings
                    .outputs
                    .insert("live".into(), authored);
            }
            "different definition" => {
                active
                    .receipt
                    .applied
                    .manifest
                    .devices
                    .get_mut(&orphan)
                    .expect("source")
                    .name = "Other strip".into();
            }
            "multiple sources" => {
                let other = Uuid::now_v7();
                active.receipt.applied.manifest.devices.insert(
                    other,
                    active.receipt.applied.manifest.devices[&orphan].clone(),
                );
                active.receipt.unresolved_bindings.push(other);
                active
                    .aliases
                    .outputs
                    .insert("wled:192.0.2.15".into(), other);
                target_aliases
                    .outputs
                    .insert("wled:192.0.2.15".into(), authored);
            }
            "destination chain" => {
                target_aliases
                    .outputs
                    .insert(format!("portable:{authored}"), Uuid::now_v7());
            }
            "synthetic proof only" => {
                active.aliases.outputs.remove("wled:192.0.2.14");
            }
            "resolved source" => {
                active
                    .receipt
                    .unresolved_bindings
                    .retain(|id| *id != orphan);
            }
            "foreign synthetic alias" => {
                active
                    .aliases
                    .outputs
                    .insert(format!("portable:{}", Uuid::now_v7()), orphan);
            }
            _ => unreachable!("scenario is enumerated above"),
        }
        std::fs::write(
            fixture.service.root.join(format!("{target}.bindings.json")),
            serde_json::to_vec(&target_aliases).expect("target aliases"),
        )
        .expect("save target aliases");
        save_active(&fixture, &active);
        let result = fixture.service.active_projection_for(target, &live);
        if scenario == "destination chain" {
            // A simultaneous injective translation is valid; it must retain both records.
            let translated = result.expect("injective chain").expect("active");
            assert_eq!(
                translated.receipt.applied.manifest.devices.len(),
                active.receipt.applied.manifest.devices.len()
            );
        } else {
            assert!(
                result.is_err(),
                "{scenario} must retain the strict collision fence"
            );
        }
    }
}
