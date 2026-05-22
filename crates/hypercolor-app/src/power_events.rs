//! Windows power-event listener.
//!
//! Registers a hidden message-only window that receives the
//! `WM_POWERBROADCAST` message and emits a single "resume" event the moment
//! the OS comes back from sleep. The supervisor uses that event to nudge
//! the daemon to rediscover devices — SMBus broker handles, network device
//! connections, and audio capture endpoints all tend to drop across a
//! suspend/resume cycle and the previous behavior was "user wonders why
//! lights stayed dark after a yawn."
//!
//! No-op on non-Windows targets so the cross-platform call sites stay
//! conditional-free.

#[cfg(target_os = "windows")]
mod windows_impl;

#[cfg(target_os = "windows")]
pub use windows_impl::start;

#[cfg(not(target_os = "windows"))]
pub fn start(_daemon_url: url::Url) {
    // Power events are a Windows-only concern in v1; Linux handles sleep
    // via systemd-logind suspend hooks, macOS via NSWorkspace notifications.
    // Both land as follow-ups when the daemon side wires the equivalent.
}
