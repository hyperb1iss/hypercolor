use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, library, openapi};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/library/favorites",
            axum::routing::get(library::list_favorites).post(library::add_favorite),
            [
                OperationDoc::get_list::<hypercolor_types::api::library::FavoriteSummary>(
                    "list_favorites",
                    "library",
                    "List favorite effects",
                ),
                OperationDoc::post::<hypercolor_types::api::library::AddFavoriteResponse>(
                    "add_favorite",
                    "library",
                    "Add favorite effect",
                )
                .body::<hypercolor_types::api::library::AddFavoriteRequest>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/library/favorites/{effect}",
            axum::routing::delete(library::remove_favorite),
            [OperationDoc::delete::<
                hypercolor_types::api::library::DeleteFavoriteResponse,
            >(
                "remove_favorite", "library", "Remove favorite effect"
            )],
        ))
        .routes(openapi::documented_route(
            "/library/presets",
            axum::routing::get(library::list_presets).post(library::create_preset),
            [
                OperationDoc::get_list::<hypercolor_types::library::EffectPreset>(
                    "list_presets",
                    "library",
                    "List presets",
                ),
                OperationDoc::post::<hypercolor_types::library::EffectPreset>(
                    "create_preset",
                    "library",
                    "Create preset",
                )
                .body::<hypercolor_types::api::library::SavePresetRequest>()
                .status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/library/presets/{id}",
            axum::routing::get(library::get_preset)
                .put(library::update_preset)
                .delete(library::delete_preset),
            [
                OperationDoc::get::<hypercolor_types::library::EffectPreset>(
                    "get_preset",
                    "library",
                    "Get preset",
                ),
                OperationDoc::put::<hypercolor_types::library::EffectPreset>(
                    "update_preset",
                    "library",
                    "Update preset",
                )
                .body::<hypercolor_types::api::library::SavePresetRequest>(),
                OperationDoc::delete::<hypercolor_types::api::library::DeletePresetResponse>(
                    "delete_preset",
                    "library",
                    "Delete preset",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/library/playlists",
            axum::routing::get(library::list_playlists).post(library::create_playlist),
            [
                OperationDoc::get_list::<hypercolor_types::library::EffectPlaylist>(
                    "list_playlists",
                    "library",
                    "List playlists",
                ),
                OperationDoc::post::<hypercolor_types::library::EffectPlaylist>(
                    "create_playlist",
                    "library",
                    "Create playlist",
                )
                .body::<hypercolor_types::api::library::SavePlaylistRequest>()
                .status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/library/playlists/active",
            axum::routing::get(library::get_active_playlist),
            [OperationDoc::get::<
                hypercolor_types::api::library::ActivePlaylistStateResponse,
            >(
                "get_active_playlist", "library", "Get active playlist"
            )],
        ))
        .routes(openapi::documented_route(
            "/library/playlists/deactivate",
            axum::routing::post(library::deactivate_playlist),
            [OperationDoc::post::<
                hypercolor_types::api::library::DeactivatePlaylistResponse,
            >(
                "deactivate_playlist", "library", "Deactivate playlist"
            )],
        ))
        .routes(openapi::documented_route(
            "/library/playlists/{id}",
            axum::routing::get(library::get_playlist)
                .put(library::update_playlist)
                .delete(library::delete_playlist),
            [
                OperationDoc::get::<hypercolor_types::library::EffectPlaylist>(
                    "get_playlist",
                    "library",
                    "Get playlist",
                ),
                OperationDoc::put::<hypercolor_types::library::EffectPlaylist>(
                    "update_playlist",
                    "library",
                    "Update playlist",
                )
                .body::<hypercolor_types::api::library::SavePlaylistRequest>(),
                OperationDoc::delete::<hypercolor_types::api::library::DeletePlaylistResponse>(
                    "delete_playlist",
                    "library",
                    "Delete playlist",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/library/playlists/{id}/activate",
            axum::routing::post(library::activate_playlist),
            [OperationDoc::post::<
                hypercolor_types::api::library::ActivatePlaylistResponse,
            >(
                "activate_playlist", "library", "Activate playlist"
            )],
        ))
}
