use super::*;
use hypercolor_types::asset::AssetId;
use hypercolor_types::layer::LayerSource;
use std::io::Read;

#[tokio::test]
async fn export_never_reads_installed_effect_or_media_payloads() {
    let fixture = Fixture::new("local");
    let before = fixture.empty(Uuid::now_v7()).await;
    let desired = authored(&fixture, &before);
    install_preexisting_resources(&fixture, before, desired.clone()).await;
    let entries = fixture.state.domains.effects.portable_sources().await;
    let entry = entries
        .iter()
        .find(|entry| desired.effects.contains_key(&entry.metadata.id.0))
        .expect("installed effect");
    let huge = std::fs::File::create(
        entry
            .source_path
            .parent()
            .expect("effect root")
            .join("large-local-data.bin"),
    )
    .expect("sparse resource");
    huge.set_len(1024 * 1024 * 1024)
        .expect("large sparse local resource");
    std::fs::remove_file(&entry.source_path).expect("unreadable installed entrypoint");
    let assets = fixture.state.asset_library.read().await;
    let media_before = assets.portable_snapshot();
    let media_path = assets
        .object_path_for_hash(&assets.records()[0].hash_sha256)
        .expect("media path");
    drop(assets);
    std::fs::remove_file(media_path).expect("unreadable local media");

    let exported = fixture.empty(desired.setup_id).await;
    assert!(exported.manifest.effects.is_empty());
    assert!(exported.manifest.assets.is_empty());
    assert_eq!(exported.manifest.scenes, desired.scenes);
    let mut archive = fixture
        .service
        .export_archive()
        .await
        .expect("configuration-only archive");
    let mut bytes = Vec::new();
    archive.read_to_end(&mut bytes).expect("archive bytes");
    let archive = hypercolor_core::portable_setup::archive::read_archive(
        bytes.as_slice(),
        &fixture.service.blobs,
    )
    .expect("no undeclared blobs");
    assert!(archive.setup.effects.is_empty());
    assert!(archive.setup.assets.is_empty());
    assert!(!bytes.windows(b"blobs/".len()).any(|part| part == b"blobs/"));

    let report = fixture
        .drive(fixture.service.restore(
            &exported.manifest,
            &SetupBindings::default(),
            RestoreOptions {
                mode: RestoreMode::Synchronize {
                    previous: Box::new(desired.clone()),
                },
                ..options(exported.revision)
            },
        ))
        .await
        .expect("configuration restore retains local resources");
    assert_eq!(report.applied.manifest.scenes, desired.scenes);
    assert_eq!(
        fixture.state.asset_library.read().await.portable_snapshot(),
        media_before
    );
    assert!(
        fixture
            .state
            .domains
            .effects
            .portable_sources()
            .await
            .iter()
            .any(|entry| desired.effects.contains_key(&entry.metadata.id.0))
    );
}

#[tokio::test]
async fn missing_local_resources_preserve_settings_and_report_replay() {
    let fixture = Fixture::new("local");
    let before = fixture.empty(Uuid::now_v7()).await;
    let mut desired = authored(&fixture, &before);
    let effect = EffectId(*desired.effects.keys().next().expect("custom effect"));
    let media = AssetId(*desired.assets.keys().next().expect("media"));
    let active = desired.selection.active_scene_id.expect("scene");
    let embedded_preset = PresetId::new();
    if let LayerSource::Effect { preset_id, .. } =
        &mut desired.scenes.get_mut(&active.0).expect("scene").zones[0].layers[0].source
    {
        *preset_id = Some(embedded_preset);
    }
    let mut layer = desired.scenes[&active.0].zones[0].layers[0].clone();
    layer.id = SceneLayerId::new();
    layer.source = LayerSource::Media {
        asset_id: media,
        playback: hypercolor_types::layer::MediaPlayback::default(),
    };
    desired.scenes.get_mut(&active.0).expect("scene").zones[0]
        .layers
        .push(layer);
    desired.retain_configuration();
    let request = options(before.revision);
    let report = fixture
        .drive(
            fixture
                .service
                .restore(&desired, &SetupBindings::default(), request.clone()),
        )
        .await
        .expect("missing resources are inactive");
    assert_eq!(report.missing_resources.effects, vec![effect]);
    assert_eq!(report.missing_resources.media, vec![media]);
    assert_eq!(report.missing_resources.presets, vec![embedded_preset]);
    assert_eq!(report.applied.manifest, desired);
    let replay = fixture
        .drive(
            fixture
                .service
                .restore(&desired, &SetupBindings::default(), request),
        )
        .await
        .expect("receipt replay");
    assert_eq!(replay.missing_resources, report.missing_resources);
    assert_eq!(fixture.empty(desired.setup_id).await.manifest, desired);
    assert_eq!(
        fixture.service.missing_resources(&desired).await,
        report.missing_resources
    );

    let mut invalid = desired;
    invalid.presets.insert(
        embedded_preset.0,
        EffectPreset {
            id: embedded_preset,
            name: "Different effect".into(),
            description: None,
            effect_id: EffectId::new(Uuid::now_v7()),
            controls: HashMap::new(),
            tags: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
        },
    );
    assert!(
        invalid
            .validate_configuration_references(
                &hypercolor_types::portable_setup::PortableReferenceCatalog::default()
            )
            .expect_err("known mismatched preset remains invalid")
            .to_string()
            .contains("another effect")
    );
}

// Model a transaction admitted by the resource-inclusive version. New public
// requests cannot install files, while existing durable work must still finish.
async fn install_preexisting_resources(
    fixture: &Fixture,
    before: PortableSetupSnapshot,
    desired: PortableSetupV1,
) {
    let requested = super::super::fingerprint(&desired).expect("request digest");
    fixture
        .drive(Box::pin(fixture.service.apply_candidate(
            before.manifest,
            desired,
            requested,
            SetupBindings::default(),
            options(before.revision),
            &crate::persistence::PersistenceShutdown::default(),
            None,
            None,
        )))
        .await
        .expect("previously admitted resource transaction");
}

#[tokio::test]
async fn old_archives_restore_configuration_without_staging_or_replacing_local_files() {
    use hypercolor_core::portable_setup::archive::write_archive;
    use hypercolor_types::portable_setup::PortableBackupV1;
    use std::io::{Seek, SeekFrom};

    let fixture = Fixture::new("local");
    let before = fixture.empty(Uuid::now_v7()).await;
    let existing = authored(&fixture, &before);
    install_preexisting_resources(&fixture, before, existing.clone()).await;
    let local_favorite = EffectId::new(Uuid::now_v7());
    fixture
        .state
        .library_store()
        .upsert_favorite(local_favorite, 17)
        .await
        .expect("local favorite");
    let media_before = fixture.state.asset_library.read().await.portable_snapshot();
    let effects_before = fixture.state.domains.effects.portable_sources().await;
    let effect_files: Vec<_> = effects_before
        .iter()
        .map(|entry| {
            (
                entry.source_path.clone(),
                std::fs::read(&entry.source_path).expect("installed HTML"),
            )
        })
        .collect();
    let media_files: Vec<_> = {
        let library = fixture.state.asset_library.read().await;
        library
            .records()
            .iter()
            .map(|record| {
                let path = library
                    .object_path_for_hash(&record.hash_sha256)
                    .expect("object");
                let bytes = std::fs::read(&path).expect("local media");
                (path, bytes)
            })
            .collect()
    };
    let source = Fixture::new("source");
    let source_before = source.empty(Uuid::now_v7()).await;
    let mut incoming = authored(&source, &source_before);
    let ignored_blob = source.stage(b"<title>Never installed from backup</title>");
    let effect = incoming
        .effects
        .values_mut()
        .next()
        .expect("incoming effect");
    effect
        .files
        .insert(effect.entrypoint.clone(), ignored_blob.clone());
    assert!(fixture.service.export_blob(&ignored_blob).is_err());
    // Include both a colliding existing identity and unrelated incoming files.
    incoming.effects.extend(existing.effects.clone());
    incoming.assets.extend(existing.assets.clone());
    for descriptor in existing
        .effects
        .values()
        .flat_map(|effect| effect.files.values())
        .chain(existing.assets.values().map(|asset| &asset.content))
    {
        source
            .service
            .stage_blob(
                descriptor,
                fixture
                    .service
                    .export_blob(descriptor)
                    .expect("old blob")
                    .file,
            )
            .expect("source archive payload");
    }
    let backup = PortableBackupV1 {
        schema_version: 1,
        setup: incoming.clone(),
        preferences: None,
        tui_preferences: None,
        favorites: Vec::new(),
    };
    for _ in 0..2 {
        let mut archive = fixture.service.archive_upload_file().expect("upload");
        write_archive(&mut archive, &backup, &source.service.blobs).expect("old full archive");
        archive.seek(SeekFrom::Start(0)).expect("rewind");
        let preview = fixture
            .service
            .preview_archive(archive)
            .await
            .expect("preview");
        assert_eq!((preview.effects, preview.media), (0, 0));
        assert!(
            fixture.service.export_blob(&ignored_blob).is_err(),
            "preview discards payloads"
        );
        let first = fixture
            .drive(fixture.service.apply_archive(preview.operation_id))
            .await
            .expect("configuration apply");
        let replay = fixture
            .drive(fixture.service.apply_archive(preview.operation_id))
            .await
            .expect("same operation replay");
        assert_eq!(first, replay);
        assert!(
            first.missing_resources.effects.contains(&local_favorite),
            "final favorite union stays visible in restore warnings"
        );
        assert!(
            fixture.service.export_blob(&ignored_blob).is_err(),
            "apply never stages payloads"
        );
        assert_eq!(
            fixture.state.asset_library.read().await.portable_snapshot(),
            media_before
        );
        let entries = fixture.state.domains.effects.portable_sources().await;
        assert_eq!(entries.len(), effects_before.len());
        for (path, bytes) in effect_files.iter().chain(&media_files) {
            assert_eq!(&std::fs::read(path).expect("local file retained"), bytes);
        }
    }
}
