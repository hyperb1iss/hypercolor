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
    let mut child = std::process::Command::new("sleep")
        .arg("27.1828")
        .arg("--")
        .spawn()
        .expect("spawn sleep");
    let processes = running_processes().expect("Linux should list processes");
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        processes.iter().any(|process| process
            .command_line
            .as_deref()
            .is_some_and(|line| line.ends_with("sleep 27.1828 --"))),
        "plain words stay unquoted"
    );

    let me = std::env::current_exe().expect("current exe");
    let me = me.to_string_lossy();
    if me.contains(' ') {
        assert!(
            processes.iter().any(|process| process
                .command_line
                .as_deref()
                .is_some_and(|line| line.starts_with(&format!("\"{me}\"")))),
            "a path with spaces is one quoted word"
        );
    }
}
