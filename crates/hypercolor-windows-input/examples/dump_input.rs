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
            for event in batch.events {
                match event {
                    RawInputEvent::Key {
                        make_code,
                        prefix,
                        vkey,
                        pressed,
                        ..
                    } => println!(
                        "{:>8} key   make={make_code:#06X} prefix={prefix:?} vk={vkey:#06X} {}",
                        batch.at_ms,
                        if *pressed { "down" } else { "up" }
                    ),
                    RawInputEvent::Button {
                        button, pressed, ..
                    } => println!(
                        "{:>8} btn   {} {}",
                        batch.at_ms,
                        button.canonical_name(),
                        if *pressed { "down" } else { "up" }
                    ),
                    RawInputEvent::Wheel { delta_hi_res, .. } => {
                        println!("{:>8} wheel {delta_hi_res:+}", batch.at_ms);
                    }
                    RawInputEvent::MotionRelative { dx, dy, .. } => {
                        println!("{:>8} move  {dx:+} {dy:+}", batch.at_ms);
                    }
                    RawInputEvent::MotionAbsolute { norm_x, norm_y, .. } => {
                        println!("{:>8} abs   {norm_x:.4} {norm_y:.4}", batch.at_ms);
                    }
                    RawInputEvent::DeviceArrived { label, kind, .. } => {
                        println!("{:>8} +dev  {label} ({kind:?})", batch.at_ms);
                    }
                    RawInputEvent::DeviceRemoved { source_id } => {
                        println!("{:>8} -dev  {source_id}", batch.at_ms);
                    }
                    RawInputEvent::StateGap { source_id } => {
                        println!("{:>8} GAP   rollover overrun on {source_id}", batch.at_ms);
                    }
                }
            }
            if let Some(cursor) = batch.cursor {
                println!(
                    "{:>8} cur   {},{} ({:.4},{:.4})",
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
