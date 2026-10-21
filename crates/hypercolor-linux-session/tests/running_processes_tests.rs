//! Process inventory through procfs.

use hypercolor_linux_session::running_processes;

#[test]
fn the_inventory_is_reported_only_on_linux() {
    let processes = running_processes();
    if cfg!(target_os = "linux") {
        let processes = processes.expect("Linux should list processes");
        assert!(!processes.is_empty(), "at least this test is running");
    } else {
        assert!(processes.is_none(), "only Linux lists processes here");
    }
}

/// The first listed command line that `matches`, polled for a few seconds.
///
/// `spawn` returns once the child is forked, and until it execs, procfs
/// still shows the parent's name and command line, so a single read right
/// after spawning races the exec.
#[cfg(target_os = "linux")]
fn wait_for_listed<F>(matches: F) -> Option<(String, String)>
where
    F: Fn(&str) -> bool,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let processes = running_processes().expect("Linux should list processes");
        let found = processes.into_iter().find_map(|process| {
            let line = process.command_line?;
            matches(&line).then_some((process.name, line))
        });
        if found.is_some() || std::time::Instant::now() >= deadline {
            return found;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_child_shows_up_with_its_name_and_command_line() {
    let mut child = std::process::Command::new("sleep")
        .arg("31.4159")
        .spawn()
        .expect("spawn sleep");
    let found = wait_for_listed(|line| line.ends_with("sleep 31.4159"));
    let _ = child.kill();
    let _ = child.wait();

    let (name, _) = found.expect("the child sleep should be listed");
    assert_eq!(name, "sleep");
}

#[cfg(target_os = "linux")]
#[test]
fn arguments_with_spaces_are_quoted() {
    // `sh -c SCRIPT NAME` keeps NAME as $0 in its own argv. The script
    // blocks in the `read` builtin on a pipe, so killing sh leaves no
    // child behind. The marker is built at runtime so no shell holding
    // this source matches.
    let marker = format!("quoting probe {}", std::process::id());
    let mut child = std::process::Command::new("sh")
        .args(["-c", "read line; true", marker.as_str()])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawn sh");
    let found = wait_for_listed(|line| line.contains(marker.as_str()));
    let _ = child.kill();
    let _ = child.wait();

    let (_, line) = found.expect("the probe shell should be listed");
    assert!(
        line.ends_with(&format!(r#"-c "read line; true" "{marker}""#)),
        "arguments with spaces are single quoted words: {line}"
    );
}
