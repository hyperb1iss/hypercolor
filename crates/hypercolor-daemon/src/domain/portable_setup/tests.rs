use super::{
    PortableSetupService, PortableSetupSnapshot, RestoreMode, RestoreOptions, SetupIdentity,
    journal,
};
use crate::app_state::AppState;
use hypercolor_types::portable_setup::{PortableSetupV1, SetupBindings};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

mod authority;
mod bindings;
mod cross_authority;
mod offline_native;
mod output_reconnect;
mod recovery;
mod runtime_preferences;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::config::ConfigManager;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::library::{EffectPlaylist, EffectPreset, PlaylistId, PresetId};
use hypercolor_types::portable_setup::{PortableAsset, PortableEffect};
use hypercolor_types::scene::SceneId;
use image::{ImageBuffer, ImageFormat, Rgba};

struct Fixture {
    directory: tempfile::TempDir,
    state: Arc<AppState>,
    service: PortableSetupService,
    config: Arc<ConfigManager>,
}

impl Fixture {
    fn new(audio_device: &str) -> Self {
        Self::with_driver_registry(
            audio_device,
            hypercolor_network::DriverModuleRegistry::new(),
        )
    }

    fn with_driver_registry(
        audio_device: &str,
        registry: hypercolor_network::DriverModuleRegistry,
    ) -> Self {
        let directory = tempfile::tempdir().expect("portable fixture should be valid");
        let mut config = HypercolorConfig::default();
        config.audio.device = audio_device.into();
        config.daemon.start_scene = "last".into();
        let config = Arc::new(ConfigManager::from_config_unchecked(
            directory.path().join("config.toml"),
            config,
        ));
        let state = Arc::new(
            crate::app_state::AppStateBuilder::new(directory.path().join("data"))
                .with_config_manager(Arc::clone(&config))
                .with_driver_registry(Arc::new(registry))
                .with_library(Arc::new(
                    crate::library::JsonLibraryStore::open(
                        directory.path().join("data/library.json"),
                    )
                    .expect("portable fixture should be valid"),
                ))
                .build(),
        );
        let service = PortableSetupService::new(Arc::clone(&state))
            .expect("portable fixture should be valid");
        Self {
            directory,
            state,
            service,
            config,
        }
    }

    async fn empty(&self, id: Uuid) -> PortableSetupSnapshot {
        self.service
            .export(
                SetupIdentity {
                    setup_id: id,
                    name: "My lighting".into(),
                },
                &SetupBindings::default(),
            )
            .await
            .expect("portable fixture should be valid")
    }

    async fn drive<T>(&self, workflow: impl Future<Output = T>) -> T {
        tokio::pin!(workflow);
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                tokio::select! {
                    result = &mut workflow => return result,
                    () = tokio::task::yield_now() => {
                        self.state.layout_publication_test_executor().execute_next_layout_publication().await.expect("portable fixture should be valid");
                    }
                }
            }
        }).await.expect("restore must reach its real receipt")
    }

    fn stage(&self, bytes: &[u8]) -> hypercolor_types::portable_setup::PortableBlob {
        let blob = hypercolor_types::portable_setup::PortableBlob {
            sha256: AssetLibrary::hash_bytes(bytes),
            byte_len: bytes.len() as u64,
        };
        self.service
            .stage_blob(&blob, bytes)
            .expect("portable fixture should be valid");
        blob
    }
}

fn options(revision: String) -> RestoreOptions {
    RestoreOptions {
        operation_id: Uuid::now_v7(),
        expected_revision: revision,
        mode: RestoreMode::Merge,
        apply_selection: true,
    }
}

fn authored(fixture: &Fixture, empty: &PortableSetupSnapshot) -> PortableSetupV1 {
    let mut setup = empty.manifest.clone();
    setup.output.canvas_width = 800;
    setup.output.canvas_height = 600;
    setup.output.brightness = 0.42;
    setup.output.paused = true;
    let effect_id = Uuid::now_v7();
    let html =
        fixture.stage(b"<title>Portable light</title><script src='../shared/effect.js'></script>");
    let script = fixture.stage(b"const color = '#8844ff';");
    setup.effects.insert(
        effect_id,
        PortableEffect {
            name: "Portable light".into(),
            entrypoint: "content/main.html".into(),
            files: BTreeMap::from([
                ("content/main.html".into(), html),
                ("shared/effect.js".into(), script),
            ]),
        },
    );
    let mut png = Cursor::new(Vec::new());
    ImageBuffer::from_pixel(2, 2, Rgba([80_u8, 30, 100, 255]))
        .write_to(&mut png, ImageFormat::Png)
        .expect("portable fixture should be valid");
    let image = PortableAsset {
        name: "Purple image".into(),
        mime_type: "image/png".into(),
        tags: vec!["personal".into()],
        content: fixture.stage(png.get_ref()),
    };
    setup.assets.insert(Uuid::now_v7(), image.clone());
    setup.assets.insert(Uuid::now_v7(), image);
    let preset = EffectPreset {
        id: PresetId::new(),
        name: "Evening".into(),
        description: None,
        effect_id: EffectId(effect_id),
        controls: HashMap::new(),
        tags: vec![],
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    setup.presets.insert(preset.id.0, preset);
    let playlist = EffectPlaylist {
        id: PlaylistId::new(),
        name: "Night".into(),
        description: None,
        items: vec![],
        loop_enabled: true,
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    setup.playlists.insert(playlist.id.0, playlist);
    let id = SceneId::new();
    let mut scene = setup.scenes[&SceneId::DEFAULT.0].clone();
    scene.name = "My evening".into();
    scene.kind = hypercolor_types::scene::SceneKind::Named;
    scene.mutation_mode = hypercolor_types::scene::SceneMutationMode::Snapshot;
    scene.zones[0].layers = vec![SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId(effect_id),
        HashMap::new(),
        HashMap::new(),
        None,
    )];
    setup.scenes.insert(id.0, scene);
    setup.selection.active_scene_id = Some(id);
    setup.selection.default_scene_id = Some(id);
    setup
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_install_round_trip_preserves_identity_resources_and_local_endpoints() {
    let source = Fixture::new("linux-local-input");
    let identity = Uuid::now_v7();
    let empty = source.empty(identity).await;
    let expected = authored(&source, &empty);
    let report = source
        .drive(source.service.restore(
            &expected,
            &SetupBindings::default(),
            options(empty.revision),
        ))
        .await
        .expect("portable fixture should be valid");
    assert_eq!(report.applied.manifest, expected);
    let exported = source
        .service
        .export(
            SetupIdentity {
                setup_id: identity,
                name: expected.name.clone(),
            },
            &SetupBindings::default(),
        )
        .await
        .expect("portable fixture should be valid");
    assert_eq!(exported.manifest, expected);
    let target = Fixture::new("windows-local-input");
    for blob in expected.assets.values().map(|asset| &asset.content).chain(
        expected
            .effects
            .values()
            .flat_map(|effect| effect.files.values()),
    ) {
        target
            .service
            .stage_blob(
                blob,
                source
                    .service
                    .export_blob(blob)
                    .expect("portable fixture should be valid")
                    .file,
            )
            .expect("portable fixture should be valid");
    }
    let empty = target.empty(identity).await;
    let restored = target
        .drive(target.service.restore(
            &exported.manifest,
            &SetupBindings::default(),
            options(empty.revision),
        ))
        .await
        .expect("portable fixture should be valid");
    assert_eq!(restored.applied.manifest, expected);
    assert_eq!(target.config.get().audio.device, "windows-local-input");
    assert_eq!(source.config.get().audio.device, "linux-local-input");
    assert_eq!(target.config.get().daemon.canvas_width, 800);
    assert_eq!(target.state.domains.layout.current().canvas_width, 800);
    assert_eq!(target.state.output_power.global_brightness(), 0.42);
    assert!(target.state.output_power.snapshot().manually_paused());
    assert_eq!(
        target
            .state
            .scene_manager
            .snapshot()
            .await
            .active_scene_id(),
        expected.selection.active_scene_id.as_ref()
    );
    let output = target
        .service
        .export(
            SetupIdentity {
                setup_id: identity,
                name: expected.name.clone(),
            },
            &SetupBindings::default(),
        )
        .await
        .expect("portable fixture should be valid");
    assert_eq!(output.manifest, expected);
    let reopened = AssetLibrary::open(target.directory.path().join("data/assets"))
        .expect("portable fixture should be valid");
    assert_eq!(reopened.records().len(), 2);
    assert_eq!(
        reopened.records()[0].hash_sha256,
        reopened.records()[1].hash_sha256
    );
}

#[tokio::test]
async fn stale_revision_and_invalid_graph_make_no_restore_intent() {
    let fixture = Fixture::new("local");
    let id = Uuid::now_v7();
    let empty = fixture.empty(id).await;
    let mut request = options("stale".into());
    assert!(
        fixture
            .service
            .restore(&empty.manifest, &SetupBindings::default(), request.clone())
            .await
            .is_err()
    );
    assert!(
        fixture
            .service
            .read_journal(id)
            .expect("portable fixture should be valid")
            .is_none()
    );
    let mut broken = empty.manifest;
    broken.selection.active_scene_id = Some(SceneId::new());
    request.expected_revision = empty.revision;
    assert!(
        fixture
            .service
            .restore(&broken, &SetupBindings::default(), request)
            .await
            .is_err()
    );
    assert!(
        fixture
            .service
            .read_journal(id)
            .expect("portable fixture should be valid")
            .is_none()
    );
}

#[tokio::test]
async fn no_active_scene_still_publishes_the_configured_canvas() {
    let fixture = Fixture::new("local");
    let id = Uuid::now_v7();
    let empty = fixture.empty(id).await;
    let mut desired = empty.manifest.clone();
    desired.selection.active_scene_id = None;
    desired.output.canvas_width = 960;
    desired.output.canvas_height = 540;
    fixture
        .drive(fixture.service.restore(
            &desired,
            &SetupBindings::default(),
            options(empty.revision),
        ))
        .await
        .expect("canvas restore succeeds without an active scene");
    let layout = fixture.state.domains.layout.current();
    assert_eq!((layout.canvas_width, layout.canvas_height), (960, 540));
    assert_eq!(fixture.config.get().daemon.canvas_width, 960);
    assert_eq!(fixture.empty(id).await.manifest, desired);
}

#[tokio::test]
async fn manual_backup_restores_preferences_and_favorites_under_one_receipt() {
    use hypercolor_types::library::FavoriteEffect;
    use hypercolor_types::portable_setup::{PortableBackupV1, PortablePreferences};

    let fixture = Fixture::new("keep-this-local-endpoint");
    let id = Uuid::now_v7();
    let empty = fixture.empty(id).await;
    let mut portable_config = fixture.config.get().as_ref().clone();
    portable_config.audio.smoothing = 0.61;
    portable_config.audio.device = "do-not-copy-this-endpoint".into();
    let favorite = FavoriteEffect {
        effect_id: EffectId::new(Uuid::now_v7()),
        added_at_ms: 123,
    };
    let backup = PortableBackupV1 {
        schema_version: 1,
        setup: empty.manifest,
        preferences: Some(PortablePreferences::from_config(&portable_config)),
        tui_preferences: None,
        favorites: vec![favorite.clone()],
    };
    let request = options(empty.revision);
    fixture
        .drive(
            fixture
                .service
                .restore_backup(&backup, &SetupBindings::default(), request.clone()),
        )
        .await
        .expect("manual archive restores all participants");
    assert_eq!(fixture.config.get().audio.smoothing, 0.61);
    assert_eq!(
        fixture.config.get().audio.device,
        "keep-this-local-endpoint"
    );
    assert_eq!(
        fixture.state.library_store().list_favorites().await,
        vec![favorite]
    );
    fixture
        .service
        .restore_backup(&backup, &SetupBindings::default(), request.clone())
        .await
        .expect("same operation returns durable receipt");
    let mut changed = backup;
    changed.favorites.clear();
    assert!(
        fixture
            .service
            .restore_backup(&changed, &SetupBindings::default(), request)
            .await
            .is_err(),
        "same operation cannot silently restore different ancillary choices"
    );
}

#[tokio::test]
async fn adapters_for_one_state_share_restore_and_shutdown_authority() {
    let fixture = Fixture::new("local");
    let other = PortableSetupService::new(Arc::clone(&fixture.state)).expect("second adapter");
    assert!(Arc::ptr_eq(
        &fixture.service.restore_runtime,
        &other.restore_runtime
    ));
    assert!(Arc::ptr_eq(
        &fixture.service.association_gate,
        &other.association_gate
    ));
    fixture
        .service
        .shutdown_when_writers_closed()
        .await
        .expect("shutdown closes authority");
    assert!(
        other.recover().await.is_err(),
        "a new adapter cannot bypass closed writers"
    );
}

#[tokio::test]
async fn held_power_transition_does_not_invert_runtime_snapshot_admission() {
    use std::task::{Context, Poll, Waker};

    let fixture = Fixture::new("local");
    let empty = fixture.empty(Uuid::now_v7()).await;
    let power = fixture.state.output_power.transition().await;
    let shutdown = crate::persistence::PersistenceShutdown::default();
    let mut attempt = Box::pin(fixture.service.apply_candidate(
        empty.manifest.clone(),
        empty.manifest,
        empty.revision.clone(),
        SetupBindings::default(),
        options(empty.revision),
        &shutdown,
        None,
        None,
    ));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(attempt.as_mut().poll(&mut context), Poll::Pending));
    fixture
        .state
        .layout_publication_test_executor()
        .acknowledge_portable_restore();
    assert!(matches!(attempt.as_mut().poll(&mut context), Poll::Pending));
    let mut runtime = Box::pin(
        fixture
            .state
            .domains
            .runtime_session
            .begin_portable_restore(),
    );
    let Poll::Ready(runtime_guard) = runtime.as_mut().poll(&mut context) else {
        panic!("ordinary power owner must still be able to persist its runtime snapshot");
    };
    drop(runtime_guard);
    drop(runtime);
    drop(power);
    fixture
        .drive(attempt)
        .await
        .expect("restore proceeds after ordinary power change");
}

#[tokio::test]
async fn missing_display_is_inert_and_survives_export_and_service_recreation() {
    use hypercolor_types::portable_setup::{PortableDevice, PortableDisplayTarget};
    use hypercolor_types::scene::{SceneKind, ZoneId, ZoneRole};

    let fixture = Fixture::new("local");
    let id = Uuid::now_v7();
    let empty = fixture.empty(id).await;
    let mut desired = empty.manifest.clone();
    let display = Uuid::now_v7();
    desired.devices.insert(
        display,
        PortableDevice {
            name: "Offline panel".into(),
            portable_key: None,
            driver_id: "test-display".into(),
            vendor_id: None,
            product_id: None,
            model: None,
            led_count: 64,
            binding_hint: None,
        },
    );
    let scene_id = Uuid::now_v7();
    let mut scene = desired.scenes[&Uuid::nil()].clone();
    scene.kind = SceneKind::Named;
    scene.name = "Panel scene".into();
    let mut zone = scene.zones[0].clone();
    zone.id = ZoneId::new();
    zone.name = "Offline display".into();
    zone.role = ZoneRole::Display;
    zone.display_target = Some(PortableDisplayTarget {
        device: display,
        blend_mode: hypercolor_types::layer::BlendMode::Alpha,
        opacity: 1.0,
    });
    scene.zones.push(zone.clone());
    desired.scenes.insert(scene_id, scene);
    desired.selection.active_scene_id = Some(SceneId(scene_id));
    let report = fixture
        .drive(fixture.service.restore(
            &desired,
            &SetupBindings::default(),
            options(empty.revision),
        ))
        .await
        .expect("unavailable display does not block authored restoration");
    assert_eq!(report.unresolved_bindings, vec![display]);
    let native = fixture.state.scene_manager.snapshot().await;
    assert!(
        !native
            .active_scene()
            .expect("selected scene")
            .zones
            .iter()
            .any(|value| value.id == zone.id),
        "unknown hardware must not receive a guessed native target"
    );
    let recreated =
        PortableSetupService::new(Arc::clone(&fixture.state)).expect("recreated adapter");
    let exported = recreated
        .export(
            SetupIdentity {
                setup_id: id,
                name: desired.name.clone(),
            },
            &SetupBindings::default(),
        )
        .await
        .expect("offline definitions remain exportable");
    assert_eq!(exported.manifest, desired);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn caller_cancellation_retains_writers_until_shutdown_and_restart_recovers() {
    let fixture = Fixture::new("machine-local");
    let id = Uuid::now_v7();
    let local_device = bindings::register_device(&fixture).await;
    let bindings = fixture
        .service
        .assign_bindings(id, None)
        .await
        .expect("initial hardware association");
    let portable_device = bindings.devices[&local_device];
    let empty = fixture
        .service
        .export(
            SetupIdentity {
                setup_id: id,
                name: "My lighting".into(),
            },
            &bindings,
        )
        .await
        .expect("initial connected setup");
    let mut expected = authored(&fixture, &empty);
    let offline_display = Uuid::now_v7();
    expected.devices.insert(
        offline_display,
        hypercolor_types::portable_setup::PortableDevice {
            name: "Offline display during restore".into(),
            portable_key: None,
            driver_id: "test-display".into(),
            vendor_id: None,
            product_id: None,
            model: None,
            led_count: 64,
            binding_hint: None,
        },
    );
    let scene_id = expected
        .selection
        .active_scene_id
        .expect("authored selected scene")
        .0;
    let scene = expected.scenes.get_mut(&scene_id).expect("authored scene");
    let mut display_zone = scene.zones[0].clone();
    display_zone.id = hypercolor_types::scene::ZoneId::new();
    display_zone.role = hypercolor_types::scene::ZoneRole::Display;
    display_zone.display_target = Some(hypercolor_types::portable_setup::PortableDisplayTarget {
        device: offline_display,
        blend_mode: hypercolor_types::layer::BlendMode::Alpha,
        opacity: 1.0,
    });
    scene.zones.push(display_zone);
    let request = options(empty.revision);
    let operation_id = request.operation_id;
    let writer =
        crate::persistence::AtomicFileWriter::new(&fixture.state.data_dir.join("scenes.json"))
            .expect("portable fixture should be valid");
    writer.set_injected_commit_failures(usize::MAX);
    let service = fixture.service.clone();
    let incoming = expected.clone();
    let caller = tokio::spawn(async move { service.restore(&incoming, &bindings, request).await });
    fixture
        .drive(async {
            while writer.pending_failure().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await;
    assert!(matches!(
        fixture
            .service
            .read_journal(id)
            .expect("portable fixture should be valid"),
        Some(journal::RestoreJournal::Pending { .. })
    ));
    caller.abort();
    assert!(caller.await.expect_err("cancelled caller").is_cancelled());
    let blocked = tokio::time::timeout(
        Duration::from_millis(20),
        fixture.state.scene_manager.snapshot(),
    )
    .await;
    assert!(
        blocked.is_err(),
        "caller cancellation must retain the ordinary scene writer"
    );
    fixture
        .state
        .layout_publication_test_executor()
        .close_renderer();
    assert_eq!(
        fixture
            .service
            .shutdown_when_writers_closed()
            .await
            .expect("portable fixture should be valid"),
        crate::extensions::ShutdownPersistenceDisposition::PreservePendingTransaction
    );
    writer.set_injected_commit_failures(0);
    writer
        .flush(Duration::from_secs(3))
        .expect("portable fixture should be valid");
    let state = Arc::new(
        crate::app_state::AppStateBuilder::new(fixture.state.data_dir.clone())
            .with_state_dir(fixture.state.state_dir.clone())
            .with_config_manager(Arc::clone(&fixture.config))
            .with_library(Arc::new(
                crate::library::JsonLibraryStore::open(fixture.state.data_dir.join("library.json"))
                    .expect("portable fixture should be valid"),
            ))
            .build(),
    );
    let service =
        PortableSetupService::new(Arc::clone(&state)).expect("portable fixture should be valid");
    let original_config = fixture.config.get().as_ref().clone();
    let mut externally_edited = original_config.clone();
    externally_edited.daemon.canvas_width = 777;
    fixture.config.update(externally_edited);
    assert!(
        service.recover().await.is_err(),
        "recovery must reject a configured canvas outside its before/candidate proof"
    );
    assert_eq!(fixture.config.get().daemon.canvas_width, 777);
    fixture.config.update(original_config);
    let recovered=tokio::time::timeout(Duration::from_secs(15), async {
        let future=service.recover();
        tokio::pin!(future);
        loop {
            tokio::select! {
                result=&mut future => break result,
                ()=tokio::task::yield_now() => { state.layout_publication_test_executor().execute_next_layout_publication().await.expect("portable fixture should be valid"); }
            }
        }
    }).await.expect("portable fixture should be valid");
    recovered.expect("portable fixture should be valid");
    let receipt = service
        .last_restore_receipt(id)
        .await
        .expect("portable fixture should be valid")
        .expect("portable fixture should be valid");
    assert_eq!(receipt.operation_id, operation_id);
    assert_eq!(receipt.applied.manifest, expected);
    assert_eq!(
        receipt
            .unresolved_bindings
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from([portable_device, offline_display]),
        "recovery must not reuse an absent native device ID"
    );
    assert!(receipt.materialized_bindings.devices.is_empty());
    assert!(matches!(
        service
            .read_journal(id)
            .expect("portable fixture should be valid"),
        Some(journal::RestoreJournal::Committed {
            finalized: true,
            ..
        })
    ));
    assert_eq!(fixture.config.get().audio.device, "machine-local");
}
