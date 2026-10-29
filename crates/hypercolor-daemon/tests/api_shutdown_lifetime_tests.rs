//! Listener completion must include cancellation of its admitted mutations.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::{Router, extract::State, routing::post};
use hypercolor_daemon::daemon::{bind_api_listener, serve_api_listeners_with_shutdown_timeout};
use tokio::sync::{Notify, Semaphore, watch};

#[derive(Clone)]
struct MutationState {
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
    dropped: Arc<AtomicBool>,
    mutations: Arc<AtomicUsize>,
}

struct HandlerLifetime(Arc<AtomicBool>);

impl Drop for HandlerLifetime {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

async fn blocked_mutation(State(state): State<MutationState>) -> &'static str {
    let _lifetime = HandlerLifetime(Arc::clone(&state.dropped));
    state.entered.notify_one();
    state
        .release
        .acquire()
        .await
        .expect("release permit")
        .forget();
    state.mutations.fetch_add(1, Ordering::AcqRel);
    "committed"
}

#[tokio::test]
async fn shutdown_drops_inflight_http_mutation_before_returning() {
    let listener = bind_api_listener("127.0.0.1:0".parse().expect("loopback address"))
        .expect("loopback listener");
    let address = listener.local_addr().expect("bound address");
    let state = MutationState {
        entered: Arc::new(Notify::new()),
        release: Arc::new(Semaphore::new(0)),
        dropped: Arc::new(AtomicBool::new(false)),
        mutations: Arc::new(AtomicUsize::new(0)),
    };
    let router = Router::new()
        .route("/mutation", post(blocked_mutation))
        .with_state(state.clone());
    let (shutdown, receiver) = watch::channel(false);
    let server = tokio::spawn(serve_api_listeners_with_shutdown_timeout(
        vec![listener],
        router,
        receiver,
        Duration::from_millis(25),
    ));
    let client = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!("http://{address}/mutation"))
            .send()
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), state.entered.notified())
        .await
        .expect("HTTP handler entered");
    shutdown.send(true).expect("shutdown receiver");
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("shutdown completes with a blocked request")
        .expect("server task joins")
        .expect("server shuts down cleanly");

    let dropped_before_return = state.dropped.load(Ordering::Acquire);
    // Release after the claimed shutdown boundary. A detached Axum connection
    // would now perform the mutation, even though its listener already exited.
    state.release.add_permits(1);
    let _response = tokio::time::timeout(Duration::from_secs(10), client)
        .await
        .expect("request completes or connection closes")
        .expect("client task joins");
    assert_eq!(
        state.mutations.load(Ordering::Acquire),
        0,
        "an admitted mutation must not resume after server shutdown returns"
    );
    assert!(
        dropped_before_return,
        "shutdown must drop the handler before reporting all requests stopped"
    );
}
