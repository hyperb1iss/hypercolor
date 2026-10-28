use super::*;
use hypercolor_core::config::ConfigManager;
use hypercolor_types::api::{
    ApiResponse,
    backups::{BackupApplyResponse, BackupPreview},
};
use hypercolor_types::config::HypercolorConfig;
use std::future::Future;
use std::time::Duration;
use uuid::Uuid;

fn state(directory: &std::path::Path, device: &str, smoothing: f32) -> Arc<AppState> {
    let mut config = HypercolorConfig::default();
    config.audio.device = device.into();
    config.audio.smoothing = smoothing;
    Arc::new(
        crate::app_state::AppStateBuilder::new(directory.join("data"))
            .with_driver_registry(Arc::new(hypercolor_network::DriverModuleRegistry::new()))
            .with_config_manager(Arc::new(ConfigManager::from_config_unchecked(
                directory.join("config.toml"),
                config,
            )))
            .with_library(Arc::new(
                crate::library::JsonLibraryStore::open(directory.join("library.json"))
                    .expect("library fixture"),
            ))
            .build(),
    )
}

async fn drive<T>(state: &AppState, future: impl Future<Output = T>) -> T {
    tokio::pin!(future);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            tokio::select! {
                result = &mut future => return result,
                () = tokio::task::yield_now() => {
                    state.layout_publication_test_executor().execute_next_layout_publication().await.expect("renderer fixture");
                }
            }
        }
    }).await.expect("restore should complete")
}

async fn json<T: serde::de::DeserializeOwned>(response: Response) -> T {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response bytes");
    serde_json::from_slice::<ApiResponse<T>>(&bytes)
        .expect("response envelope")
        .data
}

#[tokio::test]
async fn backup_routes_export_preview_restore_and_replay_without_source_credentials() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_test_writer()
        .try_init();
    let source_dir = tempfile::tempdir().expect("source directory");
    let target_dir = tempfile::tempdir().expect("target directory");
    let source = state(source_dir.path(), "source-local-device", 0.31);
    let target = state(target_dir.path(), "target-local-device", 0.85);
    source
        .tui_preferences
        .modify(|prefs| {
            prefs.theme = Some("silkcircuit-neon".into());
            prefs.show_donate = false;
        })
        .expect("source terminal preferences");
    let effect_path = source.data_dir.join("effects/user/favorite.html");
    std::fs::create_dir_all(effect_path.parent().expect("effect directory")).expect("directory");
    std::fs::write(&effect_path, "<title>Favorite effect</title>").expect("effect source");
    let entry = hypercolor_core::effect::load_html_effect_file(&effect_path)
        .expect("load effect")
        .expect("runnable HTML");
    let effect = entry.metadata.id;
    source.domains.effects.register(entry).await;
    source
        .library_store()
        .upsert_favorite(effect, 17)
        .await
        .expect("favorite");
    let exported = export_archive(State(source)).await;
    assert_eq!(exported.status(), 200);
    assert_eq!(
        exported.headers()[header::CONTENT_TYPE],
        "application/x-tar"
    );
    let archive = axum::body::to_bytes(exported.into_body(), 1024 * 1024)
        .await
        .expect("archive bytes");
    assert!(
        !archive
            .windows(b"source-local-device".len())
            .any(|part| part == b"source-local-device")
    );
    let preview_response = preview_archive(State(Arc::clone(&target)), Body::from(archive)).await;
    assert_eq!(preview_response.status(), 200);
    let preview: BackupPreview = json(preview_response).await;
    assert_eq!(preview.favorites, 1);
    assert!(target.library_store().list_favorites().await.is_empty());
    for attempt in 1..=2 {
        let response = drive(
            &target,
            apply_archive(
                State(Arc::clone(&target)),
                ApiJson(BackupApplyRequest {
                    operation_id: preview.operation_id,
                }),
            ),
        )
        .await;
        let status = response.status();
        if status != 200 {
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("failed apply response bytes");
            panic!(
                "backup apply attempt {attempt} returned {status}: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
        let result: BackupApplyResponse = json(response).await;
        assert_eq!(result.operation_id, preview.operation_id);
    }
    let favorites = target.library_store().project_favorites().await;
    assert_eq!(favorites.len(), 1);
    assert_eq!(favorites[0].effect_id, effect);
    assert_eq!(favorites[0].revision, 1);
    let config = target
        .config_manager
        .as_ref()
        .expect("config fixture")
        .get();
    assert_eq!(config.audio.device, "target-local-device");
    assert_eq!(config.audio.smoothing.to_bits(), 0.31_f32.to_bits());
    let terminal = target
        .tui_preferences
        .load()
        .expect("restored terminal preferences");
    assert_eq!(terminal.theme.as_deref(), Some("silkcircuit-neon"));
    assert!(!terminal.show_donate);
}

#[tokio::test]
async fn invalid_archive_and_stale_preview_do_not_apply_any_favorites() {
    let directory = tempfile::tempdir().expect("directory");
    let state = state(directory.path(), "local", 0.8);
    let invalid = preview_archive(State(Arc::clone(&state)), Body::from("not an archive")).await;
    assert!(invalid.status().is_client_error());
    let exported = export_archive(State(Arc::clone(&state))).await;
    let bytes = axum::body::to_bytes(exported.into_body(), 1024 * 1024)
        .await
        .expect("archive bytes");
    let preview: BackupPreview =
        json(preview_archive(State(Arc::clone(&state)), Body::from(bytes)).await).await;
    state.output_power.restore_manual_pause([0, 0, 0]).await;
    let response = apply_archive(
        State(Arc::clone(&state)),
        ApiJson(BackupApplyRequest {
            operation_id: preview.operation_id,
        }),
    )
    .await;
    assert_eq!(response.status(), 409);
    assert!(state.library_store().list_favorites().await.is_empty());
}

#[tokio::test]
async fn imported_unavailable_display_survives_the_next_manual_backup() {
    use hypercolor_core::portable_setup::{
        SetupBlobStore,
        archive::{read_archive, write_archive},
    };
    use hypercolor_types::portable_setup::{PortableDevice, PortableDisplayTarget};
    use hypercolor_types::scene::{SceneKind, ZoneId, ZoneRole};
    use std::io::{Seek, SeekFrom};

    let directory = tempfile::tempdir().expect("directory");
    let state = state(directory.path(), "local", 0.8);
    let service = PortableSetupService::new(Arc::clone(&state)).expect("service");
    let blobs = SetupBlobStore::open(directory.path().join("archive-blobs")).expect("blob store");
    let mut backup =
        read_archive(service.export_archive().await.expect("export"), &blobs).expect("archive");
    let source_identity = Uuid::now_v7();
    backup.setup.setup_id = source_identity;
    let display = Uuid::now_v7();
    backup.setup.devices.insert(
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
    let mut scene = backup.setup.scenes[&Uuid::nil()].clone();
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
    backup.setup.scenes.insert(scene_id, scene);
    let mut archive = service.archive_upload_file().expect("spool");
    write_archive(&mut archive, &backup, &blobs).expect("write archive");
    archive.seek(SeekFrom::Start(0)).expect("rewind");
    let preview = service.preview_archive(archive).await.expect("preview");
    drive(&state, service.apply_archive(preview.operation_id))
        .await
        .expect("restore");
    let recreated = PortableSetupService::new(Arc::clone(&state)).expect("recreated service");
    let exported = read_archive(
        recreated.export_archive().await.expect("export again"),
        &blobs,
    )
    .expect("archive again");
    assert_ne!(exported.setup.setup_id, source_identity);
    assert_eq!(
        exported.setup.devices[&display],
        backup.setup.devices[&display]
    );
    assert!(exported.setup.scenes[&scene_id].zones.contains(&zone));
}

#[tokio::test]
async fn preference_only_change_after_preview_requires_fresh_confirmation() {
    let directory = tempfile::tempdir().expect("directory");
    let state = state(directory.path(), "local", 0.8);
    let service = PortableSetupService::new(Arc::clone(&state)).expect("service");
    let preview = service
        .preview_archive(service.export_archive().await.expect("export"))
        .await
        .expect("preview");
    let config = state.config_manager.as_ref().expect("config");
    config.modify(|config| config.audio.smoothing = 0.42);
    let response = drive(
        &state,
        apply_archive(
            State(Arc::clone(&state)),
            ApiJson(BackupApplyRequest {
                operation_id: preview.operation_id,
            }),
        ),
    )
    .await;
    assert_eq!(response.status(), 409);
    assert_eq!(config.get().audio.smoothing.to_bits(), 0.42_f32.to_bits());
}

#[tokio::test]
async fn terminal_preference_change_after_preview_requires_fresh_confirmation() {
    let directory = tempfile::tempdir().expect("directory");
    let state = state(directory.path(), "local", 0.8);
    let service = PortableSetupService::new(Arc::clone(&state)).expect("service");
    let preview = service
        .preview_archive(service.export_archive().await.expect("export"))
        .await
        .expect("preview");
    state
        .tui_preferences
        .modify(|prefs| prefs.theme = Some("new local theme".into()))
        .expect("local edit");
    let response = drive(
        &state,
        apply_archive(
            State(Arc::clone(&state)),
            ApiJson(BackupApplyRequest {
                operation_id: preview.operation_id,
            }),
        ),
    )
    .await;
    assert_eq!(response.status(), 409);
    assert_eq!(
        state
            .tui_preferences
            .load()
            .expect("terminal preferences")
            .theme
            .as_deref(),
        Some("new local theme")
    );
}
