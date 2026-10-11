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

#[cfg(target_os = "linux")]
#[test]
fn a_child_shows_up_with_its_name_and_command_line() {
    let mut child = std::process::Command::new("sleep")
        .arg("31.4159")
        .spawn()
        .expect("spawn sleep");
    let processes = running_processes().expect("Linux should list processes");
    let _ = child.kill();
    let _ = child.wait();

    let found = processes
        .iter()
        .find(|process| {
            process
                .command_line
                .as_deref()
                .is_some_and(|line| line.ends_with("sleep 31.4159"))
        })
        .expect("the child sleep should be listed");
    assert_eq!(found.name, "sleep");
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
    std::thread::sleep(std::time::Duration::from_millis(100));
    let processes = running_processes().expect("Linux should list processes");
    let _ = child.kill();
    let _ = child.wait();

    let line = processes
        .iter()
        .filter_map(|process| process.command_line.as_deref())
        .find(|line| line.contains(marker.as_str()))
        .expect("the probe shell should be listed");
    assert!(
        line.ends_with(&format!(r#"-c "read line; true" "{marker}""#)),
        "arguments with spaces are single quoted words: {line}"
    );
}
