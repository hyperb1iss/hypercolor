use std::sync::Arc;

use axum::extract::State;
use http::StatusCode;
use hypercolor_daemon::api::devices::forget_device;
use hypercolor_daemon::api::extract::ApiJson;
use hypercolor_daemon::app_state::AppState;
use hypercolor_types::api::devices::ForgetDeviceRequest;
use hypercolor_types::api::layouts::{CreateLayoutRequest, UpdateLayoutRequest};
use hypercolor_types::scene::{SceneId, SceneKind};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, StripDirection};

fn output(id: &str, target: &str) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: target.to_owned(),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.2, 0.2),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 8,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
        brightness: None,
    }
}

#[tokio::test]
async fn forgetting_absent_controller_prunes_default_and_saved_scenes_and_layouts() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let state = Arc::new(AppState::new_with_data_dir(dir.path().to_path_buf()));
    let target = "nollie:16d5:1f01:retired";
    let outputs = vec![
        output("old-raw", target),
        output("old-attachment", target),
        output("keep", "other"),
    ];
    let mut layout_ids = Vec::new();
    for name in ["First", "Second"] {
        let layout = state
            .domains
            .layout
            .create(CreateLayoutRequest {
                name: name.to_owned(),
                ..Default::default()
            })
            .await
            .expect("create layout");
        state
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
            .expect("seed layout");
        layout_ids.push(layout.id);
    }
    let mut mutation = state.domains.scene.begin_mutation().await;
    let mut default = mutation
        .scenes()
        .list()
        .into_iter()
        .find(|scene| scene.id.is_default())
        .expect("default scene")
        .clone();
    default.zones[0].layout.zones = outputs;
    mutation
        .update_scene(default.clone())
        .expect("update default");
    default.id = SceneId::new();
    default.kind = SceneKind::Named;
    default.name = "Inactive".to_owned();
    mutation
        .create_scene(default)
        .expect("create inactive scene");
    state
        .domains
        .scene
        .commit(mutation)
        .await
        .expect("seed scenes");
    assert!(state.device_registry.is_empty().await);

    let runtime_path = state.runtime_state_path.clone();
    std::fs::create_dir_all(runtime_path.parent().expect("runtime parent"))
        .expect("runtime directory");
    if runtime_path.exists() {
        std::fs::remove_file(&runtime_path).expect("remove initial runtime snapshot");
    }
    std::fs::create_dir(&runtime_path).expect("block runtime snapshot destination");
    let failed = forget_device(
        State(Arc::clone(&state)),
        ApiJson(ForgetDeviceRequest {
            layout_device_id: target.to_owned(),
        }),
    )
    .await;
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    std::fs::remove_dir(&runtime_path).expect("unblock runtime snapshot destination");

    let response = forget_device(
        State(Arc::clone(&state)),
        ApiJson(ForgetDeviceRequest {
            layout_device_id: target.to_owned(),
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    for scene in state.domains.scene.snapshot().await.list() {
        assert_eq!(scene.zones[0].layout.zones.len(), 1);
        assert_eq!(scene.zones[0].layout.zones[0].id, "keep");
    }
    for id in layout_ids {
        let layout = state
            .domains
            .layout
            .resolve(&id)
            .await
            .expect("saved layout");
        assert_eq!(layout.zones.len(), 1);
        assert_eq!(layout.zones[0].id, "keep");
    }
    for name in ["scenes.json", "layouts.json"] {
        let data = std::fs::read_to_string(dir.path().join(name)).expect("persisted document");
        assert!(!data.contains(target), "{name} retains removed controller");
    }
    let runtime =
        std::fs::read_to_string(&state.runtime_state_path).expect("persisted runtime snapshot");
    assert!(
        !runtime.contains(target),
        "runtime snapshot retains removed controller"
    );
}

/// Device removal is a whole-session save: it checks the scene store file
/// itself, so an outside edit is rewritten even when the scene service's
/// receipts already cover the store and the removal changes no named scene.
#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn forgetting_a_default_scene_device_rewrites_an_outside_store_edit() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let state = Arc::new(AppState::new_with_data_dir(dir.path().to_path_buf()));
    let target = "nollie:16d5:1f01:default-only";
    let mut mutation = state.domains.scene.begin_mutation().await;
    let mut default = mutation
        .scenes()
        .list()
        .into_iter()
        .find(|scene| scene.id.is_default())
        .expect("default scene")
        .clone();
    let mut named = default.clone();
    default.zones[0].layout.zones = vec![output("old", target), output("keep", "other")];
    mutation
        .update_scene(default)
        .expect("bind the device in the default scene");
    named.id = SceneId::new();
    named.kind = SceneKind::Named;
    named.name = "Untouched".to_owned();
    named.zones[0].layout.zones = vec![output("keep", "other")];
    mutation.create_scene(named).expect("create named scene");
    state
        .domains
        .scene
        .commit(mutation)
        .await
        .expect("seed scenes");

    // The commit's own durable write leaves receipts covering the store.
    let path = dir.path().join("scenes.json");
    let durable = std::fs::read(&path).expect("read scene store");
    let modified = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .expect("store mtime");
    let mut edited = durable.clone();
    let digit = edited
        .iter()
        .position(u8::is_ascii_digit)
        .expect("the store holds a digit");
    edited[digit] = if edited[digit] == b'9' {
        b'0'
    } else {
        edited[digit] + 1
    };
    std::fs::write(&path, &edited).expect("edit the store in place");
    std::fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_modified(modified))
        .expect("restore the mtime");
    let writer =
        hypercolor_daemon::persistence::AtomicFileWriter::new(&path).expect("scene store writer");
    writer.set_injected_stale_stamps(true);

    let response = forget_device(
        State(Arc::clone(&state)),
        ApiJson(ForgetDeviceRequest {
            layout_device_id: target.to_owned(),
        }),
    )
    .await;
    writer.set_injected_stale_stamps(false);

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        std::fs::read(&path).expect("read scene store"),
        durable,
        "the removal rewrote the store from memory"
    );
}

#[tokio::test]
async fn forgetting_unknown_or_empty_binding_does_not_mutate_scenes() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let state = Arc::new(AppState::new_with_data_dir(dir.path().to_path_buf()));
    let revision = state.domains.scene.revision();
    for (target, status) in [
        ("", StatusCode::UNPROCESSABLE_ENTITY),
        ("missing", StatusCode::OK),
    ] {
        let response = forget_device(
            State(Arc::clone(&state)),
            ApiJson(ForgetDeviceRequest {
                layout_device_id: target.to_owned(),
            }),
        )
        .await;
        assert_eq!(response.status(), status);
        assert_eq!(state.domains.scene.revision(), revision);
    }
}

#[tokio::test]
async fn forgetting_offline_logical_controller_removes_all_segments_and_profile() {
    use hypercolor_daemon::logical_devices::{LogicalDevice, LogicalDeviceKind};
    use hypercolor_types::attachment::DeviceComponentProfile;
    use hypercolor_types::device::DeviceId;

    let dir = tempfile::tempdir().expect("temporary directory");
    let state = Arc::new(AppState::new_with_data_dir(dir.path().to_path_buf()));
    let physical = DeviceId::new();
    {
        let id = "controller-segment";
        state.logical_devices.write().await.insert(
            id.to_owned(),
            LogicalDevice {
                id: id.to_owned(),
                physical_device_id: physical,
                name: id.to_owned(),
                led_start: 0,
                led_count: 8,
                enabled: true,
                kind: LogicalDeviceKind::Segment,
            },
        );
    }
    {
        let mut logical = state.logical_devices.write().await;
        hypercolor_daemon::logical_devices::ensure_persisted_default(
            &state.logical_devices_path,
            &mut logical,
            physical,
            "controller",
            "Controller",
            8,
        )
        .expect("persist default ownership alongside segment");
        logical.clear();
        *logical = hypercolor_daemon::logical_devices::load_segments(&state.logical_devices_path)
            .expect("restore ownership after restart");
        assert_eq!(logical["controller"].kind, LogicalDeviceKind::Default);
    }
    assert!(state.device_registry.is_empty().await);
    state
        .attachment_profiles
        .write()
        .await
        .update(&physical.to_string(), DeviceComponentProfile::default());
    let response = forget_device(
        State(Arc::clone(&state)),
        ApiJson(ForgetDeviceRequest {
            layout_device_id: "controller".to_owned(),
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(state.logical_devices.read().await.is_empty());
    assert!(
        state
            .attachment_profiles
            .read()
            .await
            .get(&physical.to_string())
            .is_none()
    );
    let profiles = std::fs::read_to_string(dir.path().join("attachment-profiles.json"))
        .expect("saved profiles");
    assert!(!profiles.contains(&physical.to_string()));
    let segments =
        std::fs::read_to_string(dir.path().join("logical-devices.json")).expect("saved segments");
    assert!(!segments.contains("controller"));
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn forgetting_controller_preserves_unsaved_live_layout_outputs() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let state = Arc::new(AppState::new_with_data_dir(dir.path().to_path_buf()));
    let target = "retired";
    let saved = state
        .domains
        .layout
        .create(CreateLayoutRequest {
            name: "Saved".to_owned(),
            ..Default::default()
        })
        .await
        .expect("create saved layout");
    state
        .domains
        .layout
        .update(
            saved.id.clone(),
            UpdateLayoutRequest {
                zones: Some(vec![
                    output("saved-old", target),
                    output("saved-only", "other"),
                ]),
                ..Default::default()
            },
        )
        .await
        .expect("seed saved outputs");
    let mut live = state
        .domains
        .layout
        .resolve(&saved.id)
        .await
        .expect("saved layout");
    live.zones = vec![output("live-old", target), output("live-only", "other")];
    state.spatial_engine.test_fixture().replace(live);
    let removal = forget_device(
        State(Arc::clone(&state)),
        ApiJson(ForgetDeviceRequest {
            layout_device_id: target.to_owned(),
        }),
    );
    tokio::pin!(removal);
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            tokio::select! {
                response = &mut removal => break response,
                () = tokio::task::yield_now() => {
                    state.layout_publication_test_executor().execute_next_layout_publication()
                        .await.expect("publish pruned live layout");
                }
            }
        }
    })
    .await
    .expect("removal finishes");
    assert_eq!(response.status(), StatusCode::OK);
    let live = state.domains.layout.current();
    assert_eq!(live.zones.len(), 1);
    assert_eq!(live.zones[0].id, "live-only");
    let saved = state
        .domains
        .layout
        .resolve(&saved.id)
        .await
        .expect("saved layout");
    assert_eq!(saved.zones.len(), 1);
    assert_eq!(saved.zones[0].id, "saved-only");
}
