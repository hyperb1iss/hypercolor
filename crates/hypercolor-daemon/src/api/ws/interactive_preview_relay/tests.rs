use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Bytes;
use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputPublicationId,
    BrowserInputSource, BrowserPreviewId, InputSource,
};
use hypercolor_leptos_ext::ws::PreviewStreamId;
use hypercolor_types::canvas::PublishedSurface;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::{
    InteractivePreviewEncoder, InteractivePreviewFrameEncoder,
    spawn_interactive_preview_relay_with_encoder,
};
use crate::api::ws::relays::{PreviewOutboundItem, preview_outbound_channel};
use crate::interactive_preview::{
    InteractivePreviewFrame, PreviewCapacityLedger, PreviewResourceLease, PreviewResourceLedger,
    PreviewWorkerPool,
};
use crate::preview_runtime::PreviewPixelFormat;

struct FirstEncodeGate {
    started: AtomicBool,
    released: Mutex<bool>,
    release: Condvar,
}

impl FirstEncodeGate {
    fn new() -> Self {
        Self {
            started: AtomicBool::new(false),
            released: Mutex::new(false),
            release: Condvar::new(),
        }
    }

    fn block_first(&self) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut released = self.released.lock().unwrap_or_else(PoisonError::into_inner);
        while !*released {
            released = self
                .release
                .wait(released)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.release.notify_all();
    }
}

struct GatedEncoder {
    inner: InteractivePreviewEncoder,
    gate: Arc<FirstEncodeGate>,
    encoded_frames: Arc<Mutex<Vec<u32>>>,
}

struct GateReleaseGuard(Arc<FirstEncodeGate>);

impl Drop for GateReleaseGuard {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl InteractivePreviewFrameEncoder for GatedEncoder {
    fn encode(
        &mut self,
        preview_id: &str,
        frame: &InteractivePreviewFrame,
    ) -> anyhow::Result<Bytes> {
        self.gate.block_first();
        let encoded = self.inner.encode(preview_id, frame)?;
        self.encoded_frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(frame.frame_number);
        Ok(encoded)
    }
}

fn frame(
    publication_id: BrowserInputPublicationId,
    spec_generation: u64,
    frame_number: u32,
) -> Arc<InteractivePreviewFrame> {
    let capacity = PreviewCapacityLedger::new(u64::MAX);
    let resources = capacity
        .try_reserve(PreviewResourceLedger {
            metadata_bytes: 1,
            ..PreviewResourceLedger::default()
        })
        .expect("test frame reservation should fit");
    frame_with_resources(publication_id, spec_generation, frame_number, resources)
}

fn frame_with_resources(
    publication_id: BrowserInputPublicationId,
    spec_generation: u64,
    frame_number: u32,
    resource_lease: PreviewResourceLease,
) -> Arc<InteractivePreviewFrame> {
    Arc::new(InteractivePreviewFrame {
        publication_id,
        spec_generation,
        frame_number,
        timestamp_ms: frame_number,
        width: 1,
        height: 1,
        format: PreviewPixelFormat::Rgba,
        surface: PublishedSurface::from_vec(vec![1, 2, 3, 255], 1, 1, frame_number, frame_number),
        resource_lease,
    })
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition should become true before timeout");
}

#[tokio::test(flavor = "current_thread")]
async fn slow_encode_keeps_tokio_live_coalesces_pending_frames_and_fences_generation() {
    let mut browser = BrowserInputSource::new();
    browser.start().expect("browser input should start");
    let attachment = browser
        .handle()
        .attach(BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(41),
            BrowserPreviewId::new("main"),
        ))
        .expect("browser preview should attach");
    let publication_id = attachment.publication_id();
    let (frame_tx, frame_rx) = tokio::sync::watch::channel(None);
    let (generation_tx, generation_rx) = tokio::sync::watch::channel(1);
    let workers =
        PreviewWorkerPool::new("preview-relay-test", 1).expect("preview worker should start");
    let (outbound, receiver) = preview_outbound_channel();
    let gate = Arc::new(FirstEncodeGate::new());
    let _gate_release = GateReleaseGuard(Arc::clone(&gate));
    let encoded_frames = Arc::new(Mutex::new(Vec::new()));
    let relay = spawn_interactive_preview_relay_with_encoder(
        "main".to_owned(),
        publication_id,
        frame_rx,
        generation_rx,
        workers,
        outbound,
        CancellationToken::new(),
        Box::new(GatedEncoder {
            inner: InteractivePreviewEncoder::new(),
            gate: Arc::clone(&gate),
            encoded_frames: Arc::clone(&encoded_frames),
        }),
    );

    frame_tx.send_replace(Some(frame(publication_id, 1, 1)));
    wait_until(|| gate.started.load(Ordering::Acquire)).await;
    timeout(
        Duration::from_millis(100),
        tokio::time::sleep(Duration::from_millis(10)),
    )
    .await
    .expect("Tokio timer should advance while encode worker is blocked");

    generation_tx.send_replace(2);
    frame_tx.send_replace(Some(frame(publication_id, 2, 2)));
    frame_tx.send_replace(Some(frame(publication_id, 2, 3)));
    gate.release();
    wait_until(|| {
        encoded_frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
            == 2
    })
    .await;

    assert_eq!(
        *encoded_frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
        vec![1, 3]
    );
    let item = timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("latest generation should publish before timeout");
    let PreviewOutboundItem::Publication(publication) = item else {
        panic!("stale generation must not publish a cancellation");
    };
    assert_eq!(
        publication.interactive_fence(),
        Some(("main", publication_id))
    );
    assert_eq!(
        publication.stream(),
        &PreviewStreamId::Interactive("main".to_owned())
    );
    assert!(receiver.try_recv().is_none());

    drop(frame_tx);
    timeout(Duration::from_secs(2), relay)
        .await
        .expect("relay should stop when its frame source closes")
        .expect("relay task should stop cleanly");
}

#[tokio::test(flavor = "current_thread")]
async fn graceful_cancel_joins_active_encode_and_suppresses_publication() {
    let mut browser = BrowserInputSource::new();
    browser.start().expect("browser input should start");
    let attachment = browser
        .handle()
        .attach(BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(42),
            BrowserPreviewId::new("close"),
        ))
        .expect("browser preview should attach");
    let publication_id = attachment.publication_id();
    let (frame_tx, frame_rx) = tokio::sync::watch::channel(None);
    let (_generation_tx, generation_rx) = tokio::sync::watch::channel(1);
    let workers =
        PreviewWorkerPool::new("preview-close-test", 1).expect("preview worker should start");
    let (outbound, receiver) = preview_outbound_channel();
    let gate = Arc::new(FirstEncodeGate::new());
    let _gate_release = GateReleaseGuard(Arc::clone(&gate));
    let encoded_frames = Arc::new(Mutex::new(Vec::new()));
    let cancel = CancellationToken::new();
    let capacity = PreviewCapacityLedger::new(64);
    let reservation = capacity
        .try_reserve(PreviewResourceLedger {
            encoder_workspace_bytes: 64,
            ..PreviewResourceLedger::default()
        })
        .expect("test encode reservation should fit");
    let mut relay = spawn_interactive_preview_relay_with_encoder(
        "close".to_owned(),
        publication_id,
        frame_rx,
        generation_rx,
        workers,
        outbound,
        cancel.clone(),
        Box::new(GatedEncoder {
            inner: InteractivePreviewEncoder::new(),
            gate: Arc::clone(&gate),
            encoded_frames: Arc::clone(&encoded_frames),
        }),
    );

    frame_tx.send_replace(Some(frame_with_resources(
        publication_id,
        1,
        1,
        reservation,
    )));
    wait_until(|| gate.started.load(Ordering::Acquire)).await;
    cancel.cancel();
    frame_tx.send_replace(None);
    assert!(
        timeout(Duration::from_millis(20), &mut relay)
            .await
            .is_err()
    );
    assert_eq!(
        capacity
            .snapshot()
            .used
            .total_bytes()
            .expect("test reservation should remain representable"),
        64
    );
    assert!(receiver.try_recv().is_none());

    gate.release();
    timeout(Duration::from_secs(2), relay)
        .await
        .expect("relay should join after active encode completes")
        .expect("relay task should stop cleanly");
    assert_eq!(
        *encoded_frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
        vec![1]
    );
    assert_eq!(
        capacity
            .snapshot()
            .used
            .total_bytes()
            .expect("test reservation should remain representable"),
        0
    );
    assert!(receiver.try_recv().is_none());
}
