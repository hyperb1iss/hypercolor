use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, effects, openapi};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/effects",
            axum::routing::get(effects::list_effects),
            [
                OperationDoc::get_list::<hypercolor_types::api::effects::EffectSummary>(
                    "list_effects",
                    "effects",
                    "List effects",
                )
                .query::<effects::EffectListQuery>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/effects/rescan",
            axum::routing::post(effects::rescan_effects),
            [OperationDoc::post::<
                hypercolor_types::api::effects::RescanResponse,
            >("rescan_effects", "effects", "Rescan effects")],
        ))
        .routes(openapi::documented_route(
            "/effects/install",
            axum::routing::post(effects::install_effect),
            [
                OperationDoc::post::<hypercolor_types::api::effects::InstalledEffectResponse>(
                    "install_effect",
                    "effects",
                    "Install effect",
                )
                .status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/effects/{id}",
            axum::routing::get(effects::get_effect),
            [OperationDoc::get::<
                hypercolor_types::api::effects::EffectDetailResponse,
            >("get_effect", "effects", "Get effect")],
        ))
        .routes(openapi::documented_route(
            "/effects/{id}/cover",
            axum::routing::get(effects::get_effect_cover),
            [OperationDoc::get::<serde_json::Value>(
                "get_effect_cover",
                "effects",
                "Get effect cover image",
            )
            .binary("image/jpeg")],
        ))
        .routes(openapi::documented_route(
            "/effects/{id}/apply",
            axum::routing::post(effects::apply_effect),
            [
                OperationDoc::post::<hypercolor_types::api::scene::ApplyEffectResponse>(
                    "apply_effect",
                    "effects",
                    "Apply effect",
                )
                .optional_body::<hypercolor_types::api::scene::ApplyEffectRequest>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/effects/{id}/presets",
            axum::routing::get(effects::list_effect_presets),
            [OperationDoc::get_list::<
                hypercolor_types::api::effects::EffectPresetSummary,
            >(
                "list_effect_presets", "effects", "List effect presets"
            )],
        ))
        .routes(openapi::documented_route(
            "/effects/{id}/presets/{preset}/apply",
            axum::routing::post(effects::apply_effect_preset),
            [
                OperationDoc::post::<hypercolor_types::api::scene::ApplyEffectResponse>(
                    "apply_effect_preset",
                    "effects",
                    "Apply effect preset",
                )
                .optional_body::<hypercolor_types::api::scene::ApplyEffectRequest>(),
            ],
        ))
}
