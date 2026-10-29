use super::{Fixture, PortableSetupService, SetupBindings, SetupIdentity, options};
use hypercolor_types::api::layouts::{CreateLayoutRequest, UpdateLayoutRequest};
use hypercolor_types::scene::{SceneId, SceneKind};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, StripDirection};
use std::collections::HashSet;
use std::io::Read;
use std::sync::Arc;
use uuid::Uuid;

const OFFLINE_OUTPUTS: [&str; 3] = [
    "prismrgb:16d0:1294:a592f5275154315431202020ff012f12",
    "wled:orphan",
    "razer:1532:0f0d:001-8-4",
];

fn output(id: usize, device: &str) -> Output {
    Output {
        id: format!("authored-output-{id}"),
        name: format!("Offline light {id}"),
        device_id: device.into(),
        zone_name: None,
        position: NormalizedPosition::new(0.3, 0.7),
        size: NormalizedPosition::new(0.2, 0.4),
        rotation: 0.25,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 12,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: Some(0.6),
    }
}

#[tokio::test]
async fn first_backup_preserves_native_offline_outputs_without_claiming_hardware() {
    let source = Fixture::new("local");
    let layout = source
        .state
        .domains
        .layout
        .create(CreateLayoutRequest {
            name: "Disconnected lights".into(),
            description: None,
        })
        .await
        .expect("create saved layout");
    let outputs = OFFLINE_OUTPUTS
        .iter()
        .enumerate()
        .map(|(index, device)| output(index, device))
        .collect::<Vec<_>>();
    source
        .state
        .domains
        .layout
        .update(
            layout.id.clone(),
            UpdateLayoutRequest {
                zones: Some(outputs.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("save authored offline outputs");
    let saved_layout = source
        .state
        .domains
        .layout
        .resolve(&layout.id)
        .await
        .expect("saved layout");
    let mut scene = source
        .state
        .scene_manager
        .snapshot()
        .await
        .get(&SceneId::DEFAULT)
        .expect("default scene")
        .clone();
    scene.id = SceneId::new();
    scene.name = "Disconnected scene".into();
    scene.kind = SceneKind::Named;
    scene.zones[0].layout = saved_layout;
    let scene_id = scene.id;
    let mut mutation = source.state.domains.scene.begin_mutation().await;
    mutation.create_scene(scene).expect("authored scene");
    source
        .state
        .domains
        .scene
        .commit(mutation)
        .await
        .expect("save authored scene");

    let identity = SetupIdentity {
        setup_id: Uuid::now_v7(),
        name: "Offline rig".into(),
    };
    let bindings = source
        .service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("first portable aliases");
    assert!(bindings.devices.is_empty());
    assert!(bindings.outputs.is_empty());
    assert!(source.state.device_registry.list().await.is_empty());
    let projection = source
        .service
        .projection_bindings(identity.setup_id, &bindings)
        .expect("authored projection aliases");
    let portable_ids = OFFLINE_OUTPUTS
        .iter()
        .map(|id| projection.outputs[*id])
        .collect::<HashSet<_>>();
    assert_eq!(portable_ids.len(), OFFLINE_OUTPUTS.len());
    let snapshot = source
        .service
        .export(identity.clone(), &bindings)
        .await
        .expect("first native backup");
    snapshot
        .manifest
        .validate()
        .expect("complete offline graph");
    for projected in [
        &snapshot.manifest.layouts[&layout.id],
        &snapshot.manifest.scenes[&scene_id.0].zones[0].layout,
    ] {
        assert_eq!(projected.zones.len(), outputs.len());
        for (actual, expected) in projected.zones.iter().zip(&outputs) {
            assert_eq!(actual.device, projection.outputs[&expected.device_id]);
            assert!(snapshot.manifest.devices.contains_key(&actual.device));
            assert_eq!(actual.position, expected.position);
            assert_eq!(actual.size, expected.size);
            assert_eq!(actual.rotation, expected.rotation);
            assert_eq!(actual.brightness, expected.brightness);
        }
    }

    let recreated = PortableSetupService::new(Arc::clone(&source.state)).expect("new service");
    let again = recreated
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("durable aliases");
    assert!(again.outputs.is_empty());
    assert!(again.devices.is_empty());
    assert_eq!(
        recreated
            .projection_bindings(identity.setup_id, &again)
            .expect("durable authored aliases")
            .outputs,
        projection.outputs
    );
    assert_eq!(
        recreated
            .export(identity.clone(), &again)
            .await
            .expect("repeat export")
            .manifest,
        snapshot.manifest
    );
    let mut archive = recreated.export_archive().await.expect("file backup");
    let mut bytes = Vec::new();
    archive.read_to_end(&mut bytes).expect("archive bytes");
    for raw in OFFLINE_OUTPUTS {
        assert!(
            !bytes.windows(raw.len()).any(|part| part == raw.as_bytes()),
            "installation-only output address leaked into backup"
        );
    }

    let target = Fixture::new("another-local-input");
    let before = target
        .service
        .export(identity.clone(), &SetupBindings::default())
        .await
        .expect("target baseline");
    let applied = target
        .drive(target.service.restore(
            &snapshot.manifest,
            &SetupBindings::default(),
            options(before.revision),
        ))
        .await
        .expect("offline graph restore");
    assert_eq!(applied.applied.manifest, snapshot.manifest);
    let target_bindings = target
        .service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("restored aliases");
    assert!(target_bindings.devices.is_empty());
    assert!(target.state.device_registry.list().await.is_empty());
    assert_eq!(
        target
            .service
            .export(identity, &target_bindings)
            .await
            .expect("restored offline export")
            .manifest,
        snapshot.manifest
    );
}
