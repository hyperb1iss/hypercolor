//! Hypercolor daemon — render loop, device orchestration, HTTP/WebSocket API,
//! MCP server, and system integration.

pub mod api;
pub mod app_state;
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
pub mod launcher_claim;
pub mod layout_auto_exclusions;
pub mod layout_store;
pub mod library;
pub mod logical_devices;
pub mod macos_owner;
pub mod macos_service_identity;
/// ScreenCaptureKit TCC canary harness; the feature is macOS-only and the
/// module refuses other targets with a clear error.
#[cfg(feature = "macos-tcc-canary")]
pub mod macos_tcc_canary;
pub mod mcp;
pub mod mdns;
pub mod network;
pub mod output_power;
pub mod path_migration;
pub mod performance;
pub use hypercolor_core::persistence;
pub mod playlist_runtime;
pub mod preview_runtime;
pub mod process;
pub(crate) mod profile_import;
pub mod render_thread;
pub(crate) mod resource_summary;
pub mod runtime_state;
pub mod scene_store;
pub(crate) mod scene_transactions;
#[doc(hidden)]
pub use scene_transactions::SceneTransactionQueue;
#[cfg(feature = "persistence-test-hooks")]
#[doc(hidden)]
pub use scene_transactions::{LayoutPublicationTestExecutor, LayoutTransactionRejection};
pub mod session;
pub mod simulators;
pub mod startup;
pub mod zone_layout_preview;
