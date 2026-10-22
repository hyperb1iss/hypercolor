//! Zone previews through an output hold.
//!
//! A manual pause or a stop puts one static frame on every output and then
//! pauses the render loop, so nothing renders again until output resumes.
//! A preview following one zone has to take that frame from the hold
//! itself, or it keeps showing the zone's last live picture over dark
//! lights.

use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::Request;
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::output::set_power;
use hypercolor_types::api::output::OutputPowerMode;
use hypercolor_types::canvas::Rgba;
use tower::ServiceExt;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    (Arc::new(state), tempdir)
}

#[tokio::test]
async fn a_manual_pause_shows_every_zone_dark_in_its_preview() {
    let (state, _tmp) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scene/zones")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Side"}"#))
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");
    assert!(
        response.status().is_success(),
        "zone create: {}",
        response.status()
    );

    let mut previews_rx = state.preview_runtime.zone_preview_receiver();
    set_power(&state.domains.output, OutputPowerMode::Paused).await;

    let plan = state.scene_manager.plan_reader().load();
    assert!(plan.zones.len() >= 2, "the scene should hold two zones");
    let previews = previews_rx.borrow_and_update().clone();
    for zone in plan.zones.iter() {
        let preview = previews
            .iter()
            .find(|preview| preview.zone_id == zone.id)
            .unwrap_or_else(|| panic!("zone {} should have a held preview", zone.name));
        assert_eq!(
            preview.frame.surface().get_pixel(0, 0),
            Rgba::new(0, 0, 0, 255),
            "zone {} should preview the paused output's dark frame",
            zone.name
        );
    }
}
