//! Asset library filesystem watcher.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::Instant;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, error, info, warn};

/// Debounce window for asset filesystem changes.
pub const ASSET_WATCH_DEBOUNCE_MS: u64 = 250;

const CHANNEL_CAPACITY: usize = 64;
const INDEX_FILE: &str = "index.json";

/// Coalesced filesystem event for the asset library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetWatchEvent {
    IndexChanged,
    ObjectChanged(PathBuf),
    ObjectRemoved(PathBuf),
}

/// Watches an asset library directory for hot reload triggers.
pub struct AssetWatcher {
    _watcher: RecommendedWatcher,
}

impl AssetWatcher {
    /// Start watching an asset library root.
    pub fn start(
        root: &Path,
    ) -> Result<(Self, tokio::sync::mpsc::Receiver<AssetWatchEvent>), notify::Error> {
        let (tx, rx) = tokio::sync::mpsc::channel(CHANNEL_CAPACITY);
        let (std_tx, std_rx) = std_mpsc::channel::<notify::Event>();

        let watcher_tx = std_tx.clone();
        let mut watcher = notify::recommended_watcher(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) => {
                    let _ = watcher_tx.send(event);
                }
                Err(error) => error!(%error, "Asset filesystem watcher error"),
            },
        )?;

        if root.exists() {
            watcher.watch(root, RecursiveMode::Recursive)?;
            info!(path = %root.display(), "Watching for asset changes");
        } else {
            warn!(path = %root.display(), "Skipping missing asset watch path");
        }

        std::thread::Builder::new()
            .name("asset-watcher-bridge".into())
            .spawn(move || bridge_events_debounced(&std_rx, &tx))
            .expect("failed to spawn asset watcher bridge thread");

        Ok((Self { _watcher: watcher }, rx))
    }
}

fn bridge_events_debounced(
    std_rx: &std_mpsc::Receiver<notify::Event>,
    tx: &tokio::sync::mpsc::Sender<AssetWatchEvent>,
) {
    let debounce = std::time::Duration::from_millis(ASSET_WATCH_DEBOUNCE_MS);
    let mut last_seen: HashMap<PathBuf, EventKind> = HashMap::new();

    loop {
        let Ok(first) = std_rx.recv() else {
            debug!("Asset watcher channel closed; bridge exiting");
            return;
        };

        accumulate(&mut last_seen, &first);

        let deadline = Instant::now() + debounce;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match std_rx.recv_timeout(remaining) {
                Ok(event) => accumulate(&mut last_seen, &event),
                Err(std_mpsc::RecvTimeoutError::Timeout) => break,
                Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                    debug!("Asset watcher channel closed during debounce drain");
                    flush_events(&mut last_seen, tx);
                    return;
                }
            }
        }

        flush_events(&mut last_seen, tx);
    }
}

fn accumulate(last_seen: &mut HashMap<PathBuf, EventKind>, event: &notify::Event) {
    let incoming_priority = kind_priority(event.kind);
    if incoming_priority == 0 {
        return;
    }

    for path in &event.paths {
        if !is_asset_path(path) {
            continue;
        }
        let retain_existing = last_seen
            .get(path)
            .is_some_and(|existing| kind_priority(*existing) > incoming_priority);
        if retain_existing {
            continue;
        }
        last_seen.insert(path.clone(), event.kind);
    }
}

fn flush_events(
    last_seen: &mut HashMap<PathBuf, EventKind>,
    tx: &tokio::sync::mpsc::Sender<AssetWatchEvent>,
) {
    for (path, kind) in last_seen.drain() {
        let event = if is_index_path(&path) {
            AssetWatchEvent::IndexChanged
        } else {
            match kind {
                EventKind::Remove(_) => AssetWatchEvent::ObjectRemoved(path),
                EventKind::Create(_) | EventKind::Modify(_) => AssetWatchEvent::ObjectChanged(path),
                _ => continue,
            }
        };

        if tx.blocking_send(event).is_err() {
            debug!("Asset watcher tokio channel closed; stopping bridge");
            return;
        }
    }
}

fn kind_priority(kind: EventKind) -> u8 {
    match kind {
        EventKind::Remove(_) => 3,
        EventKind::Create(_) => 2,
        EventKind::Modify(notify::event::ModifyKind::Metadata(_)) => 0,
        EventKind::Modify(_) => 1,
        _ => 0,
    }
}

fn is_asset_path(path: &Path) -> bool {
    is_index_path(path) || is_object_path(path)
}

fn is_index_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == INDEX_FILE)
}

fn is_object_path(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(grandparent) = parent.parent() else {
        return false;
    };
    grandparent
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "objects")
}
