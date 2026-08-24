#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};

use hypercolor_linux_session::arm_parent_death;

#[test]
fn armed_child_spawns_under_its_real_parent() {
    let mut command = Command::new("/bin/true");
    command.stdin(Stdio::null()).stdout(Stdio::null());
    arm_parent_death(&mut command, std::process::id());
    let status = command.status().expect("armed child should spawn");
    assert!(status.success());
}

#[test]
fn armed_child_refuses_to_exec_when_the_parent_already_changed() {
    let mut command = Command::new("/bin/true");
    command.stdin(Stdio::null()).stdout(Stdio::null());
    // Claiming pid 1 as the parent mirrors a supervisor that died before the
    // flag was armed: the post-fork re-check must refuse the exec.
    arm_parent_death(&mut command, 1);
    let error = command
        .status()
        .expect_err("child must not exec under the wrong parent");
    // The forked child can only relay a raw errno; ESRCH is the refusal.
    assert_eq!(
        error.raw_os_error(),
        Some(nix::errno::Errno::ESRCH as i32),
        "{error}"
    );
}
