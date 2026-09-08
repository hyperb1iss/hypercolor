use hypercolor_openrgb_host::{OpenRgbOwner, StartFacts};

#[tokio::test]
async fn owner_rejects_remote_start_without_creating_host_files() {
    let directory = tempfile::tempdir().expect("directory");
    let mut owner = OpenRgbOwner::new(directory.path().to_owned());
    let result = owner
        .start(StartFacts {
            drivers: vec![],
            devices: vec![],
            endpoint: "192.0.2.1:6742".parse().expect("endpoint"),
        })
        .await;
    assert!(result.is_err());
    assert!(
        std::fs::read_dir(directory.path())
            .expect("directory")
            .next()
            .is_none()
    );
    assert_eq!(owner.stop().expect("idle stop")["stopped"], false);
}

#[cfg(unix)]
#[test]
fn owner_spawn_passes_only_its_directory_and_survives_foreground_handle_drop() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().expect("directory");
    let executable = directory.path().join("owner-fixture");
    std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$3/args\"\nsleep 0.1\nprintf complete > \"$3/completed\"\n").expect("fixture");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).expect("mode");
    let child = hypercolor_openrgb_host::spawn_owner(&executable, directory.path()).expect("spawn");
    let pid = i32::try_from(child.id()).expect("fixture pid");
    drop(child);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !directory.path().join("completed").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "detached owner should finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None).expect("reap fixture");
    let args = std::fs::read_to_string(directory.path().join("args")).expect("args");
    assert_eq!(
        args,
        format!(
            "openrgb-owner\n--data-dir\n{}\n",
            directory.path().display()
        )
    );
}

#[cfg(unix)]
#[test]
fn owner_spawn_strips_foreground_connection_environment() {
    use std::os::unix::fs::PermissionsExt;
    const FIXTURE: &str = "HYPERCOLOR_OWNER_ENV_FIXTURE";
    if std::env::var_os(FIXTURE).is_none() {
        let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "owner_spawn_strips_foreground_connection_environment",
            ])
            .env(FIXTURE, "1")
            .env("HYPERCOLOR_HOST", "remote.example")
            .env("HYPERCOLOR_PORT", "bad")
            .env("HYPERCOLOR_PROFILE", "missing-profile")
            .env("HYPERCOLOR_API_KEY", "fixture-secret")
            .status()
            .expect("isolated fixture");
        assert!(status.success());
        return;
    }
    let directory = tempfile::tempdir().expect("directory");
    let executable = directory.path().join("owner-fixture");
    std::fs::write(&executable, "#!/bin/sh\nenv > \"$3/environment\"\n").expect("fixture");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).expect("mode");
    let mut child =
        hypercolor_openrgb_host::spawn_owner(&executable, directory.path()).expect("spawn");
    assert!(child.wait().expect("wait").success());
    let environment =
        std::fs::read_to_string(directory.path().join("environment")).expect("environment");
    for name in [
        "HYPERCOLOR_HOST=",
        "HYPERCOLOR_PORT=",
        "HYPERCOLOR_PROFILE=",
        "HYPERCOLOR_API_KEY=",
    ] {
        assert!(
            !environment.lines().any(|line| line.starts_with(name)),
            "owner must not inherit {name}"
        );
    }
}
