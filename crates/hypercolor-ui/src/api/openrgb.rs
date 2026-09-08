//! Guided bridge setup uses host facts published by the daemon.
use super::{ApiResult, client};
use crate::app::WsContext;
use hypercolor_types::api::system::OpenRgbStatus;
use leptos::prelude::*;

pub async fn fetch_openrgb_status() -> ApiResult<OpenRgbStatus> {
    client::fetch_json("/api/v1/system/openrgb").await
}

pub fn status_resource() -> LocalResource<ApiResult<OpenRgbStatus>> {
    let hint = expect_context::<WsContext>().last_device_event;
    let status = super::daemon_resource(fetch_openrgb_status);
    Effect::new(move |_| {
        if hint
            .get()
            .is_some_and(|hint| refresh_status_for_event(&hint.event_type))
        {
            status.refetch();
        }
    });
    status
}

/// Per-device discovery announcements do not change the final coverage snapshot.
/// Connections and output-state changes remain live between discovery passes.
pub fn refresh_status_for_event(event: &str) -> bool {
    matches!(
        event,
        "unclaimed_devices_changed"
            | "config_changed"
            | "device_discovery_completed"
            | "device_connected"
            | "device_disconnected"
            | "device_state_changed"
    )
}
