use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, devices, openapi};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/devices",
            axum::routing::get(devices::list_devices),
            [OperationDoc::get_list::<hypercolor_types::api::devices::DeviceSummary>(
                "list_devices",
                "devices",
                "List tracked devices",
            )
            .query::<hypercolor_types::api::devices::ListDevicesQuery>()],
        ))
        .routes(openapi::documented_route(
            "/devices/discover",
            axum::routing::post(devices::discover_devices),
            [OperationDoc::post::<hypercolor_types::api::devices::DiscoverResponse>(
                "discover_devices",
                "devices",
                "Start device discovery",
            ).optional_body::<hypercolor_types::api::devices::DiscoverRequest>().also_status("202")],
        ))
        .routes(openapi::documented_route(
            "/devices/{id}",
            axum::routing::get(devices::get_device)
                .put(devices::update_device)
                .delete(devices::delete_device),
            [
                OperationDoc::get::<hypercolor_types::api::devices::DeviceSummary>("get_device", "devices", "Get one device"),
                OperationDoc::put::<hypercolor_types::api::devices::DeviceSummary>(
                    "update_device",
                    "devices",
                    "Update one device",
                ).body::<devices::UpdateDeviceRequest>(),
                OperationDoc::delete::<hypercolor_types::api::devices::DeleteDeviceResponse>(
                    "delete_device",
                    "devices",
                    "Delete one device",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/devices/{id}/attachments",
            axum::routing::get(devices::get_attachments)
                .put(devices::update_attachments)
                .delete(devices::delete_attachments),
            [
                OperationDoc::get::<hypercolor_types::api::devices::DeviceComponentsResponse>(
                    "get_attachments",
                    "devices",
                    "Get device attachments",
                ),
                OperationDoc::put::<hypercolor_types::api::devices::DeviceComponentsUpdateResponse>(
                    "update_attachments",
                    "devices",
                    "Update device attachments",
                ).body::<hypercolor_types::api::devices::UpdateAttachmentsRequest>(),
                OperationDoc::delete::<hypercolor_types::api::devices::DeleteAttachmentsResponse>(
                    "delete_attachments",
                    "devices",
                    "Delete device attachments",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/devices/{id}/identify",
            axum::routing::post(devices::identify_device),
            [OperationDoc::post::<hypercolor_types::api::devices::IdentifyDeviceResponse>(
                "identify_device",
                "devices",
                "Identify one device",
            ).optional_body::<devices::IdentifyRequest>()],
        ))
        .routes(openapi::documented_route(
            "/devices/{id}/segments/{segment}/identify",
            axum::routing::post(devices::identify_segment),
            [OperationDoc::post::<hypercolor_types::api::devices::IdentifySegmentResponse>(
                "identify_segment",
                "devices",
                "Identify one device segment",
            ).optional_body::<devices::IdentifyRequest>()],
        ))
        .routes(openapi::documented_route(
            "/devices/{id}/attachments/{slot}/identify",
            axum::routing::post(devices::identify_attachment),
            [OperationDoc::post::<hypercolor_types::api::devices::IdentifyAttachmentResponse>(
                "identify_attachment",
                "devices",
                "Identify one attachment",
            ).optional_body::<hypercolor_types::api::devices::IdentifyAttachmentRequest>()],
        ))
        .routes(openapi::documented_route(
            "/devices/{id}/pair",
            axum::routing::post(devices::pair_device).delete(devices::delete_pairing),
            [
                OperationDoc::post::<hypercolor_types::api::devices::PairDeviceResponse>(
                    "pair_device",
                    "devices",
                    "Pair one device",
                ),
                OperationDoc::delete::<hypercolor_types::api::devices::DeletePairingResponse>(
                    "delete_pairing",
                    "devices",
                    "Delete one device pairing",
                ),
            ],
        ))
}
