use super::{Fixture, PortableSetupService, SetupBindings, options};
use crate::domain::portable_setup::{fingerprint, setup_revision};
use hypercolor_types::portable_setup::PortableSetupV1;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

fn metadata(manifest: &mut PortableSetupV1) {
    let scene = manifest.scenes.values_mut().next().expect("default scene");
    scene.metadata = (0..24)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    scene.activation_brightness = Some(0.37);
}

#[tokio::test]
async fn revisions_survive_hash_map_recreation_and_json_roundtrips() {
    let fixture = Fixture::new("local");
    let mut manifest = fixture.empty(Uuid::now_v7()).await.manifest;
    metadata(&mut manifest);
    let preset = hypercolor_types::library::EffectPreset {
        id: hypercolor_types::library::PresetId::new(),
        name: "Controls".into(),
        description: None,
        effect_id: hypercolor_types::effect::EffectId::new(Uuid::now_v7()),
        controls: (0..24)
            .map(|index| {
                (
                    format!("control-{index}"),
                    hypercolor_types::control::ControlValue::Float(f64::from(index) / 7.0),
                )
            })
            .collect(),
        tags: Vec::new(),
        created_at_ms: 0,
        updated_at_ms: 0,
    };
    manifest.presets.insert(preset.id.0, preset);
    let value = serde_json::json!({"z": 1, "a": 2});
    assert_eq!(
        value
            .as_object()
            .expect("object")
            .keys()
            .next()
            .expect("key"),
        "z",
        "the regression must exercise serde_json/preserve_order"
    );
    let expected = setup_revision(&manifest).expect("revision");
    for _ in 0..32 {
        manifest = serde_json::from_slice(&serde_json::to_vec_pretty(&manifest).expect("encode"))
            .expect("decode with new HashMaps");
        assert_eq!(setup_revision(&manifest).expect("revision"), expected);
    }
    let scene = manifest.scenes.values_mut().next().expect("scene");
    scene.metadata.insert("key-0".into(), "changed".into());
    assert_ne!(
        setup_revision(&manifest).expect("changed revision"),
        expected
    );
}

async fn legacy_fixture() -> (
    Fixture,
    PortableSetupV1,
    crate::domain::portable_setup::RestoreOptions,
) {
    let fixture = Fixture::new("local");
    let before = fixture.empty(Uuid::now_v7()).await;
    let mut manifest = before.manifest;
    metadata(&mut manifest);
    let request = options(before.revision);
    fixture
        .drive(
            fixture
                .service
                .restore(&manifest, &SetupBindings::default(), request.clone()),
        )
        .await
        .expect("initial restore");
    let active_path = fixture.service.root.join("active-projection.json");
    let journal_path = fixture
        .service
        .root
        .join(format!("{}.transaction.json", manifest.setup_id));
    let mut active: Value =
        serde_json::from_slice(&std::fs::read(&active_path).expect("active")).expect("decode");
    let mut journal: Value =
        serde_json::from_slice(&std::fs::read(&journal_path).expect("journal")).expect("decode");
    let old = hypercolor_core::asset::AssetLibrary::hash_bytes(
        &serde_json::to_vec(&serde_json::to_value(&manifest).expect("value"))
            .expect("old hash bytes"),
    );
    assert_ne!(old, setup_revision(&manifest).expect("canonical"));
    active["schema_version"] = 1.into();
    for record in [&mut active, &mut journal] {
        record["receipt"]["applied"]["revision"] = old.clone().into();
        record["receipt"]["requested_revision"] = old.clone().into();
    }
    std::fs::write(
        active_path,
        serde_json::to_vec_pretty(&active).expect("active bytes"),
    )
    .expect("legacy active");
    std::fs::write(
        journal_path,
        serde_json::to_vec_pretty(&journal).expect("journal bytes"),
    )
    .expect("legacy journal");
    (fixture, manifest, request)
}

#[tokio::test]
async fn matching_legacy_commit_migrates_before_successor_restore_and_restart() {
    let (fixture, manifest, request) = legacy_fixture().await;
    fixture
        .service
        .recover()
        .await
        .expect("legacy authority recovery");
    let active: Value = serde_json::from_slice(
        &std::fs::read(fixture.service.root.join("active-projection.json")).expect("active"),
    )
    .expect("decode");
    assert_eq!(active["schema_version"], 2);
    let receipt = fixture
        .service
        .last_restore_receipt(manifest.setup_id)
        .await
        .expect("receipt")
        .expect("commit");
    assert_eq!(receipt.operation_id, request.operation_id);
    assert_eq!(
        receipt.applied.revision,
        setup_revision(&manifest).expect("canonical")
    );
    assert_ne!(
        receipt.requested_revision, receipt.applied.revision,
        "an unverifiable old request digest remains opaque"
    );
    assert!(
        fixture
            .service
            .restore(&manifest, &SetupBindings::default(), request)
            .await
            .is_err(),
        "an old request cannot be guessed from its merged applied result"
    );
    let before = fixture.empty(manifest.setup_id).await;
    let mut next = before.manifest;
    next.scenes
        .values_mut()
        .next()
        .expect("scene")
        .metadata
        .insert("successor".into(), "kept".into());
    let request = options(before.revision);
    let restored = fixture
        .drive(
            fixture
                .service
                .restore(&next, &SetupBindings::default(), request.clone()),
        )
        .await
        .expect("successor");
    let replay = fixture
        .service
        .restore(&next, &SetupBindings::default(), request)
        .await
        .expect("canonical same-operation replay");
    assert_eq!(restored.applied.revision, replay.applied.revision);
    let reopened = PortableSetupService::new(Arc::clone(&fixture.state)).expect("reopen");
    reopened.recover().await.expect("restart recovery");
    assert_eq!(
        reopened
            .read_active_projection()
            .expect("valid active")
            .expect("active")
            .receipt
            .applied
            .manifest,
        next
    );
}

#[tokio::test]
async fn legacy_projection_requires_an_identical_committed_receipt() {
    let (fixture, manifest, _) = legacy_fixture().await;
    let path = fixture
        .service
        .root
        .join(format!("{}.transaction.json", manifest.setup_id));
    let original = std::fs::read(&path).expect("journal");
    std::fs::remove_file(&path).expect("remove fixture journal");
    assert!(fixture.service.read_active_projection().is_err());
    let mut journal: Value = serde_json::from_slice(&original).expect("decode");
    journal["receipt"]["operation_id"] = Uuid::now_v7().to_string().into();
    std::fs::write(&path, serde_json::to_vec(&journal).expect("encode")).expect("write");
    assert!(fixture.service.read_active_projection().is_err());
    let mut journal: Value = serde_json::from_slice(&original).expect("decode");
    journal["receipt"]["applied"]["manifest"]["name"] = "different".into();
    std::fs::write(&path, serde_json::to_vec(&journal).expect("encode")).expect("write");
    assert!(fixture.service.read_active_projection().is_err());
}

#[tokio::test]
async fn canonical_projection_rejects_content_changes_even_with_matching_journal() {
    let (fixture, manifest, _) = legacy_fixture().await;
    fixture.service.recover().await.expect("migration");
    let active_path = fixture.service.root.join("active-projection.json");
    let mut active: Value =
        serde_json::from_slice(&std::fs::read(&active_path).expect("active")).expect("decode");
    active["receipt"]["applied"]["manifest"]["name"] = "tampered".into();
    std::fs::write(&active_path, serde_json::to_vec(&active).expect("encode")).expect("write");
    let path = fixture
        .service
        .root
        .join(format!("{}.transaction.json", manifest.setup_id));
    let mut journal: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("journal")).expect("decode");
    journal["receipt"] = active["receipt"].clone();
    std::fs::write(path, serde_json::to_vec(&journal).expect("encode")).expect("write");
    assert!(fixture.service.read_active_projection().is_err());
    assert_ne!(
        fingerprint(&active["receipt"]["applied"]["manifest"]).expect("digest"),
        active["receipt"]["applied"]["revision"]
    );
}
