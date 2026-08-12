use std::{env, process::ExitCode};

#[cfg(target_os = "macos")]
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
use hypercolor_macos_input::{
    MacosInputBatch, MacosInputConfig, MacosInputEvent, MacosInputSession,
    input_monitoring_granted, request_input_monitoring,
};

const DEFAULT_EVENTS: usize = 25;
const DEFAULT_SECONDS: u64 = 10;
const MAX_EVENTS: usize = 10_000;
const MAX_SECONDS: u64 = 300;
#[cfg(target_os = "macos")]
const BATCH_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Args {
    keyboard: bool,
    pointer: bool,
    authorize: bool,
    events: usize,
    seconds: u64,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            keyboard: false,
            pointer: true,
            authorize: false,
            events: DEFAULT_EVENTS,
            seconds: DEFAULT_SECONDS,
        }
    }
}

#[derive(Debug)]
#[cfg(target_os = "macos")]
struct DiagnosticBatch {
    epoch: u64,
    at_ms: u64,
    events: Vec<MacosInputEvent>,
    origin_x: f64,
    origin_y: f64,
    width: f64,
    height: f64,
    topology_generation: u64,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dump_macos_input: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let Some(args) = parse_args(env::args().skip(1))? else {
        print_help();
        return Ok(());
    };

    #[cfg(not(target_os = "macos"))]
    {
        let Args {
            keyboard,
            pointer,
            authorize,
            events,
            seconds,
        } = args;
        let _ = (keyboard, pointer, authorize, events, seconds);
        Err("requires macOS 15.2 or newer".to_owned())
    }

    #[cfg(target_os = "macos")]
    {
        if args.keyboard && !input_monitoring_granted() {
            if !args.authorize {
                return Err(
                    "keyboard capture needs Input Monitoring; rerun with --authorize to open the macOS permission flow"
                        .to_owned(),
                );
            }
            if !request_input_monitoring() {
                return Err(
                    "Input Monitoring was not granted; no keyboard session was started".to_owned(),
                );
            }
        }

        let started = Instant::now();
        let clock =
            Arc::new(move || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        let (sender, receiver) = mpsc::sync_channel(BATCH_CAPACITY);
        let delivery_drops = Arc::new(AtomicU64::new(0));
        let callback_delivery_drops = Arc::clone(&delivery_drops);
        let mut session = MacosInputSession::start(
            MacosInputConfig {
                keyboard: args.keyboard,
                pointer: args.pointer,
                epoch: 1,
                clock,
            },
            move |batch| {
                if matches!(
                    sender.try_send(owned_batch(batch)),
                    Err(mpsc::TrySendError::Full(_))
                ) {
                    callback_delivery_drops.fetch_add(1, Ordering::Relaxed);
                }
            },
        )
        .map_err(|error| error.to_string())?;

        let masks = session.effective_masks();
        println!(
            "session keyboard={} pointer={} keyboard_mask=0x{:x} pointer_mask=0x{:x}",
            args.keyboard, args.pointer, masks.keyboard, masks.pointer
        );

        let deadline = Instant::now() + Duration::from_secs(args.seconds);
        let mut seen = 0_usize;
        while seen < args.events {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let wait = deadline.saturating_duration_since(now);
            match receiver.recv_timeout(wait) {
                Ok(batch) => {
                    print_batch(&batch, args.events.saturating_sub(seen));
                    seen = seen.saturating_add(batch.events.len());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(
                        "native input worker stopped before the diagnostic completed".to_owned(),
                    );
                }
            }
        }

        session.stop();
        let diagnostics = session.diagnostics();
        println!(
            "summary events={} state={:?} capture_dropped={} diagnostic_delivery_dropped={} tap_disables={} unsupported_system={} invalid_scroll_phase={}",
            seen.min(args.events),
            session.worker_state(),
            diagnostics.dropped_events,
            delivery_drops.load(Ordering::Relaxed),
            diagnostics.tap_disable_count,
            diagnostics.unsupported_system_events,
            diagnostics.invalid_scroll_phases,
        );
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn owned_batch(batch: MacosInputBatch<'_>) -> DiagnosticBatch {
    DiagnosticBatch {
        epoch: batch.epoch,
        at_ms: batch.at_ms,
        events: batch.events.to_vec(),
        origin_x: batch.virtual_desktop.origin_x,
        origin_y: batch.virtual_desktop.origin_y,
        width: batch.virtual_desktop.width,
        height: batch.virtual_desktop.height,
        topology_generation: batch.virtual_desktop.topology_generation,
    }
}

#[cfg(target_os = "macos")]
fn print_batch(batch: &DiagnosticBatch, remaining: usize) {
    println!(
        "batch epoch={} at_ms={} topology_generation={} desktop=({:.3},{:.3}) {:.3}x{:.3}",
        batch.epoch,
        batch.at_ms,
        batch.topology_generation,
        batch.origin_x,
        batch.origin_y,
        batch.width,
        batch.height,
    );
    for event in batch.events.iter().take(remaining) {
        match event {
            MacosInputEvent::Key {
                virtual_keycode,
                pressed,
                autorepeat,
            } => println!(
                "event key physical_code={} pressed={} repeat={}",
                virtual_keycode, pressed, autorepeat
            ),
            MacosInputEvent::ModifierFlags {
                virtual_keycode,
                flags,
            } => println!(
                "event modifiers physical_code={} flags=0x{:x}",
                virtual_keycode,
                flags.bits()
            ),
            MacosInputEvent::Button { button, pressed } => {
                println!("event button kind={button:?} pressed={pressed}");
            }
            MacosInputEvent::Motion {
                x,
                y,
                delta_x,
                delta_y,
            } => println!("event motion global=({x:.3},{y:.3}) delta=({delta_x:.3},{delta_y:.3})"),
            MacosInputEvent::Wheel {
                fixed_delta_x,
                fixed_delta_y,
                unit,
                phase,
                momentum_phase,
            } => println!(
                "event wheel fixed=({fixed_delta_x},{fixed_delta_y}) unit={unit:?} phase={phase:?} momentum={momentum_phase:?}"
            ),
            MacosInputEvent::MediaKey {
                nx_key_type,
                pressed,
                repeat,
            } => println!(
                "event media physical_type={} pressed={} repeat={}",
                nx_key_type, pressed, repeat
            ),
            MacosInputEvent::StateGap { reason } => {
                println!("event state_gap reason={reason:?}");
            }
        }
    }
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args::default();
    let mut kinds_explicit = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--keyboard" => {
                if !kinds_explicit {
                    parsed.pointer = false;
                    kinds_explicit = true;
                }
                parsed.keyboard = true;
            }
            "--pointer" => {
                if !kinds_explicit {
                    parsed.pointer = false;
                    kinds_explicit = true;
                }
                parsed.pointer = true;
            }
            "--authorize" => parsed.authorize = true,
            "--events" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--events requires a value".to_owned())?;
                parsed.events = parse_bounded("--events", &value, 1, MAX_EVENTS)?;
            }
            "--seconds" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--seconds requires a value".to_owned())?;
                parsed.seconds = parse_bounded("--seconds", &value, 1, MAX_SECONDS)?;
            }
            _ => return Err(format!("unknown argument {arg:?}; use --help")),
        }
    }
    if !parsed.keyboard && !parsed.pointer {
        return Err("at least one of --keyboard or --pointer must be enabled".to_owned());
    }
    if parsed.authorize && !parsed.keyboard {
        return Err("--authorize is valid only with --keyboard".to_owned());
    }
    Ok(Some(parsed))
}

fn parse_bounded<T>(name: &str, value: &str, min: T, max: T) -> Result<T, String>
where
    T: std::str::FromStr + Copy + PartialOrd + std::fmt::Display,
{
    let parsed = value
        .parse::<T>()
        .map_err(|_| format!("{name} must be an integer"))?;
    if parsed < min || parsed > max {
        return Err(format!("{name} must be from {min} through {max}"));
    }
    Ok(parsed)
}

fn print_help() {
    println!(
        "dump_macos_input [--keyboard] [--pointer] [--authorize] [--events N] [--seconds N]\n\
         \n\
         Prints redacted native event kinds, physical codes, pointer geometry,\n\
         generations, and health counters. It never prints logical text.\n\
         \n\
         The default is pointer-only, 25 events, and a 10-second deadline.\n\
         --authorize may open the Input Monitoring flow and is valid only with\n\
         --keyboard. No other option presents system UI."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_bounded_pointer_only_capture() {
        assert_eq!(
            parse_args([]).expect("defaults parse"),
            Some(Args::default())
        );
    }

    #[test]
    fn explicit_kinds_compose_without_implicit_pointer() {
        let keyboard = parse_args(["--keyboard".to_owned()])
            .expect("keyboard parses")
            .expect("not help");
        assert!(keyboard.keyboard);
        assert!(!keyboard.pointer);

        let both = parse_args(["--keyboard".to_owned(), "--pointer".to_owned()])
            .expect("both parse")
            .expect("not help");
        assert!(both.keyboard);
        assert!(both.pointer);
    }

    #[test]
    fn authorization_is_keyboard_scoped() {
        assert!(parse_args(["--authorize".to_owned()]).is_err());
        assert!(parse_args(["--keyboard".to_owned(), "--authorize".to_owned()]).is_ok());
    }

    #[test]
    fn limits_are_closed_and_finite() {
        assert!(parse_args(["--events".to_owned(), "0".to_owned()]).is_err());
        assert!(parse_args(["--events".to_owned(), "10001".to_owned()]).is_err());
        assert!(parse_args(["--seconds".to_owned(), "301".to_owned()]).is_err());
    }
}
