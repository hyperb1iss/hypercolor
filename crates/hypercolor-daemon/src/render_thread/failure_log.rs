//! Logging for failures a per-frame path hits frame after frame.

/// The failure a per-frame path last logged, so one that persists logs when
/// it starts and whenever it changes rather than on every frame, and its
/// end reports how many frames it held for.
#[derive(Debug)]
pub(crate) struct FailureLog<K> {
    logged: Option<LoggedFailure<K>>,
}

#[derive(Debug)]
struct LoggedFailure<K> {
    key: K,
    failed_frames: u64,
}

impl<K> Default for FailureLog<K> {
    fn default() -> Self {
        Self { logged: None }
    }
}

impl<K: PartialEq> FailureLog<K> {
    /// Count one failed frame. Returns whether `key` starts a new failure,
    /// the first since the last [`Self::clear`] or one that differs from
    /// the failure logged, which is when it should be logged.
    pub(crate) fn note(&mut self, key: K) -> bool {
        if let Some(logged) = &mut self.logged
            && logged.key == key
        {
            logged.failed_frames = logged.failed_frames.saturating_add(1);
            return false;
        }
        self.logged = Some(LoggedFailure {
            key,
            failed_frames: 1,
        });
        true
    }

    /// End the logged failure, if one is open, returning how many frames
    /// failed since it was logged.
    pub(crate) fn clear(&mut self) -> Option<u64> {
        self.logged.take().map(|logged| logged.failed_frames)
    }
}
