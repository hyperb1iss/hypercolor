use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use hypercolor_app::linux_webkit::{nvidia_driver_present_in, should_disable_nvidia_explicit_sync};

#[test]
fn nvidia_explicit_sync_workaround_targets_wayland_nvidia_without_override() {
    assert!(should_disable_nvidia_explicit_sync(
        Some("wayland-1"),
        None,
        false,
        true
    ));
    assert!(should_disable_nvidia_explicit_sync(
        None,
        Some("wayland"),
        false,
        true
    ));
}

#[test]
fn nvidia_explicit_sync_workaround_respects_existing_env_and_other_sessions() {
    assert!(!should_disable_nvidia_explicit_sync(
        Some("wayland-1"),
        None,
        true,
        true
    ));
    assert!(!should_disable_nvidia_explicit_sync(
        Some("wayland-1"),
        None,
        false,
        false
    ));
    assert!(!should_disable_nvidia_explicit_sync(
        None,
        Some("x11"),
        false,
        true
    ));
}

#[test]
fn nvidia_driver_probe_accepts_proc_and_sys_module_layouts() {
    let proc_root = temp_root("proc");
    let proc_version = proc_root
        .join("proc")
        .join("driver")
        .join("nvidia")
        .join("version");
    fs::create_dir_all(
        proc_version
            .parent()
            .expect("version path should have parent"),
    )
    .expect("proc nvidia directory should be created");
    File::create(&proc_version).expect("proc nvidia version should be created");

    let sys_root = temp_root("sys");
    fs::create_dir_all(sys_root.join("sys").join("module").join("nvidia"))
        .expect("sys nvidia module directory should be created");

    assert!(nvidia_driver_present_in(&proc_root));
    assert!(nvidia_driver_present_in(&sys_root));
    assert!(!nvidia_driver_present_in(Path::new(
        "/definitely/not/hypercolor/nvidia"
    )));

    remove_temp_root(proc_root);
    remove_temp_root(sys_root);
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "hypercolor-app-linux-webkit-{name}-{}",
        std::process::id()
    ));
    remove_temp_root(root.clone());
    root
}

fn remove_temp_root(root: PathBuf) {
    if root.exists() {
        fs::remove_dir_all(root).expect("temporary root should be removable");
    }
}
