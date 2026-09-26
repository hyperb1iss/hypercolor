use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{Path, State};
use hypercolor_daemon::api::devices::get_device;
use hypercolor_daemon::app_state::AppState;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceFamily, DeviceId, DeviceInfo, DeviceOrigin,
    DeviceUserSettings,
};

#[tokio::test]
async fn device_api_reports_observed_name_after_override_is_cleared() {
    let directory = tempfile::tempdir().expect("isolated state directory");
    let state = Arc::new(AppState::new_with_data_dir(directory.path().to_path_buf()));
    let id = state
        .device_registry
        .add(DeviceInfo {
            id: DeviceId::new(),
            name: "Hardware name".to_owned(),
            vendor: "Fixture".to_owned(),
            family: DeviceFamily::new_static("fixture", "Fixture"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("fixture", "fixture", ConnectionType::Network),
            segments: Vec::new(),
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        })
        .await;
    state
        .device_registry
        .update_user_settings(&id, Some("Custom name".to_owned()), None, None, None)
        .await
        .expect("rename");
    for (clear, expected) in [(false, "Custom name"), (true, "Hardware name")] {
        if clear {
            let settings = state
                .device_registry
                .get(&id)
                .await
                .expect("tracked")
                .user_settings;
            state
                .device_registry
                .replace_user_settings(
                    &id,
                    DeviceUserSettings {
                        name: None,
                        ..settings
                    },
                )
                .await
                .expect("clear override");
        }
        let response = get_device(State(Arc::clone(&state)), Path(id.to_string())).await;
        assert_eq!(response.status(), http::StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("response JSON");
        assert_eq!(body["data"]["name"], expected);
    }
}
