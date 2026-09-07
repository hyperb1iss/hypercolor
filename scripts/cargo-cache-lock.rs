use std::env;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::os::raw::c_int;
use std::process::{self, Command};
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::FromRawFd;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(unix)]
unsafe extern "C" {
    fn getpgrp() -> c_int;
    fn isatty(fd: c_int) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn setpgid(pid: c_int, process_group: c_int) -> c_int;
    fn signal(signal: c_int, handler: usize) -> usize;
    fn tcgetpgrp(fd: c_int) -> c_int;
    fn tcsetpgrp(fd: c_int, process_group: c_int) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
}

#[cfg(unix)]
const SIGHUP: c_int = 1;
#[cfg(unix)]
const SIGINT: c_int = 2;
const SIGTERM: c_int = 15;
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
const SIGCONT: c_int = 19;
#[cfg(all(unix, not(any(target_os = "macos", target_os = "freebsd"))))]
const SIGCONT: c_int = 18;
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
const SIGTSTP: c_int = 18;
#[cfg(all(unix, not(any(target_os = "macos", target_os = "freebsd"))))]
const SIGTSTP: c_int = 20;
#[cfg(unix)]
const SIGTTOU: c_int = 22;
#[cfg(unix)]
const SIG_DFL: usize = 0;
#[cfg(unix)]
const SIG_IGN: usize = 1;
#[cfg(unix)]
const WNOHANG: c_int = 1;
#[cfg(unix)]
const WUNTRACED: c_int = 2;

static PENDING_SIGNAL: AtomicI32 = AtomicI32::new(0);

struct ProcessGroup {
    caller: u32,
    tty_fd: Option<c_int>,
}

#[cfg(unix)]
extern "C" fn record_signal(signal: c_int) {
    PENDING_SIGNAL.store(signal, Ordering::Relaxed);
}

#[cfg(unix)]
fn configure_process_group() -> io::Result<ProcessGroup> {
    let process_id = process::id();
    let current = unsafe { getpgrp() };
    let mut tty_fd = None;
    for fd in 0..=2 {
        unsafe {
            if isatty(fd) == 1 && tcgetpgrp(fd) == current {
                tty_fd = Some(fd);
                break;
            }
        }
    }
    if current == process_id as c_int || tty_fd.is_some() {
        let caller = u32::try_from(current)
            .map_err(|_| io::Error::other("invalid process group returned by getpgrp"))?;
        return Ok(ProcessGroup { caller, tty_fd });
    }
    if unsafe { setpgid(0, 0) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(ProcessGroup {
        caller: process_id,
        tty_fd: None,
    })
}

#[cfg(not(unix))]
fn configure_process_group() -> io::Result<ProcessGroup> {
    Ok(ProcessGroup {
        caller: process::id(),
        tty_fd: None,
    })
}

#[cfg(unix)]
fn install_signal_handlers() {
    unsafe {
        signal(SIGHUP, record_signal as *const () as usize);
        signal(SIGINT, record_signal as *const () as usize);
        signal(SIGTERM, record_signal as *const () as usize);
        signal(SIGTTOU, SIG_IGN);
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {}

#[cfg(unix)]
fn configure_child(command: &mut Command, _process_group: &ProcessGroup) {
    command.process_group(0);
    unsafe {
        command.pre_exec(|| {
            signal(SIGHUP, SIG_DFL);
            signal(SIGINT, SIG_DFL);
            signal(SIGTERM, SIG_DFL);
            signal(SIGTTOU, SIG_DFL);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_child(_command: &mut Command, _process_group: &ProcessGroup) {}

#[cfg(unix)]
fn forward_signal(process_group: u32, signal_number: c_int) {
    unsafe {
        signal(signal_number, SIG_IGN);
        kill(-(process_group as c_int), signal_number);
        if signal_number == SIGTERM {
            kill(-(process_group as c_int), SIGCONT);
        }
    }
}

#[cfg(not(unix))]
fn forward_signal(_process_group: u32, _signal_number: c_int) {}

#[cfg(unix)]
fn process_group_active(process_group: u32) -> bool {
    unsafe { kill(-(process_group as c_int), 0) == 0 }
}

#[cfg(not(unix))]
fn process_group_active(_process_group: u32) -> bool {
    false
}

#[cfg(unix)]
fn foreground(tty_fd: Option<c_int>, process_group: u32) {
    if let Some(fd) = tty_fd {
        unsafe {
            tcsetpgrp(fd, process_group as c_int);
        }
    }
}

#[cfg(not(unix))]
fn foreground(_tty_fd: Option<c_int>, _process_group: u32) {}

#[cfg(unix)]
fn stop_caller_with_child(process_group: &ProcessGroup, child_group: u32) {
    foreground(process_group.tty_fd, process_group.caller);
    unsafe {
        signal(SIGTSTP, SIG_DFL);
        kill(-(process_group.caller as c_int), SIGTSTP);
        signal(SIGTSTP, SIG_IGN);
    }
    foreground(process_group.tty_fd, child_group);
    forward_signal(child_group, SIGCONT);
}

#[cfg(not(unix))]
fn stop_caller_with_child(_process_group: &ProcessGroup, _child_group: u32) {}

fn wait_status_exit_code(status: c_int) -> Option<i32> {
    let low = status & 0x7f;
    if low == 0 {
        return Some((status >> 8) & 0xff);
    }
    if low != 0x7f {
        return Some(128 + low);
    }
    None
}

fn open_lock(mode: &str, path: &OsStr) -> io::Result<File> {
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)?;
    match mode {
        "shared" => lock.lock_shared()?,
        "exclusive" => lock.lock()?,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "lock mode must be shared or exclusive",
            ));
        }
    }
    Ok(lock)
}

fn remove_lease(path: &OsStr) -> io::Result<()> {
    fs::remove_file(path).or_else(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        }
    })
}

#[cfg(unix)]
fn release_setup_lock(pid: u32) -> io::Result<()> {
    let mut release = unsafe { File::from_raw_fd(6) };
    writeln!(release, "release")?;
    drop(release);
    drop(unsafe { File::from_raw_fd(8) });
    let mut status = 0;
    if unsafe { waitpid(pid as c_int, &mut status, 0) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn release_setup_lock(_pid: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn wait_for_child(
    child_pid: u32,
    child_group: u32,
    process_group: &ProcessGroup,
) -> io::Result<(i32, bool)> {
    let mut forwarded_signal = false;
    loop {
        let signal_number = PENDING_SIGNAL.swap(0, Ordering::Relaxed);
        if signal_number != 0 {
            forwarded_signal = true;
            forward_signal(child_group, signal_number);
        }
        let mut status = 0;
        let result = unsafe { waitpid(child_pid as c_int, &mut status, WNOHANG | WUNTRACED) };
        if result == child_pid as c_int {
            if let Some(code) = wait_status_exit_code(status) {
                return Ok((code, forwarded_signal));
            }
            if process_group.tty_fd.is_some() {
                stop_caller_with_child(process_group, child_group);
            }
        } else if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(unix))]
fn wait_for_child(
    _child_pid: u32,
    _child_group: u32,
    _process_group: &ProcessGroup,
) -> io::Result<(i32, bool)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "supervised execution requires Unix",
    ))
}

fn run_locked(mut args: env::ArgsOs) -> io::Result<i32> {
    let path = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing lock path"))?;
    let lease_path = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing lease path"))?;
    let setup_lock_pid = args
        .next()
        .and_then(|value| value.to_str().and_then(|value| value.parse::<u32>().ok()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid setup lock pid"))?;
    let program = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing command"))?;
    let lock = open_lock("shared", &path)?;
    if let Some(ready_path) = env::var_os("HYPERCOLOR_LOCK_HANDOFF_READY") {
        fs::write(ready_path, b"ready\n")?;
    }
    if let Ok(delay) = env::var("HYPERCOLOR_LOCK_HANDOFF_DELAY_MS") {
        let delay = delay
            .parse::<u64>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid handoff delay"))?;
        thread::sleep(Duration::from_millis(delay));
    }
    release_setup_lock(setup_lock_pid)?;
    let process_group = configure_process_group()?;
    fs::write(&lease_path, format!("{}\n", process_group.caller))?;
    install_signal_handlers();
    let mut command = Command::new(program);
    command.args(args);
    configure_child(&mut command, &process_group);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            remove_lease(&lease_path)?;
            return Err(error);
        }
    };
    let child_pid = child.id();
    let child_group = child_pid;
    fs::write(&lease_path, format!("{child_group}\n"))?;
    foreground(process_group.tty_fd, child_group);
    let (code, _forwarded_signal) = wait_for_child(child_pid, child_group, &process_group)?;
    foreground(process_group.tty_fd, process_group.caller);
    if !process_group_active(child_group) {
        remove_lease(&lease_path)?;
    }
    drop(lock);
    Ok(code)
}

fn main() -> io::Result<()> {
    let mut args = env::args_os();
    let _program = args.next();
    let mode = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing lock mode"))?;
    if mode == "run-shared" {
        process::exit(run_locked(args)?);
    }
    let path = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing lock path"))?;
    let mode = mode
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid lock mode"))?;
    let lock = open_lock(mode, &path)?;
    println!("locked");
    io::stdout().flush()?;
    let mut release = String::new();
    io::stdin().lock().read_line(&mut release)?;
    lock.unlock()
}
