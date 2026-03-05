//! Effect file watcher — hot-reload effects from the filesystem.
//!
//! Uses the `notify` crate to watch effect search paths for `.html` file
//! changes. Events are debounced (300ms) and filtered to `.html` files,
//! then forwarded through a `tokio::sync::mpsc` channel.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::Instant;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, error, info, warn};

/// Debounce window for filesystem events.
const DEBOUNCE_MS: u64 = 300;

/// Channel capacity for effect watch events.
const CHANNEL_CAPACITY: usize = 64;

const HTML_EXTENSION: &str = "html";

// ── Event Types ─────────────────────────────────────────────────────────

/// A filesystem change event for an effect source file.
#[derive(Debug, Clone)]
pub enum EffectWatchEvent {
    /// A new `.html` file was created.
    Created(PathBuf),
    /// An existing `.html` file was modified.
    Modified(PathBuf),
    /// An `.html` file was removed.
    Removed(PathBuf),
}

// ── EffectWatcher ───────────────────────────────────────────────────────

/// Watches effect directories for `.html` file changes and forwards
/// debounced events through a tokio mpsc channel.
pub struct EffectWatcher {
    /// Held to keep the watcher alive; dropped on `EffectWatcher` drop.
    _watcher: RecommendedWatcher,
}

impl EffectWatcher {
    /// Start watching the given search paths.
    ///
    /// Returns the watcher handle and a receiver for effect change events.
    /// The watcher runs on a background thread managed by `notify`.
    ///
    /// # Errors
    ///
    /// Returns an error if the filesystem watcher cannot be initialized.
    pub fn start(
        search_paths: &[PathBuf],
    ) -> Result<(Self, tokio::sync::mpsc::Receiver<EffectWatchEvent>), notify::Error> {
        let (tx, rx) = tokio::sync::mpsc::channel(CHANNEL_CAPACITY);
        let (std_tx, std_rx) = std_mpsc::channel::<notify::Event>();

        let watcher_tx = std_tx.clone();
        let mut watcher = notify::recommended_watcher(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) => {
                    let _ = watcher_tx.send(event);
                }
                Err(error) => {
                    error!(%error, "Filesystem watcher error");
                }
            },
        )?;

        for path in search_paths {
            if !path.exists() {
                debug!(path = %path.display(), "Skipping missing watch path");
                continue;
            }

            match watcher.watch(path, RecursiveMode::Recursive) {
                Ok(()) => info!(path = %path.display(), "Watching for effect changes"),
                Err(error) => {
                    warn!(
                        path = %path.display(),
                        %error,
                        "Failed to watch effect directory"
                    );
                }
            }
        }

        // Bridge from std_mpsc (notify's thread) to tokio mpsc with debouncing.
        std::thread::Builder::new()
            .name("effect-watcher-bridge".into())
            .spawn(move || bridge_events_debounced(&std_rx, &tx))
            .expect("failed to spawn effect watcher bridge thread");

        Ok((Self { _watcher: watcher }, rx))
    }
}

/// Bridge raw notify events from the OS thread to the tokio channel,
/// debouncing rapid saves on the same file.
fn bridge_events_debounced(
    std_rx: &std_mpsc::Receiver<notify::Event>,
    tx: &tokio::sync::mpsc::Sender<EffectWatchEvent>,
) {
    let debounce = std::time::Duration::from_millis(DEBOUNCE_MS);

    // Track last-seen time per path to debounce rapid writes.
    let mut last_seen: HashMap<PathBuf, (Instant, EventKind)> = HashMap::new();

    loop {
        // Block until the first event arrives, or the channel closes.
        let Ok(first) = std_rx.recv() else {
            debug!("Effect watcher channel closed; bridge exiting");
            return;
        };

        // Accumulate the first event.
        accumulate(&mut last_seen, &first);

        // Drain any events that arrive within the debounce window.
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
                    debug!("Effect watcher channel closed during debounce drain");
                    flush_events(&mut last_seen, tx);
                    return;
                }
            }
        }

        flush_events(&mut last_seen, tx);
    }
}

/// Record an event in the debounce accumulator.
fn accumulate(last_seen: &mut HashMap<PathBuf, (Instant, EventKind)>, event: &notify::Event) {
    for path in &event.paths {
        if is_html_file(path) {
            last_seen.insert(path.clone(), (Instant::now(), event.kind));
        }
    }
}

/// Flush all accumulated events to the tokio channel and clear state.
fn flush_events(
    last_seen: &mut HashMap<PathBuf, (Instant, EventKind)>,
    tx: &tokio::sync::mpsc::Sender<EffectWatchEvent>,
) {
    for (path, (_timestamp, kind)) in last_seen.drain() {
        let watch_event = match kind {
            EventKind::Create(_) => EffectWatchEvent::Created(path),
            EventKind::Modify(_) => EffectWatchEvent::Modified(path),
            EventKind::Remove(_) => EffectWatchEvent::Removed(path),
            _ => continue,
        };

        if tx.blocking_send(watch_event).is_err() {
            debug!("Effect watcher tokio channel closed; stopping bridge");
            return;
        }
    }
}

fn is_html_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case(HTML_EXTENSION))
}
