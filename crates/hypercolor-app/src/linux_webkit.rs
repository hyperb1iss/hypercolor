//! Linux WebKitGTK startup environment helpers.

use std::path::Path;

pub const NVIDIA_EXPLICIT_SYNC_ENV: &str = "__NV_DISABLE_EXPLICIT_SYNC";

#[must_use]
pub fn should_disable_nvidia_explicit_sync(
    wayland_display: Option<&str>,
    xdg_session_type: Option<&str>,
    explicit_sync_configured: bool,
    nvidia_driver_present: bool,
) -> bool {
    nvidia_driver_present
        && !explicit_sync_configured
        && is_wayland_session(wayland_display, xdg_session_type)
}

#[must_use]
pub fn nvidia_driver_present_in(root: &Path) -> bool {
    root.join("proc")
        .join("driver")
        .join("nvidia")
        .join("version")
        .is_file()
        || root.join("sys").join("module").join("nvidia").exists()
}

fn is_wayland_session(wayland_display: Option<&str>, xdg_session_type: Option<&str>) -> bool {
    wayland_display.is_some_and(|value| !value.is_empty())
        || xdg_session_type.is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
}

#[cfg(target_os = "linux")]
pub fn reexec_with_webkit_env_if_needed() -> anyhow::Result<()> {
    use std::{os::unix::process::CommandExt, process::Command};

    let explicit_sync_configured =
        std::env::var_os(NVIDIA_EXPLICIT_SYNC_ENV).is_some_and(|value| !value.is_empty());
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    let xdg_session_type = std::env::var("XDG_SESSION_TYPE").ok();

    if !should_disable_nvidia_explicit_sync(
        wayland_display.as_deref(),
        xdg_session_type.as_deref(),
        explicit_sync_configured,
        nvidia_driver_present_in(Path::new("/")),
    ) {
        return Ok(());
    }

    let current_exe = std::env::current_exe()?;
    let error = Command::new(current_exe)
        .args(std::env::args_os().skip(1))
        .env(NVIDIA_EXPLICIT_SYNC_ENV, "1")
        .exec();

    Err(error.into())
}
