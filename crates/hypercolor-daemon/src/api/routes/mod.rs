//! Runtime route registration grouped by API resource domain.

mod assets;
mod attachments;
mod capture;
mod config;
mod controls;
mod devices;
mod diagnose;
mod displays;
mod drivers;
mod effects;
mod layouts;
mod library;
mod output;
mod scene;
mod scenes;
mod simulators;
mod system;

use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use super::openapi::OperationDoc;
use super::{AppState, openapi, ws};

pub(super) fn versioned(asset_upload_body_limit: usize) -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .merge(assets::router(asset_upload_body_limit))
        .merge(attachments::router())
        .merge(capture::router())
        .merge(config::router())
        .merge(controls::router())
        .merge(devices::router())
        .merge(diagnose::router())
        .merge(displays::router())
        .merge(drivers::router())
        .merge(effects::router())
        .merge(layouts::router())
        .merge(library::router())
        .merge(output::router())
        .merge(scene::router())
        .merge(scenes::router())
        .merge(simulators::router())
        .merge(system::router())
        .routes(openapi::documented_route(
            "/ws",
            axum::routing::get(ws::ws_handler),
            [OperationDoc::get::<serde_json::Value>(
                "ws_handler",
                "websocket",
                "Open realtime WebSocket stream",
            )
            .status("101")
            .empty()],
        ))
}
