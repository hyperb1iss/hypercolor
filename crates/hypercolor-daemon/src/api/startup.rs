//! The API surface served on the bound listeners while the daemon starts.
//!
//! The listeners are bound before any subsystem exists, so they accept
//! connections from the first moment of startup. Until the full router is
//! installed, every request lands on a minimal startup surface: `GET
//! /health` answers `503` with the current startup phase, and every other
//! path answers `503` "starting". Installing the full router is a one-shot
//! handoff on the same listeners; nothing is rebound or dropped.
//!
//! The router is chosen once per accepted connection, so a connection
//! accepted after the handoff talks to the full router directly with no
//! per-request indirection. A connection accepted before the handoff stays
//! on the startup surface, which forwards each request to the full router
//! once it exists.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::AddExtension;
use axum::response::{IntoResponse, Response};
use axum::serve::IncomingStream;
use hypercolor_types::api::system::{HEALTH_STATUS_STARTING, HealthChecks, HealthResponse};
use tokio::net::TcpListener;
use tower::{Layer, Service, ServiceExt};

use crate::domain::DomainError;
use crate::startup::StartupProgress;

const STARTING_MESSAGE: &str = "Hypercolor daemon is starting";

/// One-shot switch from the startup surface to the full API router.
#[derive(Clone)]
#[doc(hidden)]
pub struct ApiHandoff {
    full: Arc<OnceLock<Router>>,
    startup: Router,
}

impl ApiHandoff {
    /// Serve the startup surface until [`install`](Self::install) is called.
    ///
    /// `version` is reported by the startup `/health` body. It is the daemon
    /// build's version: the served identity is not resolved until startup
    /// finishes, and a `starting` body never satisfies an identity proof.
    #[must_use]
    pub fn starting(progress: StartupProgress, version: impl Into<String>) -> Self {
        let full = Arc::new(OnceLock::new());
        let startup = Router::new()
            .fallback(startup_surface)
            .with_state(StartupSurface {
                progress,
                version: version.into(),
                full: Arc::clone(&full),
            });
        Self { full, startup }
    }

    /// Serve `router` from the first connection.
    #[must_use]
    pub fn ready(router: Router) -> Self {
        let full = Arc::new(OnceLock::new());
        let _ = full.set(router);
        Self {
            full,
            startup: Router::new(),
        }
    }

    /// Hand every listener over to the full router.
    ///
    /// Returns `false` when a router was already installed; the first one
    /// stays in place.
    pub fn install(&self, router: Router) -> bool {
        self.full.set(router).is_ok()
    }

    /// Whether the full router has been installed.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.full.get().is_some()
    }

    fn connection_router(&self) -> Router {
        self.full
            .get()
            .map_or_else(|| self.startup.clone(), Router::clone)
    }

    /// Per-connection service factory for `axum::serve`.
    pub(crate) fn make_service(&self) -> HandoffMakeService {
        HandoffMakeService {
            handoff: self.clone(),
        }
    }
}

/// Picks the router for each accepted connection and attaches the peer
/// address the security layer reads.
#[derive(Clone)]
pub(crate) struct HandoffMakeService {
    handoff: ApiHandoff,
}

impl Service<IncomingStream<'_, TcpListener>> for HandoffMakeService {
    type Response = AddExtension<Router, ConnectInfo<SocketAddr>>;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, stream: IncomingStream<'_, TcpListener>) -> Self::Future {
        let router = self.handoff.connection_router();
        std::future::ready(Ok(
            axum::Extension(ConnectInfo(*stream.remote_addr())).layer(router)
        ))
    }
}

#[derive(Clone)]
struct StartupSurface {
    progress: StartupProgress,
    version: String,
    full: Arc<OnceLock<Router>>,
}

impl StartupSurface {
    fn health(&self) -> HealthResponse {
        let starting = || HEALTH_STATUS_STARTING.to_owned();
        HealthResponse {
            status: starting(),
            version: self.version.clone(),
            uptime_seconds: self.progress.elapsed().as_secs(),
            checks: HealthChecks {
                render_loop: starting(),
                device_backends: starting(),
                event_bus: starting(),
            },
            startup: Some(self.progress.snapshot()),
        }
    }
}

async fn startup_surface(State(surface): State<StartupSurface>, request: Request) -> Response {
    if let Some(full) = surface.full.get() {
        return match full.clone().oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        };
    }

    let mut response = startup_response(&surface, &request);
    // Close after every startup answer so clients reconnect, and the
    // connection after the handoff lands on the full router directly.
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    response
}

fn startup_response(surface: &StartupSurface, request: &Request<Body>) -> Response {
    // The network access policy is part of the full router and does not
    // exist yet, so only loopback peers learn the version and phase. A
    // remote peer sees the same 503 with no details.
    if !super::security::request_is_loopback(request) {
        return DomainError::service_unavailable(STARTING_MESSAGE).into_response();
    }
    if request.method() == Method::GET && request.uri().path() == "/health" {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(surface.health()),
        )
            .into_response();
    }
    let progress = surface.progress.snapshot();
    DomainError::service_unavailable_details(
        STARTING_MESSAGE,
        serde_json::json!({
            "phase": progress.phase,
            "sequence": progress.sequence,
        }),
    )
    .into_response()
}
