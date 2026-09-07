use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use hypercolor_types::api::system::{AudioDevicesResponse, SystemResource};

use crate::api::openapi::OperationDoc;
use crate::api::{openapi, system};
use crate::app_state::AppState;
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/system",
            axum::routing::get(system::get_system),
            [OperationDoc::get::<SystemResource>(
                "get_system",
                "system",
                "Get daemon identity and authorized status",
            )],
        ))
        .routes(openapi::documented_route(
            "/system/sensors",
            axum::routing::get(system::get_sensors),
            [
                OperationDoc::get::<hypercolor_types::sensor::SystemSnapshot>(
                    "get_sensors",
                    "system",
                    "List system sensors",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/system/audio-devices",
            axum::routing::get(system::list_audio_devices),
            [OperationDoc::get::<AudioDevicesResponse>(
                "list_audio_devices",
                "system",
                "List audio input devices",
            )],
        ))
}
