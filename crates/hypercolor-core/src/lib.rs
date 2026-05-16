//! Core engine for the Hypercolor RGB lighting orchestration system.
//!
//! Contains the render loop, device backend traits, effect engine,
//! spatial sampler, event bus, and configuration management.
pub use hypercolor_types as types;

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
