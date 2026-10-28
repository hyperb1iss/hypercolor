//! Concurrent request ownership with an explicit shutdown cancellation boundary.

use std::future::Future;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub(crate) struct RequestLifetime {
    closed: CancellationToken,
    readers: Arc<RwLock<()>>,
}

impl RequestLifetime {
    pub(crate) async fn run<F: Future>(&self, future: F) -> Option<F::Output> {
        let _lease = tokio::select! {
            biased;
            () = self.closed.cancelled() => return None,
            lease = self.readers.read() => lease,
        };
        // Drop the handler before releasing admission, including on cancellation.
        {
            tokio::pin!(future);
            tokio::select! {
                biased;
                () = self.closed.cancelled() => None,
                result = &mut future => Some(result),
            }
        }
    }

    pub(crate) fn cancel(&self) {
        self.closed.cancel();
    }

    pub(crate) async fn cancelled(&self) {
        self.closed.cancelled().await;
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    pub(crate) async fn close_and_wait(&self) {
        self.cancel();
        let _drained = self.readers.write().await;
    }
}

pub(crate) async fn guard_http(
    axum::extract::State(lifetime): axum::extract::State<RequestLifetime>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    lifetime.run(next.run(request)).await.unwrap_or_else(|| {
        crate::domain::DomainError::service_unavailable("Hypercolor is shutting down")
            .into_response()
    })
}
