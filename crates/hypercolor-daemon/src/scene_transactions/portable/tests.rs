use super::*;
use crate::domain::{scene::SceneService, spatial::SpatialService};
use hypercolor_core::{bus::HypercolorBus, scene::SceneManager, spatial::SpatialEngine};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use std::future::Future;
use std::task::{Context, Poll, Waker};

fn authority() -> (LayoutTransactionAuthority, SceneTransactionQueue) {
    let layout = SpatialLayout {
        id: "test".into(),
        name: "Test".into(),
        description: None,
        canvas_width: 16,
        canvas_height: 16,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    };
    let queue = SceneTransactionQueue::new();
    (
        LayoutTransactionAuthority::new(
            SpatialService::new(SpatialEngine::new(layout.clone())),
            SceneService::in_memory(
                SceneManager::with_default_layout(layout),
                Arc::new(HypercolorBus::new()),
            ),
            queue.clone(),
        ),
        queue,
    )
}

fn accept(queue: &SceneTransactionQueue) {
    let mut pending = queue.drain();
    assert_eq!(pending.len(), 1);
    let SceneTransaction::BeginPortableRestore(request) = pending.pop().expect("one request")
    else {
        panic!("restore boundary request");
    };
    queue.accept_portable_restore(request);
}

#[tokio::test]
async fn portable_renderer_barrier_waits_for_frame_boundary_and_releases_on_drop() {
    let (authority, queue) = authority();
    let mut waiting = Box::pin(authority.begin_portable_restore());
    assert!(matches!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    assert!(!queue.portable_restore_held());
    accept(&queue);
    let barrier = waiting.await.expect("acknowledged boundary");
    assert!(queue.portable_restore_held());
    let mut ordinary_writer = Box::pin(authority.acquire_layout_update_guard());
    assert!(matches!(
        ordinary_writer
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    drop(barrier);
    assert!(!queue.portable_restore_held());
    let _guard = ordinary_writer.await;
}

#[tokio::test]
async fn portable_renderer_cancelled_begin_never_holds_a_future_frame() {
    let (authority, queue) = authority();
    let mut waiting = Box::pin(authority.begin_portable_restore());
    assert!(matches!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    drop(waiting);
    accept(&queue);
    assert!(!queue.portable_restore_held());
    let _guard = queue.acquire_layout_update_guard().await;
}

#[tokio::test]
async fn portable_renderer_close_completes_pending_begin_and_invalidates_active_hold() {
    let (authority, queue) = authority();
    let mut waiting = Box::pin(authority.begin_portable_restore());
    assert!(matches!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    queue.close();
    assert!(matches!(
        waiting.await,
        Err(LayoutUpdateError::Transaction(
            LayoutTransactionRejection::RendererStopped
        ))
    ));
    let (authority, queue) = self::authority();
    let mut waiting = Box::pin(authority.begin_portable_restore());
    assert!(matches!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    accept(&queue);
    let barrier = waiting.await.expect("acknowledged hold");
    queue.close();
    assert!(!barrier.is_active());
}
