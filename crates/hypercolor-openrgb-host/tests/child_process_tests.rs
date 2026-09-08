//! A wrapper exiting must not release authority while its descendants remain.

#[cfg(unix)]
fn fixture(directory: &std::path::Path) -> std::process::Child {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new("/bin/sh");
    command.args(["-c", "sh -c 'trap \"\" TERM; while :; do printf x >> \"$1\"; sleep 0.01; done' stubborn \"$1/heartbeat\" & wait", "wrapper"])
        .arg(directory)
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let child = command.spawn().expect("fixture spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !directory.join("heartbeat").exists() {
        assert!(std::time::Instant::now() < deadline, "descendant starts");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    child
}

#[cfg(unix)]
fn assert_descendant_stopped(directory: &std::path::Path) {
    let heartbeat = directory.join("heartbeat");
    std::thread::sleep(std::time::Duration::from_millis(50));
    let before = std::fs::metadata(&heartbeat).expect("heartbeat").len();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(
        std::fs::metadata(heartbeat).expect("heartbeat").len(),
        before,
        "stubborn descendant must stop before ownership releases"
    );
}

#[cfg(unix)]
#[test]
fn stop_kills_stubborn_descendant_after_wrapper_exits_on_term() {
    let directory = tempfile::tempdir().expect("directory");
    let mut child = fixture(directory.path());
    hypercolor_openrgb_host::stop_owned_server(&mut child, std::time::Duration::from_secs(1))
        .expect("stop tree");
    assert!(child.try_wait().expect("reaped").is_some());
    assert_descendant_stopped(directory.path());
}

#[cfg(unix)]
#[test]
fn spontaneous_wrapper_exit_cleans_descendants_before_reaping() {
    let directory = tempfile::tempdir().expect("directory");
    let mut child = fixture(directory.path());
    child.kill().expect("wrapper exits alone");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if hypercolor_openrgb_host::reap_owned_server(&mut child)
            .expect("reap tree")
            .is_some()
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "wrapper exits");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_descendant_stopped(directory.path());
}
