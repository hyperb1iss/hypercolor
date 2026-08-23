//! Core engine for the Hypercolor RGB lighting orchestration system.
//!
//! Contains the render loop, device backend traits, effect engine,
//! spatial sampler, event bus, and configuration management.
pub mod asset;
pub mod attachment;
pub mod blend_math;
pub mod bus;
pub mod config;
pub mod device;
pub mod effect;
pub mod engine;
pub mod input;
pub mod scene;
pub mod session;
pub mod spatial;
pub mod system;

/// Durable file replacement shared by every Hypercolor store.
///
/// The implementation lives in `hypercolor-persistence` so driver crates can
/// register their destinations without depending on the engine.
pub use hypercolor_persistence as persistence;
