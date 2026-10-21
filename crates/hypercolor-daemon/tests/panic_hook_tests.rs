//! The process panic hook records every thread's panic with a backtrace,
//! hands it to the registered observers, keeps it for the code that
//! catches the unwind, and still runs the hook that was installed before.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};

use hypercolor_daemon::panic_hook::{self, PanicObserver, PanicReport};

/// Observers and the hook are process-wide, so tests that panic take turns.
static PANIC_SLOT: Mutex<()> = Mutex::new(());

/// Panics the hook that was installed before ours has seen.
static PREVIOUS_HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Install a counting hook, then the daemon hook on top of it, once.
fn install_over_a_previous_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            PREVIOUS_HOOK_CALLS.fetch_add(1, Ordering::SeqCst);
            default(info);
        }));
        panic_hook::install();
    });
}

fn collecting_observer() -> (PanicObserver, Arc<Mutex<Vec<PanicReport>>>) {
    let observed: Arc<Mutex<Vec<PanicReport>>> = Arc::default();
    let sink = Arc::clone(&observed);
    let observer: PanicObserver = Arc::new(move |report: &PanicReport| {
        sink.lock()
            .expect("observer sink should lock")
            .push(report.clone());
    });
    (observer, observed)
}

fn panic_on_named_thread(name: &str, payload: &'static str) -> Option<PanicReport> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let caught = std::panic::catch_unwind(|| panic!("{payload}"));
            assert!(caught.is_err());
            panic_hook::take_recent_report(std::thread::current().id())
        })
        .expect("probe thread should spawn")
        .join()
        .expect("probe thread caught its own panic")
}

#[test]
fn hook_reports_a_named_thread_panic_to_observers_the_catcher_and_the_previous_hook() {
    let _slot = PANIC_SLOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    install_over_a_previous_hook();
    let (observer, observed) = collecting_observer();
    let registration = panic_hook::add_observer(observer);
    let previous_calls = PREVIOUS_HOOK_CALLS.load(Ordering::SeqCst);

    let recent = panic_on_named_thread("hypercolor-panic-probe", "probe panic payload")
        .expect("the hook should keep the report for the catcher");
    drop(registration);

    assert_eq!(recent.thread_name, "hypercolor-panic-probe");
    assert_eq!(recent.payload, "probe panic payload");
    assert!(
        recent
            .location
            .as_deref()
            .is_some_and(|location| location.contains("panic_hook_tests.rs")),
        "location should point at the panic site: {:?}",
        recent.location
    );
    assert!(
        recent.backtrace_excerpt().contains("panic_hook_tests"),
        "the excerpt should start near the panicking frame:\n{}",
        recent.backtrace_excerpt()
    );
    assert!(
        PREVIOUS_HOOK_CALLS.load(Ordering::SeqCst) > previous_calls,
        "the hook installed before the daemon hook should still run"
    );
    let observed = observed.lock().expect("observer sink should lock");
    assert!(
        observed
            .iter()
            .any(|report| report.thread_name == "hypercolor-panic-probe"
                && report.payload == "probe panic payload"),
        "the observer should see the panic before the unwind is caught"
    );
}

#[test]
fn dropping_a_registration_removes_only_that_observer() {
    let _slot = PANIC_SLOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    install_over_a_previous_hook();
    let (first, first_seen) = collecting_observer();
    let (second, second_seen) = collecting_observer();
    let first_registration = panic_hook::add_observer(first);
    let second_registration = panic_hook::add_observer(second);

    drop(first_registration);
    let _ = panic_on_named_thread("hypercolor-observer-probe", "second observer payload");
    drop(second_registration);

    let seen = |reports: &Arc<Mutex<Vec<PanicReport>>>| {
        reports
            .lock()
            .expect("observer sink should lock")
            .iter()
            .any(|report| report.payload == "second observer payload")
    };
    assert!(!seen(&first_seen));
    assert!(seen(&second_seen));
}
