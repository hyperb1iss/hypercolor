//! CLI subcommand modules.

pub mod audio;
pub mod brightness;
pub mod completions;
pub mod config;
pub mod controls;
pub mod devices;
pub mod diagnose;
pub mod drivers;
pub mod effects;
pub mod layouts;
pub mod library;
pub mod profiles;
pub mod scenes;
pub mod server;
pub mod servers;
pub mod service;
pub mod status;
#[cfg(feature = "tui")]
pub mod tui;
