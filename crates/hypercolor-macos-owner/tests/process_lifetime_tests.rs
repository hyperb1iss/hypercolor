#![cfg(target_os = "macos")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hypercolor_macos_owner::{wait_for_process_exit, wait_for_process_exit_within};

fn spawn_sleep(seconds: &str) -> std::process::Child {
    Command::new("/bin/sleep")
        .arg(seconds)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("fixture child should spawn")
}

#[test]
fn kqueue_watch_returns_when_the_process_exits() {
    let mut child = spawn_sleep("0.3");
    let started = Instant::now();
    wait_for_process_exit(child.id()).expect("watch should register");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "watch returned before the process exited"
    );
    child.wait().expect("child reaps");
}

#[test]
fn kqueue_watch_returns_immediately_for_an_already_exited_process() {
    let mut child = spawn_sleep("0");
    child.wait().expect("child reaps");
    let started = Instant::now();
    wait_for_process_exit(child.id()).expect("exited process is not an error");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn kqueue_watch_fires_on_sigkill_without_reaping() {
    let mut child = spawn_sleep("30");
    let pid = child.id();
    let waiter = std::thread::spawn(move || wait_for_process_exit(pid));
    std::thread::sleep(Duration::from_millis(100));
    child.kill().expect("kill fixture child");
    waiter
        .join()
        .expect("waiter thread")
        .expect("watch should observe the kill");
    child.wait().expect("child reaps");
}

#[test]
fn timed_watch_reports_a_process_that_outlives_its_window() {
    let mut child = spawn_sleep("30");
    let started = Instant::now();
    assert!(
        !wait_for_process_exit_within(child.id(), Duration::from_millis(200))
            .expect("watch should register"),
        "the process is still running"
    );
    assert!(started.elapsed() >= Duration::from_millis(150));
    child.kill().expect("kill fixture child");
    child.wait().expect("child reaps");
}

#[test]
fn timed_watch_ends_as_soon_as_the_process_exits() {
    let mut child = spawn_sleep("0.2");
    let started = Instant::now();
    assert!(
        wait_for_process_exit_within(child.id(), Duration::from_secs(30))
            .expect("watch should register"),
        "the exit lands inside the window"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    child.wait().expect("child reaps");
}

#[test]
fn timed_watch_reports_an_already_exited_process_with_a_zero_window() {
    let mut child = spawn_sleep("0");
    child.wait().expect("child reaps");
    assert!(
        wait_for_process_exit_within(child.id(), Duration::ZERO)
            .expect("an exited process is not an error")
    );
}
