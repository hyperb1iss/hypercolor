#![cfg(target_os = "macos")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hypercolor_macos_owner::wait_for_process_exit;

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
