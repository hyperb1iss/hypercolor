//! Integration tests for downstream credential authorities and public routes.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::{ConnectInfo, Extension};
use axum::routing::{get, post};
use http::{Method, Request, StatusCode, header};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api;
use hypercolor_daemon::api::security::{
    CredentialAuthority, CredentialGrant, CredentialTier, RequestAuthContext, SecurityState,
};
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::daemon::{
    DaemonExtensionInstaller, DaemonRunOptions, adopt_credential_authority,
    credential_authority_grants_control, effective_startup_bind_targets,
};
use hypercolor_daemon::extensions::{ApiExtension, PublicRateClass, PublicRoute};
use hypercolor_daemon::startup::default_config;
use hypercolor_types::config::{HypercolorConfig, NetworkAccessMode, NetworkConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use utoipa_axum::router::OpenApiRouter;

const CONTROL_KEY: &str = "client-control-key";
const READ_KEY: &str = "client-read-key";
const REVOKED_KEY: &str = "client-revoked-key";
const LAN_CLIENT: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 20);

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ── Fixtures ─────────────────────────────────────────────────────────────

struct TestAuthority {
    ceiling: CredentialTier,
    credentials: HashMap<&'static str, (CredentialTier, CancellationToken)>,
}

impl TestAuthority {
    fn new(ceiling: CredentialTier) -> Self {
        let revoked = CancellationToken::new();
        revoked.cancel();
        Self {
            ceiling,
            credentials: HashMap::from([
                (
                    CONTROL_KEY,
                    (CredentialTier::Control, CancellationToken::new()),
                ),
                (READ_KEY, (CredentialTier::Read, CancellationToken::new())),
                (REVOKED_KEY, (CredentialTier::Control, revoked)),
            ]),
        }
    }

    fn revocation(&self, key: &str) -> CancellationToken {
        self.credentials[key].1.clone()
    }
}

impl CredentialAuthority for TestAuthority {
    fn ceiling(&self) -> CredentialTier {
        self.ceiling
    }

    fn authenticate(&self, presented: &str) -> Option<CredentialGrant> {
        self.credentials
            .get(presented)
            .map(|(tier, revocation)| CredentialGrant::new(*tier, presented, revocation.clone()))
    }
}

/// Mounts probe routes under `/test-ext` and declares two of them public,
/// plus declarations the engine must refuse.
struct TestExtension;

async fn whoami(
    Extension(context): Extension<RequestAuthContext>,
    grant: Option<Extension<CredentialGrant>>,
) -> Json<Value> {
    Json(json!({
        "can_control": context.can_control(),
        "can_protected_control": context.can_protected_control(),
        "is_loopback": context.is_loopback(),
        "credential_id": grant.map(|Extension(grant)| grant.credential_id().to_owned()),
    }))
}

impl ApiExtension for TestExtension {
    fn name(&self) -> &'static str {
        "test-extension"
    }

    fn mount_api_routes(
        &self,
        router: OpenApiRouter<Arc<AppState>>,
    ) -> OpenApiRouter<Arc<AppState>> {
        router
            .route("/test-ext/whoami", get(whoami).post(whoami))
            .route("/test-ext/exchange", post(whoami))
            .route("/test-ext/probe", get(whoami))
            .route("/test-ext/private", post(whoami))
    }

    fn public_routes(&self) -> Vec<PublicRoute> {
        vec![
            PublicRoute::new(Method::POST, "/test-ext/exchange", PublicRateClass::Pairing),
            PublicRoute::new(Method::GET, "/test-ext/probe", PublicRateClass::Read),
            // Engine routes and inexact paths must never become public.
            PublicRoute::new(Method::GET, "/devices", PublicRateClass::Read),
            PublicRoute::new(Method::GET, "/capture/monitors", PublicRateClass::Read),
            // Concrete instances of engine templates are engine routes too.
            PublicRoute::new(Method::GET, "/devices/example", PublicRateClass::Read),
            PublicRoute::new(
                Method::GET,
                "/scene/zones/main/layers/base",
                PublicRateClass::Read,
            ),
            PublicRoute::new(Method::GET, "/test-ext/{id}", PublicRateClass::Read),
            PublicRoute::new(Method::GET, "test-ext/whoami", PublicRateClass::Read),
        ]
    }
}

struct TestApp {
    router: axum::Router,
    data_dir: tempfile::TempDir,
}

fn isolated_builder() -> (tempfile::TempDir, AppStateBuilder) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (tempdir, AppStateBuilder::new(data_dir))
}

fn app_with(security: impl FnOnce(SecurityState) -> SecurityState) -> TestApp {
    let (tempdir, builder) = isolated_builder();
    let mut state = builder.build();
    state.security_state = security(state.security_state.clone());
    state.api_extensions.push(Arc::new(TestExtension));
    TestApp {
        router: api::build_router(Arc::new(state), None),
        data_dir: tempdir,
    }
}

fn app_with_authority(authority: Arc<TestAuthority>) -> TestApp {
    app_with(|security| security.with_credential_authority(authority))
}

fn app_with_network(network: NetworkConfig, authority: Arc<TestAuthority>) -> TestApp {
    let (tempdir, builder) = isolated_builder();
    let manager = Arc::new(
        ConfigManager::new(tempdir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    manager.update(HypercolorConfig {
        network: network.clone(),
        ..HypercolorConfig::default()
    });
    let mut state = builder.with_config_manager(manager).build();
    state.security_state = SecurityState::from_config(&HypercolorConfig {
        network,
        ..HypercolorConfig::default()
    })
    .with_credential_authority(authority);
    state.api_extensions.push(Arc::new(TestExtension));
    TestApp {
        router: api::build_router(Arc::new(state), None),
        data_dir: tempdir,
    }
}

fn request(ip: Ipv4Addr, method: Method, path: &str, bearer: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(bearer) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let mut request = builder.body(Body::empty()).expect("request should build");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((ip, 41_000))));
    request
}

async fn send(app: &TestApp, request: Request<Body>) -> http::Response<Body> {
    app.router
        .clone()
        .oneshot(request)
        .await
        .expect("request should complete")
}

async fn json_body(response: http::Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body should read");
    serde_json::from_slice(&bytes).expect("body should be JSON")
}

fn rate_limit(response: &http::Response<Body>) -> Option<&str> {
    response
        .headers()
        .get("x-ratelimit-limit")
        .and_then(|value| value.to_str().ok())
}

// ── Authentication ───────────────────────────────────────────────────────

#[tokio::test]
async fn an_authority_key_authenticates_a_network_request_at_its_tier() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));

    let response = send(
        &app,
        request(
            LAN_CLIENT,
            Method::POST,
            "/api/v1/test-ext/whoami",
            Some(CONTROL_KEY),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        rate_limit(&response),
        Some("60"),
        "writes spend the write budget"
    );
    let body = json_body(response).await;
    assert_eq!(body["can_control"], true);
    assert_eq!(body["is_loopback"], false);
    assert_eq!(body["credential_id"], CONTROL_KEY);

    let engine_read = send(
        &app,
        request(
            LAN_CLIENT,
            Method::GET,
            "/api/v1/devices",
            Some(CONTROL_KEY),
        ),
    )
    .await;
    assert_eq!(engine_read.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_authority_grant_never_carries_protected_control() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));

    let body = json_body(
        send(
            &app,
            request(
                LAN_CLIENT,
                Method::GET,
                "/api/v1/test-ext/whoami",
                Some(CONTROL_KEY),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(body["can_control"], true);
    assert_eq!(body["can_protected_control"], false);

    let protected = send(
        &app,
        request(
            LAN_CLIENT,
            Method::GET,
            "/api/v1/capture/monitors",
            Some(CONTROL_KEY),
        ),
    )
    .await;
    assert_eq!(protected.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_unknown_or_revoked_key_is_rejected() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));

    for key in ["not-a-client-key", REVOKED_KEY] {
        let response = send(
            &app,
            request(LAN_CLIENT, Method::GET, "/api/v1/devices", Some(key)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{key}");
    }

    let missing = send(
        &app,
        request(LAN_CLIENT, Method::GET, "/api/v1/devices", None),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_read_grant_cannot_write() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));

    let read = send(
        &app,
        request(
            LAN_CLIENT,
            Method::GET,
            "/api/v1/test-ext/whoami",
            Some(READ_KEY),
        ),
    )
    .await;
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(json_body(read).await["can_control"], false);

    let write = send(
        &app,
        request(
            LAN_CLIENT,
            Method::POST,
            "/api/v1/test-ext/whoami",
            Some(READ_KEY),
        ),
    )
    .await;
    assert_eq!(write.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn grants_are_clamped_to_the_authority_ceiling() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Read)));

    let write = send(
        &app,
        request(
            LAN_CLIENT,
            Method::POST,
            "/api/v1/test-ext/whoami",
            Some(CONTROL_KEY),
        ),
    )
    .await;
    assert_eq!(write.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_authority_key_resolves_on_loopback_too() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));

    let body = json_body(
        send(
            &app,
            request(
                Ipv4Addr::LOCALHOST,
                Method::GET,
                "/api/v1/test-ext/whoami",
                Some(CONTROL_KEY),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(body["credential_id"], CONTROL_KEY);
    assert_eq!(body["is_loopback"], true);
    assert_eq!(body["can_protected_control"], false);

    // An unresolvable token on loopback keeps today's anonymous local
    // caller, except on the system route.
    let anonymous = send(
        &app,
        request(
            Ipv4Addr::LOCALHOST,
            Method::GET,
            "/api/v1/test-ext/whoami",
            Some("not-a-client-key"),
        ),
    )
    .await;
    assert_eq!(anonymous.status(), StatusCode::OK);
    assert!(json_body(anonymous).await["credential_id"].is_null());
    let system = send(
        &app,
        request(
            Ipv4Addr::LOCALHOST,
            Method::GET,
            "/api/v1/system",
            Some("not-a-client-key"),
        ),
    )
    .await;
    assert_eq!(system.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_empty_daemon_answers_as_before_and_an_authority_turns_auth_on() {
    let unsecured = app_with(|security| security);
    let open = send(
        &unsecured,
        request(LAN_CLIENT, Method::GET, "/api/v1/devices", None),
    )
    .await;
    assert_eq!(open.status(), StatusCode::OK);

    let secured = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));
    let closed = send(
        &secured,
        request(LAN_CLIENT, Method::GET, "/api/v1/devices", None),
    )
    .await;
    assert_eq!(closed.status(), StatusCode::UNAUTHORIZED);
}

// ── Public routes ────────────────────────────────────────────────────────

#[tokio::test]
async fn a_public_route_answers_without_a_credential_and_confers_none() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));

    for bearer in [None, Some("not-a-client-key"), Some(CONTROL_KEY)] {
        let response = send(
            &app,
            request(
                Ipv4Addr::new(192, 168, 1, 30),
                Method::POST,
                "/api/v1/test-ext/exchange",
                bearer,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{bearer:?}");
        let body = json_body(response).await;
        assert_eq!(body["can_control"], false, "{bearer:?}");
        assert_eq!(body["can_protected_control"], false, "{bearer:?}");
        assert!(body["credential_id"].is_null(), "{bearer:?}");
    }
}

#[tokio::test]
async fn a_public_route_never_confers_loopback_locality() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));
    let cross_site = |path: &str| {
        let mut request = request(Ipv4Addr::LOCALHOST, Method::POST, path, None);
        let headers = request.headers_mut();
        headers.insert("sec-fetch-site", "cross-site".parse().expect("header"));
        headers.insert(
            header::ORIGIN,
            "https://evil.example".parse().expect("header"),
        );
        request
    };

    // Any page can drive a loopback browser at a public route, so its
    // handler must not see that caller as local. It still spends the
    // route's rate class.
    let public = send(&app, cross_site("/api/v1/test-ext/exchange")).await;
    assert_eq!(public.status(), StatusCode::OK);
    assert_eq!(rate_limit(&public), Some("6"));
    assert_eq!(json_body(public).await["is_loopback"], false);

    // A route that is not public still meets the loopback CSRF gate.
    let private = send(&app, cross_site("/api/v1/test-ext/private")).await;
    assert_eq!(private.status(), StatusCode::FORBIDDEN);

    // A same-site loopback caller of a non-public route is still local.
    let local = send(
        &app,
        request(
            Ipv4Addr::LOCALHOST,
            Method::POST,
            "/api/v1/test-ext/private",
            None,
        ),
    )
    .await;
    assert_eq!(local.status(), StatusCode::OK);
    assert_eq!(json_body(local).await["is_loopback"], true);
}

#[tokio::test]
async fn a_public_route_is_rate_limited_in_its_declared_class_for_every_caller() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));

    for caller in [LAN_CLIENT, Ipv4Addr::LOCALHOST] {
        for attempt in 1..=6 {
            let response = send(
                &app,
                request(caller, Method::POST, "/api/v1/test-ext/exchange", None),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{caller} attempt {attempt}"
            );
            assert_eq!(rate_limit(&response), Some("6"));
        }
        let limited = send(
            &app,
            request(caller, Method::POST, "/api/v1/test-ext/exchange", None),
        )
        .await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS, "{caller}");
    }

    let other = send(
        &app,
        request(
            Ipv4Addr::new(192, 168, 1, 21),
            Method::POST,
            "/api/v1/test-ext/exchange",
            None,
        ),
    )
    .await;
    assert_eq!(other.status(), StatusCode::OK, "budgets are per client");

    let probe = send(
        &app,
        request(LAN_CLIENT, Method::GET, "/api/v1/test-ext/probe", None),
    )
    .await;
    assert_eq!(probe.status(), StatusCode::OK);
    assert_eq!(rate_limit(&probe), Some("120"));
    let head = send(
        &app,
        request(LAN_CLIENT, Method::HEAD, "/api/v1/test-ext/probe", None),
    )
    .await;
    assert_eq!(head.status(), StatusCode::OK, "GET declarations cover HEAD");
}

#[tokio::test]
async fn a_public_route_stays_behind_the_network_policy() {
    let network = NetworkConfig {
        access_mode: NetworkAccessMode::Custom,
        allowed_clients: vec!["192.168.1.0/24".to_owned()],
        ..NetworkConfig::default()
    };
    let app = app_with_network(
        network,
        Arc::new(TestAuthority::new(CredentialTier::Control)),
    );

    let inside = send(
        &app,
        request(LAN_CLIENT, Method::POST, "/api/v1/test-ext/exchange", None),
    )
    .await;
    assert_eq!(inside.status(), StatusCode::OK);

    let outside = send(
        &app,
        request(
            Ipv4Addr::new(10, 0, 0, 5),
            Method::POST,
            "/api/v1/test-ext/exchange",
            None,
        ),
    )
    .await;
    assert_eq!(outside.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn undeclared_and_refused_declarations_stay_authenticated() {
    let app = app_with_authority(Arc::new(TestAuthority::new(CredentialTier::Control)));

    for (method, path) in [
        (Method::POST, "/api/v1/test-ext/private"),
        (Method::GET, "/api/v1/test-ext/whoami"),
        (Method::GET, "/api/v1/devices"),
        (Method::GET, "/api/v1/capture/monitors"),
        // Public, these would reach the engine handler and answer 404.
        (Method::GET, "/api/v1/devices/example"),
        (Method::GET, "/api/v1/scene/zones/main/layers/base"),
    ] {
        let response = send(&app, request(LAN_CLIENT, method.clone(), path, None)).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path} must still need a credential"
        );
    }

    let wrong_method = send(
        &app,
        request(LAN_CLIENT, Method::GET, "/api/v1/test-ext/exchange", None),
    )
    .await;
    assert_eq!(
        wrong_method.status(),
        StatusCode::UNAUTHORIZED,
        "a declaration covers its own method only"
    );
}

// ── WebSocket ────────────────────────────────────────────────────────────

async fn serve(app: TestApp) -> (SocketAddr, tempfile::TempDir) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let address = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.router
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    (address, app.data_dir)
}

/// Open `/api/v1/ws` as the forwarded LAN client a loopback proxy names,
/// returning the response head and, on 101, the stream.
async fn upgrade(
    address: SocketAddr,
    token: Option<&str>,
    origin: &str,
    forwarded_for: Option<IpAddr>,
) -> (String, TcpStream) {
    let mut stream = TcpStream::connect(address).await.expect("connect");
    let query = token
        .map(|token| format!("?token={token}"))
        .unwrap_or_default();
    let forwarded = forwarded_for
        .map(|ip| format!("X-Forwarded-For: {ip}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "GET /api/v1/ws{query} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Origin: {origin}\r\n\
         {forwarded}\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Protocol: hypercolor-v1\r\n\
         \r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write upgrade");
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.expect("read head");
        head.push(byte[0]);
    }
    (String::from_utf8_lossy(&head).into_owned(), stream)
}

/// Read server frames until a close frame, returning its status code.
async fn read_until_close(stream: &mut TcpStream) -> Option<u16> {
    loop {
        let mut header = [0u8; 2];
        if stream.read_exact(&mut header).await.is_err() {
            return None;
        }
        let opcode = header[0] & 0x0F;
        let mut len = u64::from(header[1] & 0x7F);
        if len == 126 {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext).await.ok()?;
            len = u64::from(u16::from_be_bytes(ext));
        } else if len == 127 {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext).await.ok()?;
            len = u64::from_be_bytes(ext);
        }
        let mut payload = vec![0u8; usize::try_from(len).ok()?];
        stream.read_exact(&mut payload).await.ok()?;
        if opcode == 0x8 {
            return payload
                .get(..2)
                .map(|code| u16::from_be_bytes([code[0], code[1]]));
        }
    }
}

#[tokio::test]
async fn a_credentialed_upgrade_is_admitted_from_the_daemons_lan_origin() {
    let authority = Arc::new(TestAuthority::new(CredentialTier::Control));
    let (address, _dir) = serve(app_with_authority(authority)).await;
    let lan = Some(IpAddr::V4(LAN_CLIENT));

    let (head, _stream) =
        upgrade(address, Some(CONTROL_KEY), "http://192.168.1.10:9420", lan).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");

    // An anonymous loopback caller still meets the origin allowlist.
    let (head, _stream) = upgrade(address, None, "http://192.168.1.10:9420", None).await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
}

#[tokio::test]
async fn revoking_a_grant_closes_the_socket_it_opened() {
    let authority = Arc::new(TestAuthority::new(CredentialTier::Control));
    let revocation = authority.revocation(CONTROL_KEY);
    let (address, _dir) = serve(app_with_authority(authority)).await;

    let (head, mut stream) = upgrade(
        address,
        Some(CONTROL_KEY),
        "http://192.168.1.10:9420",
        Some(IpAddr::V4(LAN_CLIENT)),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");

    revocation.cancel();
    let code = tokio::time::timeout(Duration::from_secs(5), read_until_close(&mut stream))
        .await
        .expect("the session should close promptly");
    assert_eq!(code, Some(1008));
}

// ── Bind ─────────────────────────────────────────────────────────────────

struct SupplyingInstaller(Option<Arc<dyn CredentialAuthority>>);

impl DaemonExtensionInstaller for SupplyingInstaller {
    fn install(&self, _daemon: &mut hypercolor_daemon::startup::DaemonState) -> anyhow::Result<()> {
        Ok(())
    }

    fn credential_authority(&self) -> Option<Arc<dyn CredentialAuthority>> {
        self.0.clone()
    }
}

fn options_with(ceiling: CredentialTier) -> DaemonRunOptions {
    DaemonRunOptions {
        credential_authority: Some(Arc::new(TestAuthority::new(ceiling))),
        ..DaemonRunOptions::default()
    }
}

fn lan_protected() -> HypercolorConfig {
    let mut config = default_config();
    config.network.access_mode = NetworkAccessMode::LanProtected;
    config
}

#[test]
fn the_bind_rule_accepts_an_authority_that_can_grant_control() {
    let config = lan_protected();
    let control = options_with(CredentialTier::Control);
    assert!(credential_authority_grants_control(&control));

    let (targets, fell_back) = effective_startup_bind_targets(
        &control,
        &config,
        credential_authority_grants_control(&control),
        config.network.unauthenticated_remote_access_allowed(),
    );
    assert!(!fell_back);
    assert_eq!(targets, vec!["0.0.0.0:9420", "[::]:9420"]);
}

#[test]
fn the_bind_rule_refuses_a_read_only_authority_and_no_authority() {
    let config = lan_protected();
    for options in [
        options_with(CredentialTier::Read),
        DaemonRunOptions::default(),
    ] {
        assert!(!credential_authority_grants_control(&options));
        let (targets, fell_back) = effective_startup_bind_targets(
            &options,
            &config,
            credential_authority_grants_control(&options),
            config.network.unauthenticated_remote_access_allowed(),
        );
        assert!(fell_back);
        assert_eq!(targets, vec!["127.0.0.1:9420", "[::1]:9420"]);
    }
}

#[test]
fn installers_supply_at_most_one_authority_and_options_win() {
    let authority: Arc<dyn CredentialAuthority> =
        Arc::new(TestAuthority::new(CredentialTier::Control));
    let supplying = SupplyingInstaller(Some(Arc::clone(&authority)));
    let silent = SupplyingInstaller(None);

    let mut options = DaemonRunOptions::default();
    adopt_credential_authority(&mut options, &[&silent]).expect("no authority is fine");
    assert!(options.credential_authority.is_none());

    adopt_credential_authority(&mut options, &[&silent, &supplying])
        .expect("one authority is adopted");
    assert!(credential_authority_grants_control(&options));

    let mut fresh = DaemonRunOptions::default();
    adopt_credential_authority(&mut fresh, &[&supplying, &supplying])
        .expect_err("two authorities are ambiguous");

    let mut preset = options_with(CredentialTier::Read);
    adopt_credential_authority(&mut preset, &[&supplying]).expect("preset options are kept");
    assert!(!credential_authority_grants_control(&preset));
}
