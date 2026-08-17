//! Connection upgrade, handshake, heartbeat, and client message dispatch.
//!
//! Owns the per-connection state machine: sends the initial hello, spawns
//! relay tasks, runs the select loop that muxes incoming client frames,
//! outbound JSON/binary queues, and the ping/pong heartbeat.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use axum::extract::{Extension, State, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use hypercolor_leptos_ext::axum::upgrade_handler;
use hypercolor_leptos_ext::ws::PreviewTransportCapability;
use hypercolor_leptos_ext::ws::registry::{CanvasConfig, CanvasFormat, SpectrumConfig, TopicId};
use serde::Serialize;
use serde_json::json;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use uuid::Uuid;

use hypercolor_core::input::screen::{
    PixelExtent, RegisteredScreenBranchDemand, ScreenAspectPolicy, ScreenExtentRequest,
    ScreenProcessingProfile, ScreenPublicationExecutorRequest, ScreenPublicationKind,
    ScreenPublicationRequest, ScreenSourceSelector, ScreenUpscalePolicy,
};
use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputAttachment, BrowserInputChildKey, BrowserInputHandle,
    BrowserInputRegistryError, BrowserPreviewId,
};
use hypercolor_types::canvas::{DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_types::config::CaptureConfig;
use hypercolor_types::scene::{Scene, SceneId, ZoneId};
use hypercolor_types::spatial::SpatialLayout;

use super::cache::{
    WS_BUFFER_SIZE, WsClientGuard, resolve_canvas_output_size, track_ws_bytes_sent,
};
use super::command::dispatch_command;
use super::interactive_preview_relay::spawn_interactive_preview_relay;
use super::protocol::{
    BrowserInputEdgeWire, ClientMessage, ConfigStanzas, HelloFps, HelloState,
    InteractivePreviewConfig, MAX_WS_MESSAGE_BYTES, NameRef, SceneRef, ServerMessage,
    SubscriptionState, TopicSelection, WsProtocolError, parse_channels, sorted_channel_names,
    unique_sorted_channel_names, validate_interactive_preview_shape, ws_capabilities,
};
use super::relays::{
    PreviewCursorQueue, PreviewOutboundItem, PreviewOutboundSender, PreviewSendCursor,
    WS_PREVIEW_CHUNK_SENT_COUNT, WS_PREVIEW_PUBLICATION_SENT_COUNT, preview_outbound_channel,
    publish_subscriptions,
};
use super::topics::{RelayContext, spawn_relays};
use crate::api::AppState;
use crate::api::effects::active_primary_effect;
use crate::api::layouts::validate_layout_sampling_radii;
use crate::api::local::{
    TrustedLocalSocketTransport, TrustedLocalWebSocket, trusted_local_socket_pair,
};
use crate::api::scenes;
use crate::api::security::RequestAuthContext;
use crate::interaction_routing::{
    AuthoritativeClaimError, AuthoritativeClaimOutcome, InteractionRoutingControl,
};
use crate::interactive_preview::{
    InteractivePreviewExecutor, InteractivePreviewLaneLease,
    InteractivePreviewSpec as RuntimeInteractivePreviewSpec,
    InteractivePreviewTarget as RuntimeInteractivePreviewTarget,
};
use crate::preview_runtime::PreviewPixelFormat;
use crate::render_thread::{
    InputPublicationConsumer, InputPublicationDemand, InputPublicationDemandHandle,
    InputPublicationDemandRegistration, InputScreenBranchDemand,
};
use crate::session::current_global_brightness;

const WS_PROTOCOL_VERSION: &str = "1.0";
const WS_PING_INTERVAL: Duration = Duration::from_secs(30);
const WS_PONG_TIMEOUT: Duration = Duration::from_secs(10);

/// `GET /api/v1/ws` — Upgrade to WebSocket.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    auth_context: Option<Extension<RequestAuthContext>>,
) -> Response {
    if !ws_origin_allowed(&state, &headers) {
        return crate::domain::DomainError::forbidden(
            "Origin is not permitted to open a WebSocket against this daemon",
        )
        .into_response();
    }

    let auth_context =
        auth_context.map_or_else(RequestAuthContext::unsecured, |Extension(context)| context);
    let ws = ws
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES);
    upgrade_handler(ws, move |socket| {
        handle_socket(SessionSocket::Network(socket), state, auth_context, None)
    })
}

pub(crate) fn spawn_trusted_local_socket(
    state: Arc<AppState>,
    runtime: &tokio::runtime::Handle,
) -> TrustedLocalWebSocket {
    let (socket, transport) = trusted_local_socket_pair();
    let shutdown = transport.shutdown_token();
    drop(runtime.spawn(handle_socket(
        SessionSocket::Local(transport),
        state,
        crate::api::security::trusted_local_control_context(),
        Some(shutdown),
    )));
    socket
}

enum SessionSocket {
    Network(WebSocket),
    Local(TrustedLocalSocketTransport),
}

#[derive(Debug, thiserror::Error)]
enum SessionSocketError {
    #[error(transparent)]
    Network(#[from] axum::Error),
    #[error("trusted local websocket transport closed")]
    LocalClosed,
}

impl SessionSocket {
    async fn send(&mut self, message: Message) -> Result<(), SessionSocketError> {
        match self {
            Self::Network(socket) => socket.send(message).await.map_err(Into::into),
            Self::Local(socket) => socket
                .send(message)
                .await
                .map_err(|()| SessionSocketError::LocalClosed),
        }
    }

    async fn recv(&mut self) -> Option<Result<Message, SessionSocketError>> {
        match self {
            Self::Network(socket) => socket.recv().await.map(|result| result.map_err(Into::into)),
            Self::Local(socket) => socket.recv().await.map(Ok),
        }
    }
}

fn ws_origin_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };

    if is_loopback_origin(origin) {
        return true;
    }

    if !state.security_state.security_enabled() {
        return false;
    }

    state
        .config_manager
        .as_ref()
        .map(|config| config.get().web.cors_origins.clone())
        .unwrap_or_default()
        .into_iter()
        .any(|allowed| header_value_eq_origin(origin, &allowed))
}

fn header_value_eq_origin(origin: &HeaderValue, allowed: &str) -> bool {
    origin
        .to_str()
        .is_ok_and(|value| value.eq_ignore_ascii_case(allowed.trim()))
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
    // `Uri::host()` keeps the brackets on IPv6 literals (`[::1]`), which
    // `IpAddr` will not parse — strip them before classifying the address.
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Process a single WebSocket connection.
#[expect(
    clippy::too_many_lines,
    reason = "Socket loop coordinates handshake, heartbeats, relay queues, and client messages"
)]
async fn handle_socket(
    mut socket: SessionSocket,
    state: Arc<AppState>,
    auth_context: RequestAuthContext,
    shutdown: Option<CancellationToken>,
) {
    let _client_guard = WsClientGuard::register();

    let initial_subscriptions = SubscriptionState::default();
    let (subscriptions_tx, subscriptions_rx) = watch::channel(initial_subscriptions.clone());
    let mut subscriptions = initial_subscriptions;
    let default_capture = CaptureConfig::default();
    let (screen_base_width, screen_base_height, screen_grid_cols, screen_grid_rows) =
        state.config_manager.as_ref().map_or(
            (
                DEFAULT_CANVAS_WIDTH,
                DEFAULT_CANVAS_HEIGHT,
                default_capture.grid_cols,
                default_capture.grid_rows,
            ),
            |manager| {
                let config = manager.get();
                (
                    config.daemon.canvas_width,
                    config.daemon.canvas_height,
                    config.capture.grid_cols,
                    config.capture.grid_rows,
                )
            },
        );
    let Ok(screen_base_extent) = PixelExtent::new(screen_base_width, screen_base_height) else {
        warn!(
            width = screen_base_width,
            height = screen_base_height,
            "Cannot open WebSocket with an empty passive screen extent"
        );
        return;
    };
    let mut input_demand_leases = WsInputDemandLeases::new(
        state.input_publication_demands.clone(),
        state.configured_max_fps_tier.get().fps(),
        screen_base_extent,
        screen_grid_cols,
        screen_grid_rows,
    );

    // Send hello message.
    let hello = {
        ServerMessage::Hello {
            version: WS_PROTOCOL_VERSION.to_owned(),
            server: state.server_identity.clone(),
            state: build_hello_state(&state).await,
            capabilities: ws_capabilities(),
            subscriptions: sorted_channel_names(subscriptions.topics()),
        }
    };
    if send_json(&mut socket, &hello).await.is_err() {
        return;
    }

    // JSON and small binary telemetry stay count-bounded. Preview surfaces use
    // a keyed, byte-accounted latest-value router below.
    let (json_tx, mut json_rx) = tokio::sync::mpsc::channel::<Utf8Bytes>(WS_BUFFER_SIZE);
    let (binary_tx, mut binary_rx) = tokio::sync::mpsc::channel::<Bytes>(WS_BUFFER_SIZE);
    let (preview_tx, preview_rx) = preview_outbound_channel();
    let mut browser_previews = BrowserPreviewSession::new(
        state.browser_input.clone(),
        state.interaction_routing.clone(),
        state.preview_runtime.interactive_executor(),
        preview_tx.clone(),
    );

    // Spawn every registered relay task — each watches immutable
    // subscription snapshots, and the registry decides which topics a
    // task serves.
    let relay_handles = spawn_relays(&RelayContext {
        state: Arc::clone(&state),
        json_tx: json_tx.clone(),
        binary_tx: binary_tx.clone(),
        preview_tx: preview_tx.clone(),
        subscriptions: subscriptions_rx.clone(),
    });

    let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut awaiting_pong = false;
    let mut ping_sent_at = Instant::now();
    let mut zone_layout_preview_keys = HashSet::<(SceneId, ZoneId)>::new();
    let mut preview_capability = PreviewTransportCapability::default().legacy_v1();
    let mut preview_cursors = PreviewCursorQueue::with_capability(preview_capability);
    // Main loop: multiplex between incoming client messages and outbound events.
    loop {
        tokio::select! {
            () = wait_for_shutdown(shutdown.as_ref()) => break,

            // Outbound JSON: bounded queue (drop under pressure in producer tasks).
            json_msg = json_rx.recv() => {
                match json_msg {
                    Some(msg) => {
                        let sent_len = msg.len();
                        if socket.send(Message::Text(msg)).await.is_err() {
                            break;
                        }
                        track_ws_bytes_sent(sent_len);
                    }
                    None => break,
                }
            }

            binary_msg = binary_rx.recv() => {
                let Some(bytes) = binary_msg else {
                    break;
                };
                let sent_len = bytes.len();
                if socket.send(Message::Binary(bytes)).await.is_err() {
                    break;
                }
                track_ws_bytes_sent(sent_len);
            }

            outbound = preview_rx.recv() => {
                match outbound {
                    PreviewOutboundItem::Cancellation(cancellation) => {
                        if let Some(cursor) = preview_cursors.remove_cancelled(&cancellation) {
                            preview_rx.complete(cursor.publication());
                        }
                        match cancellation.try_encode() {
                            Ok(message) => {
                                let sent_len = message.len();
                                if socket.send(Message::Binary(message)).await.is_err() {
                                    break;
                                }
                                track_ws_bytes_sent(sent_len);
                            }
                            Err(error) => warn!(%error, "Failed to encode WebSocket preview cancellation"),
                        }
                    }
                    PreviewOutboundItem::Publication(publication) => {
                        let stream = publication.stream().clone();
                        let publication_id = publication.publication_id();
                        match PreviewSendCursor::with_capability(publication, preview_capability) {
                            Ok(cursor) => match preview_cursors.try_insert(cursor) {
                                Ok(Some(replaced)) => preview_rx.complete(replaced.publication()),
                                Ok(None) => {}
                                Err(error) => {
                                    warn!(%error, "Rejected queued WebSocket preview cursor");
                                    preview_rx.complete_publication(&stream, publication_id);
                                }
                            },
                            Err(error) => {
                                warn!(%error, "Rejected queued WebSocket preview publication");
                                preview_rx.complete_publication(&stream, publication_id);
                            }
                        }
                    }
                }
            }

            () = tokio::task::yield_now(), if !preview_cursors.is_empty() => {
                let mut cursor = preview_cursors
                    .pop_next()
                    .expect("preview cursor exists behind select guard");
                let interactive_is_current = cursor
                    .publication()
                    .interactive_fence()
                    .is_none_or(|(preview_id, publication_id)| {
                        browser_previews.is_current_publication(preview_id, publication_id)
                    });
                if !preview_rx.is_current(cursor.publication()) || !interactive_is_current {
                    if let Ok(message) = (hypercolor_leptos_ext::ws::PreviewCancelFrame {
                        stream: cursor.publication().stream().clone(),
                        publication_id: cursor.publication().publication_id(),
                    }).try_encode() {
                        let sent_len = message.len();
                        if socket.send(Message::Binary(message)).await.is_err() {
                            preview_rx.complete(cursor.publication());
                            break;
                        }
                        track_ws_bytes_sent(sent_len);
                    }
                    preview_rx.complete(cursor.publication());
                    continue;
                }
                let message = match cursor.next_message() {
                    Ok(Some(message)) => message,
                    Ok(None) => {
                        preview_rx.complete(cursor.publication());
                        continue;
                    }
                    Err(error) => {
                        warn!(%error, "Failed to encode WebSocket preview chunk");
                        if let Ok(message) = (hypercolor_leptos_ext::ws::PreviewCancelFrame {
                            stream: cursor.publication().stream().clone(),
                            publication_id: cursor.publication().publication_id(),
                        }).try_encode() {
                            let sent_len = message.len();
                            if socket.send(Message::Binary(message)).await.is_err() {
                                preview_rx.complete(cursor.publication());
                                break;
                            }
                            track_ws_bytes_sent(sent_len);
                        }
                        preview_rx.complete(cursor.publication());
                        continue;
                    }
                };
                let sent_len = message.len();
                if socket.send(Message::Binary(message)).await.is_err() {
                    break;
                }
                track_ws_bytes_sent(sent_len);
                if cursor.is_chunked() {
                    WS_PREVIEW_CHUNK_SENT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                if cursor.is_complete() {
                    WS_PREVIEW_PUBLICATION_SENT_COUNT
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    preview_rx.complete(cursor.publication());
                } else {
                    preview_cursors.requeue(cursor);
                }
            }

            // Keepalive heartbeat.
            _ = ping_interval.tick() => {
                if awaiting_pong && ping_sent_at.elapsed() >= WS_PONG_TIMEOUT {
                    warn!("WebSocket client timed out waiting for pong");
                    break;
                }

                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
                awaiting_pong = true;
                ping_sent_at = Instant::now();
            }

            // Inbound: process client messages.
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_client_message(
                            &text,
                            &state,
                            auth_context,
                            &mut subscriptions,
                            &subscriptions_tx,
                            &mut input_demand_leases,
                            &mut zone_layout_preview_keys,
                            &mut browser_previews,
                            &preview_tx,
                            &mut preview_capability,
                            &mut preview_cursors,
                            &mut socket,
                        )
                        .await;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        awaiting_pong = false;
                    }
                    Some(Ok(Message::Ping(payload)))
                        if socket
                            .send(Message::Pong(payload.clone()))
                            .await
                            .is_err() =>
                    {
                        break;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        warn!("WebSocket recv error: {e}");
                        break;
                    }
                    _ => {} // Ignore binary/ping/pong for now.
                }
            }
        }
    }

    abort_and_join_relays(relay_handles).await;
    browser_previews.shutdown().await;
    while let Some(cursor) = preview_cursors.pop_next() {
        preview_rx.complete(cursor.publication());
    }
    drop(input_demand_leases);
    state
        .zone_layout_previews
        .clear_many(zone_layout_preview_keys)
        .await;
    debug!("WebSocket client disconnected");
}

async fn wait_for_shutdown(shutdown: Option<&CancellationToken>) {
    match shutdown {
        Some(shutdown) => shutdown.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn abort_and_join_relays(handles: Vec<JoinHandle<()>>) {
    for handle in &handles {
        handle.abort();
    }
    for handle in handles {
        let _ = handle.await;
    }
}

pub(super) struct WsInputDemandLeases {
    demands: InputPublicationDemandHandle,
    interaction_hz: u32,
    screen_base_extent: PixelExtent,
    screen_grid_cols: NonZeroU32,
    screen_grid_rows: NonZeroU32,
    spectrum: Option<InputPublicationDemandRegistration>,
    screen: Option<InputPublicationDemandRegistration>,
    interaction: Option<InputPublicationDemandRegistration>,
    #[cfg(test)]
    screen_requested_extent: Option<PixelExtent>,
}

impl WsInputDemandLeases {
    pub(super) fn new(
        demands: InputPublicationDemandHandle,
        interaction_hz: u32,
        screen_base_extent: PixelExtent,
        screen_grid_cols: u32,
        screen_grid_rows: u32,
    ) -> Self {
        Self {
            demands,
            interaction_hz,
            screen_base_extent,
            screen_grid_cols: NonZeroU32::new(screen_grid_cols)
                .expect("validated capture grid columns are non-zero"),
            screen_grid_rows: NonZeroU32::new(screen_grid_rows)
                .expect("validated capture grid rows are non-zero"),
            spectrum: None,
            screen: None,
            interaction: None,
            #[cfg(test)]
            screen_requested_extent: None,
        }
    }

    /// Work out what engine demand a subscription state implies,
    /// without registering any of it. Every rejection this projection
    /// can raise happens before a single lease moves, so a subscribe
    /// that fails here leaves the engine exactly as it was.
    pub(super) fn project(
        &self,
        subscriptions: &SubscriptionState,
    ) -> Result<ProjectedInputDemand, WsProtocolError> {
        let screen_canvas = subscriptions.config_of::<CanvasConfig>(TopicId::ScreenCanvas);
        let screen_active = subscriptions.contains(TopicId::ScreenCanvas)
            || subscriptions.contains(TopicId::ScreenZones);
        let (screen_demand, screen_requested_extent) = if screen_active {
            let requested_hz = NonZeroU32::new(screen_canvas.fps).ok_or_else(|| {
                WsProtocolError::invalid_config(
                    "config.screen_canvas.fps",
                    "expected a non-zero cadence",
                )
            })?;
            let mut branches = Vec::with_capacity(2);
            let mut requested_extent = None;
            if subscriptions.contains(TopicId::ScreenCanvas) {
                let output = resolve_canvas_output_size(
                    self.screen_base_extent.width(),
                    self.screen_base_extent.height(),
                    screen_canvas.width,
                    screen_canvas.height,
                )
                .map_err(|error| {
                    WsProtocolError::invalid_config_resource(
                        "config.screen_canvas",
                        screen_canvas.width,
                        screen_canvas.height,
                        error.to_string(),
                    )
                })?;
                let canvas_extent =
                    PixelExtent::new(output.width, output.height).map_err(|error| {
                        WsProtocolError::invalid_config_resource(
                            "config.screen_canvas",
                            output.width,
                            output.height,
                            error.to_string(),
                        )
                    })?;
                let extent_request = ScreenExtentRequest::bounded(
                    NonZeroU32::new(screen_canvas.width),
                    NonZeroU32::new(screen_canvas.height),
                    ScreenUpscalePolicy::Never,
                );
                branches.push(screen_branch_demand(
                    ScreenPublicationKind::Surface,
                    extent_request,
                    requested_hz,
                    canvas_extent,
                ));
                requested_extent = Some(canvas_extent);
            }
            if subscriptions.contains(TopicId::ScreenZones) {
                let extent_request = ScreenExtentRequest::bounded(
                    NonZeroU32::new(self.screen_base_extent.width()),
                    NonZeroU32::new(self.screen_base_extent.height()),
                    ScreenUpscalePolicy::Never,
                );
                branches.push(screen_branch_demand(
                    ScreenPublicationKind::Zones {
                        columns: self.screen_grid_cols,
                        rows: self.screen_grid_rows,
                    },
                    extent_request,
                    requested_hz,
                    self.screen_base_extent,
                ));
                requested_extent.get_or_insert(self.screen_base_extent);
            }
            let requested_extent =
                requested_extent.expect("an active screen subscription has an extent");
            (
                Some(InputPublicationDemand::default().with_screen_branches(branches)),
                Some(requested_extent),
            )
        } else {
            (None, None)
        };

        // Only the tests read the resolved extent back; production
        // reads it through the registered branches instead.
        #[cfg(not(test))]
        let _ = screen_requested_extent;

        Ok(ProjectedInputDemand {
            spectrum: subscriptions.contains(TopicId::Spectrum).then(|| {
                InputPublicationDemand::default().with_source(
                    hypercolor_core::input::SourceKind::Audio,
                    subscriptions
                        .config_of::<SpectrumConfig>(TopicId::Spectrum)
                        .fps,
                )
            }),
            screen: screen_demand,
            interaction: subscriptions.contains(TopicId::InputEvents).then(|| {
                InputPublicationDemand::default().with_source(
                    hypercolor_core::input::SourceKind::Interaction,
                    self.interaction_hz,
                )
            }),
            #[cfg(test)]
            screen_requested_extent,
        })
    }

    /// Register a projection. Infallible by construction: everything
    /// that could refuse already did.
    pub(super) fn commit(&mut self, projected: ProjectedInputDemand) {
        Self::synchronize_domain(&self.demands, &mut self.spectrum, projected.spectrum);
        Self::synchronize_domain(&self.demands, &mut self.screen, projected.screen);
        Self::synchronize_domain(&self.demands, &mut self.interaction, projected.interaction);
        #[cfg(test)]
        {
            self.screen_requested_extent = projected.screen_requested_extent;
        }
    }

    /// Project and commit in one step. Production splits the two so a
    /// later rejection cannot strand a half-applied lease.
    #[cfg(test)]
    pub(super) fn synchronize(
        &mut self,
        subscriptions: &SubscriptionState,
    ) -> Result<(), WsProtocolError> {
        let projected = self.project(subscriptions)?;
        self.commit(projected);
        Ok(())
    }

    #[cfg(test)]
    pub(super) const fn screen_requested_extent(&self) -> Option<PixelExtent> {
        self.screen_requested_extent
    }

    fn synchronize_domain(
        demands: &InputPublicationDemandHandle,
        registration: &mut Option<InputPublicationDemandRegistration>,
        demand: Option<InputPublicationDemand>,
    ) {
        match (registration.as_ref(), demand) {
            (Some(registration), Some(demand)) => registration.update(demand),
            (None, Some(demand)) => {
                *registration =
                    Some(demands.register(InputPublicationConsumer::PassiveStream, demand));
            }
            (Some(_), None) => *registration = None,
            (None, None) => {}
        }
    }
}

/// The engine demand one subscription state implies, computed but not
/// yet registered.
pub(super) struct ProjectedInputDemand {
    spectrum: Option<InputPublicationDemand>,
    screen: Option<InputPublicationDemand>,
    interaction: Option<InputPublicationDemand>,
    #[cfg(test)]
    screen_requested_extent: Option<PixelExtent>,
}

fn screen_branch_demand(
    kind: ScreenPublicationKind,
    extent: ScreenExtentRequest,
    requested_hz: NonZeroU32,
    legacy_extent: PixelExtent,
) -> InputScreenBranchDemand {
    let request = ScreenPublicationRequest::new(
        ScreenSourceSelector::Configured,
        kind,
        ScreenPublicationExecutorRequest::Cpu,
        extent,
        ScreenAspectPolicy::Contain,
        Arc::new(ScreenProcessingProfile::default()),
    );
    InputScreenBranchDemand::new(
        RegisteredScreenBranchDemand::new(request, requested_hz),
        legacy_extent,
    )
}

/// A stable server-assigned identity for one WebSocket connection lifetime.
fn next_browser_connection_incarnation() -> BrowserConnectionIncarnation {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let value = COUNTER
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .expect("browser connection incarnation exhausted");
    BrowserConnectionIncarnation::new(value)
}

/// Owns every interactive preview attached to one WebSocket connection.
pub(super) struct BrowserPreviewSession {
    connection: BrowserConnectionIncarnation,
    browser_input: BrowserInputHandle,
    interaction_routing: InteractionRoutingControl,
    executor: Option<Arc<InteractivePreviewExecutor>>,
    outbound: PreviewOutboundSender,
    previews: HashMap<String, BrowserPreviewBinding>,
}

struct BrowserPreviewBinding {
    attachment: BrowserInputAttachment,
    config: InteractivePreviewConfig,
    lane: InteractivePreviewLaneLease,
    relay_cancel: CancellationToken,
    relay: JoinHandle<()>,
}

impl BrowserPreviewSession {
    pub(super) fn new(
        browser_input: BrowserInputHandle,
        interaction_routing: InteractionRoutingControl,
        executor: Option<Arc<InteractivePreviewExecutor>>,
        outbound: PreviewOutboundSender,
    ) -> Self {
        Self {
            connection: next_browser_connection_incarnation(),
            browser_input,
            interaction_routing,
            executor,
            outbound,
            previews: HashMap::new(),
        }
    }

    pub(super) async fn open(
        &mut self,
        preview_id: String,
        config: InteractivePreviewConfig,
    ) -> Result<ServerMessage, WsProtocolError> {
        let error_preview_id = preview_id.clone();
        self.open_unscoped(preview_id, config)
            .await
            .map_err(|error| scope_preview_error(error, &error_preview_id))
    }

    async fn open_unscoped(
        &mut self,
        preview_id: String,
        config: InteractivePreviewConfig,
    ) -> Result<ServerMessage, WsProtocolError> {
        validate_interactive_preview_shape(config.width, config.height, config.format)?;
        self.outbound
            .cancel(&hypercolor_leptos_ext::ws::PreviewStreamId::Interactive(
                preview_id.clone(),
            ))
            .map_err(|error| WsProtocolError::invalid_request(error.to_string()))?;
        if let Some(binding) = self.previews.get_mut(&preview_id) {
            binding
                .lane
                .resize_or_retarget(runtime_preview_spec(config))
                .await
                .map_err(interactive_preview_error)?;
            binding.config = config;
            return Ok(ServerMessage::InteractivePreviewOpened {
                preview_id,
                connection_incarnation: self.connection.get(),
                publication_id: binding.attachment.publication_id().get(),
                already_open: true,
                config,
            });
        }

        let executor = self.executor.clone().ok_or_else(|| WsProtocolError {
            code: "unavailable",
            message: "Interactive preview rendering is unavailable".to_owned(),
            details: None,
        })?;
        let key =
            BrowserInputChildKey::new(self.connection, BrowserPreviewId::new(preview_id.clone()));
        let attachment = self
            .browser_input
            .attach(key)
            .map_err(browser_registry_error)?;
        let lane = match executor
            .open(&attachment, runtime_preview_spec(config))
            .await
        {
            Ok(lane) => lane,
            Err(error) => {
                self.interaction_routing.close_preview(&attachment);
                return Err(interactive_preview_error(error));
            }
        };
        let publication_id = attachment.publication_id().get();
        let spec_generation = lane.spec_generation_receiver();
        let encode_workers = lane.encode_workers();
        let relay_cancel = CancellationToken::new();
        let relay = spawn_interactive_preview_relay(
            preview_id.clone(),
            attachment.publication_id(),
            lane.frame_receiver(),
            spec_generation,
            encode_workers,
            self.outbound.clone(),
            relay_cancel.clone(),
        );
        self.previews.insert(
            preview_id.clone(),
            BrowserPreviewBinding {
                attachment,
                config,
                lane,
                relay_cancel,
                relay,
            },
        );
        Ok(ServerMessage::InteractivePreviewOpened {
            preview_id,
            connection_incarnation: self.connection.get(),
            publication_id,
            already_open: false,
            config,
        })
    }

    pub(super) async fn close(&mut self, preview_id: String) -> ServerMessage {
        let binding = self.previews.remove(&preview_id);
        if let Some(binding) = &binding {
            binding.relay_cancel.cancel();
        }
        if let Err(error) =
            self.outbound
                .cancel(&hypercolor_leptos_ext::ws::PreviewStreamId::Interactive(
                    preview_id.clone(),
                ))
        {
            warn!(%error, %preview_id, "Failed to queue interactive preview cancellation");
        }
        let closed = if let Some(binding) = binding {
            close_preview_binding_and_wait(&self.interaction_routing, binding).await;
            true
        } else {
            false
        };
        ServerMessage::InteractivePreviewClosed { preview_id, closed }
    }

    pub(super) fn is_current_publication(
        &self,
        preview_id: &str,
        publication_id: hypercolor_core::input::BrowserInputPublicationId,
    ) -> bool {
        self.publication_id(preview_id) == Some(publication_id)
    }

    pub(super) fn publication_id(
        &self,
        preview_id: &str,
    ) -> Option<hypercolor_core::input::BrowserInputPublicationId> {
        self.previews
            .get(preview_id)
            .map(|binding| binding.attachment.publication_id())
    }

    pub(super) fn inject(
        &self,
        preview_id: String,
        events: Vec<BrowserInputEdgeWire>,
    ) -> Result<ServerMessage, WsProtocolError> {
        let attachment = self.active_attachment(&preview_id)?;
        let accepted_events = events.len();
        attachment
            .inject(events.into_iter().map(BrowserInputEdgeWire::into_edge))
            .map_err(browser_registry_error)?;
        Ok(ServerMessage::InputInjected {
            preview_id,
            accepted_events,
        })
    }

    pub(super) fn claim_authoritative(
        &self,
        preview_id: String,
    ) -> Result<ServerMessage, WsProtocolError> {
        let attachment = self.active_attachment(&preview_id)?;
        let outcome = self
            .interaction_routing
            .claim_authoritative(attachment)
            .map_err(|error| authoritative_claim_error(&preview_id, error))?;
        Ok(ServerMessage::InteractivePreviewAuthoritativeClaimed {
            preview_id,
            already_owned: outcome == AuthoritativeClaimOutcome::AlreadyOwned,
        })
    }

    pub(super) fn release_authoritative(&self, preview_id: String) -> ServerMessage {
        let released = self.previews.get(&preview_id).is_some_and(|binding| {
            self.interaction_routing
                .release_authoritative(&binding.attachment)
        });
        ServerMessage::InteractivePreviewAuthoritativeReleased {
            preview_id,
            released,
        }
    }

    async fn shutdown(&mut self) {
        let bindings = self
            .previews
            .drain()
            .map(|(_, binding)| binding)
            .collect::<Vec<_>>();
        for binding in bindings {
            close_preview_binding_and_wait(&self.interaction_routing, binding).await;
        }
    }

    fn active_attachment(
        &self,
        preview_id: &str,
    ) -> Result<&BrowserInputAttachment, WsProtocolError> {
        self.previews
            .get(preview_id)
            .map(|binding| &binding.attachment)
            .ok_or_else(|| {
                WsProtocolError::invalid_request(format!(
                    "Interactive preview '{preview_id}' is not active on this connection"
                ))
            })
    }
}

impl Drop for BrowserPreviewSession {
    fn drop(&mut self) {
        for (_, binding) in self.previews.drain() {
            begin_preview_binding_cleanup(&self.interaction_routing, binding);
        }
    }
}

fn begin_preview_binding_cleanup(
    interaction_routing: &InteractionRoutingControl,
    binding: BrowserPreviewBinding,
) {
    binding.relay_cancel.cancel();
    interaction_routing.close_preview(&binding.attachment);
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(finish_preview_binding_cleanup(binding)));
    } else {
        let mut lane = binding.lane;
        binding.relay.abort();
        let _ = lane.close();
    }
}

async fn close_preview_binding_and_wait(
    interaction_routing: &InteractionRoutingControl,
    binding: BrowserPreviewBinding,
) {
    binding.relay_cancel.cancel();
    interaction_routing.close_preview(&binding.attachment);
    finish_preview_binding_cleanup(binding).await;
}

async fn finish_preview_binding_cleanup(binding: BrowserPreviewBinding) {
    let BrowserPreviewBinding {
        mut lane,
        relay,
        attachment: _,
        config: _,
        relay_cancel: _,
    } = binding;
    let _ = relay.await;
    let _ = lane.close_and_wait().await;
}

const fn runtime_preview_spec(config: InteractivePreviewConfig) -> RuntimeInteractivePreviewSpec {
    RuntimeInteractivePreviewSpec {
        target: match config.target {
            super::protocol::InteractivePreviewTarget::ActiveScene => {
                RuntimeInteractivePreviewTarget::ActiveScene
            }
        },
        fps: config.fps,
        width: config.width,
        height: config.height,
        format: match config.format {
            CanvasFormat::Rgb => PreviewPixelFormat::Rgb,
            CanvasFormat::Rgba => PreviewPixelFormat::Rgba,
            CanvasFormat::Jpeg => PreviewPixelFormat::Jpeg,
        },
    }
}

fn interactive_preview_error(
    error: crate::interactive_preview::InteractivePreviewError,
) -> WsProtocolError {
    WsProtocolError::invalid_request(error.to_string())
}

fn scope_preview_error(mut error: WsProtocolError, preview_id: &str) -> WsProtocolError {
    match error.details.as_mut() {
        Some(serde_json::Value::Object(details)) => {
            details.insert("preview_id".to_owned(), json!(preview_id));
        }
        _ => error.details = Some(json!({"preview_id": preview_id})),
    }
    error
}

fn browser_registry_error(error: BrowserInputRegistryError) -> WsProtocolError {
    WsProtocolError::invalid_request(error.to_string())
}

fn authoritative_claim_error(preview_id: &str, error: AuthoritativeClaimError) -> WsProtocolError {
    match error {
        AuthoritativeClaimError::PreviewInactive => WsProtocolError::invalid_request(format!(
            "Interactive preview '{preview_id}' is not active on this connection"
        )),
        AuthoritativeClaimError::Conflict => WsProtocolError {
            code: "conflict",
            message: "Another interactive preview owns authoritative browser input".to_owned(),
            details: Some(json!({"preview_id": preview_id})),
        },
    }
}

pub(super) fn authorize_subscription_channels(
    auth_context: RequestAuthContext,
    selections: &[TopicSelection],
) -> Result<(), WsProtocolError> {
    if auth_context.can_control() {
        return Ok(());
    }

    let restricted_channels: Vec<&'static str> = selections
        .iter()
        .map(|selection| selection.topic)
        .filter(|topic| topic.requires_control())
        .map(TopicId::as_str)
        .collect();

    if restricted_channels.is_empty() {
        Ok(())
    } else {
        Err(WsProtocolError::forbidden(
            "Screen capture preview subscriptions require a control-tier API key",
            json!({"channels": restricted_channels, "required_tier": "control"}),
        ))
    }
}

/// A transport capability both ends have agreed on but neither has
/// adopted yet.
pub(super) struct StagedPreviewTransport {
    peer: PreviewTransportCapability,
    negotiated: PreviewTransportCapability,
}

/// Work out the capability this subscribe would settle on, touching
/// nothing. A subscribe that goes on to fail its demand projection must
/// leave the connection speaking exactly the transport it spoke before.
pub(super) fn stage_preview_transport(
    encoded_capability: &str,
    preview_outbound: &PreviewOutboundSender,
    preview_cursors: &PreviewCursorQueue,
) -> Result<StagedPreviewTransport, WsProtocolError> {
    let peer = PreviewTransportCapability::decode(encoded_capability).map_err(|error| {
        WsProtocolError::invalid_request(format!("Invalid preview_transport capability: {error}"))
    })?;
    let negotiated = preview_outbound
        .project_negotiation(peer)
        .map_err(|error| WsProtocolError::invalid_request(error.to_string()))?;
    preview_cursors
        .check_capability(negotiated)
        .map_err(|error| WsProtocolError::invalid_request(error.to_string()))?;
    Ok(StagedPreviewTransport { peer, negotiated })
}

/// Adopt a staged capability. The sender re-checks and swaps under one
/// lock, and the cursor queue already proved it fits, so this either
/// lands whole or refuses without changing anything.
pub(super) fn commit_preview_transport(
    staged: StagedPreviewTransport,
    preview_outbound: &PreviewOutboundSender,
    preview_cursors: &mut PreviewCursorQueue,
    preview_capability: &mut PreviewTransportCapability,
) -> Result<PreviewTransportCapability, WsProtocolError> {
    let negotiated = preview_outbound
        .negotiate_transport(staged.peer)
        .map_err(|error| WsProtocolError::invalid_request(error.to_string()))?;
    debug_assert_eq!(
        negotiated, staged.negotiated,
        "committing a staged transport must settle on what staging projected"
    );
    preview_cursors
        .set_capability(negotiated)
        .map_err(|error| WsProtocolError::invalid_request(error.to_string()))?;
    *preview_capability = negotiated;
    Ok(negotiated)
}

#[cfg(test)]
pub(super) fn negotiate_preview_transport(
    encoded_capability: &str,
    preview_outbound: &PreviewOutboundSender,
    preview_cursors: &mut PreviewCursorQueue,
    preview_capability: &mut PreviewTransportCapability,
) -> Result<PreviewTransportCapability, WsProtocolError> {
    let staged = stage_preview_transport(encoded_capability, preview_outbound, preview_cursors)?;
    commit_preview_transport(
        staged,
        preview_outbound,
        preview_cursors,
        preview_capability,
    )
}

/// Process a client subscription/unsubscription message.
async fn handle_client_message(
    text: &str,
    state: &Arc<AppState>,
    auth_context: RequestAuthContext,
    subscriptions: &mut SubscriptionState,
    subscriptions_tx: &watch::Sender<SubscriptionState>,
    input_demand_leases: &mut WsInputDemandLeases,
    zone_layout_preview_keys: &mut HashSet<(SceneId, ZoneId)>,
    browser_previews: &mut BrowserPreviewSession,
    preview_outbound: &PreviewOutboundSender,
    preview_capability: &mut PreviewTransportCapability,
    preview_cursors: &mut PreviewCursorQueue,
    socket: &mut SessionSocket,
) {
    let msg = match serde_json::from_str::<ClientMessage>(text) {
        Ok(msg) => msg,
        Err(error) => {
            let _ = send_json(
                socket,
                &WsProtocolError::invalid_request(format!("Invalid JSON message: {error}"))
                    .into_message(),
            )
            .await;
            return;
        }
    };

    match msg {
        ClientMessage::Subscribe {
            channels,
            config,
            preview_transport,
        } => {
            // Validate the whole request first. Every step below builds
            // a candidate and refuses on its own terms, so a request
            // that names four topics and mis-configures the fourth
            // leaves the connection exactly as it was.
            let selections = match parse_channels(&channels) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let _ = send_json(socket, &error.into_message()).await;
                    return;
                }
            };

            if let Err(error) = authorize_subscription_channels(auth_context, &selections) {
                let _ = send_json(socket, &error.into_message()).await;
                return;
            }

            let next_subscriptions = match subscriptions
                .subscribe(&selections, config.as_ref().map(ConfigStanzas::stanzas))
            {
                Ok(next) => next,
                Err(error) => {
                    let _ = send_json(socket, &error.into_message()).await;
                    return;
                }
            };

            let staged_transport = match preview_transport
                .as_deref()
                .map(|encoded| stage_preview_transport(encoded, preview_outbound, preview_cursors))
                .transpose()
            {
                Ok(staged) => staged,
                Err(error) => {
                    let _ = send_json(socket, &error.into_message()).await;
                    return;
                }
            };

            let projected_demand = match input_demand_leases.project(&next_subscriptions) {
                Ok(projected) => projected,
                Err(error) => {
                    let _ = send_json(socket, &error.into_message()).await;
                    return;
                }
            };

            // Commit phase. The transport goes first because adopting it
            // is the only step that can still refuse, and it refuses
            // without having changed anything.
            if let Some(staged) = staged_transport
                && let Err(error) = commit_preview_transport(
                    staged,
                    preview_outbound,
                    preview_cursors,
                    preview_capability,
                )
            {
                let _ = send_json(socket, &error.into_message()).await;
                return;
            }
            input_demand_leases.commit(projected_demand);
            *subscriptions = next_subscriptions;

            let ack = ServerMessage::Subscribed {
                channels: unique_sorted_channel_names(&selections),
                config: subscriptions.config_projection(),
                preview_transport: preview_capability.encode(),
            };
            publish_subscriptions(subscriptions_tx, subscriptions);
            let _ = send_json(socket, &ack).await;
        }
        ClientMessage::Unsubscribe { channels } => {
            let selections = match parse_channels(&channels) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let _ = send_json(socket, &error.into_message()).await;
                    return;
                }
            };

            let next_subscriptions = subscriptions.unsubscribe(&selections);
            let projected_demand = match input_demand_leases.project(&next_subscriptions) {
                Ok(projected) => projected,
                Err(error) => {
                    let _ = send_json(socket, &error.into_message()).await;
                    return;
                }
            };
            input_demand_leases.commit(projected_demand);
            *subscriptions = next_subscriptions;
            for selection in &selections {
                if let Err(error) = preview_outbound.cancel_topic(selection.topic) {
                    warn!(%error, channel = selection.topic.as_str(), "Failed to cancel unsubscribed preview channel");
                }
            }
            let remaining = sorted_channel_names(subscriptions.topics());

            let ack = ServerMessage::Unsubscribed {
                channels: unique_sorted_channel_names(&selections),
                remaining,
            };
            publish_subscriptions(subscriptions_tx, subscriptions);
            let _ = send_json(socket, &ack).await;
        }
        ClientMessage::Command {
            id,
            method,
            path,
            body,
        } => {
            let response = dispatch_command(state, auth_context, id, method, path, body).await;
            let _ = send_json(socket, &response).await;
        }
        ClientMessage::ZoneLayoutPreview {
            scene_id,
            zone_id,
            layout,
        } => {
            if let Err(error) = ensure_control_tier(auth_context) {
                let _ = send_json(socket, &error.into_message()).await;
                return;
            }

            if let Err(error) = handle_zone_layout_preview(
                state,
                zone_layout_preview_keys,
                scene_id,
                zone_id,
                layout,
            )
            .await
            {
                let _ = send_json(socket, &error.into_message()).await;
            }
        }
        ClientMessage::ZoneLayoutPreviewClear { scene_id, zone_id } => {
            if let Err(error) = ensure_control_tier(auth_context) {
                let _ = send_json(socket, &error.into_message()).await;
                return;
            }

            if let Err(error) = handle_zone_layout_preview_clear(
                state,
                zone_layout_preview_keys,
                &scene_id,
                &zone_id,
            )
            .await
            {
                let _ = send_json(socket, &error.into_message()).await;
            }
        }
        ClientMessage::InteractivePreviewOpen {
            preview_id,
            target,
            fps,
            width,
            height,
            format,
        } => {
            let error_preview_id = preview_id.clone();
            let config = InteractivePreviewConfig {
                target,
                fps,
                width,
                height,
                format,
            };
            let result = match ensure_control_tier(auth_context) {
                Ok(()) => browser_previews.open(preview_id, config).await,
                Err(error) => Err(error),
            }
            .map_err(|error| scope_preview_error(error, &error_preview_id));
            send_protocol_result(socket, result).await;
        }
        ClientMessage::InteractivePreviewClose { preview_id } => {
            let error_preview_id = preview_id.clone();
            let result = match ensure_control_tier(auth_context) {
                Ok(()) => Ok(browser_previews.close(preview_id).await),
                Err(error) => Err(error),
            }
            .map_err(|error| scope_preview_error(error, &error_preview_id));
            send_protocol_result(socket, result).await;
        }
        ClientMessage::InputInject { preview_id, events } => {
            let result = ensure_control_tier(auth_context)
                .and_then(|()| browser_previews.inject(preview_id, events));
            send_protocol_result(socket, result).await;
        }
        ClientMessage::InteractivePreviewClaimAuthoritative { preview_id } => {
            let result = ensure_control_tier(auth_context)
                .and_then(|()| browser_previews.claim_authoritative(preview_id));
            send_protocol_result(socket, result).await;
        }
        ClientMessage::InteractivePreviewReleaseAuthoritative { preview_id } => {
            let result = ensure_control_tier(auth_context)
                .map(|()| browser_previews.release_authoritative(preview_id));
            send_protocol_result(socket, result).await;
        }
    }
}

async fn send_protocol_result(
    socket: &mut SessionSocket,
    result: Result<ServerMessage, WsProtocolError>,
) {
    let message = result.unwrap_or_else(WsProtocolError::into_message);
    let _ = send_json(socket, &message).await;
}

fn ensure_control_tier(auth_context: RequestAuthContext) -> Result<(), WsProtocolError> {
    if auth_context.can_control() {
        Ok(())
    } else {
        Err(WsProtocolError::forbidden(
            "Read-only API key cannot perform write operations",
            serde_json::json!({"required_tier": "control"}),
        ))
    }
}

async fn handle_zone_layout_preview(
    state: &Arc<AppState>,
    zone_layout_preview_keys: &mut HashSet<(SceneId, ZoneId)>,
    scene_id_raw: String,
    zone_id_raw: String,
    layout: SpatialLayout,
) -> Result<(), WsProtocolError> {
    let zone_id = parse_zone_preview_id(&zone_id_raw)?;
    let (scene_id, layout) = {
        let manager = state.scene_manager.read().await;
        let scene_id = scenes::resolve_scene_id(&manager, &scene_id_raw).ok_or_else(|| {
            WsProtocolError::invalid_request(format!("Scene not found: {scene_id_raw}"))
        })?;
        let scene = manager.get(&scene_id).ok_or_else(|| {
            WsProtocolError::invalid_request(format!("Scene not found: {scene_id_raw}"))
        })?;
        let layout = validated_zone_layout_preview(scene, zone_id, layout)?;
        (scene_id, layout)
    };

    state
        .zone_layout_previews
        .set(scene_id, zone_id, layout)
        .await;
    zone_layout_preview_keys.insert((scene_id, zone_id));
    Ok(())
}

async fn handle_zone_layout_preview_clear(
    state: &Arc<AppState>,
    zone_layout_preview_keys: &mut HashSet<(SceneId, ZoneId)>,
    scene_id_raw: &str,
    zone_id_raw: &str,
) -> Result<(), WsProtocolError> {
    let scene_id = parse_scene_preview_id(scene_id_raw)?;
    let zone_id = parse_zone_preview_id(zone_id_raw)?;
    state.zone_layout_previews.clear(scene_id, zone_id).await;
    zone_layout_preview_keys.remove(&(scene_id, zone_id));
    Ok(())
}

pub(super) fn validated_zone_layout_preview(
    scene: &Scene,
    zone_id: ZoneId,
    layout: SpatialLayout,
) -> Result<SpatialLayout, WsProtocolError> {
    validate_layout_sampling_radii(&layout).map_err(WsProtocolError::invalid_request)?;

    if scene.blocks_runtime_mutation() {
        return Err(WsProtocolError::invalid_request(format!(
            "Scene '{}' is snapshot locked",
            scene.name
        )));
    }

    let Some(group) = scene.groups.iter().find(|group| group.id == zone_id) else {
        return Err(WsProtocolError::invalid_request(format!(
            "Zone not found: {zone_id}"
        )));
    };

    let stored_ids = group
        .layout
        .zones
        .iter()
        .map(|zone| zone.id.as_str())
        .collect::<HashSet<_>>();
    let request_ids = layout
        .zones
        .iter()
        .map(|zone| zone.id.as_str())
        .collect::<HashSet<_>>();
    if request_ids.len() != layout.zones.len() || stored_ids != request_ids {
        return Err(WsProtocolError::invalid_request(
            "zone layout preview must contain exactly the selected zone outputs",
        ));
    }

    let mut stored = group
        .layout
        .zones
        .iter()
        .cloned()
        .map(|zone| (zone.id.clone(), zone))
        .collect::<HashMap<_, _>>();
    let mut preview = group.layout.clone();
    preview.zones = layout
        .zones
        .into_iter()
        .filter_map(|incoming| {
            let mut merged = stored.remove(&incoming.id)?;
            merged.name = incoming.name;
            merged.position = incoming.position;
            merged.size = incoming.size;
            merged.rotation = incoming.rotation;
            merged.scale = incoming.scale;
            merged.display_order = incoming.display_order;
            merged.orientation = incoming.orientation;
            merged.shape = incoming.shape;
            merged.shape_preset = incoming.shape_preset;
            merged.sampling_mode = incoming.sampling_mode;
            merged.edge_behavior = incoming.edge_behavior;
            merged.brightness = incoming.brightness;
            Some(merged)
        })
        .collect();
    preview.canvas_width = layout.canvas_width;
    preview.canvas_height = layout.canvas_height;
    preview.default_sampling_mode = layout.default_sampling_mode;
    preview.default_edge_behavior = layout.default_edge_behavior;

    Ok(preview)
}

fn parse_scene_preview_id(raw: &str) -> Result<SceneId, WsProtocolError> {
    match raw {
        "default" => Ok(SceneId::DEFAULT),
        _ => Uuid::parse_str(raw).map(SceneId).map_err(|_| {
            WsProtocolError::invalid_request("scene_id must be a valid UUID or 'default'")
        }),
    }
}

fn parse_zone_preview_id(raw: &str) -> Result<ZoneId, WsProtocolError> {
    Uuid::parse_str(raw)
        .map(ZoneId)
        .map_err(|_| WsProtocolError::invalid_request("zone_id must be a valid UUID"))
}

async fn build_hello_state(state: &AppState) -> HelloState {
    let render_snapshot = state.render_loop.read().await.stats();
    let target_fps = render_snapshot.tier.fps();
    let capacity_fps = paced_fps(render_snapshot.avg_frame_time.as_secs_f64(), target_fps);
    let delivered_fps =
        if render_snapshot.state == hypercolor_core::engine::RenderLoopState::Running {
            state.performance.read().await.snapshot().delivered_fps
        } else {
            0.0
        };

    let (active_effect, active_preset_id) =
        active_primary_effect(state)
            .await
            .map_or((None, None), |(group, metadata)| {
                (
                    Some(NameRef {
                        id: metadata.id.to_string(),
                        name: metadata.name,
                    }),
                    group.preset_id.map(|preset_id| preset_id.to_string()),
                )
            });
    let active_scene = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager.active_scene().map(|scene| SceneRef {
            id: scene.id.to_string(),
            name: scene.name.clone(),
            snapshot_locked: scene.blocks_runtime_mutation(),
        })
    };

    let devices = state.device_registry.list().await;
    let total_leds = devices.iter().fold(0_usize, |acc, tracked| {
        let led_count = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
        acc.saturating_add(led_count)
    });

    HelloState {
        running: render_snapshot.state != hypercolor_core::engine::RenderLoopState::Stopped,
        paused: state.power_state.borrow().reported_paused(),
        brightness: brightness_percent(current_global_brightness(&state.power_state)),
        fps: HelloFps {
            target: target_fps,
            capacity: (capacity_fps * 10.0).round() / 10.0,
            delivered: (delivered_fps * 10.0).round() / 10.0,
            actual: (capacity_fps * 10.0).round() / 10.0,
        },
        effect: active_effect,
        active_preset_id,
        scene: active_scene,
        profile: None,
        layout: None,
        device_count: devices.len(),
        total_leds,
    }
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "brightness is clamped to 0-100 percent before narrowing to a byte"
)]
fn brightness_percent(brightness: f32) -> u8 {
    let scaled = (brightness.clamp(0.0, 1.0) * 100.0).round();
    if scaled <= 0.0 {
        0
    } else if scaled >= 100.0 {
        100
    } else {
        scaled as u8
    }
}

fn paced_fps(avg_frame_secs: f64, target_fps: u32) -> f64 {
    if avg_frame_secs <= 0.0 {
        return f64::from(target_fps);
    }

    (1.0 / avg_frame_secs).clamp(0.0, f64::from(target_fps))
}

/// Serialize and send a JSON message over the WebSocket.
async fn send_json(
    socket: &mut SessionSocket,
    msg: &impl Serialize,
) -> Result<(), SessionSocketError> {
    let json = serde_json::to_string(msg).unwrap_or_default();
    socket.send(Message::Text(json.into())).await.map_err(|e| {
        debug!("WebSocket send error: {e}");
        e
    })
}

#[cfg(test)]
mod hello_state_tests {
    use super::build_hello_state;
    use crate::api::AppState;
    use crate::session::{OutputOverride, set_global_brightness};

    #[tokio::test]
    async fn hello_reports_effective_pause_and_actual_brightness() {
        let state = AppState::new();
        set_global_brightness(&state.power_state, 0.42);
        state
            .power_state
            .send_modify(|power| power.session_sleeping = true);

        let hello = build_hello_state(&state).await;

        assert!(hello.paused);
        assert_eq!(hello.brightness, 42);
    }

    #[tokio::test]
    async fn hello_does_not_report_destructive_stop_as_pause() {
        let state = AppState::new();
        state
            .power_state
            .send_modify(|power| power.output_override = OutputOverride::Stopped);

        let hello = build_hello_state(&state).await;

        assert!(!hello.paused);
        assert!(hello.effect.is_none());
    }
}

#[cfg(test)]
mod security_tests {
    use super::ensure_control_tier;
    use crate::api::security::RequestAuthContext;

    #[test]
    fn read_only_auth_cannot_mutate_zone_layout_previews() {
        let error = ensure_control_tier(RequestAuthContext::read_only())
            .expect_err("read-only auth context should be rejected for mutating preview messages");
        assert_eq!(error.code, "forbidden");
        assert_eq!(
            error.message,
            "Read-only API key cannot perform write operations"
        );
        assert_eq!(
            error.details,
            Some(serde_json::json!({"required_tier": "control"}))
        );
    }

    #[test]
    fn unsecured_auth_can_mutate_zone_layout_previews() {
        ensure_control_tier(RequestAuthContext::unsecured())
            .expect("unsecured context should continue to allow local mutations");
    }
}

#[cfg(test)]
mod origin_tests {
    use super::{header_value_eq_origin, is_loopback_origin, ws_origin_allowed};
    use crate::api::AppState;
    use axum::http::{HeaderMap, HeaderValue, header};

    #[test]
    fn loopback_origin_is_allowed() {
        assert!(is_loopback_origin(&HeaderValue::from_static(
            "http://localhost:9430"
        )));
        assert!(is_loopback_origin(&HeaderValue::from_static(
            "https://127.0.0.1:9430"
        )));
        assert!(is_loopback_origin(&HeaderValue::from_static(
            "http://[::1]:9430"
        )));
    }

    #[test]
    fn non_loopback_origin_is_rejected() {
        assert!(!is_loopback_origin(&HeaderValue::from_static(
            "https://evil.example"
        )));
    }

    #[test]
    fn origin_comparison_is_case_insensitive() {
        let origin = HeaderValue::from_static("https://studio.example");
        assert!(header_value_eq_origin(&origin, "HTTPS://STUDIO.EXAMPLE"));
    }

    #[test]
    fn unsecured_daemon_rejects_non_loopback_browser_origins() {
        let state = AppState::new();

        // No Origin header: native and CLI clients are always allowed.
        assert!(ws_origin_allowed(&state, &HeaderMap::new()));

        let mut loopback = HeaderMap::new();
        loopback.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:9430"),
        );
        assert!(ws_origin_allowed(&state, &loopback));

        // A non-loopback browser origin is rejected on the default
        // unsecured daemon, where no cors_origins allowlist applies.
        let mut remote = HeaderMap::new();
        remote.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(!ws_origin_allowed(&state, &remote));
    }
}
