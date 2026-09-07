use std::fs;
use std::time::Duration;

use crate::coordinator_error::MacosOwnerExecutionError;
use crate::model::MacosDaemonOwner;
use crate::store::MacosOwnerStore;

/// Wait for an exact durable owner publication through a native file watch.
pub fn wait_for_owner_publication(
    store: &MacosOwnerStore,
    owner: MacosDaemonOwner,
    after_epoch: u64,
    timeout: Duration,
) -> Result<bool, MacosOwnerExecutionError> {
    use notify::{RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::Instant;

    let matches = || {
        store
            .load_owner_record()
            .map(|record| {
                record.is_some_and(|record| {
                    record.active_owner == owner && record.owner_epoch > after_epoch
                })
            })
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
    };
    if matches()? {
        return Ok(true);
    }
    let owner_path = store.owner_record_path();
    let directory = owner_path
        .parent()
        .ok_or_else(|| MacosOwnerExecutionError::new("owner record has no parent directory"))?
        .to_path_buf();
    fs::create_dir_all(&directory)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    let (signal_tx, signal_rx) = mpsc::sync_channel(1);
    let watched_path = owner_path.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|event| event.paths.iter().any(|path| path == &watched_path)) {
            let _ = signal_tx.try_send(());
        }
    })
    .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    watcher
        .watch(&directory, RecursiveMode::NonRecursive)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if matches()? {
        return Ok(true);
    }
    let started = Instant::now();
    loop {
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return Ok(false);
        };
        match signal_rx.recv_timeout(remaining) {
            Ok(()) if matches()? => return Ok(true),
            Ok(()) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(false),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(MacosOwnerExecutionError::new(
                    "owner publication watch disconnected",
                ));
            }
        }
    }
}
