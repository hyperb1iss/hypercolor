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

/// A log sink for tests that assert how often a per-frame path logged.
/// Install [`Self::subscriber`] with `tracing::subscriber::set_default`
/// on the thread that renders the frames.
#[cfg(all(test, feature = "wgpu"))]
#[derive(Clone, Default)]
pub(crate) struct CapturedLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

#[cfg(all(test, feature = "wgpu"))]
impl CapturedLogs {
    /// A subscriber that writes formatted `INFO` and higher events here.
    pub(crate) fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let sink = self.clone();
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || sink.clone())
            .finish()
    }

    /// Every line captured so far.
    pub(crate) fn text(&self) -> String {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// How many captured lines contain `needle`.
    pub(crate) fn lines_containing(&self, needle: &str) -> usize {
        self.text()
            .lines()
            .filter(|line| line.contains(needle))
            .count()
    }
}

#[cfg(all(test, feature = "wgpu"))]
impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
