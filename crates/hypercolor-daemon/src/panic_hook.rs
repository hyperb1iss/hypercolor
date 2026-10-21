//! Process-wide panic reporting.
//!
//! The default hook writes a panic only to stderr, and only prints a
//! backtrace when `RUST_BACKTRACE` asks for one. When stderr shares a sink
//! that stopped accepting writes (a full tmpfs, a closed pipe), a panicking
//! thread leaves no trace at all and the daemon keeps serving without it.
//!
//! This hook captures every panic on every thread: the thread name, the
//! payload, the source location, and a backtrace taken on the spot. It
//! hands the report to every registered observer (each render supervisor
//! persists its render thread's panics to the state directory, which
//! matters most in `panic = "abort"` builds where nothing runs after the
//! hook), keeps a few recent reports for code that catches the unwind,
//! chains the previous hook so stderr output stays exactly as before, and
//! records the report through `tracing` at error level.

use std::backtrace::Backtrace;
use std::collections::VecDeque;
use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, RwLock};
use std::thread::ThreadId;
use std::time::SystemTime;

/// How many recent reports stay available to [`take_recent_report`].
const RECENT_REPORT_CAPACITY: usize = 8;

/// Frames kept in [`PanicReport::backtrace_excerpt`].
const BACKTRACE_EXCERPT_FRAMES: usize = 48;

/// Upper bound on [`PanicReport::backtrace_excerpt`] in bytes.
const BACKTRACE_EXCERPT_BYTES: usize = 12 * 1024;

/// Frames near the top of a captured backtrace that may still belong to
/// the capture and the panic machinery rather than the code that panicked.
const PANIC_MACHINERY_SCAN_FRAMES: usize = 40;

/// Everything one panic left behind, captured inside the hook.
#[derive(Debug, Clone)]
pub struct PanicReport {
    /// Identity of the panicking thread.
    pub thread_id: ThreadId,
    /// Name of the panicking thread, or `<unnamed>`.
    pub thread_name: String,
    /// The panic message, when the payload is a string.
    pub payload: String,
    /// `file:line:column` of the panic, when the runtime reports one.
    pub location: Option<String>,
    /// The full backtrace captured in the hook.
    pub backtrace: String,
    /// Wall-clock time the hook ran.
    pub occurred_at: SystemTime,
}

impl PanicReport {
    fn capture(info: &PanicHookInfo<'_>) -> Self {
        let thread = std::thread::current();
        Self {
            thread_id: thread.id(),
            thread_name: thread.name().unwrap_or("<unnamed>").to_owned(),
            payload: info
                .payload_as_str()
                .unwrap_or("non-string panic payload")
                .to_owned(),
            location: info.location().map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            }),
            backtrace: Backtrace::force_capture().to_string(),
            occurred_at: SystemTime::now(),
        }
    }

    /// The backtrace without the capture and panic machinery frames,
    /// bounded in frames and bytes so it fits a persisted record.
    #[must_use]
    pub fn backtrace_excerpt(&self) -> String {
        backtrace_excerpt(&self.backtrace)
    }
}

/// Called with every panic report before the previous hook runs.
pub type PanicObserver = Arc<dyn Fn(&PanicReport) + Send + Sync>;

/// Keeps an observer registered until it is dropped.
#[must_use = "the observer is removed as soon as its registration drops"]
pub struct PanicObserverRegistration {
    id: u64,
}

impl Drop for PanicObserverRegistration {
    fn drop(&mut self) {
        OBSERVERS
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(id, _)| *id != self.id);
    }
}

static INSTALL: Once = Once::new();
static RECENT_REPORTS: Mutex<VecDeque<PanicReport>> = Mutex::new(VecDeque::new());
static OBSERVERS: RwLock<Vec<(u64, PanicObserver)>> = RwLock::new(Vec::new());
static NEXT_OBSERVER_ID: AtomicU64 = AtomicU64::new(1);

/// Install the process-wide panic hook.
///
/// Call this before any subsystem starts threads, and in particular before
/// Servo starts: its background hang monitor briefly swaps the hook and
/// restores whatever it found, so a hook installed while it holds the swap
/// would be lost. The previous hook is chained, never replaced. Calling
/// this more than once is a no-op.
pub fn install() {
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let report = PanicReport::capture(info);
            notify_observers(&report);
            remember(&report);
            previous(info);
            tracing::error!(
                target: "hypercolor_daemon::panic",
                thread = %report.thread_name,
                location = report.location.as_deref().unwrap_or("unknown"),
                payload = %report.payload,
                backtrace = %report.backtrace,
                "thread panicked"
            );
        }));
    });
}

/// Register `observer` to see every panic report until the returned
/// registration drops.
pub fn add_observer(observer: PanicObserver) -> PanicObserverRegistration {
    let id = NEXT_OBSERVER_ID.fetch_add(1, Ordering::Relaxed);
    OBSERVERS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((id, observer));
    PanicObserverRegistration { id }
}

/// Take the newest report recorded on `thread`, if one is still held.
pub fn take_recent_report(thread: ThreadId) -> Option<PanicReport> {
    take_recent_report_where(|report| report.thread_id == thread)
}

/// Take the newest held report that `matches`.
pub fn take_recent_report_where(matches: impl Fn(&PanicReport) -> bool) -> Option<PanicReport> {
    let mut reports = RECENT_REPORTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = reports.iter().rposition(matches)?;
    reports.remove(index)
}

fn notify_observers(report: &PanicReport) {
    // `try_read` keeps a panic raised while the observer list is being
    // changed from deadlocking inside its own hook.
    let observers: Vec<PanicObserver> = match OBSERVERS.try_read() {
        Ok(observers) => observers
            .iter()
            .map(|(_, observer)| Arc::clone(observer))
            .collect(),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned
            .into_inner()
            .iter()
            .map(|(_, observer)| Arc::clone(observer))
            .collect(),
        Err(std::sync::TryLockError::WouldBlock) => Vec::new(),
    };
    for observer in observers {
        observer(report);
    }
}

fn remember(report: &PanicReport) {
    let mut reports = match RECENT_REPORTS.try_lock() {
        Ok(reports) => reports,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if reports.len() == RECENT_REPORT_CAPACITY {
        reports.pop_front();
    }
    reports.push_back(report.clone());
}

/// Trim a captured backtrace to the frames worth persisting.
///
/// `Backtrace::force_capture` inside a hook records the capture itself, the
/// hook, and the panic machinery above the frame that panicked. The excerpt
/// skips that contiguous run near the top and keeps a bounded number of
/// the frames after it. Machinery frames further down (a `catch_unwind`
/// that will catch this panic, say) are never reached, because the skip
/// stops at the first frame that is not machinery.
#[must_use]
pub fn backtrace_excerpt(backtrace: &str) -> String {
    let frames = split_frames(backtrace);
    let start = panicking_frame_index(&frames);
    let mut excerpt = String::new();
    for frame in frames.iter().skip(start).take(BACKTRACE_EXCERPT_FRAMES) {
        if excerpt.len() + frame.len() > BACKTRACE_EXCERPT_BYTES {
            break;
        }
        excerpt.push_str(frame);
    }
    if excerpt.is_empty() {
        let mut end = backtrace.len().min(BACKTRACE_EXCERPT_BYTES);
        while !backtrace.is_char_boundary(end) {
            end -= 1;
        }
        excerpt.push_str(&backtrace[..end]);
    }
    excerpt
}

/// Split backtrace text into frames, each frame being its numbered symbol
/// line plus any `at file:line` lines that follow it.
fn split_frames(backtrace: &str) -> Vec<&str> {
    let mut frames = Vec::new();
    let mut frame_start: Option<usize> = None;
    let mut offset = 0;
    for line in backtrace.split_inclusive('\n') {
        if starts_frame(line) {
            if let Some(start) = frame_start {
                frames.push(&backtrace[start..offset]);
            }
            frame_start = Some(offset);
        }
        offset += line.len();
    }
    if let Some(start) = frame_start {
        frames.push(&backtrace[start..]);
    }
    frames
}

fn starts_frame(line: &str) -> bool {
    let trimmed = line.trim_start();
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && trimmed[digits..].starts_with(": ")
}

/// The frame std's own short backtraces start after: everything above it
/// is the backtrace capture, this hook, and the hook dispatch.
const SHORT_BACKTRACE_END: &str = "__rust_end_short_backtrace";

/// Index of the first frame worth keeping.
///
/// That is the frame after std's short backtrace marker, the same cut a
/// default panic message makes, so the excerpt opens with the panic entry
/// (`begin_panic_handler`, `panic_fmt`) and then the code that panicked.
/// Frame names cannot place the cut more precisely: a build without
/// debuginfo, such as the Windows CI tests, labels the caller's frames with
/// the nearest public symbol, which can be a `core::panicking` function.
/// Without the marker, the excerpt skips the contiguous run of capture and
/// panic runtime frames at the top and stops at the first frame it does not
/// recognize, so an unfamiliar frame costs a few extra lines rather than
/// hiding the panic site.
fn panicking_frame_index(frames: &[&str]) -> usize {
    frames
        .iter()
        .take(PANIC_MACHINERY_SCAN_FRAMES)
        .position(|frame| frame_symbol(frame).contains(SHORT_BACKTRACE_END))
        .map_or_else(
            || {
                frames
                    .iter()
                    .take(PANIC_MACHINERY_SCAN_FRAMES)
                    .take_while(|frame| is_capture_frame(frame) || is_panic_entry(frame))
                    .count()
            },
            |marker| marker + 1,
        )
}

fn frame_symbol(frame: &str) -> &str {
    frame.lines().next().unwrap_or_default()
}

/// Frames of the backtrace capture and this hook.
fn is_capture_frame(frame: &str) -> bool {
    let symbol = frame_symbol(frame);
    [
        "std::backtrace_rs::",
        "std::backtrace::",
        "hypercolor_daemon::panic_hook::",
        "<alloc::boxed::Box<F,A> as core::ops::function::Fn<Args>>::call",
        // The boxed hook's call on MSVC, which names its argument type.
        "PanicHookInfo",
    ]
    .iter()
    .any(|needle| symbol.contains(needle))
}

/// Frames of the runtime path from a `panic!` to the hook.
fn is_panic_entry(frame: &str) -> bool {
    let symbol = frame_symbol(frame);
    [
        "rust_begin_unwind",
        "core::panicking::",
        "core::option::expect_failed",
        "core::option::unwrap_failed",
        "core::result::unwrap_failed",
        "std::panicking::begin_panic",
        "std::panicking::panic_with_hook",
        "std::panicking::rust_panic_with_hook",
        "std::rt::begin_panic",
    ]
    .iter()
    .any(|needle| symbol.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::backtrace_excerpt;

    const CAPTURED: &str = "   0: std::backtrace::Backtrace::force_capture
             at /rustc/library/std/src/backtrace.rs:312:9
   1: hypercolor_daemon::panic_hook::PanicReport::capture
             at ./src/panic_hook.rs:60:24
   2: std::panicking::panic_with_hook
             at /rustc/library/std/src/panicking.rs:842:13
   3: core::panicking::panic_fmt
             at /rustc/library/core/src/panicking.rs:75:14
   4: hypercolor_daemon::render_thread::pipeline_driver::run_pipeline
             at ./src/render_thread/pipeline_driver.rs:40:13
   5: tokio::runtime::park::CachedParkThread::block_on
";

    #[test]
    fn excerpt_starts_at_the_frame_that_panicked() {
        let excerpt = backtrace_excerpt(CAPTURED);

        assert!(
            excerpt
                .trim_start()
                .starts_with("4: hypercolor_daemon::render_thread::pipeline_driver::run_pipeline"),
            "{excerpt}"
        );
        assert!(excerpt.contains("pipeline_driver.rs:40:13"));
        assert!(excerpt.contains("5: tokio::runtime::park"));
        assert!(!excerpt.contains("force_capture"));
    }

    /// An MSVC backtrace: symbols keep their generic arguments, and the
    /// panicking closure runs inside a `std::panicking` frame.
    const CAPTURED_MSVC: &str = "   0: std::backtrace_rs::backtrace::win64::trace
             at /rustc/library\\std\\src\\..\\..\\backtrace\\src\\backtrace\\win64.rs:85
   1: std::backtrace::Backtrace::create
             at /rustc/library\\std\\src\\backtrace.rs:331
   2: hypercolor_daemon::panic_hook::PanicReport::capture
             at .\\src\\panic_hook.rs:60
   3: hypercolor_daemon::panic_hook::install::closure$0::closure$0
             at .\\src\\panic_hook.rs:120
   4: alloc::boxed::impl$30::call<tuple$<ref$<std::panic::PanicHookInfo> >,dyn$<core::ops::function::Fn<tuple$<ref$<std::panic::PanicHookInfo> > > >,alloc::alloc::Global>
   5: std::panicking::panic_with_hook
             at /rustc/library\\std\\src\\panicking.rs:842
   6: std::panicking::begin_panic_handler::closure$0
             at /rustc/library\\std\\src\\panicking.rs:707
   7: std::sys::backtrace::__rust_end_short_backtrace<std::panicking::begin_panic_handler::closure_env$0,never$>
   8: std::panicking::begin_panic_handler
             at /rustc/library\\std\\src\\panicking.rs:698
   9: core::panicking::panic_fmt
             at /rustc/library\\core\\src\\panicking.rs:75
  10: std::panicking::catch_unwind::do_call<hypercolor_daemon::render_thread::pipeline_driver::run_pipeline::closure_env$0,tuple$<> >
             at .\\src\\render_thread\\pipeline_driver.rs:40
  11: std::panicking::catch_unwind<tuple$<>,hypercolor_daemon::render_thread::pipeline_driver::run_pipeline::closure_env$0>
  12: std::sys::backtrace::__rust_begin_short_backtrace<hypercolor_daemon::render_thread::spawn::closure_env$0,tuple$<> >
  13: std::sys::thread::windows::impl$0::new::thread_start
";

    #[test]
    fn excerpt_starts_after_the_short_backtrace_marker_with_msvc_symbols() {
        let excerpt = backtrace_excerpt(CAPTURED_MSVC);

        assert!(
            excerpt
                .trim_start()
                .starts_with("8: std::panicking::begin_panic_handler"),
            "{excerpt}"
        );
        assert!(excerpt.contains(
            "10: std::panicking::catch_unwind::do_call<hypercolor_daemon::render_thread"
        ));
        assert!(excerpt.contains("pipeline_driver.rs:40"));
        assert!(!excerpt.contains("PanicReport::capture"));
    }

    /// A Windows test build without debuginfo: the caller's frames take the
    /// nearest public symbol, here a `core::panicking` instantiation.
    const CAPTURED_WITHOUT_DEBUGINFO: &str = "   0: std::backtrace_rs::backtrace::win64::trace
   1: std::backtrace::Backtrace::force_capture
   2: <std::sync::once::Once>::call_once::<hypercolor_daemon::panic_hook::install::{closure#0}>::{closure#0}
   3: std::panicking::panic_with_hook
   4: std::panicking::panic_handler::closure$0
   5: std::sys::backtrace::__rust_end_short_backtrace<std::panicking::panic_handler::closure_env$0,never$>
   6: std::panicking::panic_handler
   7: core::panicking::panic_fmt
   8: core::panicking::assert_failed::<alloc::string::String, &str>
   9: core::panicking::assert_failed::<alloc::string::String, &str>
  10: std::sys::thread::windows::impl$0::new::thread_start
  11: BaseThreadInitThunk
";

    #[test]
    fn excerpt_keeps_misnamed_caller_frames_from_a_build_without_debuginfo() {
        let excerpt = backtrace_excerpt(CAPTURED_WITHOUT_DEBUGINFO);

        assert!(
            excerpt
                .trim_start()
                .starts_with("6: std::panicking::panic_handler"),
            "{excerpt}"
        );
        assert!(excerpt.contains("8: core::panicking::assert_failed"));
        assert!(!excerpt.contains("force_capture"));
    }

    #[test]
    fn excerpt_without_a_panic_entry_skips_only_the_capture() {
        let excerpt = backtrace_excerpt(
            "   0: std::backtrace::Backtrace::force_capture
   1: hypercolor_daemon::panic_hook::PanicReport::capture
   2: std::panicking::catch_unwind::do_call<some::caller::closure_env$0>
   3: some::caller::run
",
        );

        assert!(
            excerpt
                .trim_start()
                .starts_with("2: std::panicking::catch_unwind"),
            "{excerpt}"
        );
    }

    #[test]
    fn excerpt_without_recognizable_frames_keeps_the_raw_text() {
        assert_eq!(
            backtrace_excerpt("disabled backtrace"),
            "disabled backtrace"
        );
    }

    #[test]
    fn excerpt_is_bounded_in_frames_and_bytes() {
        use std::fmt::Write as _;

        let mut long = String::new();
        for index in 0..500 {
            writeln!(
                long,
                "  {index}: some::deeply::nested::frame_{index}\n             at ./src/lib.rs:{index}:1"
            )
            .expect("writing to a String cannot fail");
        }

        let excerpt = backtrace_excerpt(&long);

        assert!(excerpt.len() <= super::BACKTRACE_EXCERPT_BYTES);
        assert!(excerpt.contains("0: some::deeply::nested::frame_0"));
        assert!(!excerpt.contains("frame_48\n"));
    }
}
