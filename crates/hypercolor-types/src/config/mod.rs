//! Configuration types -- daemon, audio, web, capture, discovery, and driver settings.
//!
//! Optional fields use Serde defaults so a fresh install can boot entirely from
//! compile-time values. Closed nested sections reject unknown keys before a
//! whole-file rewrite can silently delete them; the root and driver maps retain
//! explicit extension doors.

mod audio;
mod capture;
mod daemon;
mod discovery;
mod display;
mod drivers;
mod effect_engine;
mod input;
mod mcp;
mod media;
mod network;
mod rendering;
mod root;
mod web;

pub use audio::*;
pub use capture::*;
pub use daemon::*;
pub use discovery::*;
pub use display::*;
pub use drivers::*;
pub use effect_engine::*;
pub use input::*;
pub use mcp::*;
pub use media::*;
pub use network::*;
pub use rendering::*;
pub use root::*;
pub use web::*;

// ─── Default Value Functions ─────────────────────────────────────────────────
// Referenced by `#[serde(default = "defaults::...")]` throughout this module.

mod defaults {
    use super::InteractionRoutePolicy;
    use super::LogLevel;
    use super::RenderAccelerationMode;
    use super::ServoGpuImportMode;
    use super::ShutdownBehavior;

    // Daemon
    pub fn listen_address() -> String {
        "127.0.0.1".into()
    }
    pub fn port() -> u16 {
        9420
    }
    pub fn target_fps() -> u32 {
        30
    }
    pub fn canvas_width() -> u32 {
        640
    }
    pub fn canvas_height() -> u32 {
        480
    }
    pub fn max_devices() -> u32 {
        32
    }
    pub fn log_level() -> LogLevel {
        LogLevel::Info
    }
    pub fn start_scene() -> String {
        "last".into()
    }
    pub fn shutdown_behavior() -> ShutdownBehavior {
        ShutdownBehavior::HardwareDefault
    }
    pub fn shutdown_color() -> String {
        "#1a1a2e".into()
    }

    // Web
    pub fn websocket_fps() -> u32 {
        30
    }
    pub const fn interactive_preview_resource_bytes() -> u64 {
        1024 * 1024 * 1024
    }

    // MCP
    pub fn mcp_base_path() -> String {
        "/mcp".into()
    }
    pub fn sse_keep_alive_secs() -> u64 {
        15
    }

    // Audio
    pub fn audio_device() -> String {
        "default".into()
    }
    pub fn fft_size() -> u32 {
        1024
    }
    pub fn smoothing() -> f32 {
        0.8
    }
    pub fn noise_gate() -> f32 {
        0.02
    }
    pub fn beat_sensitivity() -> f32 {
        0.6
    }

    // Capture
    pub fn capture_source() -> String {
        "auto".into()
    }
    /// Whether screen capture is allowed without the user opting in.
    ///
    /// Windows Desktop Duplication needs no permission grant, shows no
    /// source picker, and draws no capture indicator, so there is nothing
    /// for a user to consent to and an ambient effect can simply work. The
    /// XDG portal on Linux opens a picker the user must answer, and macOS
    /// gates capture behind a TCC prompt; forcing either at daemon start
    /// would be an ambush, so those stay opt-in.
    ///
    /// Enabling this only grants permission. Capture opens on demand and
    /// stays closed until a screen-reactive effect actually asks for it.
    pub fn capture_enabled() -> bool {
        cfg!(target_os = "windows")
    }
    pub fn capture_fps() -> u32 {
        30
    }
    pub fn capture_grid_cols() -> u32 {
        8
    }
    pub fn capture_grid_rows() -> u32 {
        6
    }
    pub fn capture_smoothing() -> f32 {
        0.3
    }
    pub fn capture_scene_cut_threshold() -> f32 {
        100.0
    }
    pub fn capture_letterbox_threshold() -> f32 {
        0.02
    }
    pub fn capture_target_led_white_x() -> f32 {
        0.3127
    }
    pub fn capture_target_led_white_y() -> f32 {
        0.3290
    }
    pub fn capture_target_led_reference_white_nits() -> f32 {
        203.0
    }
    pub fn capture_target_led_peak_nits() -> f32 {
        406.0
    }
    pub fn capture_exposure_ev() -> f32 {
        0.0
    }
    pub fn unit_scale() -> f32 {
        1.0
    }

    // Display
    pub fn face_fps_cap() -> u32 {
        30
    }

    // Discovery
    pub fn scan_interval() -> u64 {
        300
    }
    // Network
    pub fn remote_access() -> bool {
        false
    }
    pub fn network_access_mode() -> super::NetworkAccessMode {
        super::NetworkAccessMode::LocalOnly
    }
    pub fn network_client_scope() -> super::NetworkClientScope {
        super::NetworkClientScope::LocalSubnets
    }
    // Effect engine
    pub fn auto_string() -> String {
        "auto".into()
    }
    pub fn compositor_acceleration_mode() -> RenderAccelerationMode {
        RenderAccelerationMode::Auto
    }
    pub const fn servo_gpu_import_mode() -> ServoGpuImportMode {
        ServoGpuImportMode::Auto
    }
    pub const fn max_video_producers() -> u8 {
        2
    }
    pub const fn max_livestream_producers() -> u8 {
        1
    }

    // Shared
    pub fn bool_true() -> bool {
        true
    }
    pub fn bool_false() -> bool {
        false
    }
    pub const fn daemon_interaction_route() -> InteractionRoutePolicy {
        InteractionRoutePolicy::Host
    }
    pub const fn preview_interaction_route() -> InteractionRoutePolicy {
        InteractionRoutePolicy::Browser
    }
}
