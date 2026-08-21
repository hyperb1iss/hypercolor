use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{config, openapi};
use crate::app_state::AppState;
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/config",
            axum::routing::get(config::show_config),
            [OperationDoc::get::<config::ConfigDocument>(
                "show_config",
                "config",
                "Show daemon config",
            )],
        ))
        .routes(openapi::documented_route(
            "/config/schema",
            axum::routing::get(config::get_config_schema),
            [OperationDoc::get_vec::<
                hypercolor_types::config_registry::ConfigKeySchemaEntry,
            >(
                "get_config_schema", "config", "Describe every config key"
            )],
        ))
        .routes(openapi::documented_route(
            "/config/keys/{key}",
            axum::routing::get(config::get_config_key)
                .put(config::put_config_key)
                .delete(config::delete_config_key),
            [
                OperationDoc::get::<config::ConfigKeyResponse>(
                    "get_config_key",
                    "config",
                    "Read one daemon config key",
                ),
                OperationDoc::put::<config::ConfigMutationResponse>(
                    "put_config_key",
                    "config",
                    "Write one daemon config key",
                )
                .query::<hypercolor_types::api::config::ConfigApplyQuery>()
                .body::<serde_json::Value>(),
                OperationDoc::delete::<config::ConfigMutationResponse>(
                    "delete_config_key",
                    "config",
                    "Restore one daemon config key to its default",
                )
                .query::<hypercolor_types::api::config::ConfigApplyQuery>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/config/reset",
            axum::routing::post(config::reset_config),
            [OperationDoc::post::<config::ConfigMutationResponse>(
                "reset_config",
                "config",
                "Restore the whole daemon config to defaults",
            )
            .query::<hypercolor_types::api::config::ConfigApplyQuery>()],
        ))
}
