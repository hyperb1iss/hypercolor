//! Hypercolor daemon — render loop, device orchestration, HTTP/WebSocket API,
//! MCP server, and system integration.

pub mod api;
pub mod attachment_profiles;
pub mod daemon;
pub(crate) mod deadline;
pub mod device_aliases;
pub mod device_metrics;
pub mod device_settings;
pub mod discovery;
pub mod display_frames;
pub mod display_output;
pub mod display_preferences;
pub mod domain;
pub mod driver_inventory;
pub mod extensions;
pub mod interaction_routing;
pub mod interactive_preview;
pub mod layout_auto_exclusions;
pub mod layout_store;
pub mod library;
pub mod logical_devices;
pub mod macos_owner;
#[cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]
pub mod macos_tcc_canary;
pub mod mcp;
pub mod mdns;
pub mod network;
pub mod path_migration;
pub mod performance;
pub mod persistence;
pub mod playlist_runtime;
pub mod preview_runtime;
pub mod profile_import;
pub mod render_thread;
pub mod runtime_state;
pub mod scene_store;
pub mod scene_transactions;
pub mod session;
pub mod simulators;
pub mod startup;
pub mod zone_layout_preview;
