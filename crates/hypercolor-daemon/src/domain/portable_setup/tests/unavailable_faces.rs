use super::{Fixture, PortableSetupService, SetupBindings, SetupIdentity, options};
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{BlendMode, SceneLayer, SceneLayerId};
use hypercolor_types::library::{EffectPreset, FavoriteEffect, PresetId};
use hypercolor_types::portable_setup::{
    PortableBackupV1, PortableDevice, PortableDisplayFace, PortableSetupV1,
};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

fn missing_face(manifest: &mut PortableSetupV1, target: Uuid, effect_id: EffectId) {
    manifest.devices.entry(target).or_insert(PortableDevice {
        name: "Unplugged panel".into(),
        portable_key: None,
        driver_id: "test".into(),
        vendor_id: None,
        product_id: None,
        model: None,
        led_count: 0,
        binding_hint: None,
    });
    manifest.display_faces.insert(
        target,
        PortableDisplayFace {
            effect_id,
            controls: HashMap::new(),
            blend_mode: BlendMode::Alpha,
            opacity: 0.7,
        },
    );
}

#[tokio::test]
async fn unavailable_default_face_is_retained_without_native_publication() {
    let fixture = Fixture::new("local");
    let empty = fixture.empty(Uuid::now_v7()).await;
    let mut desired = empty.manifest;
    let target = Uuid::now_v7();
    missing_face(&mut desired, target, EffectId::new(Uuid::now_v7()));
    let report = fixture
        .drive(fixture.service.restore(
            &desired,
            &SetupBindings::default(),
            options(empty.revision),
        ))
        .await
        .expect("unbound face remains authored state");
    assert_eq!(report.unresolved_bindings, vec![target]);
    assert!(
        fixture
            .state
            .domains
            .display
            .preferences()
            .read()
            .await
            .iter()
            .next()
            .is_none()
    );
    let recreated = PortableSetupService::new(Arc::clone(&fixture.state)).expect("service");
    let exported = recreated
        .export(
            SetupIdentity {
                setup_id: desired.setup_id,
                name: desired.name.clone(),
            },
            &SetupBindings::default(),
        )
        .await
        .expect("unavailable authored resource remains exportable");
    assert_eq!(exported.manifest, desired);
    assert_eq!(exported.revision, report.applied.revision);
}

#[tokio::test]
async fn missing_bound_face_and_late_binding_preserve_authored_identity() {
    let fixture = Fixture::new("local");
    let setup_id = Uuid::now_v7();
    let local = super::bindings::register_device(&fixture).await;
    let bindings = fixture
        .service
        .assign_bindings(setup_id, None)
        .await
        .expect("bindings");
    let target = bindings.devices[&local];
    let identity = SetupIdentity {
        setup_id,
        name: "Panel setup".into(),
    };
    let before = fixture
        .service
        .export(identity.clone(), &bindings)
        .await
        .expect("before");
    let mut desired = before.manifest;
    let missing = EffectId::new(Uuid::now_v7());
    missing_face(&mut desired, target, missing);
    let accepted = fixture
        .drive(
            fixture
                .service
                .restore(&desired, &bindings, options(before.revision)),
        )
        .await
        .expect("missing effect is reported");
    assert_eq!(accepted.missing_resources.effects, vec![missing]);
    assert_eq!(
        accepted.applied.manifest.display_faces,
        desired.display_faces
    );
    let tracked = fixture
        .state
        .device_registry
        .get(&local)
        .await
        .expect("device");
    fixture
        .state
        .device_registry
        .set_state(&local, hypercolor_types::device::DeviceState::Reconnecting)
        .await;
    let offline = fixture
        .service
        .assign_bindings(setup_id, Some(&desired))
        .await
        .expect("offline");
    let before = fixture
        .service
        .export(identity.clone(), &offline)
        .await
        .expect("before");
    fixture
        .drive(
            fixture
                .service
                .restore(&desired, &offline, options(before.revision)),
        )
        .await
        .expect("offline restore");
    fixture
        .state
        .device_registry
        .set_state(&local, tracked.state)
        .await;
    let report = fixture
        .drive(fixture.service.reconcile_bindings(identity))
        .await
        .expect("binding succeeds without substituting an effect")
        .expect("reconciled");
    assert_eq!(report.missing_resources.effects, vec![missing]);
    assert_eq!(report.applied.manifest.display_faces, desired.display_faces);
    assert!(report.unresolved_bindings.is_empty());
}

#[tokio::test]
async fn missing_scene_preset_and_favorite_references_are_preserved_and_reported() {
    for kind in ["scene", "preset", "favorite"] {
        let fixture = Fixture::new("local");
        let empty = fixture.empty(Uuid::now_v7()).await;
        let mut desired = empty.manifest;
        let missing = EffectId::new(Uuid::now_v7());
        missing_face(&mut desired, Uuid::now_v7(), missing);
        let mut favorites = Vec::new();
        match kind {
            "scene" => {
                desired.scenes.get_mut(&Uuid::nil()).expect("Default").zones[0].layers =
                    vec![SceneLayer::from_effect(
                        SceneLayerId::new(),
                        missing,
                        HashMap::new(),
                        HashMap::new(),
                        None,
                    )];
            }
            "preset" => {
                let id = PresetId::new();
                desired.presets.insert(
                    id.0,
                    EffectPreset {
                        id,
                        name: "Saved".into(),
                        description: None,
                        effect_id: missing,
                        controls: HashMap::new(),
                        tags: Vec::new(),
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    },
                );
            }
            _ => favorites.push(FavoriteEffect {
                effect_id: missing,
                added_at_ms: 1,
            }),
        }
        let backup = PortableBackupV1 {
            schema_version: 1,
            setup: desired,
            preferences: None,
            tui_preferences: None,
            favorites,
        };
        let report = fixture
            .drive(fixture.service.restore_backup(
                &backup,
                &SetupBindings::default(),
                options(empty.revision),
            ))
            .await
            .expect("external resource is allowed");
        assert_eq!(report.missing_resources.effects, vec![missing], "{kind}");
        assert_eq!(report.applied.manifest, backup.setup);
        assert_eq!(
            fixture.state.library_store().list_favorites().await,
            backup.favorites
        );
    }
}
