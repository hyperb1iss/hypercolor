//! REST API and WebSocket server for the Hypercolor daemon.
//!
//! Assembles all route groups into a single [`axum::Router`] and adapts the
//! daemon's shared [`AppState`] into Axum handlers.

pub mod access_log;
pub mod assets;
pub mod attachments;
pub mod capture;
pub mod config;
pub mod control_values;
pub mod controls;
pub mod devices;
pub mod diagnose;
pub mod displays;
pub mod drivers;
pub mod effects;
pub mod envelope;
pub mod layouts;
pub mod library;
pub mod local;
#[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
mod macos_screen_parity;
pub mod openapi;
pub mod output;
mod routes;
pub mod scene;
pub mod scenes;
pub mod security;
pub mod simulators;
pub mod system;
pub mod ws;

use crate::app_state::AppState;

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, Method, header};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::warn;
use utoipa_axum::router::OpenApiRouter;

use self::openapi::OperationDoc;
use hypercolor_types::config::{EffectErrorFallbackPolicy, McpConfig, WebConfig};
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::event::{EffectRef, EffectStopReason, HypercolorEvent, ZoneChangeKind};
use hypercolor_types::scene::{SceneId, Zone};
use uuid::Uuid;

pub(crate) async fn persist_simulated_displays(state: &Arc<AppState>) {
    let store = state.simulated_displays.read().await;
    if let Err(error) = store.save() {
        warn!(%error, "Failed to persist simulated display store");
    }
}

pub(crate) fn publish_render_group_changed(
    state: &AppState,
    scene_id: SceneId,
    group: &Zone,
    kind: ZoneChangeKind,
) {
    state.event_bus.publish(HypercolorEvent::ZoneChanged {
        scene_id,
        zone_id: group.id,
        role: group.role,
        kind,
    });
}

#[derive(Debug, Clone)]
pub(crate) struct EffectErrorFallbackApplied {
    pub effect: EffectRef,
    pub cleared_group_count: usize,
}

/// Unload an effect from every zone of the active scene that runs it,
/// as the configured error-fallback policy demands.
///
/// `Ok(None)` means the policy did nothing: either it is `None`, or no
/// zone was running the failed effect.
pub(crate) async fn apply_effect_error_fallback(
    state: &Arc<AppState>,
    effect_id: &str,
    policy: EffectErrorFallbackPolicy,
) -> Result<Option<EffectErrorFallbackApplied>, crate::domain::DomainError> {
    match policy {
        EffectErrorFallbackPolicy::None => Ok(None),
        EffectErrorFallbackPolicy::ClearGroups => {
            clear_active_scene_effect_groups(state, effect_id).await
        }
    }
}

async fn clear_active_scene_effect_groups(
    state: &Arc<AppState>,
    effect_id: &str,
) -> Result<Option<EffectErrorFallbackApplied>, crate::domain::DomainError> {
    let effect = resolve_effect_ref_for_fallback(state, effect_id).await;

    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.active_scene_for_runtime_mutation("applying an effect error fallback")?;
    let zone_ids = mutation
        .scenes()
        .active_scene()
        .map(|scene| {
            scene
                .zones
                .iter()
                .filter(|zone| {
                    zone.effect_ids()
                        .any(|candidate| candidate.to_string() == effect_id)
                })
                .map(|zone| zone.id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if zone_ids.is_empty() {
        return Ok(None);
    }

    let cleared_zones = zone_ids
        .into_iter()
        .filter_map(|zone_id| {
            mutation.clear_zone_effect(zone_id, Some(effect.clone()), EffectStopReason::Error)
        })
        .collect::<Vec<_>>();
    if cleared_zones.is_empty() {
        return Ok(None);
    }

    crate::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await?
        .log_if_retrying("Failed to persist effect fallback");
    persist_runtime_session(state).await;

    Ok(Some(EffectErrorFallbackApplied {
        effect,
        cleared_group_count: cleared_zones.len(),
    }))
}

async fn resolve_effect_ref_for_fallback(state: &AppState, effect_id: &str) -> EffectRef {
    let parsed_id = Uuid::parse_str(effect_id).ok().map(EffectId::new);
    if let Some(parsed_id) = parsed_id
        && let Some(metadata) = state.domains.effects.metadata(parsed_id).await
    {
        return crate::domain::effect::effect_ref(&metadata);
    }

    EffectRef {
        id: effect_id.to_owned(),
        name: effect_id.to_owned(),
        engine: "unknown".to_owned(),
    }
}

/// Remove every display assignment a deleted device leaves behind: its
/// scene-bound display groups, its runtime default face zone, and its
/// stored default-face preference. The default zone and preference are
/// pruned even when scene-store persistence fails — a deleted device must
/// never keep a live render group demanding face frames, and the deleted
/// device cannot be resolved later to clear them through the displays API.
pub(crate) async fn prune_scene_display_groups_for_device(
    state: &Arc<AppState>,
    device_id: DeviceId,
) {
    // The preference goes first and unconditionally. A deleted device
    // must never keep a stored default face, and it can no longer be
    // addressed through the displays API to clear one, so this must not
    // ride on whether the scene commit lands.
    let removed_preference = {
        let mut store = state.display_preferences.write().await;
        match store.remove(device_id) {
            Ok(removed) => removed.is_some(),
            Err(error) => {
                warn!(%error, %device_id, "Failed to prune display preference for deleted device");
                false
            }
        }
    };

    let pruned = match crate::domain::display::prune_display_zones_for_device(
        &state.domains.scene,
        device_id,
    )
    .await
    {
        Ok(pruned) => pruned,
        Err(error) => {
            warn!(%error, %device_id, "Failed to prune display zones for deleted device");
            crate::domain::display::PrunedDisplayZones::empty()
        }
    };

    if pruned.removed_zones.is_empty() && pruned.removed_default.is_none() && !removed_preference {
        return;
    }
    persist_runtime_session(state).await;
}

pub(crate) async fn save_runtime_session_snapshot(state: &AppState) {
    state.domains.runtime_session.save().await;
}

pub(crate) async fn persist_runtime_session(state: &Arc<AppState>) {
    save_runtime_session_snapshot(state.as_ref()).await;
}

pub(crate) fn discovery_runtime(state: &AppState) -> crate::discovery::DiscoveryRuntime {
    state.driver_host.discovery_runtime()
}

// ── Router ───────────────────────────────────────────────────────────────

fn documented_api_routes(asset_upload_body_limit: usize) -> OpenApiRouter<Arc<AppState>> {
    routes::versioned(asset_upload_body_limit)
}

fn documented_root_routes() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::with_openapi(openapi::base_document()).routes(openapi::documented_route(
        "/health",
        axum::routing::get(system::health_check),
        [
            OperationDoc::get::<hypercolor_types::api::system::HealthResponse>(
                "health_check",
                "system",
                "Run daemon health check",
            )
            .also_status("503")
            .raw(),
        ],
    ))
}

pub(crate) fn openapi_document() -> utoipa::openapi::OpenApi {
    let asset_upload_body_limit =
        usize::try_from(assets::asset_upload_body_limit_bytes()).unwrap_or(usize::MAX);
    documented_root_routes().into_openapi().nest(
        "/api/v1",
        documented_api_routes(asset_upload_body_limit).into_openapi(),
    )
}

/// Build the complete Axum router with all API routes and middleware.
///
/// When `ui_dir` is provided, static files are served at `/` with SPA
/// fallback (all non-API, non-asset paths return `index.html`).
pub fn build_router(state: Arc<AppState>, ui_dir: Option<&Path>) -> Router {
    let security_state = state.security_state.clone();
    let (mcp_config, web_config): (McpConfig, WebConfig) =
        state.config_manager.as_ref().map_or_else(
            || (McpConfig::default(), WebConfig::default()),
            |manager| {
                let config = manager.get();
                (config.mcp.clone(), config.web.clone())
            },
        );
    let cors_origin = cors_origins(&web_config, security_state.security_enabled());
    // Sourced from the route's own ceiling so a 413 can never name a limit
    // this layer does not enforce.
    let asset_upload_body_limit =
        usize::try_from(assets::asset_upload_body_limit_bytes()).unwrap_or(usize::MAX);

    let api = documented_api_routes(asset_upload_body_limit);

    let mut api = api;
    for extension in &state.api_extensions {
        api = extension.mount_api_routes(api);
    }
    // A deleted route has to answer as one. Without a fallback scoped to
    // the API, an unmatched `/api/v1` path falls through to the SPA
    // fallback below and a browser-facing daemon answers `200 text/html`
    // for a route that no longer exists — every route-deletion fence in
    // the program is only as strong as this. Nesting resolves the inner
    // fallback first, so the SPA never sees an API path.
    let api = api.fallback(api_route_not_found);
    let (api, versioned_openapi) = api.split_for_parts();
    let api = api.method_not_allowed_fallback(api_route_not_found);
    let root = documented_root_routes();
    let (root, root_openapi) = root.split_for_parts();
    let document = root_openapi.nest("/api/v1", versioned_openapi);
    let mut router = root.nest("/api/v1", api);

    if mcp_config.enabled {
        router = router.merge(crate::mcp::build_router(Arc::clone(&state), &mcp_config));
    }

    router = router.merge(openapi::swagger(document));

    // Serve the web UI with SPA fallback when a UI directory is configured.
    //
    // Every dynamic mount above is named here so the middleware can tell
    // an asset request from an API one. The UI is the fallback, so its
    // surface is whatever those prefixes do not claim, and a browser
    // fetching a script or stylesheet attaches no bearer header.
    let mut static_assets = security::StaticAssetSurface::default();
    if let Some(ui_path) = ui_dir {
        let index = ui_path.join("index.html");
        router = router.fallback_service(ServeDir::new(ui_path).fallback(ServeFile::new(index)));
        static_assets = security::StaticAssetSurface::mounted(dynamic_route_prefixes(&mcp_config));
    }

    router
        .layer(axum::middleware::from_fn_with_state(
            security_state.with_static_assets(static_assets),
            security::enforce_security,
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(cors_origin)
                .allow_methods([
                    Method::GET,
                    Method::HEAD,
                    Method::OPTIONS,
                    Method::POST,
                    Method::PUT,
                    Method::PATCH,
                    Method::DELETE,
                ])
                .allow_headers([header::ACCEPT, header::AUTHORIZATION, header::CONTENT_TYPE]),
        )
        .layer(axum::middleware::from_fn(access_log::log_access))
        .with_state(state)
}

/// The prefixes this router answers from a handler that
/// [`StaticAssetSurface`](security::StaticAssetSurface) does not already
/// protect.
///
/// The web UI mounts as the fallback, so the security layer identifies an
/// asset request by exclusion. `/api` and `/health` are seeded by the
/// surface itself; what varies per daemon is the MCP mount, whose base
/// path is configurable, so it is derived here next to the mount.
/// Render an unmatched `/api/v1` path as the canonical `DomainError`
/// envelope, so a retired route is indistinguishable from one that
/// never existed and distinguishable from a working page.
async fn api_route_not_found(
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    // `OriginalUri`, not `Uri`: nesting strips `/api/v1` from the
    // request the fallback sees, and echoing the stripped path would
    // name an address the caller never asked for.
    crate::domain::DomainError::not_found(crate::domain::ResourceKind::Route, uri.path())
        .into_response()
}

fn dynamic_route_prefixes(mcp_config: &McpConfig) -> Vec<String> {
    let mut prefixes = Vec::new();
    if mcp_config.enabled {
        prefixes.push(crate::mcp::normalize_base_path(&mcp_config.base_path));
    }
    prefixes
}

fn cors_origins(web_config: &WebConfig, api_auth_required: bool) -> AllowOrigin {
    let configured_origins = configured_cors_origins(web_config, api_auth_required);
    AllowOrigin::predicate(move |origin: &HeaderValue, _| {
        is_allowed_cors_origin(origin, &configured_origins)
    })
}

fn configured_cors_origins(web_config: &WebConfig, api_auth_required: bool) -> Vec<HeaderValue> {
    if !api_auth_required {
        return Vec::new();
    }

    web_config
        .cors_origins
        .iter()
        .filter_map(|origin| configured_cors_origin(origin))
        .collect()
}

fn configured_cors_origin(origin: &str) -> Option<HeaderValue> {
    let origin = origin.trim();
    if !is_http_origin(origin) {
        warn!(origin, "Ignoring invalid configured CORS origin");
        return None;
    }

    match HeaderValue::from_str(origin) {
        Ok(value) => Some(value),
        Err(error) => {
            warn!(origin, %error, "Ignoring invalid configured CORS origin");
            None
        }
    }
}

fn is_allowed_cors_origin(origin: &HeaderValue, configured_origins: &[HeaderValue]) -> bool {
    is_loopback_origin(origin)
        || security::is_trusted_tauri_origin(origin)
        || configured_origins.iter().any(|allowed| allowed == origin)
}

fn is_http_origin(origin: &str) -> bool {
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    matches!(uri.scheme_str(), Some("http" | "https"))
        && uri.host().is_some()
        && uri
            .path_and_query()
            .is_none_or(|path| matches!(path.as_str(), "" | "/"))
}

fn is_loopback_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return false;
    }

    let Some(host) = uri.host() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod cors_tests {
    use axum::http::HeaderValue;
    use hypercolor_types::config::WebConfig;

    use super::{configured_cors_origins, is_allowed_cors_origin};

    fn origin(value: &str) -> HeaderValue {
        HeaderValue::from_str(value).expect("origin should be a valid header value")
    }

    #[test]
    fn loopback_origin_is_allowed_without_api_auth() {
        let configured = configured_cors_origins(&WebConfig::default(), false);

        assert!(is_allowed_cors_origin(
            &origin("http://localhost:9430"),
            &configured
        ));
        assert!(is_allowed_cors_origin(
            &origin("http://127.0.0.1:9430"),
            &configured
        ));
        for native_origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ] {
            assert!(is_allowed_cors_origin(&origin(native_origin), &configured));
        }
        assert!(!is_allowed_cors_origin(
            &origin("tauri://attacker.example"),
            &configured
        ));
    }

    #[test]
    fn configured_origin_requires_api_auth() {
        let config = WebConfig {
            cors_origins: vec!["https://studio.example".to_owned()],
            ..WebConfig::default()
        };

        let unsecured = configured_cors_origins(&config, false);
        assert!(!is_allowed_cors_origin(
            &origin("https://studio.example"),
            &unsecured
        ));

        let secured = configured_cors_origins(&config, true);
        assert!(is_allowed_cors_origin(
            &origin("https://studio.example"),
            &secured
        ));
    }

    #[test]
    fn invalid_configured_origin_is_ignored() {
        let config = WebConfig {
            cors_origins: vec![
                "*".to_owned(),
                "https://studio.example/path".to_owned(),
                "https://studio.example".to_owned(),
            ],
            ..WebConfig::default()
        };

        let configured = configured_cors_origins(&config, true);

        assert_eq!(configured, vec![origin("https://studio.example")]);
    }
}

#[cfg(test)]
mod static_asset_surface_tests {
    use hypercolor_types::config::McpConfig;

    use super::dynamic_route_prefixes;

    #[test]
    fn the_mcp_mount_is_named() {
        let prefixes = dynamic_route_prefixes(&McpConfig {
            enabled: true,
            base_path: "/mcp".to_owned(),
            ..McpConfig::default()
        });

        assert_eq!(prefixes, vec!["/mcp"]);
    }

    #[test]
    fn a_relocated_mcp_mount_follows_its_configured_path() {
        // The exemption would hand an unauthenticated caller the MCP
        // surface if this tracked the default instead of the config.
        let prefixes = dynamic_route_prefixes(&McpConfig {
            enabled: true,
            base_path: "agents/".to_owned(),
            ..McpConfig::default()
        });

        assert_eq!(prefixes, vec!["/agents"]);
    }

    #[test]
    fn a_disabled_mcp_server_contributes_no_prefix() {
        let prefixes = dynamic_route_prefixes(&McpConfig {
            enabled: false,
            ..McpConfig::default()
        });

        assert!(prefixes.is_empty());
    }
}
