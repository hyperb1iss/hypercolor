use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::input::{SourceStatus, SourceStatusRegistry};
use tokio::task::{JoinHandle, JoinSet};

const GRAPH_RECONCILE_INTERVAL: Duration = Duration::from_millis(250);
const TRANSITION_BUFFER_SIZE: usize = 64;

pub(crate) struct InputStatusEventPublisher {
    task: JoinHandle<()>,
}

impl InputStatusEventPublisher {
    pub(crate) fn start(registry: SourceStatusRegistry, event_bus: Arc<HypercolorBus>) -> Self {
        Self {
            task: tokio::spawn(run(registry, event_bus)),
        }
    }
}

impl Drop for InputStatusEventPublisher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(registry: SourceStatusRegistry, event_bus: Arc<HypercolorBus>) {
    let (status_tx, mut status_rx) =
        tokio::sync::mpsc::channel::<Arc<SourceStatus>>(TRANSITION_BUFFER_SIZE);
    let mut watchers = JoinSet::new();
    let mut graph_generation = rebuild_watchers(&registry, &status_tx, &mut watchers);
    let mut graph_reconcile = tokio::time::interval_at(
        tokio::time::Instant::now() + GRAPH_RECONCILE_INTERVAL,
        GRAPH_RECONCILE_INTERVAL,
    );
    graph_reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            Some(status) = status_rx.recv() => {
                let _ = event_bus.publish_input_source_status(&status);
            },
            _ = graph_reconcile.tick() => {
                let current_generation = registry.snapshot().source_graph_generation();
                if current_generation != graph_generation {
                    graph_generation = rebuild_watchers(&registry, &status_tx, &mut watchers);
                }
            }
        }
    }
}

fn rebuild_watchers(
    registry: &SourceStatusRegistry,
    status_tx: &tokio::sync::mpsc::Sender<Arc<SourceStatus>>,
    watchers: &mut JoinSet<()>,
) -> u64 {
    *watchers = JoinSet::new();
    let snapshot = registry.snapshot();
    for handle in snapshot.handles() {
        let mut subscription = handle.subscribe();
        let status_tx = status_tx.clone();
        watchers.spawn(async move {
            if status_tx.send(subscription.snapshot()).await.is_err() {
                return;
            }
            while let Some(status) = subscription.changed().await {
                if status_tx.send(status).await.is_err() {
                    break;
                }
            }
        });
    }
    snapshot.source_graph_generation()
}
