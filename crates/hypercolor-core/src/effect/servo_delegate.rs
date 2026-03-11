//! Servo `WebView` delegate for Hypercolor effect rendering.
//!
//! The delegate captures embedder callbacks we care about for headless
//! HTML effect rendering:
//! - frame readiness notifications
//! - page load completion
//! - console messages from effect scripts

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use servo::{ConsoleLogLevel, LoadStatus, WebView, WebViewDelegate};
use tracing::{debug, error, info, trace, warn};

/// Bounded console message captured from Servo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleMessage {
    /// Console log level (`log`, `debug`, `warn`, etc.).
    pub level: String,
    /// Message payload emitted by page JavaScript.
    pub message: String,
}

/// Maximum number of console messages kept in memory.
const MAX_CONSOLE_MESSAGES: usize = 128;

/// `WebView` delegate implementation used by Hypercolor's Servo integration.
///
/// This type is `Send + Sync` friendly (atomics + mutex) so renderer-side
/// orchestration can observe callback state from outside Servo internals.
pub struct HypercolorWebViewDelegate {
    frame_ready: AtomicBool,
    frame_count: AtomicU64,
    page_loaded: AtomicBool,
    last_url: Mutex<Option<String>>,
    console_messages: Mutex<VecDeque<ConsoleMessage>>,
}

impl HypercolorWebViewDelegate {
    /// Create a fresh delegate with empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            frame_ready: AtomicBool::new(false),
            frame_count: AtomicU64::new(0),
            page_loaded: AtomicBool::new(false),
            last_url: Mutex::new(None),
            console_messages: Mutex::new(VecDeque::with_capacity(MAX_CONSOLE_MESSAGES)),
        }
    }

    /// Returns true when Servo has signaled a frame is ready to paint.
    #[must_use]
    pub fn is_frame_ready(&self) -> bool {
        self.frame_ready.load(Ordering::Acquire)
    }

    /// Returns true once and resets the frame-ready flag.
    pub fn take_frame_ready(&self) -> bool {
        self.frame_ready.swap(false, Ordering::AcqRel)
    }

    /// Total number of frame-ready notifications received.
    #[must_use]
    pub fn frame_count(&self) -> u64 {
        self.frame_count.load(Ordering::Acquire)
    }

    /// Returns whether the page has reached `LoadStatus::Complete`.
    #[must_use]
    pub fn is_page_loaded(&self) -> bool {
        self.page_loaded.load(Ordering::Acquire)
    }

    /// Returns true once and resets the page-loaded flag.
    pub fn take_page_loaded(&self) -> bool {
        self.page_loaded.swap(false, Ordering::AcqRel)
    }

    /// Clear per-navigation state before loading a new page.
    pub fn reset_navigation_state(&self) {
        self.frame_ready.store(false, Ordering::Release);
        self.page_loaded.store(false, Ordering::Release);
        self.with_last_url_mut(|last_url| {
            *last_url = None;
        });
        self.with_console_messages(VecDeque::clear);
    }

    /// Drain all buffered console messages in FIFO order.
    pub fn drain_console_messages(&self) -> Vec<ConsoleMessage> {
        self.with_console_messages(|messages| messages.drain(..).collect())
    }

    /// Return up to `limit` most recent console messages without draining.
    pub fn recent_console_messages(&self, limit: usize) -> Vec<ConsoleMessage> {
        if limit == 0 {
            return Vec::new();
        }
        self.with_console_messages(|messages| {
            let start = messages.len().saturating_sub(limit);
            messages.iter().skip(start).cloned().collect()
        })
    }

    /// Returns the most recently observed URL.
    #[must_use]
    pub fn last_url(&self) -> Option<String> {
        self.with_last_url(Clone::clone)
    }

    fn on_new_frame_ready(&self) {
        let frame_number = self.frame_count.fetch_add(1, Ordering::AcqRel) + 1;
        self.frame_ready.store(true, Ordering::Release);
        trace!(frame_number, "Servo reported a new frame");
    }

    fn on_page_loaded(&self) {
        self.page_loaded.store(true, Ordering::Release);
        info!("Servo page load completed");
    }

    fn on_url_changed(&self, url: &str) {
        self.with_last_url_mut(|last_url| {
            *last_url = Some(url.to_owned());
        });
        debug!(url = url, "Servo URL changed");
    }

    fn on_console_message(&self, level: &ConsoleLogLevel, message: &str) {
        let level_label = level_label(level);
        self.with_console_messages(|messages| {
            if messages.len() == MAX_CONSOLE_MESSAGES {
                let _ = messages.pop_front();
            }
            messages.push_back(ConsoleMessage {
                level: level_label.to_owned(),
                message: message.to_owned(),
            });
        });

        match level {
            ConsoleLogLevel::Log | ConsoleLogLevel::Info => {
                trace!(message = message, "Servo console");
            }
            ConsoleLogLevel::Debug => debug!(message = message, "Servo console"),
            ConsoleLogLevel::Warn => warn!(message = message, "Servo console"),
            ConsoleLogLevel::Error => error!(message = message, "Servo console"),
            ConsoleLogLevel::Trace => trace!(message = message, "Servo console"),
        }
    }

    fn with_console_messages<T>(&self, f: impl FnOnce(&mut VecDeque<ConsoleMessage>) -> T) -> T {
        match self.console_messages.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }

    fn with_last_url<T>(&self, f: impl FnOnce(&Option<String>) -> T) -> T {
        match self.last_url.lock() {
            Ok(guard) => f(&guard),
            Err(poisoned) => {
                let guard = poisoned.into_inner();
                f(&guard)
            }
        }
    }

    fn with_last_url_mut<T>(&self, f: impl FnOnce(&mut Option<String>) -> T) -> T {
        match self.last_url.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }
}

impl WebViewDelegate for HypercolorWebViewDelegate {
    fn notify_url_changed(&self, _webview: WebView, url: reqwest::Url) {
        self.on_url_changed(url.as_str());
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.on_new_frame_ready();
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if matches!(status, LoadStatus::Complete) {
            self.on_page_loaded();
        }
    }

    fn show_console_message(&self, _webview: WebView, level: ConsoleLogLevel, message: String) {
        self.on_console_message(&level, &message);
    }
}

impl Default for HypercolorWebViewDelegate {
    fn default() -> Self {
        Self::new()
    }
}

fn level_label(level: &ConsoleLogLevel) -> &'static str {
    match level {
        ConsoleLogLevel::Log => "log",
        ConsoleLogLevel::Debug => "debug",
        ConsoleLogLevel::Info => "info",
        ConsoleLogLevel::Warn => "warn",
        ConsoleLogLevel::Error => "error",
        ConsoleLogLevel::Trace => "trace",
    }
}

#[cfg(test)]
mod tests {
    use servo::ConsoleLogLevel;

    use super::*;

    #[test]
    fn frame_ready_flag_is_edge_triggered() {
        let delegate = HypercolorWebViewDelegate::new();

        assert!(!delegate.is_frame_ready());
        assert!(!delegate.take_frame_ready());

        delegate.on_new_frame_ready();
        assert!(delegate.is_frame_ready());
        assert!(delegate.take_frame_ready());
        assert!(!delegate.is_frame_ready());
        assert!(!delegate.take_frame_ready());
        assert_eq!(delegate.frame_count(), 1);
    }

    #[test]
    fn page_loaded_flag_tracks_complete_status() {
        let delegate = HypercolorWebViewDelegate::new();

        assert!(!delegate.is_page_loaded());
        assert!(!delegate.take_page_loaded());

        delegate.on_page_loaded();
        assert!(delegate.is_page_loaded());
        assert!(delegate.take_page_loaded());
        assert!(!delegate.is_page_loaded());
        assert!(!delegate.take_page_loaded());
    }

    #[test]
    fn console_messages_are_bounded_and_drained() {
        let delegate = HypercolorWebViewDelegate::new();
        for idx in 0..(MAX_CONSOLE_MESSAGES + 2) {
            delegate.on_console_message(&ConsoleLogLevel::Info, &format!("m{idx}"));
        }

        let drained = delegate.drain_console_messages();
        assert_eq!(drained.len(), MAX_CONSOLE_MESSAGES);
        assert_eq!(drained[0].message, "m2");
        assert_eq!(drained[MAX_CONSOLE_MESSAGES - 1].message, "m129");
        assert!(delegate.drain_console_messages().is_empty());
    }

    #[test]
    fn last_url_tracks_latest_value() {
        let delegate = HypercolorWebViewDelegate::new();
        assert_eq!(delegate.last_url(), None);

        delegate.on_url_changed("file:///tmp/one.html");
        assert_eq!(delegate.last_url().as_deref(), Some("file:///tmp/one.html"));

        delegate.on_url_changed("file:///tmp/two.html");
        assert_eq!(delegate.last_url().as_deref(), Some("file:///tmp/two.html"));
    }

    #[test]
    fn recent_console_messages_returns_tail_without_draining() {
        let delegate = HypercolorWebViewDelegate::new();
        delegate.on_console_message(&ConsoleLogLevel::Info, "first");
        delegate.on_console_message(&ConsoleLogLevel::Warn, "second");
        delegate.on_console_message(&ConsoleLogLevel::Error, "third");

        let recent = delegate.recent_console_messages(2);
        assert_eq!(
            recent,
            vec![
                ConsoleMessage {
                    level: "warn".to_owned(),
                    message: "second".to_owned()
                },
                ConsoleMessage {
                    level: "error".to_owned(),
                    message: "third".to_owned()
                }
            ]
        );

        let drained = delegate.drain_console_messages();
        assert_eq!(drained.len(), 3);
        assert!(delegate.drain_console_messages().is_empty());
    }

    #[test]
    fn reset_navigation_state_clears_navigation_tracking() {
        let delegate = HypercolorWebViewDelegate::new();

        delegate.on_new_frame_ready();
        delegate.on_page_loaded();
        delegate.on_url_changed("file:///tmp/effect.html");
        delegate.on_console_message(&ConsoleLogLevel::Warn, "stale warning");

        delegate.reset_navigation_state();

        assert!(!delegate.is_frame_ready());
        assert!(!delegate.is_page_loaded());
        assert_eq!(delegate.last_url(), None);
        assert!(delegate.drain_console_messages().is_empty());
        assert_eq!(delegate.frame_count(), 1);
    }
}
