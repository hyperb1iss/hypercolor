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
///
/// Filters out non-lifecycle events such as `Access` (emitted for every
/// `CLOSE_WRITE` on Linux inotify) and `Metadata` (chmod/chown). Without this
/// filter the latest-event-wins debounce would often settle on the trailing
/// `Access(Close(Write))` event and drop real content changes on the floor.
///
/// When multiple relevant events arrive for the same path within the debounce
/// window, the most significant one is kept (`Remove` > `Create` > `Modify`)
/// so the emitted `EffectWatchEvent` reflects the net state transition rather
/// than whichever inotify event happened to arrive last.
fn accumulate(last_seen: &mut HashMap<PathBuf, (Instant, EventKind)>, event: &notify::Event) {
    let incoming_priority = kind_priority(event.kind);
    if incoming_priority == 0 {
        return;
    }
    for path in &event.paths {
        if !is_html_file(path) {
            continue;
        }
        let retain_existing = last_seen
            .get(path)
            .is_some_and(|(_, existing)| kind_priority(*existing) > incoming_priority);
        if retain_existing {
            continue;
        }
        last_seen.insert(path.clone(), (Instant::now(), event.kind));
    }
}

/// Priority ordering for lifecycle events in the debounce window.
///
/// Returns `0` for events we don't forward (access, metadata, unknown).
fn kind_priority(kind: EventKind) -> u8 {
    match kind {
        EventKind::Remove(_) => 3,
        EventKind::Create(_) => 2,
        EventKind::Modify(notify::event::ModifyKind::Metadata(_)) => 0,
        EventKind::Modify(_) => 1,
        _ => 0,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Instant;

    use notify::event::{
        AccessKind, AccessMode, CreateKind, DataChange, Event, EventKind, MetadataKind, ModifyKind,
        RemoveKind,
    };

    use super::{accumulate, kind_priority};

    fn html_event(kind: EventKind, path: &str) -> Event {
        Event {
            kind,
            paths: vec![PathBuf::from(path)],
            attrs: notify::event::EventAttributes::default(),
        }
    }

    #[test]
    fn access_events_are_ignored() {
        // inotify emits CLOSE_WRITE after every successful write, mapped to
        // EventKind::Access(Close(Write)). That must not overwrite a preceding
        // Modify event in the debounce window — otherwise the watcher never
        // fires a reload.
        let mut acc: HashMap<PathBuf, (Instant, EventKind)> = HashMap::new();
        let path = "/tmp/neon-city.html";

        accumulate(
            &mut acc,
            &html_event(EventKind::Modify(ModifyKind::Data(DataChange::Any)), path),
        );
        accumulate(
            &mut acc,
            &html_event(
                EventKind::Access(AccessKind::Close(AccessMode::Write)),
                path,
            ),
        );

        let (_, stored) = acc
            .get(&PathBuf::from(path))
            .expect("modify event retained");
        assert!(
            matches!(stored, EventKind::Modify(_)),
            "trailing Access event must not clobber the Modify event: {stored:?}"
        );
    }

    #[test]
    fn remove_beats_create_beats_modify() {
        let path = "/tmp/neon-city.html";

        let mut acc: HashMap<PathBuf, (Instant, EventKind)> = HashMap::new();
        accumulate(
            &mut acc,
            &html_event(EventKind::Create(CreateKind::File), path),
        );
        accumulate(
            &mut acc,
            &html_event(EventKind::Modify(ModifyKind::Data(DataChange::Any)), path),
        );
        let (_, retained) = acc
            .get(&PathBuf::from(path))
            .expect("path should be tracked after accumulate");
        assert!(
            matches!(retained, EventKind::Create(_)),
            "Create should outrank a subsequent Modify: {retained:?}"
        );

        accumulate(
            &mut acc,
            &html_event(EventKind::Remove(RemoveKind::File), path),
        );
        let (_, retained) = acc
            .get(&PathBuf::from(path))
            .expect("path should be tracked after accumulate");
        assert!(
            matches!(retained, EventKind::Remove(_)),
            "Remove should outrank everything else: {retained:?}"
        );
    }

    #[test]
    fn non_html_paths_are_skipped() {
        let mut acc: HashMap<PathBuf, (Instant, EventKind)> = HashMap::new();
        accumulate(
            &mut acc,
            &html_event(
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                "/tmp/not-an-effect.txt",
            ),
        );
        assert!(acc.is_empty());
    }

    #[test]
    fn metadata_changes_do_not_trigger_reload() {
        assert_eq!(
            kind_priority(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))),
            0,
            "chmod/chown should not trigger an effect reload"
        );
        assert_eq!(kind_priority(EventKind::Any), 0);
    }
}
