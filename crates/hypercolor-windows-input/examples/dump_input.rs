//! Live Raw Input dump, for the hardware acceptance pass.
//!
//! ```text
//! cargo run -p hypercolor-windows-input --example dump_input
//! ```
//!
//! What this can establish: that real hardware produces the edges we expect,
//! that positional names match the Linux daemon's for the same physical keys,
//! that a held key repeats, that a hi-res wheel reports sub-notch values, and
//! that unplugging a keyboard mid-hold empties its held set.
//!
//! What it cannot: handle reuse, stale buffered identity, pointer shadowing,
//! and generation behaviour are all invisible to an eyeball on a dump, which
//! is exactly why they are covered by unit tests instead.

fn main() {
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("dump_input needs Windows: Raw Input has no equivalent elsewhere.");
        std::process::exit(1);
    }

    #[cfg(target_os = "windows")]
    windows_main();
}

/// Numbers each delivered batch.
///
/// An event appearing twice has two very different causes, and the acceptance
/// pass has to be able to tell them apart: the same batch number twice is a
/// delivery bug, while two batch numbers with different devices is two HID
/// collections of one physical keyboard legitimately reporting the same key.
#[cfg(target_os = "windows")]
static NEXT_BATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Tail of a device path — enough to tell two collections of the same physical
/// device apart, without a full interface path on every line.
#[cfg(target_os = "windows")]
fn short_id(source_id: &str) -> String {
    let trimmed = source_id.trim_end_matches('}');
    let tail: String = trimmed.chars().rev().take(12).collect();
    tail.chars().rev().collect()
}

#[cfg(target_os = "windows")]
fn windows_main() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use hypercolor_windows_input::{
        RawInputConfig, RawInputEvent, RawInputSession, SessionState, interactive_session_state,
    };

    if interactive_session_state() == SessionState::NoInteractiveSession {
        eprintln!(
            "no interactive window station — Raw Input cannot see input here. \
             Run this in your own desktop session, not as a service."
        );
        std::process::exit(1);
    }

    let epoch = Instant::now();
    let running = Arc::new(AtomicBool::new(true));

    println!("watching keyboard and mouse. ctrl-c to stop.\n");

    let session = RawInputSession::start(
        RawInputConfig {
            keyboard: true,
            mouse: true,
            clock: Arc::new(move || u64::try_from(epoch.elapsed().as_millis()).unwrap_or(u64::MAX)),
            epoch: 1,
        },
        |batch| {
            // Batches are numbered and events carry their device, so the two
            // ways an event can appear twice stay distinguishable: the same
            // batch number twice means a delivery bug, two batch numbers with
            // different devices means two HID collections legitimately
            // reporting the same physical key.
            let batch_no = NEXT_BATCH.fetch_add(1, Ordering::Relaxed);
            for event in batch.events {
                match event {
                    RawInputEvent::Key {
                        source_id,
                        make_code,
                        prefix,
                        vkey,
                        pressed,
                    } => println!(
                        "{:>8} #{batch_no:<5} key   make={make_code:#06X} prefix={prefix:?} \
                         vk={vkey:#06X} {:4} {}",
                        batch.at_ms,
                        if *pressed { "down" } else { "up" },
                        short_id(source_id)
                    ),
                    RawInputEvent::Button {
                        source_id,
                        button,
                        pressed,
                    } => println!(
                        "{:>8} #{batch_no:<5} btn   {} {:4} {}",
                        batch.at_ms,
                        button.canonical_name(),
                        if *pressed { "down" } else { "up" },
                        short_id(source_id)
                    ),
                    RawInputEvent::Wheel {
                        source_id,
                        delta_hi_res,
                    } => println!(
                        "{:>8} #{batch_no:<5} wheel {delta_hi_res:+} {}",
                        batch.at_ms,
                        short_id(source_id)
                    ),
                    RawInputEvent::MotionRelative { source_id, dx, dy } => println!(
                        "{:>8} #{batch_no:<5} move  {dx:+} {dy:+} {}",
                        batch.at_ms,
                        short_id(source_id)
                    ),
                    RawInputEvent::MotionAbsolute {
                        source_id,
                        norm_x,
                        norm_y,
                        virtual_desktop,
                    } => println!(
                        "{:>8} #{batch_no:<5} abs   {norm_x:.4} {norm_y:.4} {} {}",
                        batch.at_ms,
                        if *virtual_desktop { "vdesk" } else { "primary" },
                        short_id(source_id)
                    ),
                    RawInputEvent::DeviceArrived {
                        source_id,
                        label,
                        kind,
                    } => println!(
                        "{:>8} #{batch_no:<5} +dev  {label} ({kind:?}) {}",
                        batch.at_ms,
                        short_id(source_id)
                    ),
                    RawInputEvent::DeviceRemoved { source_id } => println!(
                        "{:>8} #{batch_no:<5} -dev  {}",
                        batch.at_ms,
                        short_id(source_id)
                    ),
                    RawInputEvent::StateGap { source_id } => println!(
                        "{:>8} #{batch_no:<5} GAP   rollover overrun on {}",
                        batch.at_ms,
                        short_id(source_id)
                    ),
                }
            }
            if let Some(cursor) = batch.cursor {
                println!(
                    "{:>8} #{batch_no:<5} cur   {},{} ({:.4},{:.4})",
                    batch.at_ms, cursor.x, cursor.y, cursor.norm_x, cursor.norm_y
                );
            }
        },
    );

    let mut session = match session {
        Ok(session) => session,
        Err(error) => {
            eprintln!("could not start: {error}");
            std::process::exit(1);
        }
    };

    println!("{} device(s) registered\n", session.device_count());

    let flag = Arc::clone(&running);
    if ctrlc_handler(move || flag.store(false, Ordering::Release)).is_err() {
        eprintln!("note: ctrl-c handler unavailable; stopping after 60s instead");
    }

    let deadline = Instant::now() + Duration::from_secs(60);
    while running.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }

    session.stop();
    println!("\nstopped.");
}

/// Install a console control handler without taking a dependency for it.
#[cfg(target_os = "windows")]
fn ctrlc_handler(on_break: impl Fn() + Send + 'static) -> Result<(), ()> {
    use std::sync::Mutex;
    use std::sync::OnceLock;

    use windows::Win32::System::Console::SetConsoleCtrlHandler;
    use windows::core::BOOL;

    type Handler = Box<dyn Fn() + Send>;
    static HANDLER: OnceLock<Mutex<Option<Handler>>> = OnceLock::new();

    unsafe extern "system" fn on_ctrl(_kind: u32) -> BOOL {
        if let Some(slot) = HANDLER.get()
            && let Ok(guard) = slot.lock()
            && let Some(handler) = guard.as_ref()
        {
            handler();
        }
        BOOL(1)
    }

    let slot = HANDLER.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(Box::new(on_break));
    }

    // SAFETY: installs a process-wide console handler with a `'static`
    // function pointer; the boxed closure it calls lives in a `OnceLock` that
    // outlives the process.
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true) }.map_err(|_| ())
}
